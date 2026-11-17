//! Backup: an exact copy of a folder somewhere else, with what each run
//! replaced or removed kept as safety copies beside it.

use egui::Ui;
use gotthard_config::Backup;
use gotthard_core::sync::{self, CompareOptions, ConflictPolicy, ExecOptions, Mode, State};
use gotthard_core::{integrity, is_cancelled};

use crate::app::{App, Confirm};
use crate::icons::Icon;
use crate::views::plan::{Labels, PlanPanel};
use crate::views::sync::{endpoint, list_entry, nested};
use crate::{theme, widgets};

const LABELS: Labels = Labels {
    master: "Source",
    device: "Backup",
    the_master: "the source",
    the_device: "the backup",
    what: "Backup",
    run: "Back up",
    run_icon: Icon::Backup,
};

#[derive(Default)]
pub struct BackupView {
    pub selected: usize,
    /// The backup whose settings are open.
    editing: Option<String>,
    pub panel: PlanPanel,
}

impl App {
    pub fn backup_ui(&mut self, ui: &mut Ui) {
        widgets::page(
            ui,
            Icon::Backup,
            "Backup",
            "An exact copy of a folder on another disk. Files a run replaces or removes stay \
             restorable under Safety copies.",
        );
        if self.settings.backups.is_empty() {
            widgets::empty(
                ui,
                Icon::Backup,
                "No backups yet",
                "A backup is a plain copy of a folder that you can open with Explorer, kept up \
                 to date whenever you run it.",
            );
            ui.vertical_centered(|ui| {
                if widgets::primary(ui, true, Icon::Plus, "Add a backup").clicked() {
                    self.add_backup();
                }
            });
            return;
        }
        self.backup.selected = self.backup.selected.min(self.settings.backups.len() - 1);

        egui::Panel::left("backups")
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
                for (index, backup) in self.settings.backups.iter().enumerate() {
                    let name = if backup.name.is_empty() {
                        "(no name)"
                    } else {
                        &backup.name
                    };
                    let (about, color) = match &backup.last_run {
                        Some(last) => (format!("Last backup {last}"), theme::SUCCESS),
                        None => ("Never run".to_owned(), ui.visuals().weak_text_color()),
                    };
                    let selected = self.backup.selected == index;
                    if list_entry(ui, selected, Icon::Backup, color, name, &about).clicked() {
                        self.backup.selected = index;
                    }
                }
                ui.add_space(6.0);
                if widgets::button(ui, true, Icon::Plus, "Add…").clicked() {
                    self.add_backup();
                }
            });
        egui::CentralPanel::no_frame().show(ui, |ui| self.backup_detail(ui));
    }

    fn add_backup(&mut self) {
        self.settings.backups.push(Backup {
            name: format!("Backup {}", self.settings.backups.len() + 1),
            ..Backup::default()
        });
        self.backup.selected = self.settings.backups.len() - 1;
        self.save_settings();
    }

    fn backup_detail(&mut self, ui: &mut Ui) {
        let index = self.backup.selected;
        let before = self.settings.backups[index].clone();
        let busy = self.busy();
        let set =
            !before.source.as_os_str().is_empty() && !before.destination.as_os_str().is_empty();
        let editing = !set || self.backup.editing.as_deref() == Some(&before.id);
        let (mut compare, mut check, mut remove, mut toggle) = (false, false, false, false);
        theme::card(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let backup = &mut self.settings.backups[index];
            let side = (ui.available_width() - 64.0) / 2.0;
            ui.horizontal_top(|ui| {
                let state = |path: &std::path::Path, missing: &str| {
                    if path.as_os_str().is_empty() {
                        ("No folder chosen".to_owned(), theme::WARN)
                    } else if path.is_dir() {
                        ("Ready".to_owned(), theme::SUCCESS)
                    } else {
                        (missing.to_owned(), theme::WARN)
                    }
                };
                let (text, color) = state(&backup.source, "This folder doesn't exist");
                endpoint(
                    ui,
                    side,
                    Icon::Computer,
                    "FOLDER TO BACK UP",
                    &backup.source,
                    &text,
                    color,
                );
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(40.0, 76.0), egui::Sense::hover());
                Icon::ArrowRight.image(24.0, theme::ACCENT).paint_at(
                    ui,
                    egui::Rect::from_center_size(rect.center(), egui::vec2(24.0, 24.0)),
                );
                let (mut text, color) = state(
                    &backup.destination,
                    "Not there yet: created by the first backup",
                );
                if let (Some(last), true) = (&backup.last_run, backup.destination.is_dir()) {
                    text = format!("Last backup {last}");
                }
                endpoint(
                    ui,
                    side,
                    Icon::Drive,
                    "BACKUP",
                    &backup.destination,
                    &text,
                    color,
                );
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                compare = widgets::primary(ui, set && !busy, Icon::Search, "Compare").clicked();
                check =
                    widgets::button(ui, set && !busy, Icon::Pulse, "Check the backup for damage")
                        .clicked();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    remove = widgets::button(ui, !busy, Icon::Trash, "Remove…").clicked();
                    let icon = if editing {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    };
                    toggle = widgets::button(ui, set, icon, "Settings").clicked();
                });
            });
            if editing {
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);
                egui::Grid::new("backup")
                    .striped(false)
                    .num_columns(2)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Name");
                        ui.add(egui::TextEdit::singleline(&mut backup.name).desired_width(280.0));
                        ui.end_row();
                        ui.label("Folder to back up");
                        widgets::folder_field(ui, &mut backup.source, r"C:\Users\me\Documents");
                        ui.end_row();
                        ui.label("Backup goes to");
                        widgets::folder_field(ui, &mut backup.destination, r"F:\Backup\Documents");
                        ui.end_row();
                    });
            }
        });
        ui.add_space(10.0);
        if toggle {
            self.backup.editing = if editing {
                None
            } else {
                Some(before.id.clone())
            };
        }

        let backup = self.settings.backups[index].clone();
        if backup != before {
            if backup.source != before.source || backup.destination != before.destination {
                self.backup.panel.clear();
            }
            // The settings stay open while they're being filled in.
            if !set {
                self.backup.editing = Some(before.id.clone());
            }
            self.save_settings();
        }
        if self.backup.panel.owner != backup.id {
            self.backup.panel.clear();
            self.backup.panel.owner = backup.id.clone();
        }
        if compare {
            if !backup.source.is_dir() {
                self.error(format!(
                    "The folder {} doesn't exist.",
                    backup.source.display()
                ));
            } else if nested(&backup.source, &backup.destination) {
                self.error("The folder and its backup are the same or inside each other.");
            } else if let Err(e) = std::fs::create_dir_all(&backup.destination) {
                self.error(format!(
                    "The backup's folder {} can't be created: {e}. Is the disk connected?",
                    backup.destination.display()
                ));
            } else {
                let options = CompareOptions {
                    mode: Mode::Mirror,
                    conflicts: ConflictPolicy::Ask,
                    filter: self.settings.filter(),
                    by_content: false,
                    case_insensitive: cfg!(windows),
                };
                self.backup.panel.clear();
                let backup = backup.clone();
                self.spawn(ui.ctx(), "Comparing", move |progress| {
                    let result = sync::compare(
                        &backup.source,
                        &backup.destination,
                        None,
                        &options,
                        progress,
                    );
                    Box::new(move |app| match result {
                        Ok(plan) => app.backup.panel.set(&backup.id, plan, None),
                        Err(e) if is_cancelled(&e) => {}
                        Err(e) => app.error(format!("{e:#}")),
                    })
                });
            }
        } else if check {
            let (root, filter) = (backup.destination.clone(), self.settings.filter());
            self.backup.panel.clear();
            self.spawn(ui.ctx(), "Checking the backup", move |progress| {
                let result = integrity::check(&root, &filter, progress);
                Box::new(move |app| match result {
                    Ok(report) => app.backup.panel.integrity = Some(report),
                    Err(e) => app.error(format!("{e:#}")),
                })
            });
        } else if remove {
            self.confirm = Some(Confirm {
                title: format!("Remove “{}”?", backup.name),
                text: "Gotthard forgets this backup. The files of both folders stay as they are."
                    .into(),
                action: "Remove".into(),
                run: Box::new(move |app, _| {
                    if index < app.settings.backups.len() {
                        app.settings.backups.remove(index);
                        app.backup.panel.clear();
                        app.save_settings();
                    }
                }),
            });
        }

        if self.backup.panel.ui(ui, &LABELS, busy, false) {
            let Some(plan) = self.backup.panel.plan.take() else {
                return;
            };
            let options = ExecOptions {
                verify: self.settings.verify_copies,
                removal: self.settings.removal,
            };
            self.spawn(ui.ctx(), "Backing up", move |progress| {
                // A backup only looks at what's there: no history to keep.
                let mut state = State::default();
                let result = sync::execute(&plan, &mut state, &options, progress);
                Box::new(move |app| match result {
                    Ok(report) => {
                        app.backup.panel.report = Some(report);
                        if let Some(b) = app.settings.backups.iter_mut().find(|b| b.id == backup.id)
                        {
                            b.last_run = Some(widgets::now());
                        }
                        app.save_settings();
                    }
                    Err(e) => app.error(format!("{e:#}")),
                })
            });
        } else if self.backup.panel.plan.is_none()
            && self.backup.panel.report.is_none()
            && self.backup.panel.integrity.is_none()
            && !editing
        {
            widgets::empty(
                ui,
                Icon::Search,
                "Compare first",
                "Compare shows what the backup would copy and remove. Nothing changes before \
                 you start it.",
            );
        }
    }
}
