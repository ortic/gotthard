//! Duplicates: files with the same content, whatever they're called.

use std::collections::HashSet;
use std::path::PathBuf;

use egui::{RichText, Ui};
use gotthard_core::dupes::{self, DupeOptions, DupeResult, Keep};
use gotthard_core::fsx::{Removal, format_bytes, format_time};
use gotthard_core::{Report, is_cancelled};

use crate::app::{App, Confirm, empty_hint};
use crate::{theme, widgets};

#[derive(Clone, Copy)]
enum Row {
    Group(usize),
    File(usize, usize),
}

pub struct DupesView {
    pub folders: Vec<PathBuf>,
    /// Smaller files are left out, in KB.
    pub min_kb: u64,
    pub result: Option<DupeResult>,
    /// Group and file of the files to remove.
    pub marked: HashSet<(usize, usize)>,
    pub report: Option<Report>,
    rows: Vec<Row>,
}

impl Default for DupesView {
    fn default() -> Self {
        Self {
            folders: Vec::new(),
            min_kb: 1,
            result: None,
            marked: HashSet::new(),
            report: None,
            rows: Vec::new(),
        }
    }
}

impl DupesView {
    pub fn set(&mut self, result: DupeResult) {
        self.rows = result
            .groups
            .iter()
            .enumerate()
            .flat_map(|(g, group)| {
                std::iter::once(Row::Group(g))
                    .chain((0..group.files.len()).map(move |f| Row::File(g, f)))
            })
            .collect();
        self.marked.clear();
        self.result = Some(result);
    }

    /// Marks every file but the one `keep` keeps of each group.
    pub fn mark_all_but(&mut self, keep: Keep) {
        let Some(result) = &self.result else { return };
        self.marked.clear();
        for (g, group) in result.groups.iter().enumerate() {
            let keeper = dupes::keeper(group, keep);
            self.marked.extend(
                (0..group.files.len())
                    .filter(|f| *f != keeper)
                    .map(|f| (g, f)),
            );
        }
    }
}

