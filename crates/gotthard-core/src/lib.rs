//! Gotthard's file operations, without any UI.
//!
//! Everything that changes files goes through [`fsx`]: a copy is written
//! beside its target, verified and then renamed into place, and what a copy
//! replaces or a deletion removes goes where the [`fsx::Removal`] says, by
//! default into the folder's safety copies ([`vault`]).

pub mod dupes;
pub mod fsx;
pub mod integrity;
pub mod master;
pub mod merge;
pub mod ops;
pub mod rename;
pub mod scan;
pub mod similar;
pub mod sync;
pub mod vault;

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Gotthard's own folder in a synchronized folder: the master's marker, the
/// safety copies and the integrity manifest. Never scanned.
pub const META_DIR: &str = ".gotthard";
/// The ending of a copy that isn't complete yet. Never scanned.
pub const TMP_SUFFIX: &str = ".gotthard-tmp";

/// The error of an operation the user cancelled.
#[derive(Debug, thiserror::Error)]
#[error("cancelled")]
pub struct Cancelled;

/// Whether `error` is a cancellation.
pub fn is_cancelled(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Cancelled>().is_some()
}

/// What a long operation has done so far, and the way to cancel it. Shared
/// between the worker and the window.
#[derive(Default)]
pub struct Progress {
    cancel: AtomicBool,
    pub items_done: AtomicU64,
    pub items_total: AtomicU64,
    pub bytes_done: AtomicU64,
    pub bytes_total: AtomicU64,
    phase: Mutex<String>,
    current: Mutex<String>,
}

impl Progress {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// An error once the operation is cancelled.
    pub fn check(&self) -> Result<(), Cancelled> {
        if self.cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }

    /// Starts a phase: its name, and the counters from zero.
    pub fn phase(&self, name: &str, items: u64, bytes: u64) {
        *self.phase.lock().unwrap_or_else(|e| e.into_inner()) = name.to_owned();
        self.items_done.store(0, Ordering::Relaxed);
        self.bytes_done.store(0, Ordering::Relaxed);
        self.items_total.store(items, Ordering::Relaxed);
        self.bytes_total.store(bytes, Ordering::Relaxed);
    }

    pub fn current(&self, what: &str) {
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        current.clear();
        current.push_str(what);
    }

    pub fn item(&self) {
        self.items_done.fetch_add(1, Ordering::Relaxed);
    }

    pub fn bytes(&self, n: u64) {
        self.bytes_done.fetch_add(n, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> ProgressSnapshot {
        ProgressSnapshot {
            phase: self.phase.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            current: self
                .current
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            items_done: self.items_done.load(Ordering::Relaxed),
            items_total: self.items_total.load(Ordering::Relaxed),
            bytes_done: self.bytes_done.load(Ordering::Relaxed),
            bytes_total: self.bytes_total.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ProgressSnapshot {
    pub phase: String,
    pub current: String,
    pub items_done: u64,
    pub items_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

impl ProgressSnapshot {
    /// 0..=1 when the total is known: by bytes, else by items.
    pub fn fraction(&self) -> Option<f32> {
        if self.bytes_total > 0 {
            Some((self.bytes_done as f64 / self.bytes_total as f64).min(1.0) as f32)
        } else if self.items_total > 0 {
            Some((self.items_done as f64 / self.items_total as f64).min(1.0) as f32)
        } else {
            None
        }
    }
}

/// What an operation did, file by file where it didn't go as planned.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub copied: usize,
    pub deleted: usize,
    pub bytes: u64,
    /// Left as they are, with the reason.
    pub skipped: Vec<(String, String)>,
    pub failed: Vec<(String, String)>,
    pub cancelled: bool,
}

impl Report {
    pub fn clean(&self) -> bool {
        self.failed.is_empty() && self.skipped.is_empty() && !self.cancelled
    }
}
