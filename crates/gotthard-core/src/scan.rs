//! Reads a folder tree.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::fsx::Stamp;
use crate::{META_DIR, Progress, TMP_SUFFIX};

#[derive(Clone, Debug)]
pub struct Entry {
    /// The path below the root, with `/` between the names.
    pub rel: String,
    pub is_dir: bool,
    /// Of files; folders have none.
    pub stamp: Stamp,
}

/// What a scan leaves out: patterns like `*.bak`, `Thumbs.db` or
/// `projects/*/target`, matched without regard to case against a file's
/// name and against its path below the root.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    pub exclude: Vec<String>,
}

impl Filter {
    /// What Windows and its programs strew around.
    pub fn standard() -> Self {
        Self {
            exclude: [
                "$RECYCLE.BIN",
                "System Volume Information",
                "Thumbs.db",
                "desktop.ini",
                ".DS_Store",
                "~$*",
            ]
            .map(String::from)
            .to_vec(),
        }
    }

    fn compile(&self) -> Result<GlobSet> {
        let mut set = GlobSetBuilder::new();
        for pattern in &self.exclude {
            let pattern = pattern.trim().replace('\\', "/");
            if pattern.is_empty() {
                continue;
            }
            set.add(
                GlobBuilder::new(&pattern)
                    .case_insensitive(true)
                    .literal_separator(true)
                    .build()
                    .with_context(|| format!("the exclude pattern {pattern} isn't valid"))?,
            );
        }
        Ok(set.build()?)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub root: PathBuf,
    /// Sorted by path.
    pub entries: Vec<Entry>,
    /// What couldn't be read: path and reason.
    pub errors: Vec<(String, String)>,
    /// Symbolic links and junctions, which are left alone.
    pub links: usize,
}

impl Tree {
    pub fn files(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| !e.is_dir)
    }
}

/// What two paths are the same file by: Windows' file systems don't tell
/// `Photo.JPG` from `photo.jpg`.
pub fn key(rel: &str, case_insensitive: bool) -> String {
    if case_insensitive {
        rel.to_lowercase()
    } else {
        rel.to_owned()
    }
}

/// Everything below `root` that `filter` doesn't exclude. Links aren't
/// followed, Gotthard's own folder and unfinished copies are left out.
pub fn scan(root: &Path, filter: &Filter, progress: &Progress) -> Result<Tree> {
    match fs::metadata(root) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => bail!("{} isn't a folder", root.display()),
        Err(e) => bail!("the folder {} can't be read: {e}", root.display()),
    }
    let exclude = filter.compile()?;
    let mut tree = Tree {
        root: root.to_path_buf(),
        ..Tree::default()
    };
    let mut pending = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = pending.pop() {
        progress.check()?;
        progress.current(&dir.to_string_lossy());
        let reader = match fs::read_dir(&dir) {
            Ok(reader) => reader,
            Err(e) => {
                // The root must be readable: an unreadable root isn't an
                // empty folder.
                if prefix.is_empty() {
                    bail!("the folder {} can't be read: {e}", root.display());
                }
                tree.errors.push((prefix, e.to_string()));
                continue;
            }
        };
        for entry in reader {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    tree.errors.push((prefix.clone(), e.to_string()));
                    continue;
                }
            };
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                tree.errors.push((
                    format!("{prefix}/{}", name.to_string_lossy()),
                    "the name isn't valid Unicode".into(),
                ));
                continue;
            };
            if prefix.is_empty() && name == META_DIR || name.ends_with(TMP_SUFFIX) {
                continue;
            }
            let rel = if prefix.is_empty() {
                name.to_owned()
            } else {
                format!("{prefix}/{name}")
            };
            if exclude.is_match(name) || exclude.is_match(&rel) {
                continue;
            }
            let meta = match entry.metadata() {
                Ok(meta) => meta,
                Err(e) => {
                    tree.errors.push((rel, e.to_string()));
                    continue;
                }
            };
            if meta.file_type().is_symlink() {
                tree.links += 1;
            } else if meta.is_dir() {
                pending.push((entry.path(), rel.clone()));
                tree.entries.push(Entry {
                    rel,
                    is_dir: true,
                    stamp: Stamp::default(),
                });
            } else if meta.is_file() {
                progress.item();
                tree.entries.push(Entry {
                    rel,
                    is_dir: false,
                    stamp: Stamp::of(&meta),
                });
            }
        }
    }
    tree.entries.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(tree)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_out_what_the_filter_and_gotthard_exclude() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for file in [
            "a.txt",
            "sub/b.txt",
            "sub/Thumbs.db",
            "sub/deep/c.bak",
            ".gotthard/master.json",
            "sub/.gotthard/kept.txt",
            "x.txt.gotthard-tmp",
            "build/out/o.bin",
        ] {
            let path = root.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "x").unwrap();
        }
        let mut filter = Filter::standard();
        filter.exclude.extend(["*.BAK".into(), "build/out".into()]);
        let tree = scan(root, &filter, &Progress::default()).unwrap();
        let rels: Vec<&str> = tree.entries.iter().map(|e| e.rel.as_str()).collect();
        assert_eq!(
            rels,
            [
                "a.txt",
                "build",
                "sub",
                "sub/.gotthard",
                "sub/.gotthard/kept.txt",
                "sub/b.txt",
                "sub/deep"
            ]
        );
        assert_eq!(tree.files().count(), 3);
    }

    #[test]
    fn a_missing_root_is_an_error_not_an_empty_folder() {
        let dir = tempfile::tempdir().unwrap();
        let result = scan(
            &dir.path().join("gone"),
            &Filter::default(),
            &Progress::default(),
        );
        assert!(result.is_err());
    }
}
