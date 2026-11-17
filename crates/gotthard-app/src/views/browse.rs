//! Browse: two folders side by side, to copy and move between, and to send
//! files to the other tools.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use egui::{Key, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use gotthard_core::fsx::{Removal, format_bytes, format_time, to_millis};
use gotthard_core::{Report, ops, rename};

use crate::app::{App, Confirm, View};
use crate::icons::{self, Icon};
use crate::theme::{self, bold};
use crate::widgets;

struct Entry {
    name: String,
    is_dir: bool,
    size: u64,
    mtime: i64,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum SortBy {
    #[default]
    Name,
    Size,
    Changed,
}

#[derive(Default)]
pub struct Pane {
    pub dir: PathBuf,
    entries: Vec<Entry>,
    selected: BTreeSet<usize>,
    /// Where a range selected with Shift starts, and the row the keys move.
    anchor: Option<usize>,
    /// The folder `entries` are of.
    loaded: Option<PathBuf>,
    error: Option<String>,
    sort: SortBy,
    descending: bool,
    /// The path is being typed instead of shown as its folders.
    typing: Option<String>,
    /// A row the keys moved to, to bring into view.
    reveal: Option<usize>,
    /// The folder to select once its parent is shown: where "up" came from.
    came_from: Option<String>,
}

impl Pane {
    fn load(&mut self) {
        self.entries.clear();
        self.selected.clear();
        self.anchor = None;
        self.error = None;
        self.loaded = Some(self.dir.clone());
        match std::fs::read_dir(&self.dir) {
            Ok(reader) => {
                for entry in reader.flatten() {
                    let Ok(meta) = entry.metadata() else { continue };
                    let name = entry.file_name().to_string_lossy().into_owned();
                    // Gotthard's own folder is looked after under Safety copies.
                    if name == gotthard_core::META_DIR {
                        continue;
                    }
                    self.entries.push(Entry {
                        name,
                        is_dir: meta.is_dir(),
                        size: meta.len(),
                        mtime: meta.modified().map(to_millis).unwrap_or(0),
                    });
                }
                self.sort();
                if let Some(name) = self.came_from.take()
                    && let Some(index) = self.entries.iter().position(|e| e.name == name)
                {
                    self.selected = BTreeSet::from([index]);
                    self.anchor = Some(index);
                    self.reveal = Some(index);
                }
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Folders first, then by the chosen column.
    fn sort(&mut self) {
        let (by, descending) = (self.sort, self.descending);
        self.entries.sort_by(|a, b| {
            let order = match by {
                SortBy::Name => std::cmp::Ordering::Equal,
                SortBy::Size => a.size.cmp(&b.size),
                SortBy::Changed => a.mtime.cmp(&b.mtime),
            }
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            let order = if descending { order.reverse() } else { order };
            b.is_dir.cmp(&a.is_dir).then(order)
        });
    }

    fn sort_by(&mut self, by: SortBy) {
        if self.sort == by {
            self.descending = !self.descending;
        } else {
            self.sort = by;
            self.descending = by != SortBy::Name;
        }
        let selected: Vec<String> = self
            .selected
            .iter()
            .filter_map(|i| self.entries.get(*i))
            .map(|e| e.name.clone())
            .collect();
        self.sort();
        self.selected = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| selected.contains(&e.name))
            .map(|(i, _)| i)
            .collect();
        self.anchor = self.selected.first().copied();
    }

    fn go(&mut self, dir: PathBuf) {
        self.dir = dir;
        self.loaded = None;
        self.typing = None;
    }

    fn up(&mut self) {
        if let Some(parent) = self.dir.parent().map(Path::to_path_buf) {
            self.came_from = self
                .dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned());
            self.go(parent);
        }
    }

    pub fn selection(&self) -> Vec<PathBuf> {
        self.selected
            .iter()
            .filter_map(|i| self.entries.get(*i))
            .map(|e| self.dir.join(&e.name))
            .collect()
    }

    /// Moves the selection by `step` rows.
    fn step(&mut self, step: isize, extend: bool) {
        if self.entries.is_empty() {
            return;
        }
        let last = self.entries.len() as isize - 1;
        let from = self
            .reveal
            .or(self.selected.last().copied().filter(|_| step > 0))
            .or(self.selected.first().copied());
        let to = match from {
            Some(from) => (from as isize + step).clamp(0, last) as usize,
            None => 0,
        };
        if extend && let Some(anchor) = self.anchor {
            self.selected = (anchor.min(to)..=anchor.max(to)).collect();
        } else {
            self.selected = BTreeSet::from([to]);
            self.anchor = Some(to);
        }
        self.reveal = Some(to);
    }
}

pub struct BrowseView {
    pub panes: [Pane; 2],
    /// The pane the buttons act on; the other one is where files go.
    pub active: usize,
    /// The name of a folder being created.
    new_folder: Option<String>,
    /// A file being renamed, and its new name.
    renaming: Option<(PathBuf, String)>,
    pub report: Option<Report>,
    home: PathBuf,
    drives: Vec<PathBuf>,
}

impl Default for BrowseView {
    fn default() -> Self {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_default();
        let pane = || Pane {
            dir: home.clone(),
            ..Pane::default()
        };
        Self {
            panes: [pane(), pane()],
            active: 0,
            new_folder: None,
            renaming: None,
            report: None,
            drives: drives(),
            home,
        }
    }
}

impl BrowseView {
    /// Reads both folders again.
    pub fn refresh(&mut self) {
        for pane in &mut self.panes {
            pane.loaded = None;
        }
    }
}

/// The drives that are there.
fn drives() -> Vec<PathBuf> {
    if cfg!(windows) {
        ('A'..='Z')
            .map(|letter| PathBuf::from(format!("{letter}:\\")))
            .filter(|drive| drive.exists())
            .collect()
    } else {
        vec![PathBuf::from("/")]
    }
}

/// Places to jump to: an icon, a name, a folder.
type Places = Vec<(Icon, String, PathBuf)>;

enum PaneEvent {
    None,
    Activate,
    Open(PathBuf),
}

/// The folders of a path, each to click: `C:` › `Users` › `me`.
fn breadcrumbs(ui: &mut Ui, pane: &mut Pane) -> bool {
    let mut parts: Vec<(String, PathBuf)> = Vec::new();
    for ancestor in pane.dir.ancestors() {
        let name = match ancestor.file_name() {
            Some(name) => name.to_string_lossy().into_owned(),
            None => ancestor
                .to_string_lossy()
                .trim_end_matches(['\\', '/'])
                .to_owned(),
        };
        let name = if name.is_empty() {
            "/".to_owned()
        } else {
            name
        };
        parts.push((name, ancestor.to_path_buf()));
    }
    parts.reverse();
    // As many of the last folders as fit.
    let width = ui.available_width() - 34.0;
    let mut shown = 0;
    let mut used = 0.0;
    for (name, _) in parts.iter().rev() {
        used += name.chars().count() as f32 * 8.0 + 30.0;
        if used > width && shown > 0 {
            break;
        }
        shown += 1;
    }
    let mut go = None;
    ui.spacing_mut().item_spacing.x = 2.0;
    if shown < parts.len() {
        ui.weak("…");
    }
    let first = parts.len() - shown;
    for (index, (name, path)) in parts.iter().enumerate().skip(first) {
        if index > first {
            ui.add(Icon::ChevronRight.image(11.0, ui.visuals().weak_text_color()));
        }
        let last = index + 1 == parts.len();
        let text = if last {
            bold(name)
        } else {
            RichText::new(name)
        };
        if ui
            .add(egui::Button::new(text).frame_when_inactive(false))
            .clicked()
            && !last
        {
            go = Some(path.clone());
        }
    }
    match go {
        Some(path) => {
            pane.came_from = pane
                .dir
                .strip_prefix(&path)
                .ok()
                .and_then(|rest| rest.iter().next())
                .map(|n| n.to_string_lossy().into_owned());
            pane.go(path);
            true
        }
        None => false,
    }
}

fn pane_ui(ui: &mut Ui, pane: &mut Pane, active: bool, places: &Places, id: usize) -> PaneEvent {
    let mut event = PaneEvent::None;
    if pane.loaded.as_ref() != Some(&pane.dir) && pane.typing.is_none() {
        pane.load();
    }
    let accent = if active {
        theme::ACCENT
    } else {
        theme::border(ui.visuals())
    };
    egui::Frame::new()
        .fill(theme::surface(ui.visuals()))
        .stroke(egui::Stroke::new(if active { 1.5 } else { 1.0 }, accent))
        .corner_radius(8)
        .inner_margin(8)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if widgets::icon_button(
                    ui,
                    pane.dir.parent().is_some(),
                    Icon::ArrowUp,
                    "Up one folder (Backspace)",
                )
                .clicked()
                {
                    pane.up();
                    event = PaneEvent::Activate;
                }
                let places_button = widgets::icon_button(ui, true, Icon::Home, "Places");
                egui::Popup::menu(&places_button).show(|ui| {
                    for (icon, name, path) in places {
                        let color = match icon {
                            Icon::Folder => icons::kind::FOLDER,
                            _ => ui.visuals().text_color(),
                        };
                        let button = egui::Button::image_and_text(icon.image(16.0, color), name)
                            .frame_when_inactive(false);
                        if ui.add(button).clicked() {
                            pane.go(path.clone());
                            event = PaneEvent::Activate;
                            ui.close();
                        }
                    }
                });
                match &mut pane.typing {
                    Some(text) => {
                        let edit =
                            ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
                        if edit.lost_focus() {
                            let typed = PathBuf::from(text.trim());
                            if ui.input(|i| i.key_pressed(Key::Enter)) && typed.is_dir() {
                                pane.go(typed);
                            } else {
                                pane.typing = None;
                            }
                        } else {
                            edit.request_focus();
                        }
                        event = PaneEvent::Activate;
                    }
                    None => {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::icon_button(ui, true, Icon::Rename, "Type the path")
                                .clicked()
                            {
                                pane.typing = Some(pane.dir.to_string_lossy().into_owned());
                                event = PaneEvent::Activate;
                            }
                            ui.with_layout(
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    if breadcrumbs(ui, pane) {
                                        event = PaneEvent::Activate;
                                    }
                                },
                            );
                        });
                    }
                }
            });
            ui.add_space(2.0);
            if let Some(error) = &pane.error {
                widgets::notice(
                    ui,
                    Icon::Alert,
                    theme::DANGER,
                    &format!("This folder can't be read: {error}"),
                );
            }
            let weak = ui.visuals().weak_text_color();
            let mut enter = None;
            let mut click = None;
            let mut sort = None;
            let footer = 26.0;
            let height = (ui.available_height() - footer - 44.0).max(60.0);
            ui.push_id(("pane", id), |ui| {
                let mut table = TableBuilder::new(ui)
                    .striped(true)
                    .auto_shrink(false)
                    .max_scroll_height(height)
                    .min_scrolled_height(height)
                    .sense(egui::Sense::click())
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::remainder().at_least(120.0).clip(true))
                    .column(Column::exact(84.0))
                    .column(Column::exact(124.0));
                if let Some(row) = pane.reveal.take() {
                    table = table.scroll_to_row(row, None);
                }
                table
                    .header(26.0, |mut header| {
                        for (title, by) in [
                            ("Name", SortBy::Name),
                            ("Size", SortBy::Size),
                            ("Changed", SortBy::Changed),
                        ] {
                            header.col(|ui| {
                                let mark = match (pane.sort == by, pane.descending) {
                                    (false, _) => "",
                                    (true, false) => "  ▲",
                                    (true, true) => "  ▼",
                                };
                                let text =
                                    RichText::new(format!("{title}{mark}")).size(13.0).weak();
                                if ui
                                    .add(egui::Button::new(text).frame(false))
                                    .on_hover_text("Sort by this column")
                                    .clicked()
                                {
                                    sort = Some(by);
                                }
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(26.0, pane.entries.len(), |mut row| {
                            let index = row.index();
                            let entry = &pane.entries[index];
                            row.set_selected(pane.selected.contains(&index));
                            row.col(|ui| {
                                widgets::file_label(
                                    ui,
                                    &entry.name,
                                    entry.is_dir,
                                    RichText::new(&entry.name),
                                );
                            });
                            row.col(|ui| {
                                if !entry.is_dir {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(format_bytes(entry.size)).color(weak),
                                        )
                                        .selectable(false),
                                    );
                                }
                            });
                            row.col(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format_time(entry.mtime)).color(weak),
                                    )
                                    .selectable(false),
                                );
                            });
                            let response = row.response();
                            if response.double_clicked() {
                                enter = Some(index);
                            } else if response.clicked() {
                                click = Some(index);
                            }
                        });
                    });
            });
            if pane.entries.is_empty() && pane.error.is_none() {
                ui.weak("This folder is empty.");
            }
            // What's here and what's selected.
            ui.horizontal(|ui| {
                ui.set_height(footer - 6.0);
                let folders = pane.entries.iter().filter(|e| e.is_dir).count();
                let files = pane.entries.len() - folders;
                let mut text = format!(
                    "{}, {}",
                    widgets::count(folders, "folder"),
                    widgets::count(files, "file")
                );
                if !pane.selected.is_empty() {
                    let bytes: u64 = pane
                        .selected
                        .iter()
                        .filter_map(|i| pane.entries.get(*i))
                        .filter(|e| !e.is_dir)
                        .map(|e| e.size)
                        .sum();
                    text = format!(
                        "{} selected ({}) of {text}",
                        pane.selected.len(),
                        format_bytes(bytes)
                    );
                }
                ui.label(RichText::new(text).size(13.0).weak());
            });
            if let Some(by) = sort {
                pane.sort_by(by);
                event = PaneEvent::Activate;
            }
            if let Some(index) = click {
                let modifiers = ui.input(|i| i.modifiers);
                if modifiers.shift
                    && let Some(anchor) = pane.anchor
                {
                    let (from, to) = (anchor.min(index), anchor.max(index));
                    pane.selected = (from..=to).collect();
                } else if modifiers.command {
                    if !pane.selected.remove(&index) {
                        pane.selected.insert(index);
                    }
                    pane.anchor = Some(index);
                } else {
                    pane.selected = BTreeSet::from([index]);
                    pane.anchor = Some(index);
                }
                event = PaneEvent::Activate;
            }
            if let Some(index) = enter {
                let entry = &pane.entries[index];
                let path = pane.dir.join(&entry.name);
                if entry.is_dir {
                    pane.go(path);
                    event = PaneEvent::Activate;
                } else {
                    event = PaneEvent::Open(path);
                }
            }
        });
    event
}

