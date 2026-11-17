//! Where Gotthard keeps its settings, and what's in them.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use gotthard_core::fsx::Removal;
use gotthard_core::scan::Filter;
use gotthard_core::sync::{ConflictPolicy, Mode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl Paths {
    /// The folders of this user: `%APPDATA%\Gotthard` on Windows.
    pub fn detect() -> Result<Self> {
        let dirs = ProjectDirs::from("com", "ortic", "Gotthard")
            .context("this user has no home folder")?;
        Ok(Self {
            config_dir: dirs.config_dir().to_path_buf(),
            data_dir: dirs.data_local_dir().to_path_buf(),
        })
    }

    /// Everything below one folder: tests, portable use.
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            config_dir: dir.join("config"),
            data_dir: dir.join("data"),
        }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("settings.toml")
    }

    pub fn log_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    /// What both sides of a pair looked like after its last run.
    pub fn state_file(&self, pair_id: &str) -> PathBuf {
        self.data_dir.join("state").join(format!("{pair_id}.json"))
    }

    /// The last mass rename, to undo it.
    pub fn rename_journal(&self) -> PathBuf {
        self.data_dir.join("last-rename.json")
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    /// As Windows is set.
    #[default]
    System,
    Dark,
    Light,
}

/// A folder of the master copy and the folder of this computer it's
/// synchronized with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pair {
    pub id: String,
    pub name: String,
    /// The identity in the master's marker; the path alone isn't trusted.
    pub master_id: String,
    /// Where the master was last; its drive letter may change.
    pub master_path: PathBuf,
    pub device_path: PathBuf,
    pub mode: Mode,
    pub conflicts: ConflictPolicy,
    pub by_content: bool,
    /// `2026-11-17 14:05`.
    pub last_sync: Option<String>,
}

impl Default for Pair {
    fn default() -> Self {
        Self {
            id: new_id(),
            name: String::new(),
            master_id: String::new(),
            master_path: PathBuf::new(),
            device_path: PathBuf::new(),
            mode: Mode::TwoWay,
            conflicts: ConflictPolicy::Ask,
            by_content: false,
            last_sync: None,
        }
    }
}

/// A folder and where its backup goes: an exact copy, with what each run
/// replaced or removed kept as safety copies beside it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Backup {
    pub id: String,
    pub name: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub last_run: Option<String>,
}

impl Default for Backup {
    fn default() -> Self {
        Self {
            id: new_id(),
            name: String::new(),
            source: PathBuf::new(),
            destination: PathBuf::new(),
            last_run: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: Theme,
    /// Read every copy back and compare it with the original.
    pub verify_copies: bool,
    /// Where replaced and deleted files go.
    pub removal: Removal,
    /// Safety copies older than this many days may be cleaned up.
    pub keep_safety_copies_days: u32,
    /// Patterns of files no operation touches.
    pub exclude: Vec<String>,
    pub pairs: Vec<Pair>,
    pub backups: Vec<Backup>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            verify_copies: true,
            removal: Removal::Vault,
            keep_safety_copies_days: 90,
            exclude: Filter::standard().exclude,
            pairs: Vec::new(),
            backups: Vec::new(),
        }
    }
}

impl Settings {
    /// The saved settings; the defaults when there are none. A file that
    /// can't be read is an error: saving over it would lose the pairs.
    pub fn load(paths: &Paths) -> Result<Self> {
        let file = paths.settings_file();
        match fs::read_to_string(&file) {
            Ok(text) => {
                toml::from_str(&text).with_context(|| format!("{} isn't valid", file.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("can't read {}", file.display())),
        }
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        let file = paths.settings_file();
        fs::create_dir_all(&paths.config_dir)
            .with_context(|| format!("can't create {}", paths.config_dir.display()))?;
        let tmp = file.with_extension("toml.tmp");
        fs::write(&tmp, toml::to_string_pretty(self)?)
            .with_context(|| format!("can't write {}", tmp.display()))?;
        fs::rename(&tmp, &file).with_context(|| format!("can't write {}", file.display()))
    }

    pub fn filter(&self) -> Filter {
        Filter {
            exclude: self.exclude.clone(),
        }
    }

    /// The folders Gotthard keeps safety copies in: masters, their
    /// counterparts and backup destinations.
    pub fn known_roots(&self) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = self
            .pairs
            .iter()
            .flat_map(|p| [p.master_path.clone(), p.device_path.clone()])
            .chain(self.backups.iter().map(|b| b.destination.clone()))
            .filter(|p| !p.as_os_str().is_empty())
            .collect();
        roots.sort();
        roots.dedup();
        roots
    }
}

/// An id for a pair or a backup.
pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seed = format!(
        "{:?}|{}|{}",
        std::time::SystemTime::now(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    blake3::hash(seed.as_bytes()).to_hex()[..12].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_saving() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        assert_eq!(Settings::load(&paths).unwrap(), Settings::default());
        let mut settings = Settings::default();
        settings.pairs.push(Pair {
            name: "Photos".into(),
            master_path: r"E:\Master\Photos".into(),
            device_path: r"C:\Users\me\Pictures".into(),
            mode: Mode::Mirror,
            ..Pair::default()
        });
        settings.backups.push(Backup::default());
        settings.removal = Removal::RecycleBin;
        settings.save(&paths).unwrap();
        assert_eq!(Settings::load(&paths).unwrap(), settings);
        assert_ne!(settings.pairs[0].id, settings.backups[0].id);

        fs::write(paths.settings_file(), "pairs = 3").unwrap();
        assert!(Settings::load(&paths).is_err());
    }
}
