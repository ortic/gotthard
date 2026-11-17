//! The result of a comparison: what a synchronization or a backup would
//! do, file by file, to check and adjust before it runs.

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use gotthard_core::Report;
use gotthard_core::fsx::format_bytes;
use gotthard_core::integrity::IntegrityReport;
use gotthard_core::sync::{Action, Op, Plan, Side, State, Summary, Why};

use crate::{theme, widgets};

/// How the two sides and the operation are called.
pub struct Labels {
    pub master: &'static str,
    pub device: &'static str,
    /// How the reasons call the sides: "the master", "this computer".
    pub the_master: &'static str,
    pub the_device: &'static str,
    /// "Synchronization", "Backup".
    pub what: &'static str,
    /// "Synchronize", "Back up".
    pub run: &'static str,
}

#[derive(Default)]
pub struct PlanPanel {
    /// The pair or backup the plan is of.
    pub owner: String,
    pub plan: Option<Plan>,
    pub state: Option<State>,
    pub report: Option<Report>,
    pub integrity: Option<IntegrityReport>,
    show_equal: bool,
    /// The user has seen the warning about a mass deletion.
    confirmed: bool,
    search: String,
    /// The items shown, and the totals; rebuilt when something changes.
    visible: Vec<usize>,
    summary: Summary,
    dirty: bool,
}

impl PlanPanel {
    pub fn set(&mut self, owner: &str, plan: Plan, state: Option<State>) {
        self.owner = owner.to_owned();
        self.plan = Some(plan);
        self.state = state;
        self.report = None;
        self.integrity = None;
        self.confirmed = false;
        self.dirty = true;
    }

    /// Forgets the plan, e.g. when its folders change.
    pub fn clear(&mut self) {
        self.plan = None;
        self.state = None;
        self.report = None;
        self.integrity = None;
    }

