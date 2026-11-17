//! Copying, moving and deleting what the user picked in the file browser.

use std::fs;
use std::path::{Path, PathBuf};

use crate::fsx::{self, Displacer, Removal, join_rel};
use crate::scan::{self, Filter};
use crate::{Progress, Report, is_cancelled};

/// Copies or moves files and folders into the folder `target`. Nothing is
/// overwritten: what exists in the target is left alone and reported. A
/// moved file is deleted only after its copy is verified.
pub fn transfer(
    sources: &[PathBuf],
    target: &Path,
    move_files: bool,
    verify: bool,
    progress: &Progress,
) -> Report {
    let mut report = Report::default();
    // Nothing is replaced, so nothing is displaced.
    let displacer = Displacer::new(Removal::Vault);
    progress.phase(if move_files { "Moving" } else { "Copying" }, 0, 0);
    let fold = |p: &Path| p.to_string_lossy().to_lowercase();

    // What to copy: (source, target, size); folders to create.
    let mut files: Vec<(PathBuf, PathBuf, u64)> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    // Sources that are folders, to remove once they're empty.
    let mut moved_dirs: Vec<PathBuf> = Vec::new();
    for source in sources {
        let name = source.to_string_lossy().into_owned();
        let Some(file_name) = source.file_name() else {
            report.skipped.push((name, "has no name".into()));
            continue;
        };
        let to = target.join(file_name);
        let meta = match fs::symlink_metadata(source) {
            Ok(meta) => meta,
            Err(e) => {
                report.failed.push((name, e.to_string()));
                continue;
            }
        };
        if fold(&to) == fold(source) {
            report
                .skipped
                .push((name, "is in this folder already".into()));
            continue;
        }
        if meta.file_type().is_symlink() {
            report.skipped.push((name, "links are left alone".into()));
        } else if meta.is_dir() {
            if to.ancestors().any(|a| fold(a) == fold(source)) {
                report
                    .skipped
                    .push((name, "a folder can't go into itself".into()));
                continue;
            }
            // The same volume and nothing in the way: a rename moves it whole.
            if move_files && fs::symlink_metadata(&to).is_err() && fs::rename(source, &to).is_ok() {
                report.copied += 1;
                continue;
            }
            match scan::scan(source, &Filter::default(), progress) {
                Ok(tree) => {
                    dirs.push(to.clone());
                    for entry in &tree.entries {
                        let (from, into) =
                            (join_rel(source, &entry.rel), join_rel(&to, &entry.rel));
                        if entry.is_dir {
                            dirs.push(into);
                        } else {
                            files.push((from, into, entry.stamp.size));
                        }
                    }
                    for (rel, error) in tree.errors {
                        report.failed.push((format!("{name}/{rel}"), error));
                    }
                    if move_files {
                        moved_dirs.push(source.clone());
                    }
                }
                Err(e) if is_cancelled(&e) => {
                    report.cancelled = true;
                    return report;
                }
                Err(e) => report.failed.push((name, format!("{e:#}"))),
            }
        } else {
            if move_files && fs::symlink_metadata(&to).is_err() && fs::rename(source, &to).is_ok() {
                report.copied += 1;
                continue;
            }
            files.push((source.clone(), to, meta.len()));
        }
    }

    progress.phase(
        if move_files { "Moving" } else { "Copying" },
        files.len() as u64,
        files.iter().map(|f| f.2).sum(),
    );
    for dir in dirs {
        if let Err(e) = fs::create_dir_all(&dir) {
            report
                .failed
                .push((dir.to_string_lossy().into_owned(), e.to_string()));
        }
    }
    for (from, to, size) in files {
        if progress.cancelled() {
            report.cancelled = true;
            break;
        }
        let name = from.to_string_lossy().into_owned();
        progress.current(&name);
        progress.item();
        if fs::symlink_metadata(&to).is_ok() {
            progress.bytes(size);
            report
                .skipped
                .push((name, "exists in the target; not overwritten".into()));
            continue;
        }
        match fsx::copy_file(
            &from,
            &to,
            target,
            verify || move_files,
            &displacer,
            progress,
        ) {
            Ok(stamp) => {
                report.copied += 1;
                report.bytes += stamp.size;
                if move_files && let Err(e) = fsx::delete(&from) {
                    report.failed.push((name, format!("{e:#}")));
                }
            }
            Err(e) if is_cancelled(&e) => {
                report.cancelled = true;
                break;
            }
            Err(e) => report.failed.push((name, format!("{e:#}"))),
        }
    }
    for dir in moved_dirs {
        crate::vault::remove_empty_dirs(&dir);
    }
    report
}

/// Removes files and folders. `root_of` names the folder whose safety
/// copies take a path; without one a path goes to the Recycle Bin instead.
pub fn remove(
    paths: &[PathBuf],
    removal: Removal,
    root_of: impl Fn(&Path) -> Option<PathBuf>,
    progress: &Progress,
) -> Report {
    let mut report = Report::default();
    progress.phase("Deleting", paths.len() as u64, 0);
    let vault = Displacer::new(Removal::Vault);
    for path in paths {
        if progress.cancelled() {
            report.cancelled = true;
            break;
        }
        progress.item();
        let name = path.to_string_lossy().into_owned();
        let result = match (removal, root_of(path)) {
            (Removal::Vault, Some(root)) => vault.remove(&root, path),
            (Removal::Vault, None) => Displacer::new(Removal::RecycleBin).remove(path, path),
            (other, _) => Displacer::new(other).remove(path, path),
        };
        match result {
            Ok(()) => report.deleted += 1,
            Err(e) => report.failed.push((name, format!("{e:#}"))),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_and_moves_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        fs::create_dir_all(a.join("album/empty")).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("album/1.jpg"), "one").unwrap();
        fs::write(a.join("note.txt"), "note").unwrap();
        fs::write(b.join("note.txt"), "other").unwrap();

        let sources = [a.join("album"), a.join("note.txt")];
        let report = transfer(&sources, &b, false, true, &Progress::default());
        assert_eq!(report.copied, 1);
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(fs::read_to_string(b.join("album/1.jpg")).unwrap(), "one");
        assert!(b.join("album/empty").is_dir());
        assert_eq!(fs::read_to_string(b.join("note.txt")).unwrap(), "other");

        // The folder exists in the target now: its files move one by one,
        // and what's there stays in the source.
        fs::write(a.join("album/2.jpg"), "two").unwrap();
        let report = transfer(&sources[..1], &b, true, true, &Progress::default());
        assert_eq!((report.copied, report.skipped.len()), (1, 1));
        assert_eq!(fs::read_to_string(b.join("album/2.jpg")).unwrap(), "two");
        assert!(a.join("album/1.jpg").exists());
        assert!(!a.join("album/2.jpg").exists());
        assert!(!a.join("album/empty").exists());

        let report = transfer(
            &[b.join("album")],
            &b.join("album"),
            true,
            true,
            &Progress::default(),
        );
        assert_eq!(report.skipped.len(), 1);

        let report = remove(
            &[b.join("album")],
            Removal::Vault,
            |_| Some(b.clone()),
            &Progress::default(),
        );
        assert_eq!(report.deleted, 1);
        assert!(!b.join("album").exists());
        assert_eq!(crate::vault::list(&b).unwrap()[0].files.len(), 2);
    }
}
