//! The result of a comparison: what a synchronization or a backup would
//! do, file by file, to check and adjust before it runs.

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use gotthard_core::Report;
use gotthard_core::fsx::format_bytes;
use gotthard_core::integrity::IntegrityReport;
use gotthard_core::sync::{Action, Op, Plan, Side, State, Summary, Why};

use crate::icons::Icon;
use crate::theme::bold;
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
    pub run_icon: Icon,
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
            widgets::notice(ui, Icon::Alert, theme::WARN, warning);
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
            run = widgets::primary(ui, ready, labels.run_icon, &text).clicked();
            ui.add_space(6.0);
            let normal = ui.visuals().text_color();
            if s.to_device > 0 {
                widgets::badge(
                    ui,
                    Some(Icon::ArrowRight),
                    &format!(
                        "{}: {} ({})",
                        labels.device,
                        s.to_device,
                        format_bytes(s.to_device_bytes)
                    ),
                    theme::ACCENT,
                );
            }
            if s.to_master > 0 {
                widgets::badge(
                    ui,
                    Some(Icon::ArrowLeft),
                    &format!(
                        "{}: {} ({})",
                        labels.master,
                        s.to_master,
                        format_bytes(s.to_master_bytes)
                    ),
                    theme::ACCENT,
                );
            }
            if s.delete_device + s.delete_master > 0 {
                widgets::badge(
                    ui,
                    Some(Icon::Trash),
                    &format!("Deletions: {}", s.delete_device + s.delete_master),
                    theme::DANGER,
                );
            }
            if s.conflicts > 0 {
                widgets::badge(
                    ui,
                    Some(Icon::Alert),
                    &format!("Undecided: {}", s.conflicts),
                    theme::WARN,
                );
            }
            widgets::badge(
                ui,
                Some(Icon::Equal),
                &format!("Equal: {}", s.equal),
                normal.gamma_multiply(0.7),
            );
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
            ui.add_space(8.0);
            Icon::Search.show(ui);
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("Find a file")
                        .desired_width(200.0),
                )
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
        theme::well(ui.style()).show(ui, |ui| {
            // The widths follow the window: the file and the reason share
            // what the fixed columns leave.
            let spare = (ui.available_width() - 168.0 * 2.0 - 116.0 - 4.0 * 8.0 - 14.0).max(290.0);
            let why = (spare * 0.36).clamp(110.0, 250.0);
            TableBuilder::new(ui)
                .striped(true)
                .auto_shrink(false)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(spare - why).clip(true))
                .column(Column::exact(168.0).clip(true))
                .column(Column::exact(116.0))
                .column(Column::exact(168.0).clip(true))
                .column(Column::exact(why).clip(true))
                .header(26.0, |mut header| {
                    for title in ["File", labels.master, "Action", labels.device, "Why"] {
                        header.col(|ui| widgets::column_title(ui, title));
                    }
                })
                .body(|body| {
                    body.rows(28.0, visible.len(), |mut row| {
                        let index = visible[row.index()];
                        let item = &plan.items[index];
                        let equal = item.why == Why::Equal;
                        let shade = |text: String| {
                            let text = RichText::new(text);
                            if equal { text.weak() } else { text }
                        };
                        row.col(|ui| {
                            let name = item.rel.rsplit('/').next().unwrap_or(&item.rel);
                            widgets::file_label(ui, name, item.is_dir, shade(item.rel.clone()))
                                .on_hover_text(&item.rel);
                        });
                        row.col(|ui| {
                            ui.label(shade(widgets::stamp_text(item.master, item.is_dir)));
                        });
                        row.col(|ui| {
                            let weak = ui.visuals().weak_text_color();
                            // Text, icon, the icon after the text, color.
                            let (text, icon, after, color) = match item.op() {
                                Op::Copy { to: Side::Device } => {
                                    ("Copy", Icon::ArrowRight, true, theme::ACCENT)
                                }
                                Op::Copy { to: Side::Master } => {
                                    ("Copy", Icon::ArrowLeft, false, theme::ACCENT)
                                }
                                Op::CreateDir(Side::Device) => {
                                    ("Create", Icon::ArrowRight, true, theme::ACCENT)
                                }
                                Op::CreateDir(Side::Master) => {
                                    ("Create", Icon::ArrowLeft, false, theme::ACCENT)
                                }
                                Op::Delete(Side::Device) => {
                                    ("Delete", Icon::ArrowRight, true, theme::DANGER)
                                }
                                Op::Delete(Side::Master) => {
                                    ("Delete", Icon::ArrowLeft, false, theme::DANGER)
                                }
                                Op::None if item.action == Action::Conflict => {
                                    ("Undecided", Icon::Alert, false, theme::WARN)
                                }
                                Op::None if equal => ("Equal", Icon::Equal, false, weak),
                                Op::None => ("Leave", Icon::Close, false, weak),
                            };
                            let label = RichText::new(text).color(color);
                            let image = icon.image(14.0, color);
                            if directions && item.can_change() {
                                let button = if after {
                                    egui::Button::new((label, image))
                                } else {
                                    egui::Button::image_and_text(image, label)
                                }
                                .fill(theme::soft(color, ui.visuals()))
                                .min_size(egui::vec2(104.0, 22.0));
                                let response = ui
                                    .add(button)
                                    .on_hover_text("Choose what happens to this file");
                                egui::Popup::menu(&response).show(|ui| {
                                    for (label, icon, action) in [
                                        (
                                            format!("{} wins", labels.master),
                                            Icon::ArrowRight,
                                            Action::ToDevice,
                                        ),
                                        (
                                            format!("{} wins", labels.device),
                                            Icon::ArrowLeft,
                                            Action::ToMaster,
                                        ),
                                        (
                                            "Leave both as they are".to_owned(),
                                            Icon::Close,
                                            Action::Nothing,
                                        ),
                                    ] {
                                        if widgets::button(ui, true, icon, &label).clicked() {
                                            change = Some((index, action));
                                            ui.close();
                                        }
                                    }
                                });
                            } else {
                                ui.add(image);
                                ui.label(label);
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
                            let full = why.text().to_owned();
                            ui.add(
                                egui::Label::new(if item.conflict {
                                    why.color(theme::WARN)
                                } else {
                                    why.weak()
                                })
                                .truncate(),
                            )
                            .on_hover_text(full);
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
        ui.horizontal(|ui| {
            let icon = if report.damaged.is_empty() {
                Icon::CheckCircle
            } else {
                Icon::Alert
            };
            ui.add(icon.image(18.0, color));
            ui.label(bold(format!(
                "{head}: {} intact, {} new, {} changed since the last check, {} gone, {} damaged.",
                report.intact,
                report.new.len(),
                report.modified.len(),
                report.missing.len(),
                report.damaged.len()
            )));
        });
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
