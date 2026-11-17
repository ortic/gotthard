//! Synchronizes a folder of the master copy with a folder of this computer.
//!
//! [`compare`] reads both folders and proposes a [`Plan`]; nothing is
//! changed until [`execute`] runs it. For two-way synchronization the
//! [`State`] remembers what both sides looked like after the last run, which
//! tells a file deleted on one side from a file created on the other, and a
//! change on one side from a conflict.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::fsx::{self, Displacer, Removal, Stamp, format_bytes, join_rel};
use crate::scan::{self, Entry, Filter, Tree};
use crate::{Progress, Report, is_cancelled};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Changes of either side go to the other.
    #[default]
    TwoWay,
    /// The computer's folder becomes an exact copy of the master.
    Mirror,
    /// New and newer files of the computer go to the master; nothing is
    /// deleted and nothing newer is replaced.
    Collect,
}

/// What happens to a file changed on both sides since the last run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictPolicy {
    /// The user decides, file by file.
    #[default]
    Ask,
    MasterWins,
    NewerWins,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Master,
    Device,
}

impl Side {
    pub fn other(self) -> Side {
        match self {
            Side::Master => Side::Device,
            Side::Device => Side::Master,
        }
    }
}

/// The direction an item is synchronized in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Nothing,
    /// The computer gets what the master has: the file, or its absence.
    ToDevice,
    /// The master gets what the computer has.
    ToMaster,
    /// Not decided: left alone until the user picks a direction.
    Conflict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Equal,
    OnlyMaster,
    OnlyDevice,
    MasterChanged,
    DeviceChanged,
    MasterDeleted,
    DeviceDeleted,
    /// Changed on both sides since the last run.
    BothChanged,
    /// Changed on one side, deleted on the other.
    ChangedAndDeleted,
    /// Different, and no last run to tell which side changed.
    Differs,
    /// A file on one side, a folder on the other.
    TypeMismatch,
    /// Collect: the master's file is newer and stays.
    MasterNewer,
}

impl Why {
    pub fn text(self) -> &'static str {
        match self {
            Why::Equal => "equal",
            Why::OnlyMaster => "only on the master",
            Why::OnlyDevice => "only on this computer",
            Why::MasterChanged => "changed on the master",
            Why::DeviceChanged => "changed on this computer",
            Why::MasterDeleted => "deleted on the master",
            Why::DeviceDeleted => "deleted on this computer",
            Why::BothChanged => "changed on both sides",
            Why::ChangedAndDeleted => "changed on one side, deleted on the other",
            Why::Differs => "different on both sides",
            Why::TypeMismatch => "a file on one side, a folder on the other",
            Why::MasterNewer => "newer on the master",
        }
    }
}

/// What executing an item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    None,
    CreateDir(Side),
    Copy { to: Side },
    Delete(Side),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub rel: String,
    pub key: String,
    pub is_dir: bool,
    /// What's there on each side; a folder's stamp is empty.
    pub master: Option<Stamp>,
    pub device: Option<Stamp>,
    pub action: Action,
    pub why: Why,
    /// Both sides have something the other would lose.
    pub conflict: bool,
}

impl Item {
    pub fn op(&self) -> Op {
        let (from, to, side) = match self.action {
            Action::Nothing | Action::Conflict => return Op::None,
            Action::ToDevice => (self.master, self.device, Side::Device),
            Action::ToMaster => (self.device, self.master, Side::Master),
        };
        match (from, to) {
            (Some(_), None) if self.is_dir => Op::CreateDir(side),
            (Some(_), Some(_)) if self.is_dir => Op::None,
            (Some(_), _) => Op::Copy { to: side },
            (None, Some(_)) => Op::Delete(side),
            (None, None) => Op::None,
        }
    }

    /// The directions the user may choose from.
    pub fn can_change(&self) -> bool {
        self.why != Why::TypeMismatch && self.why != Why::Equal
    }
}

/// What both sides looked like after the last run.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct State {
    pub master_id: String,
    pub device_root: String,
    pub entries: BTreeMap<String, Synced>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Synced {
    #[serde(default)]
    pub is_dir: bool,
    pub master: Stamp,
    pub device: Stamp,
}

