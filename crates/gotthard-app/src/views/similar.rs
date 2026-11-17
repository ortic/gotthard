//! Similar names: versions and copies of a file that aren't identical.

use std::path::PathBuf;

use egui::{RichText, Ui};
use gotthard_core::fsx::{format_bytes, format_time};
use gotthard_core::is_cancelled;
use gotthard_core::rename::Rule;
use gotthard_core::similar::{self, SimilarGroup, SimilarOptions};

use crate::app::{App, View, empty_hint};
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
        widgets::heading(
            ui,
            "Similar names",
            "Find copies and versions of a file by their names: “Report (1)”, “Report - Copy”, \
             “report_final_v2”.",
        );
        let busy = self.busy();
        let mut search = false;
        theme::card(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.strong("Folders to search");
            widgets::folder_list(ui, &mut self.similar.folders, "Add a folder…");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let ready = !widgets::set_folders(&self.similar.folders).is_empty();
                search = widgets::primary(ui, ready && !busy, "Search").clicked();
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
            empty_hint(ui, "Search lists the files whose names belong together.");
            return;
        };
        if groups.is_empty() {
            empty_hint(ui, "No similar names found.");
            return;
        }
        ui.label(format!(
            "{} of similar names.",
            widgets::count(groups.len(), "group")
        ));
        ui.add_space(4.0);
        let mut rename = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                // Far more groups than fit a screen are rare; the first ones do.
                for (index, group) in groups.iter().take(2000).enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&group.name).strong());
                        ui.weak(format!("{} files", group.files.len()));
                        if ui.small_button("Rename these…").clicked() {
                            rename = Some(index);
                        }
                    });
                    for file in &group.files {
                        ui.horizontal(|ui| {
                            ui.add_space(12.0);
                            ui.weak(format!(
                                "{} · {}",
                                format_time(file.mtime),
                                format_bytes(file.size)
                            ));
                            let label = egui::Label::new(file.path.to_string_lossy())
                                .truncate()
                                .sense(egui::Sense::click());
                            if ui
                                .add(label)
                                .on_hover_text("Double-click to show the file's folder")
                                .double_clicked()
                                && let Some(folder) = file.path.parent()
                            {
                                widgets::open_in_os(folder);
                            }
                        });
                    }
                    ui.add_space(6.0);
                }
                if groups.len() > 2000 {
                    ui.weak(format!("… and {} more groups", groups.len() - 2000));
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
