//! Merge folders: several folder trees become one.

use std::path::PathBuf;

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use gotthard_core::fsx::format_bytes;
use gotthard_core::merge::{self, MergeAction, MergeOptions, MergePlan, OnConflict};
use gotthard_core::{Report, is_cancelled};

use crate::app::{App, empty_hint};
use crate::{theme, widgets};

#[derive(Default)]
pub struct MergeView {
    pub sources: Vec<PathBuf>,
    pub target: PathBuf,
    pub move_files: bool,
    pub on_conflict: OnConflict,
    pub plan: Option<MergePlan>,
    pub report: Option<Report>,
    show_identical: bool,
}

fn conflict_title(policy: OnConflict) -> &'static str {
    match policy {
        OnConflict::KeepBoth => "Keep both (the incoming file gets a number)",
        OnConflict::KeepNewer => "Keep the newer one",
        OnConflict::KeepTarget => "Keep the one in the target",
        OnConflict::Replace => "Replace the one in the target",
    }
}

impl App {
    fn merge_options(&self) -> MergeOptions {
        MergeOptions {
            move_files: self.merge.move_files,
            on_conflict: self.merge.on_conflict,
            filter: self.settings.filter(),
            verify: self.settings.verify_copies,
            removal: self.settings.removal,
        }
    }

    pub fn merge_ui(&mut self, ui: &mut Ui) {
        widgets::heading(
            ui,
            "Merge folders",
            "Bring the files of several folder trees together in one. Files with the same content \
             are taken once.",
        );
        let busy = self.busy();
        let mut changed = false;
        let mut compare = false;
        theme::card(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.strong("Folders to merge");
            changed |= widgets::folder_list(ui, &mut self.merge.sources, "Add a folder…");
            ui.add_space(6.0);
            ui.strong("Into");
            changed |= widgets::folder_field(ui, &mut self.merge.target, "The folder that gets everything");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                changed |= ui
                    .radio_value(&mut self.merge.move_files, false, "Copy the files")
                    .changed();
                changed |= ui
                    .radio_value(
                        &mut self.merge.move_files,
                        true,
                        "Move them (the folders are emptied once their files are safely in the target)",
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("A different file with the same path:");
                egui::ComboBox::from_id_salt("on-conflict")
                    .width(300.0)
                    .selected_text(conflict_title(self.merge.on_conflict))
                    .show_ui(ui, |ui| {
                        for policy in [
                            OnConflict::KeepBoth,
                            OnConflict::KeepNewer,
                            OnConflict::KeepTarget,
                            OnConflict::Replace,
                        ] {
                            changed |= ui
                                .selectable_value(
                                    &mut self.merge.on_conflict,
                                    policy,
                                    conflict_title(policy),
                                )
                                .changed();
                        }
                    });
            });
            ui.add_space(6.0);
            let ready = !widgets::set_folders(&self.merge.sources).is_empty()
                && !self.merge.target.as_os_str().is_empty();
            compare = widgets::primary(ui, ready && !busy, "Compare").clicked();
        });
        if changed {
            self.merge.plan = None;
        }
        if compare {
            let sources = widgets::set_folders(&self.merge.sources);
            let target = self.merge.target.clone();
            let options = self.merge_options();
            self.merge.plan = None;
            self.merge.report = None;
            self.spawn(ui.ctx(), "Comparing", move |progress| {
                let result = merge::plan(&sources, &target, &options, progress);
                Box::new(move |app| match result {
                    Ok(plan) => app.merge.plan = Some(plan),
                    Err(e) if is_cancelled(&e) => {}
                    Err(e) => app.error(format!("{e:#}")),
                })
            });
        }
        ui.add_space(8.0);
        if let Some(report) = &self.merge.report {
            widgets::report(ui, report, "Merge");
            ui.add_space(6.0);
        }
        let Some(plan) = &self.merge.plan else {
            if self.merge.report.is_none() {
                empty_hint(
                    ui,
                    "Compare shows where every file would go. Nothing changes before you start the merge.",
                );
            }
            return;
        };
        for warning in &plan.warnings {
            theme::banner(ui.style(), theme::WARN).show(ui, |ui| {
                ui.label(warning);
            });
        }
        let (files, bytes) = plan.to_copy();
        let identical = plan.count(&MergeAction::Identical);
        let mut run = false;
        ui.horizontal_wrapped(|ui| {
            let work = files > 0 || (self.merge.move_files && identical > 0);
            let verb = if self.merge.move_files {
                "Move"
            } else {
                "Copy"
            };
            let text = if work {
                format!(
                    "Merge: {verb} {} ({})",
                    widgets::count(files, "file"),
                    format_bytes(bytes)
                )
            } else {
                "Nothing to do".to_owned()
            };
            run = widgets::primary(ui, work && !busy, &text).clicked();
            ui.label(format!("New: {}", plan.count(&MergeAction::Copy)));
            ui.label(format!("Kept both: {}", plan.count(&MergeAction::KeepBoth)));
            ui.label(format!("Replacing: {}", plan.count(&MergeAction::Replace)));
            ui.label(format!(
                "Staying in their folder: {}",
                plan.count(&MergeAction::KeepTarget)
            ));
            ui.weak(format!("Already there: {identical}"));
            ui.checkbox(&mut self.merge.show_identical, "Show files already there");
        });
        ui.add_space(4.0);
        let visible: Vec<usize> = plan
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| self.merge.show_identical || i.action != MergeAction::Identical)
            .map(|(i, _)| i)
            .collect();
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(160.0).clip(true))
            .column(Column::remainder().at_least(160.0).clip(true))
            .column(Column::initial(90.0))
            .column(Column::initial(260.0).clip(true))
            .header(24.0, |mut header| {
                for title in ["From", "File", "Size", "What happens"] {
                    header.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(24.0, visible.len(), |mut row| {
                    let item = &plan.items[visible[row.index()]];
                    row.col(|ui| {
                        let source = &plan.sources[item.source];
                        let name = source
                            .file_name()
                            .map_or(source.to_string_lossy(), |n| n.to_string_lossy());
                        ui.add(egui::Label::new(RichText::new(name).weak()).truncate())
                            .on_hover_text(source.to_string_lossy());
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(&item.rel).truncate());
                    });
                    row.col(|ui| {
                        ui.label(format_bytes(item.stamp.size));
                    });
                    row.col(|ui| {
                        let (text, color) = match &item.action {
                            MergeAction::Copy => ("goes to the target".to_owned(), theme::ACCENT),
                            MergeAction::Identical => (
                                "is there already".to_owned(),
                                ui.visuals().weak_text_color(),
                            ),
                            MergeAction::KeepBoth => {
                                let name = item.target_rel.rsplit('/').next().unwrap_or_default();
                                (format!("kept beside it as {name}"), theme::WARN)
                            }
                            MergeAction::Replace => {
                                ("replaces the target's version".to_owned(), theme::DANGER)
                            }
                            MergeAction::KeepTarget => (
                                "stays: the target's version is kept".to_owned(),
                                theme::WARN,
                            ),
                        };
                        ui.add(egui::Label::new(RichText::new(text).color(color)).truncate());
                    });
                });
            });
        if run {
            let plan = self.merge.plan.take().expect("shown above");
            let options = self.merge_options();
            self.spawn(ui.ctx(), "Merging", move |progress| {
                let result = merge::execute(&plan, &options, progress);
                Box::new(move |app| match result {
                    Ok(report) => app.merge.report = Some(report),
                    Err(e) => app.error(format!("{e:#}")),
                })
            });
        }
    }
}
