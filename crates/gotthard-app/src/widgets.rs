//! Small pieces the views share.

use std::path::{Path, PathBuf};

use egui::{Color32, RichText, Ui, vec2};
use gotthard_core::Report;
use gotthard_core::fsx::{Stamp, format_bytes, format_time};

use crate::icons::{self, Icon};
use crate::theme::{self, bold};

/// The head of a view: its icon, its title and a line on what it's for.
pub fn page(ui: &mut Ui, icon: Icon, title: &str, about: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(40.0, 40.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, 10.0, theme::soft(theme::ACCENT, ui.visuals()));
        icon.image(22.0, theme::ACCENT).paint_at(
            ui,
            egui::Rect::from_center_size(rect.center(), vec2(22.0, 22.0)),
        );
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.heading(title);
            ui.label(RichText::new(about).weak());
        });
    });
    ui.add_space(10.0);
}

fn filled(
    ui: &mut Ui,
    enabled: bool,
    icon: Option<Icon>,
    text: &str,
    fill: Color32,
) -> egui::Response {
    let label = RichText::new(text)
        .color(Color32::WHITE)
        .family(theme::semibold());
    let button = match icon {
        Some(icon) => egui::Button::image_and_text(icon.image(16.0, Color32::WHITE), label),
        None => egui::Button::new(label),
    };
    ui.add_enabled(enabled, button.fill(fill).min_size(vec2(0.0, 32.0)))
}

/// The button of a view's main step.
pub fn primary(ui: &mut Ui, enabled: bool, icon: Icon, text: &str) -> egui::Response {
    filled(ui, enabled, Some(icon), text, theme::ACCENT)
}

/// A button for something that deletes.
pub fn danger(ui: &mut Ui, enabled: bool, text: &str) -> egui::Response {
    filled(ui, enabled, Some(Icon::Trash), text, theme::DANGER)
}

/// A button with an icon before its text.
pub fn button(ui: &mut Ui, enabled: bool, icon: Icon, text: &str) -> egui::Response {
    let color = ui.visuals().text_color();
    ui.add_enabled(
        enabled,
        egui::Button::image_and_text(icon.image(16.0, color), text),
    )
}

/// A button that is only an icon; `hint` says what it does.
pub fn icon_button(ui: &mut Ui, enabled: bool, icon: Icon, hint: &str) -> egui::Response {
    let color = ui.visuals().text_color();
    ui.add_enabled(enabled, egui::Button::image(icon.image(16.0, color)))
        .on_hover_text(hint)
}

/// A small rounded label: a count, a state.
pub fn badge(ui: &mut Ui, icon: Option<Icon>, text: &str, color: Color32) {
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_owned(), egui::FontId::proportional(13.0), color);
    let icon_width = if icon.is_some() { 18.0 } else { 0.0 };
    let size = vec2(galley.size().x + icon_width + 20.0, 26.0);
    // One piece, so a row of badges wraps between them.
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 13.0, theme::soft(color, ui.visuals()));
    let mut x = rect.left() + 10.0;
    if let Some(icon) = icon {
        let at =
            egui::Rect::from_center_size(egui::pos2(x + 6.5, rect.center().y), vec2(13.0, 13.0));
        icon.image(13.0, color).paint_at(ui, at);
        x += icon_width;
    }
    ui.painter().galley(
        egui::pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
}

/// An empty view: what's missing and what to do about it.
pub fn empty(ui: &mut Ui, icon: Icon, title: &str, text: &str) {
    ui.add_space((ui.available_height() * 0.18).clamp(16.0, 90.0));
    ui.vertical_centered(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(64.0, 64.0), egui::Sense::hover());
        ui.painter().circle_filled(
            rect.center(),
            32.0,
            theme::soft(theme::ACCENT, ui.visuals()),
        );
        icon.image(30.0, theme::ACCENT).paint_at(
            ui,
            egui::Rect::from_center_size(rect.center(), vec2(30.0, 30.0)),
        );
        ui.add_space(8.0);
        ui.label(bold(title).size(17.0));
        ui.add_space(2.0);
        ui.scope(|ui| {
            ui.set_max_width(460.0);
            ui.label(RichText::new(text).weak());
        });
        ui.add_space(10.0);
    });
}

/// A file's or folder's icon and its name, cut where the space ends.
pub fn file_label(ui: &mut Ui, name: &str, is_dir: bool, text: RichText) -> egui::Response {
    let (icon, color) = icons::for_entry(name, is_dir);
    ui.spacing_mut().item_spacing.x = 6.0;
    ui.add(icon.image(16.0, color));
    ui.add(egui::Label::new(text).truncate().selectable(false))
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
        let width = (ui.available_width() - 110.0).max(120.0);
        let edit = egui::TextEdit::singleline(&mut text)
            .hint_text(hint)
            .desired_width(width);
        if ui.add(edit).changed() {
            *path = PathBuf::from(text.trim());
            changed = true;
        }
        if button(ui, true, Icon::Folder, "Browse…").clicked()
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
            ui.add(Icon::Folder.image(16.0, icons::kind::FOLDER));
            let mut text = folder.to_string_lossy().into_owned();
            let width = (ui.available_width() - 160.0).max(120.0);
            if ui
                .add(egui::TextEdit::singleline(&mut text).desired_width(width))
                .changed()
            {
                *folder = PathBuf::from(text.trim());
                changed = true;
            }
            if button(ui, true, Icon::Folder, "Browse…").clicked()
                && let Some(picked) = pick_folder(folder)
            {
                *folder = picked;
                changed = true;
            }
            if icon_button(ui, true, Icon::Close, "Take this folder off the list").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        folders.remove(i);
        changed = true;
    }
    if button(ui, true, Icon::Plus, add_label).clicked() {
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
    let (color, icon) = if !report.failed.is_empty() {
        (theme::DANGER, Icon::Alert)
    } else if !report.skipped.is_empty() || report.cancelled {
        (theme::WARN, Icon::Info)
    } else {
        (theme::SUCCESS, Icon::CheckCircle)
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
        ui.horizontal(|ui| {
            ui.add(icon.image(18.0, color));
            ui.label(bold(format!("{what} {state}: {}.", parts.join(", "))));
        });
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

/// A banner with an icon: a warning, a hint.
pub fn notice(ui: &mut Ui, icon: Icon, color: Color32, text: &str) {
    theme::banner(ui.style(), color).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.add(icon.image(18.0, color));
            ui.add(egui::Label::new(text).wrap());
        });
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

/// The header cell of a table.
pub fn column_title(ui: &mut Ui, title: &str) {
    ui.label(RichText::new(title).weak().size(13.0));
}
