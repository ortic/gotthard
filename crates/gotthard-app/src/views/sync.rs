//! Synchronize: folders of the master copy and the folders of this computer
//! they're kept in step with.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::Ui;
use gotthard_config::Pair;
use gotthard_core::fsx::{self, format_bytes};
use gotthard_core::sync::{self, CompareOptions, ConflictPolicy, ExecOptions, Mode, State};
use gotthard_core::{integrity, is_cancelled, master};

use crate::app::{App, Confirm, empty_hint, status_dot};
use crate::views::plan::{Labels, PlanPanel};
use crate::{theme, widgets};

const LABELS: Labels = Labels {
    master: "Master",
    device: "This computer",
    the_master: "the master",
    the_device: "this computer",
    what: "Synchronization",
    run: "Synchronize",
};

#[derive(Clone, Debug, PartialEq)]
pub enum MasterStatus {
    /// No folder chosen.
    NotSet,
    /// The folder exists but isn't marked as a master.
    NotMarked,
    Connected {
        path: PathBuf,
        name: String,
        free: Option<u64>,
    },
    /// The disk isn't connected, or the folder is gone.
    Missing,
    /// The folder at the path is another master.
    Other(String),
}

#[derive(Default)]
pub struct SyncView {
    pub selected: usize,
    pub panel: PlanPanel,
    /// Looked up every few seconds: the disk may come and go.
    status: HashMap<String, (Instant, MasterStatus)>,
}

/// Where the pair's master is. Adopts the marker of a folder chosen anew
/// and follows a master to another drive letter; true when the pair changed.
fn master_status(pair: &mut Pair) -> (MasterStatus, bool) {
    if pair.master_path.as_os_str().is_empty() {
        return (MasterStatus::NotSet, false);
    }
    let connected = |path: &Path, marker: master::Marker| MasterStatus::Connected {
        path: path.to_path_buf(),
        name: marker.name,
        free: fsx::free_space(path),
    };
    if pair.master_id.is_empty() {
        return match master::read(&pair.master_path) {
            Some(marker) => {
                pair.master_id = marker.id.clone();
                (connected(&pair.master_path, marker), true)
            }
            None if pair.master_path.is_dir() => (MasterStatus::NotMarked, false),
            None => (MasterStatus::Missing, false),
        };
    }
    match master::locate(&pair.master_path, &pair.master_id) {
        Some(path) => {
            let moved = path != pair.master_path;
            pair.master_path = path;
            let marker = master::read(&pair.master_path).expect("located by its marker");
            (connected(&pair.master_path, marker), moved)
        }
        None => match master::read(&pair.master_path) {
            Some(other) => (MasterStatus::Other(other.name), false),
            None => (MasterStatus::Missing, false),
        },
    }
}

/// Whether one folder is the other or lies inside it.
pub fn nested(a: &Path, b: &Path) -> bool {
    let fold = |p: &Path| p.to_string_lossy().to_lowercase();
    a.ancestors().any(|x| fold(x) == fold(b)) || b.ancestors().any(|x| fold(x) == fold(a))
}

impl App {
    fn status_of(&mut self, index: usize) -> MasterStatus {
        let id = self.settings.pairs[index].id.clone();
        if let Some((at, status)) = self.sync.status.get(&id)
            && at.elapsed() < Duration::from_secs(3)
        {
            return status.clone();
        }
        let (status, changed) = master_status(&mut self.settings.pairs[index]);
        if changed {
            self.save_settings();
        }
        self.sync
            .status
            .insert(id, (Instant::now(), status.clone()));
        status
    }

