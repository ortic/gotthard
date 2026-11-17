//! The file operations everything else is built from.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{META_DIR, Progress, TMP_SUFFIX};

/// Size and time of last change, what files are compared by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub size: u64,
    /// Milliseconds since 1970 (UTC).
    pub mtime: i64,
}

/// FAT and exFAT store times in steps of two seconds.
pub const TIME_TOLERANCE_MS: i64 = 2000;

impl Stamp {
    pub fn of(meta: &fs::Metadata) -> Stamp {
        Stamp {
            size: meta.len(),
            mtime: meta.modified().map(to_millis).unwrap_or(0),
        }
    }

    /// The same size and, within what file systems round to, the same time.
    pub fn same(&self, other: &Stamp) -> bool {
        self.size == other.size && (self.mtime - other.mtime).abs() <= TIME_TOLERANCE_MS
    }

    pub fn time(&self) -> SystemTime {
        from_millis(self.mtime)
    }
}

pub fn to_millis(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(e) => -(e.duration().as_millis() as i64),
    }
}

pub fn from_millis(ms: i64) -> SystemTime {
    if ms >= 0 {
        UNIX_EPOCH + Duration::from_millis(ms as u64)
    } else {
        UNIX_EPOCH - Duration::from_millis(ms.unsigned_abs())
    }
}

/// The stamp of what's at `path` and whether it's a folder; `None` when
/// nothing is there. Links aren't followed.
pub fn stat(path: &Path) -> io::Result<Option<(Stamp, bool)>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(Some((Stamp::of(&meta), meta.is_dir()))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// `root` joined with a relative path that uses `/`.
pub fn join_rel(root: &Path, rel: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    path.extend(rel.split('/').filter(|part| !part.is_empty()));
    path
}

/// `path` below `root` with `/` between the names.
pub fn rel_of(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Option<Vec<&str>> = rel.iter().map(|part| part.to_str()).collect();
    Some(parts?.join("/"))
}

const CHUNK: usize = 1 << 20;

/// The BLAKE3 hash of a file's content.
pub fn hash_file(path: &Path, progress: Option<&Progress>) -> Result<blake3::Hash> {
    let mut file = File::open(path).with_context(|| format!("can't read {}", path.display()))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; CHUNK];
    loop {
        if let Some(progress) = progress {
            progress.check()?;
        }
        let n = file
            .read(&mut buffer)
            .with_context(|| format!("can't read {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        if let Some(progress) = progress {
            progress.bytes(n as u64);
        }
    }
    Ok(hasher.finalize())
}

/// The hash of the first `len` bytes: tells most different files of one
/// size apart without reading them whole.
pub fn hash_prefix(path: &Path, len: usize) -> Result<blake3::Hash> {
    let file = File::open(path).with_context(|| format!("can't read {}", path.display()))?;
    let mut buffer = Vec::with_capacity(len);
    file.take(len as u64)
        .read_to_end(&mut buffer)
        .with_context(|| format!("can't read {}", path.display()))?;
    Ok(blake3::hash(&buffer))
}

/// Where a file goes that a copy replaces or the user deletes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Removal {
    /// Into the folder's safety copies (`.gotthard/versions`), from where
    /// it can be restored.
    #[default]
    Vault,
    /// Into the Recycle Bin.
    RecycleBin,
    /// Gone.
    Permanent,
}

/// Removes files the way one run of an operation does: with [`Removal::Vault`]
/// everything of the run goes below one folder named after its start.
#[derive(Clone, Debug)]
pub struct Displacer {
    pub removal: Removal,
    pub session: String,
}

/// How the safety copies of a run are named; sorts by time.
pub const SESSION_FORMAT: &str = "%Y-%m-%d_%H-%M-%S";

impl Displacer {
    pub fn new(removal: Removal) -> Self {
        Self {
            removal,
            session: chrono::Local::now().format(SESSION_FORMAT).to_string(),
        }
    }