/// What the toolbar and the keys ask for.
#[derive(Default)]
struct Wanted {
    copy: bool,
    moving: bool,
    delete: bool,
    rename: bool,
    new_folder: bool,
}

impl App {
    /// The folder whose safety copies take a deleted `path`: the innermost
    /// of the synchronized folders and backups it lies in.
    fn root_of(&self, path: &Path) -> Option<PathBuf> {
        self.settings
            .known_roots()
            .into_iter()
            .filter(|root| path.starts_with(root) && path != root)
            .max_by_key(|root| root.as_os_str().len())
    }

    pub fn browse_ui(&mut self, ui: &mut Ui) {
        widgets::page(
            ui,
            Icon::Panes,
            "Browse",
            "Two folders side by side. The buttons act on the selection of the framed side; \
             copies and moves go to the other side.",
        );
        let busy = self.busy();
        let active = self.browse.active;
        let selection = self.browse.panes[active].selection();
        let here = self.browse.panes[active].dir.clone();
        let there = self.browse.panes[1 - active].dir.clone();
        let any = !selection.is_empty() && !busy;
        let arrow = if active == 0 {
            Icon::ArrowRight
        } else {
            Icon::ArrowLeft
        };

        let mut wanted = Wanted::default();
        ui.horizontal_wrapped(|ui| {
            wanted.copy = widgets::button(ui, any, arrow, "Copy")
                .on_hover_text("Copy the selection to the other side (F5)")
                .clicked();
            wanted.moving = widgets::button(ui, any, arrow, "Move")
                .on_hover_text("Move the selection to the other side (F6)")
                .clicked();
            wanted.rename = widgets::button(ui, any, Icon::Rename, "Rename…")
                .on_hover_text("Rename the file (F2); several files go to Rename")
                .clicked();
            wanted.new_folder = widgets::button(ui, !busy, Icon::FolderPlus, "New folder…")
                .on_hover_text("F7")
                .clicked();
            wanted.delete = widgets::button(ui, any, Icon::Trash, "Delete…")
                .on_hover_text("Delete")
                .clicked();
            ui.separator();
            if widgets::button(ui, !busy, Icon::Duplicates, "Find duplicates here").clicked() {
                self.dupes.folders = vec![here.clone()];
                self.view = View::Duplicates;
            }
            if widgets::button(ui, !busy, Icon::Similar, "Find similar names here").clicked() {
                self.similar.folders = vec![here.clone()];
                self.view = View::Similar;
            }
            if widgets::icon_button(ui, true, Icon::Refresh, "Read both folders again").clicked() {
                self.browse.refresh();
                self.browse.drives = drives();
            }
        });
        if let Some(report) = &self.browse.report {
            ui.add_space(4.0);
            widgets::report(ui, report, "The operation");
        }
        ui.add_space(6.0);

        let mut places: Places = vec![(Icon::Home, "Home".to_owned(), self.browse.home.clone())];
        for drive in &self.browse.drives {
            places.push((
                Icon::Drive,
                drive.to_string_lossy().into_owned(),
                drive.clone(),
            ));
        }
        for root in self.settings.known_roots() {
            places.push((Icon::Folder, root.to_string_lossy().into_owned(), root));
        }

        // The keys, unless something is being typed.
        let typing = ui.memory(|m| m.focused().is_some())
            || self.confirm.is_some()
            || self.browse.new_folder.is_some()
            || self.browse.renaming.is_some();
        let mut open = None;
        if !typing {
            let pane = &mut self.browse.panes[active];
            let (shift, command) = ui.input(|i| (i.modifiers.shift, i.modifiers.command));
            let pressed = |key: Key| ui.input(|i| i.key_pressed(key));
            if pressed(Key::ArrowDown) {
                pane.step(1, shift);
            } else if pressed(Key::ArrowUp) {
                pane.step(-1, shift);
            } else if pressed(Key::PageDown) {
                pane.step(15, shift);
            } else if pressed(Key::PageUp) {
                pane.step(-15, shift);
            } else if pressed(Key::Home) {
                pane.step(isize::MIN / 2, shift);
            } else if pressed(Key::End) {
                pane.step(isize::MAX / 2, shift);
            } else if pressed(Key::Backspace) {
                pane.up();
            } else if pressed(Key::A) && command {
                pane.selected = (0..pane.entries.len()).collect();
            } else if pressed(Key::Enter)
                && let [index] = pane.selected.iter().copied().collect::<Vec<_>>()[..]
            {
                let entry = &pane.entries[index];
                let path = pane.dir.join(&entry.name);
                if entry.is_dir {
                    pane.go(path);
                } else {
                    open = Some(path);
                }
            } else if pressed(Key::Tab) {
                self.browse.active = 1 - active;
            }
            wanted.copy |= any && pressed(Key::F5);
            wanted.moving |= any && pressed(Key::F6);
            wanted.rename |= any && pressed(Key::F2);
            wanted.new_folder |= !busy && pressed(Key::F7);
            wanted.delete |= any && pressed(Key::Delete);
        }
        if let Some(path) = open {
            widgets::open_in_os(&path);
        }

        let mut events = Vec::new();
        ui.columns(2, |columns| {
            for (index, column) in columns.iter_mut().enumerate() {
                let pane = &mut self.browse.panes[index];
                events.push(pane_ui(column, pane, active == index, &places, index));
            }
        });
        for (index, event) in events.into_iter().enumerate() {
            match event {
                PaneEvent::None => {}
                PaneEvent::Activate => self.browse.active = index,
                PaneEvent::Open(path) => {
                    self.browse.active = index;
                    widgets::open_in_os(&path);
                }
            }
        }

        if wanted.rename {
            if let [one] = selection.as_slice() {
                let name = one
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.browse.renaming = Some((one.clone(), name));
            } else {
                let files = selection.iter().filter(|p| p.is_file()).cloned().collect();
                self.rename.load(files);
                self.view = View::Rename;
            }
        }
        if wanted.new_folder {
            self.browse.new_folder = Some(String::new());
        }
        let (copy, moving, delete) = (wanted.copy, wanted.moving, wanted.delete);
        if copy || moving {
            let verify = self.settings.verify_copies;
            self.browse.report = None;
            let title = if moving { "Moving" } else { "Copying" };
            self.spawn(ui.ctx(), title, move |progress| {
                let report = ops::transfer(&selection, &there, moving, verify, progress);
                Box::new(move |app| {
                    app.browse.report = Some(report);
                    app.browse.refresh();
                })
            });
        } else if delete {
            let removal = self.settings.removal;
            let roots: Vec<Option<PathBuf>> = selection.iter().map(|p| self.root_of(p)).collect();
            let where_to = match removal {
                Removal::Permanent => "They are deleted for good.",
                Removal::Vault if roots.iter().all(Option::is_some) => {
                    "They go to the folder's safety copies and can be restored from there."
                }
                _ => "They go to the Recycle Bin.",
            };
            self.confirm = Some(Confirm {
                title: format!("Delete {} items?", selection.len()),
                text: where_to.into(),
                action: "Delete".into(),
                run: Box::new(move |app, ctx| {
                    app.browse.report = None;
                    app.spawn(ctx, "Deleting", move |progress| {
                        let lookup = |path: &Path| {
                            selection
                                .iter()
                                .position(|p| p == path)
                                .and_then(|i| roots[i].clone())
                        };
                        let report = ops::remove(&selection, removal, lookup, progress);
                        Box::new(move |app| {
                            app.browse.report = Some(report);
                            app.browse.refresh();
                        })
                    });
                }),
            });
        }
        self.browse_dialogs(ui.ctx(), &here);
    }

