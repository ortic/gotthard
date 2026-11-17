//! Finds files a disk has damaged silently.
//!
//! The manifest (`.gotthard/manifest.json`) records every file's size, time
//! and content hash. A file whose size and time are as recorded but whose
//! content hashes differently wasn't changed by a program: the disk
//! returned other bytes than were written.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::fsx::{self, Stamp, join_rel};
use crate::scan::{self, Filter};
use crate::{META_DIR, Progress, is_cancelled};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub entries: BTreeMap<String, Record>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub stamp: Stamp,
    pub hash: String,
}

#[derive(Clone, Debug, Default)]
pub struct IntegrityReport {
    /// Files whose content is as recorded.
    pub intact: usize,
    /// Recorded for the first time.
    pub new: Vec<String>,
    /// Changed since the last check (size or time differ); recorded anew.
    pub modified: Vec<String>,
    /// Recorded, but gone.
    pub missing: Vec<String>,
    /// Same size and time as recorded, other content.
    pub damaged: Vec<String>,
    pub errors: Vec<(String, String)>,
    pub cancelled: bool,
}

pub fn manifest_path(root: &Path) -> PathBuf {
    root.join(META_DIR).join("manifest.json")
}

pub fn load(root: &Path) -> Manifest {
    fs::read_to_string(manifest_path(root))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(root: &Path, manifest: &Manifest) -> Result<()> {
    let path = manifest_path(root);
    fs::create_dir_all(path.parent().expect("in a folder"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec(manifest)?)?;
    fs::rename(&tmp, &path).with_context(|| format!("can't write {}", path.display()))
}

/// Reads every file of `root`, compares it with the manifest and records
/// what's new or changed. Damaged files keep their record, so they're
/// reported until they're replaced by a good copy or [`accept`]ed.
pub fn check(root: &Path, filter: &Filter, progress: &Progress) -> Result<IntegrityReport> {
    progress.phase("Reading the folder", 0, 0);
    let tree = scan::scan(root, filter, progress)?;
    let mut manifest = load(root);
    let mut report = IntegrityReport::default();
    let total: u64 = tree.files().map(|e| e.stamp.size).sum();
    progress.phase("Checking files", tree.files().count() as u64, total);

    let mut seen = Vec::new();
    for entry in tree.files() {
        if progress.cancelled() {
            report.cancelled = true;
            break;
        }
        progress.current(&entry.rel);
        progress.item();
        seen.push(entry.rel.clone());
        let hash = match fsx::hash_file(&join_rel(root, &entry.rel), Some(progress)) {
            Ok(hash) => hash.to_hex().to_string(),
            Err(e) if is_cancelled(&e) => {
                seen.pop();
                report.cancelled = true;
                break;
            }
            Err(e) => {
                report.errors.push((entry.rel.clone(), format!("{e:#}")));
                continue;
            }
        };
        let record = Record {
            stamp: entry.stamp,
            hash,
        };
        match manifest.entries.get(&entry.rel) {
            None => {
                report.new.push(entry.rel.clone());
                manifest.entries.insert(entry.rel.clone(), record);
            }
            Some(known) if known.stamp != entry.stamp => {
                report.modified.push(entry.rel.clone());
                manifest.entries.insert(entry.rel.clone(), record);
            }
            Some(known) if known.hash != record.hash => report.damaged.push(entry.rel.clone()),
            Some(_) => report.intact += 1,
        }
    }
    if !report.cancelled {
        let unreadable = |rel: &str| tree.errors.iter().any(|(dir, _)| rel.starts_with(dir));
        let gone: Vec<String> = manifest
            .entries
            .keys()
            .filter(|rel| seen.binary_search(rel).is_err() && !unreadable(rel))
            .cloned()
            .collect();
        for rel in gone {
            manifest.entries.remove(&rel);
            report.missing.push(rel);
        }
    }
    report.errors.extend(tree.errors);
    save(root, &manifest)?;
    Ok(report)
}

/// Records a damaged file's content as it is now.
pub fn accept(root: &Path, rel: &str) -> Result<()> {
    let path = join_rel(root, rel);
    let mut manifest = load(root);
    let stamp = fsx::stat(&path)?
        .map(|(stamp, _)| stamp)
        .with_context(|| format!("{rel} is gone"))?;
    manifest.entries.insert(
        rel.to_owned(),
        Record {
            stamp,
            hash: fsx::hash_file(&path, None)?.to_hex().to_string(),
        },
    );
    save(root, &manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    #[test]
    fn tells_damage_from_change() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for name in ["a.txt", "b.txt", "c.txt"] {
            fs::write(root.join(name), "content").unwrap();
        }
        let check = || check(root, &Filter::default(), &Progress::default()).unwrap();
        let first = check();
        assert_eq!(first.new.len(), 3);
        assert_eq!(check().intact, 3);

        // Other bytes with the time kept: what a failing disk does.
        let time = fs::metadata(root.join("a.txt"))
            .unwrap()
            .modified()
            .unwrap();
        fs::write(root.join("a.txt"), "CONTENT").unwrap();
        fs::File::options()
            .write(true)
            .open(root.join("a.txt"))
            .unwrap()
            .set_modified(time)
            .unwrap();
        // A change by a program moves the time.
        fs::write(root.join("b.txt"), "edited").unwrap();
        fs::File::options()
            .write(true)
            .open(root.join("b.txt"))
            .unwrap()
            .set_modified(SystemTime::now() + std::time::Duration::from_secs(60))
            .unwrap();
        fs::remove_file(root.join("c.txt")).unwrap();

        let report = check();
        assert_eq!(report.damaged, ["a.txt"]);
        assert_eq!(report.modified, ["b.txt"]);
        assert_eq!(report.missing, ["c.txt"]);
        // Still reported until accepted.
        assert_eq!(check().damaged, ["a.txt"]);
        accept(root, "a.txt").unwrap();
        let report = check();
        assert!(report.damaged.is_empty());
        assert_eq!(report.intact, 2);
    }
}
