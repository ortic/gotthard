//! Browse: two folders side by side, to copy and move between, and to send
//! files to the other tools.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use gotthard_core::fsx::{Removal, format_bytes, format_time, to_millis};
use gotthard_core::{Report, ops, rename};

use crate::app::{App, Confirm, View};
use crate::{theme, widgets};

struct Entry {
    name: String,
    is_dir: bool,
    size: u64,
    mtime: i64,
}

#[derive(Default)]
pub struct Pane {
    pub dir: PathBuf,
    entries: Vec<Entry>,
    selected: BTreeSet<usize>,
    /// Where a range selected with Shift starts.
    anchor: Option<usize>,
    /// The folder `entries` are of.
    loaded: Option<PathBuf>,
    error: Option<String>,
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
                self.entries.sort_by(|a, b| {
                    b.is_dir
                        .cmp(&a.is_dir)
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn go(&mut self, dir: PathBuf) {
        self.dir = dir;
        self.loaded = None;
    }

    pub fn selection(&self) -> Vec<PathBuf> {
        self.selected
            .iter()
            .filter_map(|i| self.entries.get(*i))
            .map(|e| self.dir.join(&e.name))
            .collect()
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
    places: Vec<PathBuf>,
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
            places: drives(&home),
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

/// The home folder and the drives that are there.
fn drives(home: &Path) -> Vec<PathBuf> {
    let mut places = vec![home.to_path_buf()];
    if cfg!(windows) {
        places.extend(
            ('A'..='Z')
                .map(|letter| PathBuf::from(format!("{letter}:\\")))
                .filter(|drive| drive.exists()),
        );
    } else {
        places.push(PathBuf::from("/"));
    }
    places
}

enum PaneEvent {
    None,
    Activate,
    Open(PathBuf),
}

fn pane_ui(ui: &mut Ui, pane: &mut Pane, active: bool, places: &[PathBuf], id: usize) -> PaneEvent {
    let mut event = PaneEvent::None;
    if pane.loaded.as_ref() != Some(&pane.dir) {
        pane.load();
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(pane.dir.parent().is_some(), egui::Button::new("Up"))
            .clicked()
            && let Some(parent) = pane.dir.parent()
        {
            pane.go(parent.to_path_buf());
            event = PaneEvent::Activate;
        }
        egui::ComboBox::from_id_salt(("places", id))
            .width(70.0)
            .selected_text("Places")
            .show_ui(ui, |ui| {
                for place in places {
                    if ui
                        .selectable_label(false, place.to_string_lossy())
                        .clicked()
                    {
                        pane.go(place.clone());
                        event = PaneEvent::Activate;
                    }
                }
            });
        let mut text = pane.dir.to_string_lossy().into_owned();
        let edit = ui.add(egui::TextEdit::singleline(&mut text).desired_width(f32::INFINITY));
        if edit.changed() {
            pane.dir = PathBuf::from(text.trim());
        }
        if edit.gained_focus() {
            event = PaneEvent::Activate;
        }
    });
    if let Some(error) = &pane.error {
        ui.colored_label(theme::DANGER, format!("This folder can't be read: {error}"));
    }
    let weak = ui.visuals().weak_text_color();
    let accent = if active {
        theme::ACCENT
    } else {
        theme::border(ui.visuals())
    };
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, accent))
        .corner_radius(4)
        .inner_margin(2)
        .show(ui, |ui| {
            let mut enter = None;
            let mut click = None;
            ui.push_id(("pane", id), |ui| {
                TableBuilder::new(ui)
                    .striped(true)
                    .auto_shrink(false)
                    .sense(egui::Sense::click())
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::remainder().at_least(120.0).clip(true))
                    .column(Column::exact(80.0))
                    .column(Column::exact(120.0))
                    .header(22.0, |mut header| {
                        for title in ["Name", "Size", "Changed"] {
                            header.col(|ui| {
                                ui.strong(title);
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(22.0, pane.entries.len(), |mut row| {
                            let index = row.index();
                            let entry = &pane.entries[index];
                            row.set_selected(pane.selected.contains(&index));
                            row.col(|ui| {
                                let name = if entry.is_dir {
                                    RichText::new(format!("{}/", entry.name)).strong()
                                } else {
                                    RichText::new(&entry.name)
                                };
                                ui.add(egui::Label::new(name).truncate().selectable(false));
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
        widgets::heading(
            ui,
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
        let arrow = if active == 0 { "→" } else { "←" };

        let (mut copy, mut moving, mut delete) = (false, false, false);
        ui.horizontal_wrapped(|ui| {
            copy = ui
                .add_enabled(any, egui::Button::new(format!("Copy {arrow}")))
                .clicked();
            moving = ui
                .add_enabled(any, egui::Button::new(format!("Move {arrow}")))
                .clicked();
            if ui.add_enabled(any, egui::Button::new("Rename…")).clicked() {
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
            if ui
                .add_enabled(!busy, egui::Button::new("New folder…"))
                .clicked()
            {
                self.browse.new_folder = Some(String::new());
            }
            delete = ui.add_enabled(any, egui::Button::new("Delete…")).clicked();
            ui.separator();
            if ui
                .add_enabled(!busy, egui::Button::new("Find duplicates here"))
                .clicked()
            {
                self.dupes.folders = vec![here.clone()];
                self.view = View::Duplicates;
            }
            if ui
                .add_enabled(!busy, egui::Button::new("Find similar names here"))
                .clicked()
            {
                self.similar.folders = vec![here.clone()];
                self.view = View::Similar;
            }
            if ui.button("Refresh").clicked() {
                self.browse.refresh();
                self.browse.places = drives(&self.browse.places[0].clone());
            }
        });
        if let Some(report) = &self.browse.report {
            ui.add_space(4.0);
            widgets::report(ui, report, "The operation");
        }
        ui.add_space(4.0);

        let places = self.browse.places.clone();
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
                ui.heading("New folder");
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
                    if widgets::primary(ui, ok, "Create").clicked() || (ok && enter) {
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
                ui.heading("Rename");
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
                    if widgets::primary(ui, ok, "Rename").clicked() || (ok && enter) {
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
