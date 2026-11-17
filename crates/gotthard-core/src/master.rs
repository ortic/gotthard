//! The master copy: a folder, usually on an external disk, marked with an
//! identity of its own in `.gotthard/master.json`.
//!
//! The marker is what a synchronization checks before it trusts a path: a
//! disk that got another drive letter is found again, and a folder that
//! merely has the master's path (another disk at its letter, an empty mount
//! point) is refused instead of being taken for a master with every file
//! deleted.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::META_DIR;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    pub id: String,
    pub name: String,
    /// When the folder became a master, `YYYY-MM-DD`.
    pub created: String,
}

fn marker_path(root: &Path) -> PathBuf {
    root.join(META_DIR).join("master.json")
}

/// The marker of the master in `root`, if it is one.
pub fn read(root: &Path) -> Option<Marker> {
    serde_json::from_str(&fs::read_to_string(marker_path(root)).ok()?).ok()
}

/// Makes `root` a master, or returns the marker it already has.
pub fn create(root: &Path, name: &str) -> Result<Marker> {
    if !root.is_dir() {
        bail!("{} isn't a folder", root.display());
    }
    if let Some(marker) = read(root) {
        return Ok(marker);
    }
    let now = chrono::Local::now();
    // Unique enough for the disks of one household: time, process and path.
    let seed = format!(
        "{}|{}|{}",
        now.timestamp_nanos_opt().unwrap_or_default(),
        std::process::id(),
        root.display()
    );
    let marker = Marker {
        id: blake3::hash(seed.as_bytes()).to_hex()[..16].to_owned(),
        name: name.to_owned(),
        created: now.format("%Y-%m-%d").to_string(),
    };
    let path = marker_path(root);
    fs::create_dir_all(path.parent().expect("in a folder"))?;
    fs::write(&path, serde_json::to_string_pretty(&marker)?)
        .with_context(|| format!("can't write {}", path.display()))?;
    hide(&root.join(META_DIR));
    Ok(marker)
}

/// Gotthard's folder is hidden in Explorer.
fn hide(_dir: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashing up.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("attrib")
            .creation_flags(CREATE_NO_WINDOW)
            .arg("+h")
            .arg(_dir)
            .status();
    }
}

/// Where the master `id` is now: at `last` where it was, or at the same
/// path of another drive letter.
pub fn locate(last: &Path, id: &str) -> Option<PathBuf> {
    let is_it = |path: &Path| read(path).is_some_and(|marker| marker.id == id);
    if is_it(last) {
        return Some(last.to_path_buf());
    }
    other_drives(last).into_iter().find(|path| is_it(path))
}

/// `E:\Master` on the other drive letters.
fn other_drives(path: &Path) -> Vec<PathBuf> {
    let text = path.to_string_lossy();
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic() => ('A'..='Z')
            .filter(|drive| !drive.eq_ignore_ascii_case(&letter))
            .map(|drive| PathBuf::from(format!("{drive}{}", &text[1..])))
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_master_keeps_its_identity() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(dir.path()), None);
        let marker = create(dir.path(), "Photos").unwrap();
        assert_eq!(marker.id.len(), 16);
        assert_eq!(create(dir.path(), "Other").unwrap(), marker);
        assert_eq!(locate(dir.path(), &marker.id).as_deref(), Some(dir.path()));
        assert_eq!(locate(dir.path(), "another"), None);
    }

    #[test]
    fn looks_on_the_other_drive_letters() {
        let drives = other_drives(Path::new(r"E:\Master\Photos"));
        assert_eq!(drives.len(), 25);
        assert_eq!(drives[0], Path::new(r"A:\Master\Photos"));
        assert!(!drives.contains(&PathBuf::from(r"E:\Master\Photos")));
        assert!(other_drives(Path::new("/mnt/master")).is_empty());
    }
}