    fn browse_dialogs(&mut self, ctx: &egui::Context, here: &Path) {
        if let Some(name) = &mut self.browse.new_folder {
            let mut done = None;
            let modal = egui::Modal::new(egui::Id::new("new-folder")).show(ctx, |ui| {
                ui.label(bold("New folder").size(18.0));
                ui.label(format!("in {}", here.display()));
                let edit = ui.add(egui::TextEdit::singleline(name).desired_width(320.0));
                edit.request_focus();
                let problem = rename::name_problem(name).filter(|_| !name.is_empty());
                if let Some(problem) = &problem {
                    ui.colored_label(theme::DANGER, problem);
                }
                ui.horizontal(|ui| {
                    let ok = !name.is_empty() && problem.is_none();
                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if widgets::primary(ui, ok, Icon::FolderPlus, "Create").clicked()
                        || (ok && enter)
                    {
                        done = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        done = Some(false);
                    }
                });
            });
            if modal.should_close() {
                done = done.or(Some(false));
            }
            if let Some(create) = done {
                let name = self.browse.new_folder.take().unwrap_or_default();
                if create {
                    if let Err(e) = std::fs::create_dir(here.join(&name)) {
                        self.error(format!("The folder {name} can't be created: {e}"));
                    }
                    self.browse.refresh();
                }
            }
        }
        if let Some((path, name)) = &mut self.browse.renaming {
            let mut done = None;
            let modal = egui::Modal::new(egui::Id::new("rename-one")).show(ctx, |ui| {
                ui.label(bold("Rename").size(18.0));
                let edit = ui.add(egui::TextEdit::singleline(name).desired_width(360.0));
                edit.request_focus();
                let target = path.with_file_name(&*name);
                let same = target == *path;
                let problem = rename::name_problem(name).or_else(|| {
                    (!same
                        && target.exists()
                        && target.to_string_lossy().to_lowercase()
                            != path.to_string_lossy().to_lowercase())
                    .then(|| "a file with this name exists".to_owned())
                });
                if let Some(problem) = &problem {
                    ui.colored_label(theme::DANGER, problem);
                }
                ui.horizontal(|ui| {
                    let ok = problem.is_none() && !same;
                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if widgets::primary(ui, ok, Icon::Rename, "Rename").clicked() || (ok && enter) {
                        done = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        done = Some(false);
                    }
                });
            });
            if modal.should_close() {
                done = done.or(Some(false));
            }
            if let Some(apply) = done {
                let (path, name) = self.browse.renaming.take().expect("shown above");
                if apply {
                    if let Err(e) = std::fs::rename(&path, path.with_file_name(&name)) {
                        self.error(format!("Can't rename to {name}: {e}"));
                    }
                    self.browse.refresh();
                }
            }
        }
    }
}