impl App {
    pub fn dupes_ui(&mut self, ui: &mut Ui) {
        widgets::heading(
            ui,
            "Duplicates",
            "Find files with exactly the same content, compared byte for byte by their hash.",
        );
        let busy = self.busy();
        let mut search = false;
        theme::card(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.strong("Folders to search");
            widgets::folder_list(ui, &mut self.dupes.folders, "Add a folder…");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let ready = !widgets::set_folders(&self.dupes.folders).is_empty();
                search = widgets::primary(ui, ready && !busy, "Search").clicked();
                ui.label("Leave out files smaller than");
                ui.add(
                    egui::DragValue::new(&mut self.dupes.min_kb)
                        .range(0..=10_000_000)
                        .suffix(" KB"),
                );
            });
        });
        if search {
            let roots = widgets::set_folders(&self.dupes.folders);
            let options = DupeOptions {
                min_size: self.dupes.min_kb * 1024,
                filter: self.settings.filter(),
            };
            self.dupes.result = None;
            self.dupes.report = None;
            self.spawn(ui.ctx(), "Searching duplicates", move |progress| {
                let result = dupes::find(&roots, &options, progress);
                Box::new(move |app| match result {
                    Ok(result) => app.dupes.set(result),
                    Err(e) if is_cancelled(&e) => {}
                    Err(e) => app.error(format!("{e:#}")),
                })
            });
        }
        ui.add_space(8.0);
        if let Some(report) = &self.dupes.report {
            widgets::report(ui, report, "Removing duplicates");
            ui.add_space(6.0);
        }
        let Some(result) = &self.dupes.result else {
            if self.dupes.report.is_none() {
                empty_hint(
                    ui,
                    "Search lists the duplicates. Nothing is removed before you say so.",
                );
            }
            return;
        };
        if result.groups.is_empty() {
            empty_hint(
                ui,
                &format!("No duplicates among the {} files.", result.scanned),
            );
            return;
        }
        let marked_bytes: u64 = self
            .dupes
            .marked
            .iter()
            .map(|(g, _)| result.groups[*g].size)
            .sum();
        let marked = self.dupes.marked.len();
        let mut keep = None;
        let mut unmark = false;
        let mut remove = false;
        ui.horizontal_wrapped(|ui| {
            remove = widgets::danger(
                ui,
                marked > 0 && !busy,
                &format!(
                    "Remove {} ({})",
                    widgets::count(marked, "marked file"),
                    format_bytes(marked_bytes)
                ),
            )
            .clicked();
            ui.label(format!(
                "{} among {}; {} could be freed.",
                widgets::count(result.groups.len(), "group"),
                widgets::count(result.scanned, "file"),
                format_bytes(result.wasted())
            ));
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Mark all but");
            for (text, value) in [
                ("the oldest", Keep::Oldest),
                ("the newest", Keep::Newest),
                ("the one in the first folder", Keep::FirstFolder),
                ("the shortest path", Keep::ShortestPath),
            ] {
                if ui.button(text).clicked() {
                    keep = Some(value);
                }
            }
            unmark = ui.button("Unmark all").clicked();
        });
        if !result.errors.is_empty() {
            ui.collapsing(
                RichText::new(format!("Unreadable ({})", result.errors.len())).color(theme::WARN),
                |ui| {
                    for (file, why) in result.errors.iter().take(200) {
                        ui.label(format!("{file}: {why}"));
                    }
                },
            );
        }
        ui.add_space(4.0);

        let mut toggle = None;
        let rows = &self.dupes.rows;
        let marks = &self.dupes.marked;
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(
            ui,
            24.0,
            rows.len(),
            |ui, range| {
                for row in &rows[range] {
                    match *row {
                        Row::Group(g) => {
                            let group = &result.groups[g];
                            ui.horizontal(|ui| {
                                ui.set_height(24.0);
                                ui.label(
                                    RichText::new(format!(
                                        "{} files of {} each",
                                        group.files.len(),
                                        format_bytes(group.size)
                                    ))
                                    .strong(),
                                );
                                ui.weak(format!("{} to free", format_bytes(group.wasted())));
                            });
                        }
                        Row::File(g, f) => {
                            let file = &result.groups[g].files[f];
                            ui.horizontal(|ui| {
                                ui.set_height(24.0);
                                ui.add_space(12.0);
                                let mut on = marks.contains(&(g, f));
                                if ui.checkbox(&mut on, "").changed() {
                                    toggle = Some((g, f));
                                }
                                ui.weak(format_time(file.mtime));
                                let text = RichText::new(file.path.to_string_lossy());
                                let label =
                                    egui::Label::new(if on { text.strikethrough() } else { text })
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
                    }
                }
            },
        );
        if let Some(mark) = toggle
            && !self.dupes.marked.remove(&mark)
        {
            self.dupes.marked.insert(mark);
        }
        if let Some(keep) = keep {
            self.dupes.mark_all_but(keep);
        }
        if unmark {
            self.dupes.marked.clear();
        }
        if remove {
            let removal = self.settings.removal;
            let where_to = match removal {
                Removal::Vault => {
                    "They go to the safety copies of the folder they're in and can be restored from there."
                }
                Removal::RecycleBin => "They go to the Recycle Bin.",
                Removal::Permanent => "They are deleted for good.",
            };
            self.confirm = Some(Confirm {
                title: format!("Remove {}?", widgets::count(marked, "duplicate")),
                text: format!("One copy of every file stays. {where_to}"),
                action: "Remove them".into(),
                run: Box::new(move |app, ctx| {
                    let Some(result) = app.dupes.result.take() else {
                        return;
                    };
                    let mut marked: Vec<(usize, usize)> = app.dupes.marked.drain().collect();
                    marked.sort_unstable();
                    app.spawn(ctx, "Removing duplicates", move |progress| {
                        let report = dupes::remove(&result, &marked, removal, progress);
                        Box::new(move |app| app.dupes.report = Some(report))
                    });
                }),
            });
        }
    }
}
