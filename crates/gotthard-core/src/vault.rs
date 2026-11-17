//! The safety copies of a folder: what synchronizations, merges and
//! deletions replaced or removed, in `.gotthard/versions/<time of the run>/`
//! with the files' paths below it.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::fsx::{self, Displacer, SESSION_FORMAT, join_rel};
use crate::scan::{self, Filter};
use crate::{META_DIR, Progress};

pub fn versions_dir(root: &Path) -> PathBuf {
    root.join(META_DIR).join("versions")
}

/// The safety copies of one run.
#[derive(Clone, Debug)]
pub struct Session {
    /// The time of the run, [`SESSION_FORMAT`].
    pub name: String,
    pub files: Vec<VaultFile>,
    pub bytes: u64,
}

#[derive(Clone, Debug)]
pub struct VaultFile {
    pub rel: String,
    pub size: u64,
}

impl Session {
    pub fn time(&self) -> Option<chrono::NaiveDateTime> {
        chrono::NaiveDateTime::parse_from_str(&self.name, SESSION_FORMAT).ok()
    }
}

/// The runs that left safety copies in `root`, the latest first.
pub fn list(root: &Path) -> Result<Vec<Session>> {
    let dir = versions_dir(root);
    let Ok(reader) = fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut sessions = Vec::new();
    for entry in reader.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let tree = scan::scan(&entry.path(), &Filter::default(), &Progress::default())?;
        let files: Vec<VaultFile> = tree
            .files()
            .map(|e| VaultFile {
                rel: e.rel.clone(),
                size: e.stamp.size,
            })
            .collect();
        sessions.push(Session {
            bytes: files.iter().map(|f| f.size).sum(),
            name,
            files,
        });
    }
    sessions.sort_by(|a, b| b.name.cmp(&a.name));
    Ok(sessions)
}

/// Puts a safety copy back where the file was. A file that's there now
/// becomes a safety copy itself. Returns the restored file's path.
pub fn restore(root: &Path, session: &str, rel: &str, displacer: &Displacer) -> Result<PathBuf> {
    let source = join_rel(&versions_dir(root).join(session), rel);
    if !source.is_file() {
        bail!("the safety copy {rel} of {session} is gone");
    }
    let target = join_rel(root, rel);
    if fs::symlink_metadata(&target).is_ok() {
        displacer.remove(root, &target)?;
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fsx::move_path(&source, &target).with_context(|| format!("can't restore {rel}"))?;
    remove_empty_dirs(&versions_dir(root).join(session));
    Ok(target)
}

/// Deletes the safety copies of one run for good.
pub fn delete_session(root: &Path, session: &str) -> Result<()> {
    if session.is_empty() || session.contains(['/', '\\']) || session.starts_with('.') {
        bail!("{session} isn't a run");
    }
    let dir = versions_dir(root).join(session);
    fs::remove_dir_all(&dir).with_context(|| format!("can't delete {}", dir.display()))
}

/// Deletes for good the safety copies of runs more than `days` days ago.
/// Returns the runs and bytes deleted.
pub fn purge_older(root: &Path, days: u32) -> Result<(usize, u64)> {
    let limit = chrono::Local::now().naive_local() - chrono::Duration::days(days.into());
    let (mut runs, mut bytes) = (0, 0);
    for session in list(root)? {
        if session.time().is_some_and(|time| time < limit) {
            delete_session(root, &session.name)?;
            runs += 1;
            bytes += session.bytes;
        }
    }
    Ok((runs, bytes))
}

/// Removes `dir` and the folders in it as far as they hold no files.
pub fn remove_empty_dirs(dir: &Path) -> bool {
    let Ok(reader) = fs::read_dir(dir) else {
        return false;
    };
    let mut empty = true;
    for entry in reader.flatten() {
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        if !(is_dir && remove_empty_dirs(&entry.path())) {
            empty = false;
        }
    }
    empty && fs::remove_dir(dir).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsx::Removal;

    #[test]
    fn restores_and_purges() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs/a.txt"), "first").unwrap();
        let old = Displacer {
            removal: Removal::Vault,
            session: "2020-01-01_10-00-00".into(),
        };
        old.remove(root, &root.join("docs/a.txt")).unwrap();
        fs::write(root.join("docs/a.txt"), "second").unwrap();

        let sessions = list(root).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].files[0].rel, "docs/a.txt");
        assert_eq!(sessions[0].bytes, 5);

        let now = Displacer::new(Removal::Vault);
        restore(root, &old.session, "docs/a.txt", &now).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("docs/a.txt")).unwrap(),
            "first"
        );
        // The run it came from is empty and gone; what it replaced is kept.
        let sessions = list(root).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, now.session);

        assert_eq!(purge_older(root, 30).unwrap().0, 0);
        old.remove(root, &root.join("docs/a.txt")).unwrap();
        assert_eq!(purge_older(root, 30).unwrap(), (1, 5));
        assert!(delete_session(root, "../..").is_err());
    }
}
