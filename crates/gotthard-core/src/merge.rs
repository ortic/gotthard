//! Merges folder trees into one: every file of the sources ends up at its
//! path in the target, once.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::fsx::{self, Displacer, Removal, Stamp, format_bytes, join_rel, numbered};
use crate::scan::{self, Filter};
use crate::{Progress, Report, is_cancelled, vault};

/// What happens to a file whose path is taken by a different file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OnConflict {
    /// Both stay: the incoming file becomes `name (2).ext`.
    #[default]
    KeepBoth,
    KeepNewer,
    /// The file already in the target stays.
    KeepTarget,
    /// The incoming file replaces it.
    Replace,
}

#[derive(Clone, Debug)]
pub struct MergeOptions {
    /// The sources' files are removed once they're safely in the target.
    pub move_files: bool,
    pub on_conflict: OnConflict,
    pub filter: Filter,
    pub verify: bool,
    /// Where the target's files go that are replaced.
    pub removal: Removal,
}

impl Default for MergeOptions {
    fn default() -> Self {
        Self {
            move_files: false,
            on_conflict: OnConflict::KeepBoth,
            filter: Filter::standard(),
            verify: true,
            removal: Removal::Vault,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeAction {
    /// The path is free.
    Copy,
    /// The same content is already there: nothing to copy.
    Identical,
    /// Copied beside the different file that's there, under a new name.
    KeepBoth,
    /// Replaces the different file that's there.
    Replace,
    /// Stays in its source: the file that's there is kept.
    KeepTarget,
}

#[derive(Clone, Debug)]
pub struct MergeItem {
    /// The index of the source folder.
    pub source: usize,
    pub rel: String,
    /// Where it goes in the target; other than `rel` for [`MergeAction::KeepBoth`].
    pub target_rel: String,
    pub stamp: Stamp,
    pub action: MergeAction,
}

#[derive(Clone, Debug, Default)]
pub struct MergePlan {
    pub sources: Vec<PathBuf>,
    pub target: PathBuf,
    pub items: Vec<MergeItem>,
    pub warnings: Vec<String>,
}

impl MergePlan {
    /// Files and bytes that will be written to the target.
    pub fn to_copy(&self) -> (usize, u64) {
        self.items
            .iter()
            .filter(|i| {
                matches!(
                    i.action,
                    MergeAction::Copy | MergeAction::KeepBoth | MergeAction::Replace
                )
            })
            .fold((0, 0), |(n, bytes), i| (n + 1, bytes + i.stamp.size))
    }

    pub fn count(&self, action: &MergeAction) -> usize {
        self.items.iter().filter(|i| &i.action == action).count()
    }
}

/// What's at a path of the target, now or once the items before are done.
struct Occupant {
    path: PathBuf,
    stamp: Stamp,
    /// The item that puts it there; none for a file the target has.
    item: Option<usize>,
    /// The target has a file at this path now.
    in_target: bool,
}

fn inside(inner: &Path, outer: &Path) -> bool {
    let fold = |p: &Path| p.to_string_lossy().to_lowercase();
    inner
        .ancestors()
        .any(|ancestor| fold(ancestor) == fold(outer))
}

/// Reads the folders and proposes what to do with every file of the
/// sources. Changes nothing.
pub fn plan(
    sources: &[PathBuf],
    target: &Path,
    options: &MergeOptions,
    progress: &Progress,
) -> Result<MergePlan> {
    for source in sources {
        if inside(source, target) || inside(target, source) {
            bail!(
                "{} and {} lie inside each other; merge folders that are apart.",
                source.display(),
                target.display()
            );
        }
    }
    let mut plan = MergePlan {
        sources: sources.to_vec(),
        target: target.to_path_buf(),
        ..MergePlan::default()
    };
    progress.phase("Reading the folders", 0, 0);
    let mut taken: HashMap<String, Occupant> = HashMap::new();
    if target.exists() {
        let tree = scan::scan(target, &options.filter, progress)?;
        for (rel, error) in &tree.errors {
            plan.warnings
                .push(format!("Unreadable in the target: {rel} ({error})"));
        }
        for entry in tree.files() {
            taken.insert(
                entry.rel.to_lowercase(),
                Occupant {
                    path: join_rel(target, &entry.rel),
                    stamp: entry.stamp,
                    item: None,
                    in_target: true,
                },
            );
        }
    }
    let trees = sources
        .iter()
        .map(|source| scan::scan(source, &options.filter, progress))
        .collect::<Result<Vec<_>>>()?;

    progress.phase("Comparing", 0, 0);
    let mut hashes: HashMap<PathBuf, Option<blake3::Hash>> = HashMap::new();
    let mut hash = |path: &Path| {
        *hashes
            .entry(path.to_path_buf())
            .or_insert_with(|| fsx::hash_file(path, Some(progress)).ok())
    };
    for (source, tree) in trees.iter().enumerate() {
        for (rel, error) in &tree.errors {
            plan.warnings.push(format!(
                "Unreadable in {}: {rel} ({error})",
                tree.root.display()
            ));
        }
        for entry in tree.files() {
            progress.check()?;
            progress.current(&entry.rel);
            let path = join_rel(&tree.root, &entry.rel);
            let index = plan.items.len();
            let key = entry.rel.to_lowercase();
            let mut item = MergeItem {
                source,
                rel: entry.rel.clone(),
                target_rel: entry.rel.clone(),
                stamp: entry.stamp,
                action: MergeAction::Copy,
            };
            let occupant = |in_target| Occupant {
                path: path.clone(),
                stamp: entry.stamp,
                item: Some(index),
                in_target,
            };
            match taken.get(&key) {
                None => {
                    taken.insert(key, occupant(false));
                }
                Some(there) => {
                    let identical = there.stamp.size == entry.stamp.size
                        && matches!((hash(&there.path), hash(&path)), (Some(a), Some(b)) if a == b);
                    let replace = match options.on_conflict {
                        _ if identical => false,
                        OnConflict::KeepBoth | OnConflict::KeepTarget => false,
                        OnConflict::KeepNewer => entry.stamp.mtime > there.stamp.mtime,
                        OnConflict::Replace => true,
                    };
                    if identical {
                        item.action = MergeAction::Identical;
                    } else if replace {
                        let in_target = there.in_target;
                        // An incoming file before this one loses its place.
                        if let Some(earlier) = there.item {
                            plan.items[earlier].action = MergeAction::KeepTarget;
                        }
                        item.action = if in_target {
                            MergeAction::Replace
                        } else {
                            MergeAction::Copy
                        };
                        taken.insert(key, occupant(in_target));
                    } else if options.on_conflict == OnConflict::KeepBoth {
                        let (dir, name) = match entry.rel.rfind('/') {
                            Some(slash) => (&entry.rel[..=slash], &entry.rel[slash + 1..]),
                            None => ("", entry.rel.as_str()),
                        };
                        let free = (2..)
                            .map(|n| format!("{dir}{}", numbered(name, n)))
                            .find(|rel| !taken.contains_key(&rel.to_lowercase()))
                            .expect("a free name");
                        taken.insert(free.to_lowercase(), occupant(false));
                        item.target_rel = free;
                        item.action = MergeAction::KeepBoth;
                    } else {
                        item.action = MergeAction::KeepTarget;
                    }
                }
            }
            plan.items.push(item);
        }
    }
    Ok(plan)
}

/// Runs the plan. With `move_files` a source file is deleted only after its
/// content is in the target: copied and verified, or compared once more
/// with the identical file that's there.
pub fn execute(plan: &MergePlan, options: &MergeOptions, progress: &Progress) -> Result<Report> {
    let (files, bytes) = plan.to_copy();
    if let Some(free) = fsx::free_space(&plan.target)
        && bytes > free
    {
        bail!(
            "Not enough space in the target: {} to copy, {} free.",
            format_bytes(bytes),
            format_bytes(free)
        );
    }
    std::fs::create_dir_all(&plan.target)?;
    progress.phase("Merging", files as u64, bytes);
    let displacer = Displacer::new(options.removal);
    let mut report = Report::default();
    for item in &plan.items {
        if progress.cancelled() {
            report.cancelled = true;
            break;
        }
        progress.current(&item.rel);
        let source = join_rel(&plan.sources[item.source], &item.rel);
        let target = join_rel(&plan.target, &item.target_rel);
        let name = source.to_string_lossy().into_owned();
        if !matches!(fsx::stat(&source), Ok(Some((stamp, false))) if stamp == item.stamp) {
            report
                .skipped
                .push((name, "changed since the comparison; compare again".into()));
            continue;
        }
        let done = match item.action {
            MergeAction::KeepTarget => {
                report
                    .skipped
                    .push((name, "stays: the target keeps its own version".into()));
                continue;
            }
            MergeAction::Identical => {
                if !options.move_files {
                    continue;
                }
                // About to delete the source: the same content, still?
                match (fsx::hash_file(&source, None), fsx::hash_file(&target, None)) {
                    (Ok(a), Ok(b)) if a == b => Ok(()),
                    _ => {
                        report.skipped.push((
                            name,
                            "stays: the target's file is no longer identical".into(),
                        ));
                        continue;
                    }
                }
            }
            MergeAction::Copy | MergeAction::KeepBoth | MergeAction::Replace => {
                progress.item();
                // Only Replace expects a file at the target.
                if item.action != MergeAction::Replace && std::fs::symlink_metadata(&target).is_ok()
                {
                    report.skipped.push((
                        name,
                        "the target got a file of this name since the comparison".into(),
                    ));
                    continue;
                }
                fsx::copy_file(
                    &source,
                    &target,
                    &plan.target,
                    options.verify || options.move_files,
                    &displacer,
                    progress,
                )
                .map(|stamp| {
                    report.copied += 1;
                    report.bytes += stamp.size;
                })
            }
        };
        match done {
            Ok(()) if options.move_files => match fsx::delete(&source) {
                Ok(()) => report.deleted += 1,
                Err(e) => report.failed.push((name, format!("{e:#}"))),
            },
            Ok(()) => {}
            Err(e) if is_cancelled(&e) => {
                report.cancelled = true;
                break;
            }
            Err(e) => report.failed.push((name, format!("{e:#}"))),
        }
    }
    if options.move_files {
        for source in &plan.sources {
            if let Ok(reader) = std::fs::read_dir(source) {
                for entry in reader.flatten() {
                    if entry.file_type().is_ok_and(|t| t.is_dir()) {
                        vault::remove_empty_dirs(&entry.path());
                    }
                }
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, SystemTime};

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

    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let (a, b, t) = (
            dir.path().join("a"),
            dir.path().join("b"),
            dir.path().join("target"),
        );
        write(&t, "docs/plan.txt", "target", 5);
        write(&t, "same.txt", "same", 5);
        write(&a, "docs/plan.txt", "from a!", 3);
        write(&a, "same.txt", "same", 9);
        write(&a, "new/a.txt", "a", 3);
        write(&b, "docs/plan.txt", "from b!!", 1);
        write(&b, "new/a.txt", "a", 1);
        (dir, a, b, t)
    }

    #[test]
    fn keeps_both_versions_and_copies_identical_files_once() {
        let (_dir, a, b, t) = setup();
        let options = MergeOptions::default();
        let plan = plan(&[a.clone(), b.clone()], &t, &options, &Progress::default()).unwrap();
        let actions: Vec<(&str, &MergeAction)> = plan
            .items
            .iter()
            .map(|i| (i.target_rel.as_str(), &i.action))
            .collect();
        assert_eq!(
            actions,
            [
                ("docs/plan (2).txt", &MergeAction::KeepBoth),
                ("new/a.txt", &MergeAction::Copy),
                ("same.txt", &MergeAction::Identical),
                ("docs/plan (3).txt", &MergeAction::KeepBoth),
                ("new/a.txt", &MergeAction::Identical),
            ]
        );
        let report = execute(&plan, &options, &Progress::default()).unwrap();
        assert!(report.clean(), "{report:?}");
        assert_eq!(read(&t, "docs/plan.txt").as_deref(), Some("target"));
        assert_eq!(read(&t, "docs/plan (2).txt").as_deref(), Some("from a!"));
        assert_eq!(read(&t, "docs/plan (3).txt").as_deref(), Some("from b!!"));
        assert_eq!(read(&a, "new/a.txt").as_deref(), Some("a"));
    }

    #[test]
    fn moving_empties_the_sources_and_the_newest_wins() {
        let (_dir, a, b, t) = setup();
        let options = MergeOptions {
            move_files: true,
            on_conflict: OnConflict::KeepNewer,
            ..MergeOptions::default()
        };
        let plan = plan(&[a.clone(), b.clone()], &t, &options, &Progress::default()).unwrap();
        assert_eq!(plan.count(&MergeAction::Replace), 1);
        // a's newer version loses to b's, which is newer still.
        assert_eq!(plan.items[0].action, MergeAction::KeepTarget);
        let report = execute(&plan, &options, &Progress::default()).unwrap();
        assert_eq!(report.skipped.len(), 1, "{report:?}");
        assert!(report.failed.is_empty());
        assert_eq!(read(&t, "docs/plan.txt").as_deref(), Some("from b!!"));
        assert_eq!(read(&t, "new/a.txt").as_deref(), Some("a"));
        // The version that lost stays in its source; the rest is gone.
        assert_eq!(read(&a, "docs/plan.txt").as_deref(), Some("from a!"));
        assert!(!a.join("new").exists());
        assert!(!a.join("same.txt").exists());
        assert_eq!(fs::read_dir(&b).unwrap().count(), 0);
        // The target's replaced version is a safety copy.
        assert_eq!(vault::list(&t).unwrap()[0].files[0].rel, "docs/plan.txt");
    }

    #[test]
    fn refuses_folders_inside_each_other() {
        let (_dir, a, _b, t) = setup();
        let options = MergeOptions::default();
        assert!(plan(&[t.join("docs")], &t, &options, &Progress::default()).is_err());
        assert!(plan(std::slice::from_ref(&t), &t, &options, &Progress::default()).is_err());
        assert!(plan(&[a], &t, &options, &Progress::default()).is_ok());
    }
}
