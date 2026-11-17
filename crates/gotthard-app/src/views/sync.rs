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

use crate::app::{App, Confirm};
use crate::icons::Icon;
use crate::theme::bold;
use crate::views::plan::{Labels, PlanPanel};
use crate::{theme, widgets};

const LABELS: Labels = Labels {
    master: "Master",
    device: "This computer",
    the_master: "the master",
    the_device: "this computer",
    what: "Synchronization",
    run: "Synchronize",
    run_icon: Icon::Sync,
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
    /// The pair whose settings are open.
    editing: Option<String>,
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
        widgets::page(
            ui,
            Icon::Sync,
            "Synchronize",
            "Keep folders of this computer in step with the master copy on your external disk.",
        );
        if self.settings.pairs.is_empty() {
            widgets::empty(
                ui,
                Icon::Sync,
                "Nothing is synchronized yet",
                "Choose a folder on your external disk as the master, and the folder of this \
                 computer that belongs to it.",
            );
            ui.vertical_centered(|ui| {
                if widgets::primary(ui, true, Icon::Plus, "Add a synchronized folder").clicked() {
                    self.add_pair();
                }
            });
            return;
        }
        self.sync.selected = self.sync.selected.min(self.settings.pairs.len() - 1);

        egui::Panel::left("sync-pairs")
            .default_size(230.0)
            .size_range(180.0..=340.0)
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                left: 0,
                right: 10,
                top: 0,
                bottom: 0,
            }))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                for index in 0..self.settings.pairs.len() {
                    let status = self.status_of(index);
                    let pair = &self.settings.pairs[index];
                    let color = match status {
                        MasterStatus::Connected { .. } => theme::SUCCESS,
                        MasterStatus::Other(_) => theme::DANGER,
                        MasterStatus::NotMarked => theme::WARN,
                        _ => ui.visuals().weak_text_color(),
                    };
                    let name = if pair.name.is_empty() {
                        "(no name)"
                    } else {
                        &pair.name
                    };
                    let about = match &pair.last_sync {
                        Some(last) => format!("{} · {last}", mode_short(pair.mode)),
                        None => format!("{} · never run", mode_short(pair.mode)),
                    };
                    let selected = self.sync.selected == index;
                    if list_entry(ui, selected, Icon::Drive, color, name, &about).clicked() {
                        self.sync.selected = index;
                    }
                }
                ui.add_space(6.0);
                if widgets::button(ui, true, Icon::Plus, "Add…").clicked() {
                    self.add_pair();
                }
            });

        egui::CentralPanel::no_frame().show(ui, |ui| self.pair_ui(ui));
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
        let connected = matches!(status, MasterStatus::Connected { .. });
        let complete = !before.master_path.as_os_str().is_empty()
            && !before.device_path.as_os_str().is_empty();
        let editing = !complete || self.sync.editing.as_deref() == Some(&before.id);
        let (mut compare, mut check, mut remove, mut make_master) = (false, false, false, false);
        let mut toggle = false;

        theme::card(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let pair = &mut self.settings.pairs[index];
            // Where the files travel: the master, the direction, this computer.
            let side = (ui.available_width() - 64.0) / 2.0;
            ui.horizontal_top(|ui| {
                let (state, color) = match &status {
                    MasterStatus::NotSet => ("No folder chosen".to_owned(), theme::WARN),
                    MasterStatus::NotMarked => ("Not marked as a master yet".to_owned(), theme::WARN),
                    MasterStatus::Connected { name, free, .. } => (
                        match free {
                            Some(free) => format!("“{name}” connected · {} free", format_bytes(*free)),
                            None => format!("“{name}” connected"),
                        },
                        theme::SUCCESS,
                    ),
                    MasterStatus::Missing => (
                        "Not connected: plug in the disk".to_owned(),
                        ui.visuals().weak_text_color(),
                    ),
                    MasterStatus::Other(name) => {
                        (format!("Another master (“{name}”) is here"), theme::DANGER)
                    }
                };
                endpoint(ui, side, Icon::Drive, "MASTER COPY", &pair.master_path, &state, color);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(40.0, 76.0), egui::Sense::hover());
                let icon = match pair.mode {
                    Mode::TwoWay => Icon::ArrowBoth,
                    Mode::Mirror => Icon::ArrowRight,
                    Mode::Collect => Icon::ArrowLeft,
                };
                icon.image(24.0, theme::ACCENT).paint_at(
                    ui,
                    egui::Rect::from_center_size(rect.center(), egui::vec2(24.0, 24.0)),
                );
                let (state, color) = if pair.device_path.as_os_str().is_empty() {
                    ("No folder chosen".to_owned(), theme::WARN)
                } else if pair.device_path.is_dir() {
                    (
                        match &pair.last_sync {
                            Some(last) => format!("Last synchronized {last}"),
                            None => "Not synchronized yet".to_owned(),
                        },
                        theme::SUCCESS,
                    )
                } else {
                    ("This folder doesn't exist".to_owned(), theme::DANGER)
                };
                endpoint(ui, side, Icon::Computer, "THIS COMPUTER", &pair.device_path, &state, color);
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                compare = widgets::primary(ui, connected && !busy, Icon::Search, "Compare").clicked();
                check = widgets::button(ui, connected && !busy, Icon::Pulse, "Check the master for damage")
                    .on_hover_text(
                        "Reads every file of the master and compares it with what was recorded \
                         at the last check.",
                    )
                    .clicked();
                if status == MasterStatus::NotMarked {
                    make_master =
                        widgets::button(ui, true, Icon::Check, "Make it the master").clicked();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    remove = widgets::button(ui, !busy, Icon::Trash, "Remove…").clicked();
                    let icon = if editing {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    };
                    toggle = widgets::button(ui, complete, icon, "Settings").clicked();
                });
            });

            if editing {
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);
                egui::Grid::new("pair")
                    .striped(false)
                    .num_columns(2)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Name");
                        ui.add(egui::TextEdit::singleline(&mut pair.name).desired_width(280.0));
                        ui.end_row();

                        ui.label("Master folder");
                        if widgets::folder_field(ui, &mut pair.master_path, r"E:\Master\Photos") {
                            // Another folder: its own marker counts.
                            pair.master_id.clear();
                        }
                        ui.end_row();

                        ui.label("This computer");
                        widgets::folder_field(ui, &mut pair.device_path, r"C:\Users\me\Pictures");
                        ui.end_row();

                        ui.label("Direction");
                        ui.vertical(|ui| {
                            egui::ComboBox::from_id_salt("mode")
                                .width(280.0)
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
                                    "This computer's folder becomes an exact copy of the master; \
                                     what it has beyond the master is removed."
                                }
                                Mode::Collect => {
                                    "New and newer files of this computer go to the master. \
                                     Nothing is deleted, nothing newer is replaced."
                                }
                            });
                        });
                        ui.end_row();

                        if pair.mode == Mode::TwoWay {
                            ui.label("Changed on both sides");
                            egui::ComboBox::from_id_salt("conflicts")
                                .width(280.0)
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
            }
        });
        ui.add_space(10.0);

        if toggle {
            self.sync.editing = if editing {
                None
            } else {
                Some(before.id.clone())
            };
        }
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
            // The settings stay open while they're being filled in.
            if !complete {
                self.sync.editing = Some(before.id.clone());
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
            && !editing
        {
            widgets::empty(
                ui,
                Icon::Search,
                "Compare first",
                "Compare shows what a synchronization would copy and delete. Nothing changes \
                 before you start it.",
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

fn mode_short(mode: Mode) -> &'static str {
    match mode {
        Mode::TwoWay => "Both ways",
        Mode::Mirror => "Mirror",
        Mode::Collect => "Collect",
    }
}

/// One end of a pair: its icon, what it is, where it is and how it's doing.
pub fn endpoint(
    ui: &mut Ui,
    width: f32,
    icon: Icon,
    caption: &str,
    path: &Path,
    state: &str,
    color: egui::Color32,
) {
    egui::Frame::new()
        .fill(theme::surface(ui.visuals()))
        .stroke(egui::Stroke::new(1.0, theme::border(ui.visuals())))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.set_width(width - 26.0);
            ui.horizontal(|ui| {
                ui.add(icon.image(28.0, theme::ACCENT));
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(egui::RichText::new(caption).size(11.0).weak());
                    let text = if path.as_os_str().is_empty() {
                        "—".to_owned()
                    } else {
                        path.to_string_lossy().into_owned()
                    };
                    ui.add(egui::Label::new(bold(text)).truncate());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(8.0, 14.0), egui::Sense::hover());
                        ui.painter().circle_filled(rect.center(), 4.0, color);
                        ui.add(
                            egui::Label::new(egui::RichText::new(state).size(13.0).weak())
                                .truncate(),
                        );
                    });
                });
            });
        });
}