    /// Removes the file or folder at `path`, which lies in `root`.
    pub fn remove(&self, root: &Path, path: &Path) -> Result<()> {
        match self.removal {
            Removal::Vault => {
                let rel = rel_of(root, path)
                    .filter(|rel| !rel.is_empty())
                    .with_context(|| {
                        format!("{} isn't inside {}", path.display(), root.display())
                    })?;
                let target = unique_path(&join_rel(
                    &root.join(META_DIR).join("versions").join(&self.session),
                    &rel,
                ));
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)
                        .with_context(|| format!("can't create {}", parent.display()))?;
                }
                move_path(path, &target)
                    .with_context(|| format!("can't move {} to the safety copies", path.display()))
            }
            Removal::RecycleBin => trash::delete(path)
                .with_context(|| format!("can't move {} to the Recycle Bin", path.display())),
            Removal::Permanent => delete(path),
        }
    }
}

/// Deletes a file or a folder with everything in it, also when it's
/// read-only.
pub fn delete(path: &Path) -> Result<()> {
    let meta =
        fs::symlink_metadata(path).with_context(|| format!("can't delete {}", path.display()))?;
    let result = if meta.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path).or_else(|e| {
            if e.kind() != io::ErrorKind::PermissionDenied {
                return Err(e);
            }
            clear_readonly(path, &meta)?;
            fs::remove_file(path)
        })
    };
    result.with_context(|| format!("can't delete {}", path.display()))
}

