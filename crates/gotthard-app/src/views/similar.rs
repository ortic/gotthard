//! Similar names: versions and copies of a file that aren't identical.

use std::path::PathBuf;

use egui::Ui;
use gotthard_core::fsx::{format_bytes, format_time};
use gotthard_core::is_cancelled;
use gotthard_core::rename::Rule;
use gotthard_core::similar::{self, SimilarGroup, SimilarOptions};

use crate::app::{App, View};
use crate::icons::{self, Icon};
use crate::theme::bold;
use crate::{theme, widgets};

pub struct SimilarView {
    pub folders: Vec<PathBuf>,
    /// 50..=100; 100 groups only names equal once cleaned.
    pub likeness: u32,
    pub same_extension: bool,
    pub groups: Option<Vec<SimilarGroup>>,
}

impl Default for SimilarView {
    fn default() -> Self {
        Self {
            folders: Vec::new(),
            likeness: 100,
            same_extension: true,
            groups: None,
        }
    }
}

impl App {
    pub fn similar_ui(&mut self, ui: &mut Ui) {
        widgets::page(
            ui,
            Icon::Similar,
            "Similar names",
            "Find copies and versions of a file by their names: “Report (1)”, “Report - Copy”, \
             “report_final_v2”.",
        );
        let busy = self.busy();
        let mut search = false;
        theme::card(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(bold("Folders to search"));
            widgets::folder_list(ui, &mut self.similar.folders, "Add a folder…");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let ready = !widgets::set_folders(&self.similar.folders).is_empty();
                search = widgets::primary(ui, ready && !busy, Icon::Search, "Search").clicked();
                ui.add_space(8.0);
                ui.label("Names alike by at least");
                ui.add(egui::Slider::new(&mut self.similar.likeness, 50..=100).suffix(" %"))
                    .on_hover_text(
                        "100 %: the same name once copy and version markers are removed. Less: \
                         also names that differ in a few letters.",
                    );
                ui.checkbox(&mut self.similar.same_extension, "Only the same file type");
            });
        });
        if search {
            let roots = widgets::set_folders(&self.similar.folders);
            let options = SimilarOptions {
                threshold: f64::from(self.similar.likeness) / 100.0,
                same_extension: self.similar.same_extension,
                filter: self.settings.filter(),
            };
            self.similar.groups = None;
            self.spawn(ui.ctx(), "Searching similar names", move |progress| {
                let result = similar::find(&roots, &options, progress);
                Box::new(move |app| match result {
                    Ok(groups) => app.similar.groups = Some(groups),
                    Err(e) if is_cancelled(&e) => {}
                    Err(e) => app.error(format!("{e:#}")),
                })
            });
        }
        ui.add_space(8.0);
        let Some(groups) = &self.similar.groups else {
            widgets::empty(
                ui,
                Icon::Similar,
                "Find copies and versions",
                "Search lists the files whose names belong together, to compare or to rename them.",
            );
            return;
        };
        if groups.is_empty() {
            widgets::empty(
                ui,
                Icon::CheckCircle,
                "No similar names",
                "No two files here have names that belong together.",
            );
            return;
        }
        ui.horizontal(|ui| {
            widgets::badge(
                ui,
                Some(Icon::Similar),
                &widgets::count(groups.len(), "group"),
                theme::ACCENT,
            );
        });
        ui.add_space(6.0);
        let mut rename = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                // Far more groups than fit a screen are rare; the first ones do.
                for (index, group) in groups.iter().take(1000).enumerate() {
                    theme::card(ui.style()).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(bold(&group.name));
                            ui.weak(widgets::count(group.files.len(), "file"));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if widgets::button(ui, true, Icon::Rename, "Rename these…")
                                        .clicked()
                                    {
                                        rename = Some(index);
                                    }
                                },
                            );
                        });
                        for file in &group.files {
                            ui.horizontal(|ui| {
                                let name = file
                                    .path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                let (icon, color) = icons::for_file(&name);
                                ui.add(icon.image(16.0, color));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.weak(format!(
                                            "{} · {}",
                                            format_bytes(file.size),
                                            format_time(file.mtime)
                                        ));
                                        ui.with_layout(
                                            egui::Layout::left_to_right(egui::Align::Center),
                                            |ui| {
                                                let label =
                                                    egui::Label::new(file.path.to_string_lossy())
                                                        .truncate()
                                                        .sense(egui::Sense::click());
                                                if ui
                                                    .add(label)
                                                    .on_hover_text(
                                                        "Double-click to show the file's folder",
                                                    )
                                                    .double_clicked()
                                                    && let Some(folder) = file.path.parent()
                                                {
                                                    widgets::open_in_os(folder);
                                                }
                                            },
                                        );
                                    },
                                );
                            });
                        }
                    });
                    ui.add_space(6.0);
                }
                if groups.len() > 1000 {
                    ui.weak(format!("… and {} more groups", groups.len() - 1000));
                }
            });
        if let Some(index) = rename {
            let files = groups[index].files.iter().map(|f| f.path.clone()).collect();
            self.rename.load(files);
            if self.rename.rules.is_empty() {
                self.rename.rules.push(Rule::Tidy);
            }
            self.view = View::Rename;
        }
    }
}