/// An entry of a list at the left of a view: an icon, a name and a line
/// below it.
pub fn list_entry(
    ui: &mut Ui,
    selected: bool,
    icon: Icon,
    color: egui::Color32,
    name: &str,
    about: &str,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 50.0), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, name)
    });
    let visuals = ui.visuals();
    if selected {
        ui.painter()
            .rect_filled(rect, 8.0, theme::soft(theme::ACCENT, visuals));
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 8.0, theme::hover(visuals));
    }
    let at = rect.left_center() + egui::vec2(22.0, 0.0);
    icon.image(
        20.0,
        if selected {
            theme::ACCENT
        } else {
            visuals.text_color()
        },
    )
    .paint_at(ui, egui::Rect::from_center_size(at, egui::vec2(20.0, 20.0)));
    // The state, as a dot at the icon's corner.
    ui.painter()
        .circle_filled(at + egui::vec2(9.0, 8.0), 5.0, theme::base(visuals));
    ui.painter()
        .circle_filled(at + egui::vec2(9.0, 8.0), 3.5, color);
    let clip = ui
        .painter()
        .with_clip_rect(rect.shrink2(egui::vec2(6.0, 0.0)));
    clip.text(
        rect.left_center() + egui::vec2(44.0, -9.0),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::new(14.5, theme::semibold()),
        visuals.text_color(),
    );
    clip.text(
        rect.left_center() + egui::vec2(44.0, 10.0),
        egui::Align2::LEFT_CENTER,
        about,
        egui::FontId::proportional(12.0),
        visuals.weak_text_color(),
    );
    response
}
