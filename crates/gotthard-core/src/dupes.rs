//! Finds files with the same content, whatever their names.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anyhow::Result;
use rayon::prelude::*;

use crate::fsx::{self, Displacer, Removal, join_rel};
use crate::scan::{self, Filter};
use crate::{Progress, Report};

#[derive(Clone, Debug)]
pub struct DupeOptions {
    /// Smaller files are left out; empty files always are.
    pub min_size: u64,
    pub filter: Filter,
}

impl Default for DupeOptions {
    fn default() -> Self {
        Self {
            min_size: 1,
            filter: Filter::standard(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DupeFile {
    pub path: PathBuf,
    /// The index of the searched folder the file is in.
    pub root: usize,
    pub mtime: i64,
}

/// Files with the same content.
#[derive(Clone, Debug)]
pub struct DupeGroup {
    pub size: u64,
    pub files: Vec<DupeFile>,
}

impl DupeGroup {
    /// The bytes freed when one file of the group is kept.
    pub fn wasted(&self) -> u64 {
        self.size * (self.files.len() as u64 - 1)
    }
}

#[derive(Clone, Debug, Default)]
pub struct DupeResult {
    pub roots: Vec<PathBuf>,
    /// The groups that waste most first.
    pub groups: Vec<DupeGroup>,
    pub scanned: usize,
    pub errors: Vec<(String, String)>,
}

impl DupeResult {
    pub fn wasted(&self) -> u64 {
        self.groups.iter().map(DupeGroup::wasted).sum()
    }
}

/// As much of a file as tells most different files of one size apart.
const PREFIX: usize = 64 * 1024;

struct Candidate {
    file: DupeFile,
    size: u64,
}

pub fn find(roots: &[PathBuf], options: &DupeOptions, progress: &Progress) -> Result<DupeResult> {
    let mut result = DupeResult {
        roots: roots.to_vec(),
        ..DupeResult::default()
    };
    progress.phase("Reading the folders", 0, 0);
    let mut seen = HashSet::new();
    let mut by_size: HashMap<u64, Vec<Candidate>> = HashMap::new();
    for (index, root) in roots.iter().enumerate() {
        let tree = scan::scan(root, &options.filter, progress)?;
        result.errors.extend(tree.errors.iter().cloned());
        for entry in tree.files() {
            let path = join_rel(root, &entry.rel);
            // A folder given twice, or inside another one.
            if !seen.insert(path.clone()) {
                continue;
            }
            result.scanned += 1;
            let size = entry.stamp.size;
            if size >= options.min_size.max(1) {
                by_size.entry(size).or_default().push(Candidate {
                    file: DupeFile {
                        path,
                        root: index,
                        mtime: entry.stamp.mtime,
                    },
                    size,
                });
            }
        }
    }
    let candidates: Vec<Candidate> = by_size
        .into_values()
        .filter(|files| files.len() > 1)
        .flatten()
        .collect();

    // The same size: compare the beginnings, then what still matches whole.
    progress.phase("Comparing beginnings", candidates.len() as u64, 0);
    let errors = std::sync::Mutex::new(Vec::new());
    let fail = |c: &Candidate, e: anyhow::Error| {
        errors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((c.file.path.to_string_lossy().into_owned(), format!("{e:#}")));
    };
    let regroup = |hashed: Vec<(blake3::Hash, Candidate)>| -> Vec<Candidate> {
        let mut groups: HashMap<(u64, blake3::Hash), Vec<Candidate>> = HashMap::new();
        for (hash, c) in hashed {
            groups.entry((c.size, hash)).or_default().push(c);
        }
        groups
            .into_values()
            .filter(|g| g.len() > 1)
            .flatten()
            .collect()
    };
    let hashed: Vec<(blake3::Hash, Candidate)> = candidates
        .into_par_iter()
        .filter_map(|c| {
            if progress.cancelled() {
                return None;
            }
            progress.item();
            match fsx::hash_prefix(&c.file.path, PREFIX) {
                Ok(hash) => Some((hash, c)),
                Err(e) => {
                    fail(&c, e);
                    None
                }
            }
        })
        .collect();
    progress.check()?;
    let candidates = regroup(hashed);

    let bytes = candidates.iter().map(|c| c.size).sum();
    progress.phase("Comparing content", candidates.len() as u64, bytes);
    let hashed: Vec<(blake3::Hash, Candidate)> = candidates
        .into_par_iter()
        .filter_map(|c| {
            progress.item();
            match fsx::hash_file(&c.file.path, Some(progress)) {
                Ok(hash) => Some((hash, c)),
                Err(e) => {
                    if !crate::is_cancelled(&e) {
                        fail(&c, e);
                    }
                    None
                }
            }
        })
        .collect();
    progress.check()?;

    let mut groups: HashMap<blake3::Hash, DupeGroup> = HashMap::new();
    for (hash, c) in hashed {
        groups
            .entry(hash)
            .or_insert_with(|| DupeGroup {
                size: c.size,
                files: Vec::new(),
            })
            .files
            .push(c.file);
    }
    result.groups = groups
        .into_values()
        .filter_map(|mut group| {
            // Hard links are one file under several names: deleting one
            // frees nothing.
            let mut handles: Vec<same_file::Handle> = Vec::new();
            group.files.retain(|file| {
                let Ok(handle) = same_file::Handle::from_path(&file.path) else {
                    return true;
                };
                if handles.contains(&handle) {
                    return false;
                }
                handles.push(handle);
                true
            });
            group.files.sort_by(|a, b| a.path.cmp(&b.path));
            (group.files.len() > 1).then_some(group)
        })
        .collect();
    result.groups.sort_by(|a, b| {
        b.wasted()
            .cmp(&a.wasted())
            .then_with(|| a.files[0].path.cmp(&b.files[0].path))
    });
    result
        .errors
        .extend(errors.into_inner().unwrap_or_else(|e| e.into_inner()));
    Ok(result)
}

/// Which file of a group stays when the others are marked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    Oldest,
    Newest,
    /// The one in the folder listed first, e.g. the master.
    FirstFolder,
    ShortestPath,
}

/// The index of the file of `group` that `keep` keeps.
pub fn keeper(group: &DupeGroup, keep: Keep) -> usize {
    let indices = 0..group.files.len();
    let file = |i: &usize| &group.files[*i];
    match keep {
        Keep::Oldest => indices.min_by_key(|i| file(i).mtime),
        Keep::Newest => indices.max_by_key(|i| file(i).mtime),
        Keep::FirstFolder => indices.min_by_key(|i| file(i).root),
        Keep::ShortestPath => indices.min_by_key(|i| file(i).path.as_os_str().len()),
    }
    .unwrap_or(0)
}

/// Removes the marked files (group index, file index). At least one file of
/// every group stays, and a file that changed since the search is skipped.
pub fn remove(
    result: &DupeResult,
    marked: &[(usize, usize)],
    removal: Removal,
    progress: &Progress,
) -> Report {
    let displacer = Displacer::new(removal);
    let mut report = Report::default();
    progress.phase("Removing duplicates", marked.len() as u64, 0);
    let mut removed: HashSet<(usize, usize)> = HashSet::new();
    for &(g, f) in marked {
        if progress.cancelled() {
            report.cancelled = true;
            break;
        }
        progress.item();
        let Some(group) = result.groups.get(g) else {
            continue;
        };
        let Some(file) = group.files.get(f) else {
            continue;
        };
        let name = file.path.to_string_lossy().into_owned();
        progress.current(&name);
        let unchanged = |file: &DupeFile| {
            matches!(fsx::stat(&file.path), Ok(Some((stamp, false)))
                if stamp.size == group.size && stamp.mtime == file.mtime)
        };
        // A copy that's still there as it was found.
        let survivor = group
            .files
            .iter()
            .enumerate()
            .any(|(i, other)| i != f && !removed.contains(&(g, i)) && unchanged(other));
        if !survivor {
            report
                .skipped
                .push((name, "the last copy of this file stays".into()));
            continue;
        }
        if !unchanged(file) {
            report
                .skipped
                .push((name, "changed since the search; search again".into()));
            continue;
        }
        match displacer.remove(&result.roots[file.root], &file.path) {
            Ok(()) => {
                removed.insert((g, f));
                report.deleted += 1;
                report.bytes += group.size;
            }
            Err(e) => report.failed.push((name, format!("{e:#}"))),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_the_same_content_under_other_names() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::create_dir_all(a.join("sub")).unwrap();
        fs::create_dir_all(&b).unwrap();
        let big = "x".repeat(200_000);
        fs::write(a.join("one.bin"), &big).unwrap();
        fs::write(a.join("sub/copy of one.bin"), &big).unwrap();
        fs::write(b.join("renamed.dat"), &big).unwrap();
        // The same size and beginning, another end.
        fs::write(b.join("other.bin"), format!("{}y", &big[1..])).unwrap();
        fs::write(a.join("small.txt"), "hi").unwrap();
        fs::write(b.join("small2.txt"), "hi").unwrap();
        fs::write(a.join("empty1"), "").unwrap();
        fs::write(a.join("empty2"), "").unwrap();

        let roots = [a.clone(), b.clone(), a.clone()];
        let result = find(&roots, &DupeOptions::default(), &Progress::default()).unwrap();
        assert_eq!(result.scanned, 8);
        assert_eq!(result.groups.len(), 2);
        assert_eq!(result.groups[0].files.len(), 3);
        assert_eq!(result.groups[0].wasted(), 400_000);
        assert_eq!(result.groups[1].size, 2);
        assert_eq!(keeper(&result.groups[0], Keep::ShortestPath), 0);

        // The last copy stays, whatever is marked.
        let report = remove(
            &result,
            &[(0, 0), (0, 1), (0, 2)],
            Removal::Vault,
            &Progress::default(),
        );
        assert_eq!(report.deleted, 2);
        assert_eq!(report.skipped.len(), 1);
        assert!(b.join("renamed.dat").exists());
        assert!(!a.join("one.bin").exists());
        assert_eq!(crate::vault::list(&a).unwrap()[0].files.len(), 2);
    }
}