    pub fn sync_ui(&mut self, ui: &mut Ui) {
        widgets::heading(
            ui,
            "Synchronize",
            "Keep folders of this computer in step with the master copy on your external disk.",
        );
        if self.settings.pairs.is_empty() {
            empty_hint(
                ui,
                "Nothing is synchronized yet. Choose a folder on your external disk as the master \
                 and the folder of this computer that belongs to it.",
            );
            ui.vertical_centered(|ui| {
                ui.add_space(8.0);
                if widgets::primary(ui, true, "Add a synchronized folder").clicked() {
                    self.add_pair();
                }
            });
            return;
        }
        self.sync.selected = self.sync.selected.min(self.settings.pairs.len() - 1);

        egui::Panel::left("sync-pairs")
            .default_size(210.0)
            .size_range(150.0..=320.0)
            .show(ui, |ui| {
                ui.add_space(4.0);
                for index in 0..self.settings.pairs.len() {
                    let status = self.status_of(index);
                    let pair = &self.settings.pairs[index];
                    let name = if pair.name.is_empty() {
                        "(no name)"
                    } else {
                        &pair.name
                    };
                    let color = match status {
                        MasterStatus::Connected { .. } => theme::SUCCESS,
                        MasterStatus::Other(_) => theme::DANGER,
                        _ => ui.visuals().weak_text_color(),
                    };
                    ui.horizontal(|ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().circle_filled(rect.center(), 4.5, color);
                        let selected = self.sync.selected == index;
                        if ui.selectable_label(selected, name).clicked() && !selected {
                            self.sync.selected = index;
                        }
                    });
                }
                ui.add_space(8.0);
                if ui.button("Add…").clicked() {
                    self.add_pair();
                }
            });