impl State {
    pub fn new(master_id: &str, device_root: &Path) -> Self {
        Self {
            master_id: master_id.to_owned(),
            device_root: device_root.to_string_lossy().into_owned(),
            entries: BTreeMap::new(),
        }
    }

    /// The state at `path` when it's the one of this master and this
    /// folder: the state of another pair would turn every file into a
    /// deletion.
    pub fn load(path: &Path, master_id: &str, device_root: &Path) -> Option<Self> {
        let text = fs::read_to_string(path).ok()?;
        let state: State = serde_json::from_str(&text).ok()?;
        (state.master_id == master_id && state.device_root == device_root.to_string_lossy())
            .then_some(state)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec(self)?)
            .with_context(|| format!("can't write {}", tmp.display()))?;
        fs::rename(&tmp, path).with_context(|| format!("can't write {}", path.display()))
    }
}

#[derive(Clone, Debug)]
pub struct CompareOptions {
    pub mode: Mode,
    pub conflicts: ConflictPolicy,
    pub filter: Filter,
    /// Files of the same size but different times are read and compared:
    /// the same content is no difference.
    pub by_content: bool,
    pub case_insensitive: bool,
}

impl Default for CompareOptions {
    fn default() -> Self {
        Self {
            mode: Mode::TwoWay,
            conflicts: ConflictPolicy::Ask,
            filter: Filter::standard(),
            by_content: false,
            case_insensitive: cfg!(windows),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub master_root: PathBuf,
    pub device_root: PathBuf,
    pub mode: Mode,
    pub items: Vec<Item>,
    /// State entries of files gone on both sides.
    pub forget: Vec<String>,
    /// Things the user should know before running the plan.
    pub warnings: Vec<String>,
    /// Running the plan would delete most of a side: ask again.
    pub mass_deletion: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub to_device: usize,
    pub to_device_bytes: u64,
    pub to_master: usize,
    pub to_master_bytes: u64,
    pub delete_device: usize,
    pub delete_master: usize,
    pub conflicts: usize,
    pub equal: usize,
}

impl Summary {
    pub fn changes(&self) -> usize {
        self.to_device + self.to_master + self.delete_device + self.delete_master
    }
}

impl Plan {
    pub fn summary(&self) -> Summary {
        let mut s = Summary::default();
        for item in &self.items {
            match item.op() {
                Op::Copy { to: Side::Device } => {
                    s.to_device += 1;
                    s.to_device_bytes += item.master.map_or(0, |m| m.size);
                }
                Op::Copy { to: Side::Master } => {
                    s.to_master += 1;
                    s.to_master_bytes += item.device.map_or(0, |d| d.size);
                }
                Op::CreateDir(Side::Device) => s.to_device += 1,
                Op::CreateDir(Side::Master) => s.to_master += 1,
                Op::Delete(Side::Device) => s.delete_device += 1,
                Op::Delete(Side::Master) => s.delete_master += 1,
                Op::None if item.action == Action::Conflict => s.conflicts += 1,
                Op::None => s.equal += 1,
            }
        }
        s
    }

    pub fn root(&self, side: Side) -> &Path {
        match side {
            Side::Master => &self.master_root,
            Side::Device => &self.device_root,
        }
    }

    /// Decides every open conflict: the given side's version wins.
    pub fn resolve_conflicts(&mut self, winner: Side) {
        for item in &mut self.items {
            if item.action == Action::Conflict && item.can_change() {
                item.action = match winner {
                    Side::Master => Action::ToDevice,
                    Side::Device => Action::ToMaster,
                };
            }
        }
    }
}

/// Reads both folders and proposes what to do. Changes nothing.
pub fn compare(
    master_root: &Path,
    device_root: &Path,
    state: Option<&State>,
    options: &CompareOptions,
    progress: &Progress,
) -> Result<Plan> {
    progress.phase("Reading both folders", 0, 0);
    let (master, device) = rayon::join(
        || scan::scan(master_root, &options.filter, progress),
        || scan::scan(device_root, &options.filter, progress),
    );
    let (master, device) = (master?, device?);
    plan(&master, &device, state, options, progress)
}

fn plan(
    master: &Tree,
    device: &Tree,
    state: Option<&State>,
    options: &CompareOptions,
    progress: &Progress,
) -> Result<Plan> {
    progress.phase("Comparing", 0, 0);
    let ci = options.case_insensitive;
    let mut pairs: BTreeMap<String, (Option<&Entry>, Option<&Entry>)> = BTreeMap::new();
    for entry in &master.entries {
        pairs.entry(scan::key(&entry.rel, ci)).or_default().0 = Some(entry);
    }
    for entry in &device.entries {
        pairs.entry(scan::key(&entry.rel, ci)).or_default().1 = Some(entry);
    }

    let mut warnings = Vec::new();
    // What couldn't be read isn't missing: leave everything below alone.
    let unreadable: Vec<String> = master
        .errors
        .iter()
        .chain(&device.errors)
        .map(|(rel, _)| scan::key(rel, ci))
        .collect();
    for (side, tree) in [("the master", master), ("this computer", device)] {
        for (rel, error) in &tree.errors {
            warnings.push(format!("Left alone, unreadable on {side}: {rel} ({error})"));
        }
        if tree.links > 0 {
            warnings.push(format!(
                "{} links on {side} are left alone (Gotthard doesn't follow links).",
                tree.links
            ));
        }
    }
    let is_unreadable = |key: &str| {
        unreadable.iter().any(|prefix| {
            key == prefix
                || key
                    .strip_prefix(prefix.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        })
    };

    let same_content = |rel_m: &str, rel_d: &str| -> bool {
        let a = fsx::hash_file(&join_rel(&master.root, rel_m), Some(progress));
        let b = fsx::hash_file(&join_rel(&device.root, rel_d), Some(progress));
        matches!((a, b), (Ok(a), Ok(b)) if a == b)
    };

    let mut items = Vec::new();
    for (key, (m, d)) in &pairs {
        progress.check()?;
        if is_unreadable(key) {
            continue;
        }
        let known = match options.mode {
            Mode::TwoWay => state.and_then(|s| s.entries.get(key)),
            Mode::Mirror | Mode::Collect => None,
        };
        let rel = m.or(*d).map(|e| e.rel.clone()).unwrap_or_default();
        let is_dir = m.or(*d).is_some_and(|e| e.is_dir);
        let stamp = |e: &Option<&Entry>| e.map(|e| e.stamp);
        let mut item = Item {
            rel,
            key: key.clone(),
            is_dir,
            master: stamp(m),
            device: stamp(d),
            action: Action::Nothing,
            why: Why::Equal,
            conflict: false,
        };
        // The same on both sides, whatever the history says.
        let equal = |m: &Entry, d: &Entry| {
            m.is_dir && d.is_dir
                || !m.is_dir
                    && !d.is_dir
                    && (m.stamp.same(&d.stamp)
                        || options.by_content
                            && m.stamp.size == d.stamp.size
                            && same_content(&m.rel, &d.rel))
        };
        let (action, why) = match (m, d) {
            (None, None) => continue,
            (Some(m), Some(d)) if m.is_dir != d.is_dir => (Action::Conflict, Why::TypeMismatch),
            (Some(m), Some(d)) if equal(m, d) => (Action::Nothing, Why::Equal),
            _ => match options.mode {
                Mode::Mirror => match (m, d) {
                    (Some(_), None) => (Action::ToDevice, Why::OnlyMaster),
                    (None, Some(_)) => (Action::ToDevice, Why::OnlyDevice),
                    _ => (Action::ToDevice, Why::Differs),
                },
                Mode::Collect => match (m, d) {
                    (Some(_), None) => (Action::Nothing, Why::OnlyMaster),
                    (None, Some(_)) => (Action::ToMaster, Why::OnlyDevice),
                    (Some(m), Some(d)) if d.stamp.mtime > m.stamp.mtime => {
                        (Action::ToMaster, Why::DeviceChanged)
                    }
                    _ => (Action::Nothing, Why::MasterNewer),
                },
                Mode::TwoWay => match known {
                    None => match (m, d) {
                        (Some(_), None) => (Action::ToDevice, Why::OnlyMaster),
                        (None, Some(_)) => (Action::ToMaster, Why::OnlyDevice),
                        _ => (Action::Conflict, Why::Differs),
                    },
                    Some(known) => {
                        let changed = |e: &Option<&Entry>, was: &Stamp| {
                            e.map(|e| e.is_dir != known.is_dir || !e.is_dir && !e.stamp.same(was))
                        };
                        // None: deleted; Some(changed).
                        match (changed(m, &known.master), changed(d, &known.device)) {
                            (None, None) => continue,
                            (Some(false), Some(false)) => (Action::Nothing, Why::Equal),
                            (Some(true), Some(false)) => (Action::ToDevice, Why::MasterChanged),
                            (Some(false), Some(true)) => (Action::ToMaster, Why::DeviceChanged),
                            (None, Some(false)) => (Action::ToDevice, Why::MasterDeleted),
                            (Some(false), None) => (Action::ToMaster, Why::DeviceDeleted),
                            (Some(true), Some(true)) => (Action::Conflict, Why::BothChanged),
                            (None, Some(true)) | (Some(true), None) => {
                                (Action::Conflict, Why::ChangedAndDeleted)
                            }
                        }
                    }
                },
            },
        };
        item.action = action;
        item.why = why;
        if action == Action::Conflict && why != Why::TypeMismatch {
            item.conflict = true;
            item.action = match options.conflicts {
                ConflictPolicy::Ask => Action::Conflict,
                ConflictPolicy::MasterWins => Action::ToDevice,
                ConflictPolicy::NewerWins => match (item.master, item.device) {
                    (Some(m), Some(d)) if m.mtime > d.mtime => Action::ToDevice,
                    (Some(m), Some(d)) if d.mtime > m.mtime => Action::ToMaster,
                    // The changed file wins over its deletion.
                    (Some(_), None) => Action::ToDevice,
                    (None, Some(_)) => Action::ToMaster,
                    _ => Action::Conflict,
                },
            };
        }
        items.push(item);
    }

    keep_folders_in_use(&mut items);

    let forget = state
        .map(|s| {
            s.entries
                .keys()
                .filter(|key| {
                    pairs
                        .get(*key)
                        .is_none_or(|(m, d)| m.is_none() && d.is_none())
                        && !is_unreadable(key)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();

    let mut plan = Plan {
        master_root: master.root.clone(),
        device_root: device.root.clone(),
        mode: options.mode,
        items,
        forget,
        warnings,
        mass_deletion: false,
    };
    let summary = plan.summary();
    for (deleted, tree, name) in [
        (summary.delete_master, master, "the master"),
        (summary.delete_device, device, "this computer"),
    ] {
        let files = tree.entries.len();
        if deleted >= 10 && deleted * 2 > files {
            plan.mass_deletion = true;
            plan.warnings.insert(
                0,
                format!(
                    "{deleted} of the {files} files and folders on {name} would be deleted. \
                     Check that both folders are the right ones."
                ),
            );
        }
    }
    Ok(plan)
}

/// A folder deleted on one side stays where a file below it stays: the
/// folder then goes to the other side instead, with that file.
fn keep_folders_in_use(items: &mut [Item]) {
    for side in [Side::Master, Side::Device] {
        let mut stays: BTreeSet<String> = BTreeSet::new();
        for item in items.iter() {
            let there = match side {
                Side::Master => item.master,
                Side::Device => item.device,
            }
            .is_some();
            let after = match item.op() {
                Op::Delete(s) if s == side => false,
                Op::Copy { to } | Op::CreateDir(to) if to == side => true,
                _ => there,
            };
            if after {
                let mut key = item.key.as_str();
                while let Some(slash) = key.rfind('/') {
                    key = &key[..slash];
                    if !stays.insert(key.to_owned()) {
                        break;
                    }
                }
            }
        }
        for item in items.iter_mut() {
            if item.is_dir && item.op() == Op::Delete(side) && stays.contains(&item.key) {
                // Creates the folder on the other side instead.
                item.action = match side {
                    Side::Master => Action::ToDevice,
                    Side::Device => Action::ToMaster,
                };
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ExecOptions {
    /// Read every copy back and compare it with its original.
    pub verify: bool,
    pub removal: Removal,
}

impl Default for ExecOptions {
    fn default() -> Self {
        Self {
            verify: true,
            removal: Removal::Vault,
        }
    }
}

/// Runs the plan and brings `state` up to date with what was done. Items
/// whose files changed since the comparison are skipped.
pub fn execute(
    plan: &Plan,
    state: &mut State,
    options: &ExecOptions,
    progress: &Progress,
) -> Result<Report> {
    let summary = plan.summary();
    for (bytes, side, name) in [
        (summary.to_master_bytes, Side::Master, "the master"),
        (summary.to_device_bytes, Side::Device, "this computer"),
    ] {
        if let Some(free) = fsx::free_space(plan.root(side))
            && bytes > free
        {
            bail!(
                "Not enough space on {name}: {} to copy, {} free.",
                format_bytes(bytes),
                format_bytes(free)
            );
        }
    }
    progress.phase(
        "Synchronizing",
        summary.changes() as u64,
        summary.to_master_bytes + summary.to_device_bytes,
    );
    let displacer = Displacer::new(options.removal);
    let mut report = Report::default();
    for key in &plan.forget {
        state.entries.remove(key);
    }

    let path = |item: &Item, side: Side| join_rel(plan.root(side), &item.rel);
    // Still what the comparison saw?
    let unchanged = |item: &Item| -> bool {
        [(Side::Master, item.master), (Side::Device, item.device)]
            .into_iter()
            .all(|(side, planned)| {
                let now = fsx::stat(&path(item, side)).ok().flatten();
                match (planned, now) {
                    (None, None) => true,
                    (Some(_), Some((_, true))) => item.is_dir,
                    (Some(planned), Some((now, false))) => !item.is_dir && planned == now,
                    _ => false,
                }
            })
    };
    const CHANGED: &str = "changed since the comparison; compare again";

    let run = |report: &mut Report, state: &mut State| -> Result<()> {
        // Folders first, files into them, deletions last, folders after
        // their content.
        for item in &plan.items {
            let Op::CreateDir(side) = item.op() else {
                if item.why == Why::Equal && item.action == Action::Nothing {
                    state.entries.insert(item.key.clone(), synced(item));
                }
                continue;
            };
            progress.check()?;
            progress.item();
            let target = path(item, side);
            match fs::create_dir_all(&target) {
                Ok(()) => {
                    report.copied += 1;
                    state.entries.insert(
                        item.key.clone(),
                        Synced {
                            is_dir: true,
                            ..Synced::default()
                        },
                    );
                }
                Err(e) => report.failed.push((item.rel.clone(), e.to_string())),
            }
        }
        for item in &plan.items {
            let Op::Copy { to } = item.op() else { continue };
            progress.check()?;
            progress.current(&item.rel);
            progress.item();
            if !unchanged(item) {
                report.skipped.push((item.rel.clone(), CHANGED.into()));
                continue;
            }
            let (source, target) = (path(item, to.other()), path(item, to));
            match fsx::copy_file(
                &source,
                &target,
                plan.root(to),
                options.verify,
                &displacer,
                progress,
            ) {
                Ok(copied) => {
                    report.copied += 1;
                    report.bytes += copied.size;
                    let original = fsx::stat(&source)?.map(|(s, _)| s).unwrap_or_default();
                    let (master, device) = match to {
                        Side::Device => (original, copied),
                        Side::Master => (copied, original),
                    };
                    state.entries.insert(
                        item.key.clone(),
                        Synced {
                            is_dir: false,
                            master,
                            device,
                        },
                    );
                }
                Err(e) if is_cancelled(&e) => return Err(e),
                Err(e) => report.failed.push((item.rel.clone(), format!("{e:#}"))),
            }
        }
        for item in plan.items.iter().rev() {
            let Op::Delete(side) = item.op() else {
                continue;
            };
            progress.check()?;
            progress.current(&item.rel);
            progress.item();
            if !unchanged(item) {
                report.skipped.push((item.rel.clone(), CHANGED.into()));
                continue;
            }
            let target = path(item, side);
            let result = if item.is_dir {
                match fs::read_dir(&target).map(|mut entries| entries.next().is_none()) {
                    Ok(true) => fs::remove_dir(&target).map_err(anyhow::Error::from),
                    Ok(false) => {
                        report.skipped.push((
                            item.rel.clone(),
                            "the folder still holds files the synchronization leaves alone".into(),
                        ));
                        continue;
                    }
                    Err(e) => Err(e.into()),
                }
            } else {
                displacer.remove(plan.root(side), &target)
            };
            match result {
                Ok(()) => {
                    report.deleted += 1;
                    state.entries.remove(&item.key);
                }
                Err(e) => report.failed.push((item.rel.clone(), format!("{e:#}"))),
            }
        }
        Ok(())
    };
    match run(&mut report, state) {
        Ok(()) => {}
        Err(e) if is_cancelled(&e) => report.cancelled = true,
        Err(e) => return Err(e),
    }
    Ok(report)
}

fn synced(item: &Item) -> Synced {
    Synced {
        is_dir: item.is_dir,
        master: item.master.unwrap_or_default(),
        device: item.device.unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    struct Pair {
        _dir: tempfile::TempDir,
        master: PathBuf,
        device: PathBuf,
        state: State,
        options: CompareOptions,
    }

    fn pair() -> Pair {
        let dir = tempfile::tempdir().unwrap();
        let master = dir.path().join("master");
        let device = dir.path().join("device");
        fs::create_dir_all(&master).unwrap();
        fs::create_dir_all(&device).unwrap();
        Pair {
            state: State::new("m", &device),
            _dir: dir,
            master,
            device,
            options: CompareOptions::default(),
        }
    }

    /// Writes a file whose time is `age` hours ago: different content of
    /// the same length must differ in time as it does on a real disk.
    fn write(root: &Path, rel: &str, content: &str, age: u64) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(age * 3600))
            .unwrap();
    }

    fn read(root: &Path, rel: &str) -> Option<String> {
        fs::read_to_string(root.join(rel)).ok()
    }

    impl Pair {
        fn compare(&self) -> Plan {
            compare(
                &self.master,
                &self.device,
                Some(&self.state),
                &self.options,
                &Progress::default(),
            )
            .unwrap()
        }

        fn run(&mut self, plan: &Plan) -> Report {
            execute(
                plan,
                &mut self.state,
                &ExecOptions::default(),
                &Progress::default(),
            )
            .unwrap()
        }

        fn sync(&mut self) -> Report {
            let plan = self.compare();
            self.run(&plan)
        }
    }

    fn actions(plan: &Plan) -> Vec<(&str, Op, Why)> {
        plan.items
            .iter()
            .filter(|i| i.why != Why::Equal)
            .map(|i| (i.rel.as_str(), i.op(), i.why))
            .collect()
    }

    #[test]
    fn first_run_copies_both_ways_and_deletes_nothing() {
        let mut p = pair();
        write(&p.master, "docs/a.txt", "a", 5);
        write(&p.device, "pics/b.jpg", "b", 5);
        write(&p.master, "same.txt", "same", 5);
        write(&p.device, "same.txt", "same", 5);
        let plan = p.compare();
        assert_eq!(
            actions(&plan),
            [
                ("docs", Op::CreateDir(Side::Device), Why::OnlyMaster),
                ("docs/a.txt", Op::Copy { to: Side::Device }, Why::OnlyMaster),
                ("pics", Op::CreateDir(Side::Master), Why::OnlyDevice),
                ("pics/b.jpg", Op::Copy { to: Side::Master }, Why::OnlyDevice),
            ]
        );
        let report = p.run(&plan);
        assert!(report.clean(), "{report:?}");
        assert_eq!(read(&p.device, "docs/a.txt").as_deref(), Some("a"));
        assert_eq!(read(&p.master, "pics/b.jpg").as_deref(), Some("b"));
        assert_eq!(p.compare().summary().changes(), 0);
    }

    #[test]
    fn changes_and_deletions_go_to_the_other_side() {
        let mut p = pair();
        write(&p.master, "a.txt", "a", 9);
        write(&p.master, "b.txt", "b", 9);
        write(&p.master, "old/c.txt", "c", 9);
        p.sync();

        write(&p.device, "a.txt", "a2", 1);
        fs::remove_file(p.master.join("b.txt")).unwrap();
        fs::remove_dir_all(p.device.join("old")).unwrap();
        let plan = p.compare();
        assert_eq!(
            actions(&plan),
            [
                ("a.txt", Op::Copy { to: Side::Master }, Why::DeviceChanged),
                ("b.txt", Op::Delete(Side::Device), Why::MasterDeleted),
                ("old", Op::Delete(Side::Master), Why::DeviceDeleted),
                ("old/c.txt", Op::Delete(Side::Master), Why::DeviceDeleted),
            ]
        );
        let report = p.run(&plan);
        assert!(report.clean(), "{report:?}");
        assert_eq!(read(&p.master, "a.txt").as_deref(), Some("a2"));
        assert!(!p.device.join("b.txt").exists());
        assert!(!p.master.join("old").exists());
        // What was replaced and deleted is in the safety copies.
        let kept = crate::vault::list(&p.master).unwrap();
        let rels: Vec<&str> = kept[0].files.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels, ["a.txt", "old/c.txt"]);
        assert_eq!(p.compare().summary().changes(), 0);
    }

    #[test]
    fn a_change_on_both_sides_is_a_conflict_until_decided() {
        let mut p = pair();
        write(&p.master, "a.txt", "a", 9);
        write(&p.master, "b.txt", "b", 9);
        p.sync();
        write(&p.master, "a.txt", "master", 2);
        write(&p.device, "a.txt", "device!", 1);
        write(&p.master, "b.txt", "changed", 1);
        fs::remove_file(p.device.join("b.txt")).unwrap();

        let mut plan = p.compare();
        assert_eq!(
            actions(&plan),
            [
                ("a.txt", Op::None, Why::BothChanged),
                ("b.txt", Op::None, Why::ChangedAndDeleted),
            ]
        );
        assert_eq!(plan.summary().conflicts, 2);
        // Undecided conflicts are left alone.
        assert!(p.run(&plan).clean());
        assert_eq!(read(&p.device, "a.txt").as_deref(), Some("device!"));

        plan.resolve_conflicts(Side::Master);
        assert!(p.run(&plan).clean());
        assert_eq!(read(&p.device, "a.txt").as_deref(), Some("master"));
        assert_eq!(read(&p.device, "b.txt").as_deref(), Some("changed"));
        // The version that lost is kept.
        let kept = crate::vault::list(&p.device).unwrap();
        assert_eq!(kept[0].files[0].rel, "a.txt");

        // The policies decide on their own.
        write(&p.master, "a.txt", "older", 2);
        write(&p.device, "a.txt", "newer!", 1);
        p.options.conflicts = ConflictPolicy::NewerWins;
        let plan = p.compare();
        assert_eq!(plan.items[0].op(), Op::Copy { to: Side::Master });
        assert!(plan.items[0].conflict);
        p.options.conflicts = ConflictPolicy::MasterWins;
        assert_eq!(p.compare().items[0].op(), Op::Copy { to: Side::Device });
    }

    #[test]
    fn the_same_change_on_both_sides_is_no_conflict() {
        let mut p = pair();
        write(&p.master, "a.txt", "a", 9);
        p.sync();
        write(&p.master, "a.txt", "same", 1);
        write(&p.device, "a.txt", "same", 1);
        assert_eq!(actions(&p.compare()), []);
        // Same size, another time: the content decides when asked to.
        write(&p.device, "a.txt", "same", 3);
        assert_eq!(p.compare().summary().conflicts, 1);
        p.options.by_content = true;
        assert_eq!(p.compare().summary().conflicts, 0);
    }

    #[test]
    fn a_folder_deleted_on_one_side_stays_for_a_new_file_of_the_other() {
        let mut p = pair();
        write(&p.master, "dir/a.txt", "a", 9);
        p.sync();
        fs::remove_dir_all(p.master.join("dir")).unwrap();
        write(&p.device, "dir/new.txt", "new", 1);
        let plan = p.compare();
        assert_eq!(
            actions(&plan),
            [
                ("dir", Op::CreateDir(Side::Master), Why::MasterDeleted),
                ("dir/a.txt", Op::Delete(Side::Device), Why::MasterDeleted),
                (
                    "dir/new.txt",
                    Op::Copy { to: Side::Master },
                    Why::OnlyDevice
                ),
            ]
        );
        assert!(p.run(&plan).clean());
        assert_eq!(read(&p.master, "dir/new.txt").as_deref(), Some("new"));
        assert!(!p.device.join("dir/a.txt").exists());
        assert_eq!(p.compare().summary().changes(), 0);
    }

    #[test]
    fn mirror_makes_the_computer_a_copy_of_the_master() {
        let mut p = pair();
        p.options.mode = Mode::Mirror;
        write(&p.master, "a.txt", "master", 5);
        write(&p.device, "a.txt", "device!", 1);
        write(&p.device, "extra.txt", "x", 1);
        write(&p.master, "new.txt", "n", 1);
        let report = p.sync();
        assert!(report.clean(), "{report:?}");
        assert_eq!(read(&p.device, "a.txt").as_deref(), Some("master"));
        assert_eq!(read(&p.device, "new.txt").as_deref(), Some("n"));
        assert!(!p.device.join("extra.txt").exists());
        assert_eq!(read(&p.master, "a.txt").as_deref(), Some("master"));
        assert!(!p.master.join("extra.txt").exists());
    }

    #[test]
    fn collect_adds_to_the_master_and_never_deletes() {
        let mut p = pair();
        p.options.mode = Mode::Collect;
        write(&p.master, "only-master.txt", "m", 5);
        write(&p.master, "newer-on-master.txt", "master", 1);
        write(&p.device, "newer-on-master.txt", "device!", 5);
        write(&p.master, "newer-on-device.txt", "master", 5);
        write(&p.device, "newer-on-device.txt", "device!", 1);
        write(&p.device, "new.txt", "n", 1);
        let report = p.sync();
        assert!(report.clean(), "{report:?}");
        assert_eq!(read(&p.master, "only-master.txt").as_deref(), Some("m"));
        assert_eq!(
            read(&p.master, "newer-on-master.txt").as_deref(),
            Some("master")
        );
        assert_eq!(
            read(&p.master, "newer-on-device.txt").as_deref(),
            Some("device!")
        );
        assert_eq!(read(&p.master, "new.txt").as_deref(), Some("n"));
        assert!(!p.device.join("only-master.txt").exists());
    }

    #[test]
    fn an_emptied_side_asks_again_before_deleting_the_other() {
        let mut p = pair();
        for n in 0..12 {
            write(&p.master, &format!("f{n}.txt"), "x", 5);
        }
        p.sync();
        fs::remove_dir_all(&p.device).unwrap();
        fs::create_dir_all(&p.device).unwrap();
        let plan = p.compare();
        assert_eq!(plan.summary().delete_master, 12);
        assert!(plan.mass_deletion);
        assert!(plan.warnings[0].contains("12 of the 12"));
    }

    #[test]
    fn a_file_changed_after_the_comparison_is_skipped() {
        let mut p = pair();
        write(&p.master, "a.txt", "a", 9);
        p.sync();
        write(&p.master, "a.txt", "master", 2);
        let plan = p.compare();
        write(&p.device, "a.txt", "meanwhile", 1);
        let report = p.run(&plan);
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(read(&p.device, "a.txt").as_deref(), Some("meanwhile"));
    }

    #[test]
    fn the_state_of_another_pair_is_not_used() {
        let p = pair();
        let file = p.master.join("state.json");
        p.state.save(&file).unwrap();
        assert!(State::load(&file, "m", &p.device).is_some());
        assert!(State::load(&file, "other", &p.device).is_none());
        assert!(State::load(&file, "m", &p.master).is_none());
    }
}