    fn refresh(&mut self) {
        let Some(plan) = &self.plan else { return };
        let needle = self.search.to_lowercase();
        self.visible = plan
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| self.show_equal || item.why != Why::Equal)
            .filter(|(_, item)| needle.is_empty() || item.rel.to_lowercase().contains(&needle))
            .map(|(i, _)| i)
            .collect();
        self.summary = plan.summary();
        self.dirty = false;
    }

    /// Shows the plan. True when the user starts it.
    pub fn ui(&mut self, ui: &mut Ui, labels: &Labels, busy: bool, directions: bool) -> bool {
        if let Some(report) = &self.report {
            widgets::report(ui, report, labels.what);
            ui.add_space(6.0);
        }
        if let Some(report) = &self.integrity {
            integrity_ui(ui, report);
            ui.add_space(6.0);
        }
        if self.plan.is_none() {
            return false;
        }
        if self.dirty {
            self.refresh();
        }
        let plan = self.plan.as_mut().expect("checked above");
        for warning in &plan.warnings {
            theme::banner(ui.style(), theme::WARN).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(warning);
            });
        }
        let s = self.summary;
        let mut run = false;
        ui.horizontal_wrapped(|ui| {
            let changes = s.changes();
            let ready = !busy && changes > 0 && (!plan.mass_deletion || self.confirmed);
            let text = if changes == 0 {
                "Nothing to do".to_owned()
            } else {
                format!("{} ({})", labels.run, widgets::count(changes, "change"))
            };
            run = widgets::primary(ui, ready, &text).clicked();
            let chip = |ui: &mut Ui, text: String, color: egui::Color32| {
                ui.label(RichText::new(text).color(color));
            };
            let normal = ui.visuals().text_color();
            if s.to_device > 0 {
                chip(
                    ui,
                    format!(
                        "→ {}: {} ({})",
                        labels.device,
                        s.to_device,
                        format_bytes(s.to_device_bytes)
                    ),
                    normal,
                );
            }
            if s.to_master > 0 {
                chip(
                    ui,
                    format!(
                        "← {}: {} ({})",
                        labels.master,
                        s.to_master,
                        format_bytes(s.to_master_bytes)
                    ),
                    normal,
                );
            }
            if s.delete_device + s.delete_master > 0 {
                chip(
                    ui,
                    format!("Deletions: {}", s.delete_device + s.delete_master),
                    theme::DANGER,
                );
            }
            if s.conflicts > 0 {
                chip(ui, format!("Undecided: {}", s.conflicts), theme::WARN);
            }
            ui.weak(format!("Equal: {}", s.equal));
        });
        if plan.mass_deletion {
            ui.checkbox(
                &mut self.confirmed,
                RichText::new("I have checked both folders: delete these files")
                    .color(theme::DANGER),
            );
        }
        ui.horizontal(|ui| {
            if ui
                .checkbox(&mut self.show_equal, "Show equal files")
                .changed()
            {
                self.dirty = true;
            }
            ui.label("Find:");
            if ui
                .add(egui::TextEdit::singleline(&mut self.search).desired_width(180.0))
                .changed()
            {
                self.dirty = true;
            }
            if directions && s.conflicts > 0 {
                ui.separator();
                ui.label("Decide all undecided:");
                if ui.button(format!("{} wins", labels.master)).clicked() {
                    plan.resolve_conflicts(Side::Master);
                    self.dirty = true;
                }
                if ui.button(format!("{} wins", labels.device)).clicked() {
                    plan.resolve_conflicts(Side::Device);
                    self.dirty = true;
                }
            }
        });
        ui.add_space(4.0);

        let mut change = None;
        let visible = &self.visible;
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::remainder().at_least(200.0).clip(true))
            .column(Column::initial(190.0).clip(true))
            .column(Column::initial(120.0))
            .column(Column::initial(190.0).clip(true))
            .column(Column::initial(230.0).clip(true))
            .header(24.0, |mut header| {
                for title in ["File", labels.master, "", labels.device, "Why"] {
                    header.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(24.0, visible.len(), |mut row| {
                    let index = visible[row.index()];
                    let item = &plan.items[index];
                    let equal = item.why == Why::Equal;
                    let shade = |text: String| {
                        let text = RichText::new(text);
                        if equal { text.weak() } else { text }
                    };
                    row.col(|ui| {
                        let name = if item.is_dir {
                            format!("{}/", item.rel)
                        } else {
                            item.rel.clone()
                        };
                        ui.add(egui::Label::new(shade(name)).truncate());
                    });
                    row.col(|ui| {
                        ui.label(shade(widgets::stamp_text(item.master, item.is_dir)));
                    });
                    row.col(|ui| {
                        let (text, color) = match item.op() {
                            Op::Copy { to: Side::Device } => ("copy →", theme::ACCENT),
                            Op::Copy { to: Side::Master } => ("← copy", theme::ACCENT),
                            Op::CreateDir(Side::Device) => ("create →", theme::ACCENT),
                            Op::CreateDir(Side::Master) => ("← create", theme::ACCENT),
                            Op::Delete(Side::Device) => ("delete →", theme::DANGER),
                            Op::Delete(Side::Master) => ("← delete", theme::DANGER),
                            Op::None if item.action == Action::Conflict => {
                                ("undecided", theme::WARN)
                            }
                            Op::None if equal => ("=", ui.visuals().weak_text_color()),
                            Op::None => ("leave", ui.visuals().weak_text_color()),
                        };
                        let text = RichText::new(text).color(color).strong();
                        if directions && item.can_change() {
                            ui.menu_button(text, |ui| {
                                for (label, action) in [
                                    (format!("{} wins  →", labels.master), Action::ToDevice),
                                    (format!("←  {} wins", labels.device), Action::ToMaster),
                                    ("Leave both as they are".to_owned(), Action::Nothing),
                                ] {
                                    if ui.button(label).clicked() {
                                        change = Some((index, action));
                                        ui.close();
                                    }
                                }
                            });
                        } else {
                            ui.label(text);
                        }
                    });
                    row.col(|ui| {
                        ui.label(shade(widgets::stamp_text(item.device, item.is_dir)));
                    });
                    row.col(|ui| {
                        let why = RichText::new(
                            item.why
                                .text()
                                .replace("the master", labels.the_master)
                                .replace("this computer", labels.the_device),
                        );
                        ui.label(if item.conflict {
                            why.color(theme::WARN)
                        } else {
                            why.weak()
                        });
                    });
                });
            });
        if let Some((index, action)) = change {
            plan.items[index].action = action;
            self.dirty = true;
        }
        run
    }
}

/// What a check for damage found.
pub fn integrity_ui(ui: &mut Ui, report: &IntegrityReport) {
    let color = if !report.damaged.is_empty() {
        theme::DANGER
    } else if report.cancelled || !report.errors.is_empty() {
        theme::WARN
    } else {
        theme::SUCCESS
    };
    theme::banner(ui.style(), color).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let head = if report.cancelled {
            "Check cancelled"
        } else if report.damaged.is_empty() {
            "No damage found"
        } else {
            "Damaged files found"
        };
        ui.label(
            RichText::new(format!(
                "{head}: {} intact, {} new, {} changed since the last check, {} gone, {} damaged.",
                report.intact,
                report.new.len(),
                report.modified.len(),
                report.missing.len(),
                report.damaged.len()
            ))
            .strong(),
        );
        if !report.damaged.is_empty() {
            ui.label(
                "These files have the size and time they had at the last check, but other \
                 content: the disk returned other data than was written. Replace them from a \
                 good copy (the other side of a synchronization, a backup).",
            );
            egui::ScrollArea::vertical()
                .id_salt("damaged")
                .max_height(140.0)
                .show(ui, |ui| {
                    for file in &report.damaged {
                        ui.colored_label(theme::DANGER, file);
                    }
                });
        }
        if !report.errors.is_empty() {
            ui.collapsing(format!("Unreadable ({})", report.errors.len()), |ui| {
                for (file, why) in report.errors.iter().take(200) {
                    ui.label(format!("{file}: {why}"));
                }
            });
        }
    });
}
