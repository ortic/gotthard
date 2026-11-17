//! Small pieces the views share.

use std::path::{Path, PathBuf};

use egui::{Color32, RichText, Ui};
use gotthard_core::Report;
use gotthard_core::fsx::{Stamp, format_bytes, format_time};

use crate::theme;

/// The title of a view and a line on what it's for.
pub fn heading(ui: &mut Ui, title: &str, about: &str) {
    ui.heading(title);
    ui.label(RichText::new(about).weak());
    ui.add_space(6.0);
}

/// The button of a view's main step.
pub fn primary(ui: &mut Ui, enabled: bool, text: &str) -> egui::Response {
    let button = egui::Button::new(RichText::new(text).color(Color32::WHITE).strong())
        .fill(theme::ACCENT)
        .min_size(egui::vec2(0.0, 28.0));
    ui.add_enabled(enabled, button)
}

/// A button for something that deletes.
pub fn danger(ui: &mut Ui, enabled: bool, text: &str) -> egui::Response {
    let button = egui::Button::new(RichText::new(text).color(Color32::WHITE).strong())
        .fill(theme::DANGER)
        .min_size(egui::vec2(0.0, 28.0));
    ui.add_enabled(enabled, button)
}

/// Asks for a folder. Blocks until the dialog closes.
pub fn pick_folder(start: &Path) -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    let mut dialog = rfd::FileDialog::new();
    if start.is_dir() {
        dialog = dialog.set_directory(start);
    }
    dialog.pick_folder()
}

/// Asks for files. Blocks until the dialog closes.
pub fn pick_files(start: &Path) -> Vec<PathBuf> {
    if cfg!(test) {
        return Vec::new();
    }
    let mut dialog = rfd::FileDialog::new();
    if start.is_dir() {
        dialog = dialog.set_directory(start);
    }
    dialog.pick_files().unwrap_or_default()
}

/// A folder's path to type or to choose. True when it changed.
pub fn folder_field(ui: &mut Ui, path: &mut PathBuf, hint: &str) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let mut text = path.to_string_lossy().into_owned();
        let width = (ui.available_width() - 90.0).max(120.0);
        let edit = egui::TextEdit::singleline(&mut text)
            .hint_text(hint)
            .desired_width(width);
        if ui.add(edit).changed() {
            *path = PathBuf::from(text.trim());
            changed = true;
        }
        if ui.button("Browse…").clicked()
            && let Some(folder) = pick_folder(path)
        {
            *path = folder;
            changed = true;
        }
    });
    changed
}

/// A list of folders with buttons to add and remove. True when it changed.
pub fn folder_list(ui: &mut Ui, folders: &mut Vec<PathBuf>, add_label: &str) -> bool {
    let mut changed = false;
    let mut remove = None;
    for (i, folder) in folders.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let mut text = folder.to_string_lossy().into_owned();
            let width = (ui.available_width() - 180.0).max(120.0);
            if ui
                .add(egui::TextEdit::singleline(&mut text).desired_width(width))
                .changed()
            {
                *folder = PathBuf::from(text.trim());
                changed = true;
            }
            if ui.button("Browse…").clicked()
                && let Some(picked) = pick_folder(folder)
            {
                *folder = picked;
                changed = true;
            }
            if ui.button("Remove").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        folders.remove(i);
        changed = true;
    }
    if ui.button(add_label).clicked() {
        match pick_folder(Path::new("")) {
            Some(folder) => folders.push(folder),
            // No dialog (or cancelled with nothing there yet): a line to type in.
            None if folders.is_empty() || cfg!(test) => folders.push(PathBuf::new()),
            None => {}
        }
        changed = true;
    }
    changed
}

/// The folders that are set (not empty lines).
pub fn set_folders(folders: &[PathBuf]) -> Vec<PathBuf> {
    folders
        .iter()
        .filter(|f| !f.as_os_str().is_empty())
        .cloned()
        .collect()
}

/// `1.5 MB · 2026-11-17 14:05`.
pub fn stamp_text(stamp: Option<Stamp>, is_dir: bool) -> String {
    match stamp {
        None => String::new(),
        Some(_) if is_dir => "folder".into(),
        Some(stamp) => format!(
            "{} · {}",
            format_bytes(stamp.size),
            format_time(stamp.mtime)
        ),
    }
}

/// What an operation did, with the files that didn't go as planned.
pub fn report(ui: &mut Ui, report: &Report, what: &str) {
    let color = if !report.failed.is_empty() {
        theme::DANGER
    } else if !report.skipped.is_empty() || report.cancelled {
        theme::WARN
    } else {
        theme::SUCCESS
    };
    theme::banner(ui.style(), color).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let mut parts = Vec::new();
        if report.copied > 0 {
            parts.push(format!(
                "{} copied ({})",
                report.copied,
                format_bytes(report.bytes)
            ));
        }
        if report.deleted > 0 {
            parts.push(format!("{} removed", report.deleted));
        }
        if !report.skipped.is_empty() {
            parts.push(format!("{} left alone", report.skipped.len()));
        }
        if !report.failed.is_empty() {
            parts.push(format!("{} failed", report.failed.len()));
        }
        if parts.is_empty() {
            parts.push("nothing to do".into());
        }
        let state = if report.cancelled {
            "cancelled"
        } else {
            "done"
        };
        ui.label(RichText::new(format!("{what} {state}: {}.", parts.join(", "))).strong());
        for (title, list, color) in [
            ("Failed", &report.failed, theme::DANGER),
            ("Left alone", &report.skipped, theme::WARN),
        ] {
            if list.is_empty() {
                continue;
            }
            egui::CollapsingHeader::new(
                RichText::new(format!("{title} ({})", list.len())).color(color),
            )
            .id_salt((what, title))
            .default_open(list.len() <= 8)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt((what, title, "list"))
                    .max_height(160.0)
                    .show(ui, |ui| {
                        for (file, why) in list.iter().take(500) {
                            ui.label(format!("{file}: {why}"));
                        }
                        if list.len() > 500 {
                            ui.weak(format!("… and {} more", list.len() - 500));
                        }
                    });
            });
        }
    });
}

/// Opens a file with its program, or a folder in Explorer.
pub fn open_in_os(path: &Path) {
    if cfg!(test) {
        return;
    }
    #[cfg(windows)]
    let result = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(path).spawn();
    if let Err(e) = result {
        tracing::warn!("can't open {}: {e}", path.display());
    }
}

/// Now, as shown for a last run: `2026-11-17 14:05`.
pub fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
}

/// `1 file`, `3 files`.
pub fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}