        egui::CentralPanel::no_frame().show(ui, |ui| {
            ui.add_space(4.0);
            self.pair_ui(ui);
        });
    }

    fn add_pair(&mut self) {
        self.settings.pairs.push(Pair {
            name: format!("Folder {}", self.settings.pairs.len() + 1),
            ..Pair::default()
        });
        self.sync.selected = self.settings.pairs.len() - 1;
        self.save_settings();
    }

    fn pair_ui(&mut self, ui: &mut Ui) {
        let index = self.sync.selected;
        let status = self.status_of(index);
        let before = self.settings.pairs[index].clone();
        let busy = self.busy();
        let (mut compare, mut check, mut remove, mut make_master) = (false, false, false, false);

        theme::card(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let pair = &mut self.settings.pairs[index];
            egui::Grid::new("pair")
                .striped(false)
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut pair.name).desired_width(260.0));
                    ui.end_row();

                    ui.label("Master folder");
                    ui.vertical(|ui| {
                        if widgets::folder_field(ui, &mut pair.master_path, r"E:\Master\Photos") {
                            // Another folder: its own marker counts.
                            pair.master_id.clear();
                        }
                        match &status {
                            MasterStatus::NotSet => {
                                ui.weak("The folder on the external disk that holds the master copy.");
                            }
                            MasterStatus::NotMarked => {
                                ui.horizontal(|ui| {
                                    status_dot(ui, theme::WARN, "This folder isn't marked as a master yet.");
                                    make_master = ui.button("Make it the master").clicked();
                                });
                            }
                            MasterStatus::Connected { name, free, .. } => {
                                let free = free.map_or(String::new(), |f| {
                                    format!(" · {} free", format_bytes(f))
                                });
                                status_dot(
                                    ui,
                                    theme::SUCCESS,
                                    &format!("Master “{name}” is connected{free}"),
                                );
                            }
                            MasterStatus::Missing => status_dot(
                                ui,
                                ui.visuals().weak_text_color(),
                                "Not connected. Plug in the disk with the master copy.",
                            ),
                            MasterStatus::Other(name) => status_dot(
                                ui,
                                theme::DANGER,
                                &format!("This is another master (“{name}”), not the one of this pair."),
                            ),
                        }
                    });
                    ui.end_row();

                    ui.label("This computer");
                    widgets::folder_field(ui, &mut pair.device_path, r"C:\Users\me\Pictures");
                    ui.end_row();

                    ui.label("Direction");
                    ui.vertical(|ui| {
                        egui::ComboBox::from_id_salt("mode")
                            .width(260.0)
                            .selected_text(mode_title(pair.mode))
                            .show_ui(ui, |ui| {
                                for mode in [Mode::TwoWay, Mode::Mirror, Mode::Collect] {
                                    ui.selectable_value(&mut pair.mode, mode, mode_title(mode));
                                }
                            });
                        ui.weak(match pair.mode {
                            Mode::TwoWay => {
                                "Changes and deletions of either side go to the other."
                            }
                            Mode::Mirror => {
                                "This computer's folder becomes an exact copy of the master; what it \
                                 has beyond the master is removed."
                            }
                            Mode::Collect => {
                                "New and newer files of this computer go to the master. Nothing is \
                                 deleted, nothing newer is replaced."
                            }
                        });
                    });
                    ui.end_row();

                    if pair.mode == Mode::TwoWay {
                        ui.label("Changed on both sides");
                        egui::ComboBox::from_id_salt("conflicts")
                            .width(260.0)
                            .selected_text(conflict_title(pair.conflicts))
                            .show_ui(ui, |ui| {
                                for policy in [
                                    ConflictPolicy::Ask,
                                    ConflictPolicy::MasterWins,
                                    ConflictPolicy::NewerWins,
                                ] {
                                    ui.selectable_value(
                                        &mut pair.conflicts,
                                        policy,
                                        conflict_title(policy),
                                    );
                                }
                            });
                        ui.end_row();
                    }

                    ui.label("");
                    ui.checkbox(
                        &mut pair.by_content,
                        "Compare the content of files whose times differ (slower, fewer false differences)",
                    );
                    ui.end_row();
                });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let connected = matches!(status, MasterStatus::Connected { .. });
                compare = widgets::primary(ui, connected && !busy, "Compare").clicked();
                check = ui
                    .add_enabled(connected && !busy, egui::Button::new("Check the master for damage"))
                    .on_hover_text(
                        "Reads every file of the master and compares it with what was recorded at \
                         the last check.",
                    )
                    .clicked();
                if let Some(last) = &pair.last_sync {
                    ui.weak(format!("Last synchronized {last}"));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    remove = ui.add_enabled(!busy, egui::Button::new("Remove…")).clicked();
                });
            });
        });
        ui.add_space(8.0);

        if self.settings.pairs[index] != before {
            let pair = &self.settings.pairs[index];
            if pair.master_path != before.master_path
                || pair.device_path != before.device_path
                || pair.mode != before.mode
                || pair.conflicts != before.conflicts
                || pair.by_content != before.by_content
            {
                self.sync.panel.clear();
                self.sync.status.remove(&pair.id);
            }
            self.save_settings();
        }
        if make_master {
            let pair = &mut self.settings.pairs[index];
            match master::create(&pair.master_path, &pair.name) {
                Ok(marker) => {
                    pair.master_id = marker.id;
                    self.sync.status.remove(&before.id);
                    self.save_settings();
                }
                Err(e) => self.error(format!("{e:#}")),
            }
        }
        if compare {
            self.compare_pair(ui.ctx(), index);
        }
        if check && let MasterStatus::Connected { path, .. } = &status {
            let (root, filter) = (path.clone(), self.settings.filter());
            self.sync.panel.clear();
            self.sync.panel.owner = before.id.clone();
            self.spawn(ui.ctx(), "Checking the master", move |progress| {
                let result = integrity::check(&root, &filter, progress);
                Box::new(move |app| match result {
                    Ok(report) => app.sync.panel.integrity = Some(report),
                    Err(e) => app.error(format!("{e:#}")),
                })
            });
        }
        if remove {
            let name = before.name.clone();
            self.confirm = Some(Confirm {
                title: format!("Remove “{name}”?"),
                text: "Gotthard forgets this pair. The files of both folders stay as they are."
                    .into(),
                action: "Remove".into(),
                run: Box::new(move |app, _| {
                    if index < app.settings.pairs.len() {
                        app.settings.pairs.remove(index);
                        app.sync.panel.clear();
                        app.save_settings();
                    }
                }),
            });
        }

        if self.sync.panel.owner != before.id {
            self.sync.panel.clear();
            self.sync.panel.owner = before.id.clone();
        }
        let directions = self.settings.pairs[index].mode == Mode::TwoWay;
        if self.sync.panel.ui(ui, &LABELS, busy, directions) {
            self.run_pair(ui.ctx(), index);
        } else if self.sync.panel.plan.is_none()
            && self.sync.panel.report.is_none()
            && self.sync.panel.integrity.is_none()
        {
            empty_hint(
                ui,
                "Compare shows what a synchronization would do. Nothing changes before you start it.",
            );
        }
    }

    fn compare_pair(&mut self, ctx: &egui::Context, index: usize) {
        self.sync.status.remove(&self.settings.pairs[index].id);
        let MasterStatus::Connected { path: master, .. } = self.status_of(index) else {
            self.error("The master isn't connected.");
            return;
        };
        let pair = self.settings.pairs[index].clone();
        if !pair.device_path.is_dir() {
            self.error(format!(
                "This computer's folder {} doesn't exist.",
                pair.device_path.display()
            ));
            return;
        }
        if nested(&master, &pair.device_path) {
            self.error("The master and this computer's folder are the same or inside each other.");
            return;
        }
        let state = self
            .paths
            .as_ref()
            .and_then(|p| State::load(&p.state_file(&pair.id), &pair.master_id, &pair.device_path));
        let options = CompareOptions {
            mode: pair.mode,
            conflicts: pair.conflicts,
            filter: self.settings.filter(),
            by_content: pair.by_content,
            case_insensitive: cfg!(windows),
        };
        self.sync.panel.clear();
        self.spawn(ctx, "Comparing", move |progress| {
            let result = sync::compare(
                &master,
                &pair.device_path,
                state.as_ref(),
                &options,
                progress,
            );
            Box::new(move |app| match result {
                Ok(plan) => app.sync.panel.set(&pair.id, plan, state),
                Err(e) if is_cancelled(&e) => {}
                Err(e) => app.error(format!("{e:#}")),
            })
        });
    }

    fn run_pair(&mut self, ctx: &egui::Context, index: usize) {
        let pair = self.settings.pairs[index].clone();
        let Some(plan) = self.sync.panel.plan.take() else {
            return;
        };
        // The disk at the path is still this pair's master?
        if master::read(&plan.master_root).is_none_or(|m| m.id != pair.master_id) {
            self.error("The master isn't connected any more. Compare again.");
            return;
        }
        let mut state = self
            .sync
            .panel
            .state
            .take()
            .unwrap_or_else(|| State::new(&pair.master_id, &pair.device_path));
        let options = ExecOptions {
            verify: self.settings.verify_copies,
            removal: self.settings.removal,
        };
        let state_file = self.paths.as_ref().map(|p| p.state_file(&pair.id));
        self.spawn(ctx, "Synchronizing", move |progress| {
            let result = sync::execute(&plan, &mut state, &options, progress);
            let saved = match (&result, state_file) {
                (Ok(_), Some(file)) => state.save(&file),
                _ => Ok(()),
            };
            Box::new(move |app| {
                match result {
                    Ok(report) => {
                        app.sync.panel.report = Some(report);
                        if let Some(pair) = app.settings.pairs.iter_mut().find(|p| p.id == pair.id)
                        {
                            pair.last_sync = Some(widgets::now());
                        }
                        app.save_settings();
                    }
                    Err(e) => app.error(format!("{e:#}")),
                }
                if let Err(e) = saved {
                    app.error(format!(
                        "The synchronization ran, but what it did couldn't be recorded ({e:#}). \
                         The next comparison treats the folders as never synchronized: it deletes \
                         nothing and asks about files that differ."
                    ));
                }
            })
        });
    }
}

fn mode_title(mode: Mode) -> &'static str {
    match mode {
        Mode::TwoWay => "Both ways",
        Mode::Mirror => "Master → this computer (mirror)",
        Mode::Collect => "This computer → master (collect)",
    }
}

fn conflict_title(policy: ConflictPolicy) -> &'static str {
    match policy {
        ConflictPolicy::Ask => "I decide, file by file",
        ConflictPolicy::MasterWins => "The master's version wins",
        ConflictPolicy::NewerWins => "The newer version wins",
    }
}