#[allow(clippy::permissions_set_readonly_false)]
fn clear_readonly(path: &Path, meta: &fs::Metadata) -> io::Result<()> {
    let mut permissions = meta.permissions();
    if permissions.readonly() {
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

/// Renames, or copies and deletes where renaming can't be done (another
/// volume).
pub fn move_path(from: &Path, to: &Path) -> Result<()> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    let meta = fs::symlink_metadata(from)?;
    if meta.is_dir() {
        copy_dir_plain(from, to)?;
        fs::remove_dir_all(from)?;
    } else {
        fs::copy(from, to)?;
        delete(from)?;
    }
    Ok(())
}

fn copy_dir_plain(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_plain(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// `path` when nothing is there, else the first free of `name (2).ext`,
/// `name (3).ext`, …
pub fn unique_path(path: &Path) -> PathBuf {
    if fs::symlink_metadata(path).is_err() {
        return path.to_path_buf();
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    (2..)
        .map(|n| path.with_file_name(numbered(&name, n)))
        .find(|candidate| fs::symlink_metadata(candidate).is_err())
        .expect("a free name")
}

/// `name (n).ext`.
pub fn numbered(name: &str, n: u32) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} ({n}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} ({n})"),
    }
}

/// Copies `source` to `target`, which lies in `target_root`.
///
/// The content is written to a temporary file beside the target; with
/// `verify` it's read back and compared with what was read from the source.
/// Only then is the file that was at `target` removed through `displacer`
/// and the copy renamed into place, with the source's time. Returns the
/// target's stamp.
pub fn copy_file(
    source: &Path,
    target: &Path,
    target_root: &Path,
    verify: bool,
    displacer: &Displacer,
    progress: &Progress,
) -> Result<Stamp> {
    let parent = target
        .parent()
        .with_context(|| format!("{} has no folder", target.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("can't create {}", parent.display()))?;
    let name = target
        .file_name()
        .with_context(|| format!("{} has no name", target.display()))?;
    let mut tmp_name = name.to_os_string();
    tmp_name.push(TMP_SUFFIX);
    let tmp = parent.join(tmp_name);

    let result = (|| -> Result<()> {
        let mut input =
            File::open(source).with_context(|| format!("can't read {}", source.display()))?;
        let meta = input.metadata()?;
        let mut output =
            File::create(&tmp).with_context(|| format!("can't write {}", tmp.display()))?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0u8; CHUNK];
        loop {
            progress.check()?;
            let n = input
                .read(&mut buffer)
                .with_context(|| format!("can't read {}", source.display()))?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            output
                .write_all(&buffer[..n])
                .with_context(|| format!("can't write {}", tmp.display()))?;
            progress.bytes(n as u64);
        }
        output
            .sync_all()
            .with_context(|| format!("can't write {}", tmp.display()))?;
        if let Ok(modified) = meta.modified() {
            output.set_modified(modified)?;
        }
        drop(output);
        if verify && hash_file(&tmp, None)? != hasher.finalize() {
            bail!(
                "the copy of {} doesn't match the original (verification failed)",
                source.display()
            );
        }
        if fs::symlink_metadata(target).is_ok() {
            match displacer.removal {
                // Renaming over the old file leaves no moment without one.
                Removal::Permanent => {
                    if let Ok(old) = fs::symlink_metadata(target) {
                        let _ = clear_readonly(target, &old);
                    }
                }
                _ => displacer.remove(target_root, target)?,
            }
        }
        fs::rename(&tmp, target).with_context(|| format!("can't write {}", target.display()))?;
        if meta.permissions().readonly() {
            let _ = fs::set_permissions(target, meta.permissions());
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(stat(target)?.map(|(stamp, _)| stamp).unwrap_or_default())
}

/// The bytes free for the user on the volume of `path` (or of the nearest
/// folder above it that exists).
pub fn free_space(path: &Path) -> Option<u64> {
    existing_ancestor(path).and_then(|p| fs4::available_space(p).ok())
}

/// The size of the volume of `path`.
pub fn total_space(path: &Path) -> Option<u64> {
    existing_ancestor(path).and_then(|p| fs4::total_space(p).ok())
}

fn existing_ancestor(path: &Path) -> Option<&Path> {
    path.ancestors().find(|p| p.exists())
}

/// `1.5 GB`.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `2026-11-17 14:05` in local time.
pub fn format_time(ms: i64) -> String {
    let time: chrono::DateTime<chrono::Local> = from_millis(ms).into();
    time.format("%Y-%m-%d %H:%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_replaces_through_the_vault_and_keeps_the_time() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("target");
        let source = dir.path().join("a.txt");
        fs::write(&source, "new").unwrap();
        let old = SystemTime::now() - Duration::from_secs(86_400);
        File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let target = root.join("sub/a.txt");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "old").unwrap();

        let displacer = Displacer::new(Removal::Vault);
        let stamp = copy_file(
            &source,
            &target,
            &root,
            true,
            &displacer,
            &Progress::default(),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        assert!(stamp.same(&stat(&source).unwrap().unwrap().0));
        let kept = root
            .join(META_DIR)
            .join("versions")
            .join(&displacer.session)
            .join("sub/a.txt");
        assert_eq!(fs::read_to_string(kept).unwrap(), "old");
        // No temporary file is left.
        assert_eq!(fs::read_dir(root.join("sub")).unwrap().count(), 1);
    }

    #[test]
    fn a_cancelled_copy_leaves_the_target_alone() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("a.txt");
        fs::write(&source, "new").unwrap();
        let target = dir.path().join("b.txt");
        fs::write(&target, "old").unwrap();
        let progress = Progress::default();
        progress.cancel();
        let result = copy_file(
            &source,
            &target,
            dir.path(),
            true,
            &Displacer::new(Removal::Vault),
            &progress,
        );
        assert!(crate::is_cancelled(&result.unwrap_err()));
        assert_eq!(fs::read_to_string(&target).unwrap(), "old");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn names() {
        assert_eq!(numbered("a.tar.gz", 2), "a.tar (2).gz");
        assert_eq!(numbered(".profile", 3), ".profile (3)");
        assert_eq!(numbered("notes", 2), "notes (2)");
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(300 * 1024 * 1024), "300 MB");
        assert_eq!(
            join_rel(Path::new("r"), "a/b"),
            Path::new("r").join("a").join("b")
        );
    }
}
