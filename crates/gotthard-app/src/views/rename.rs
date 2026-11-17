//! Rename: many files at once, by rules, with every new name shown before
//! anything is renamed.

use std::path::PathBuf;

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use gotthard_core::rename::{self, Case, Journal, Place, Preview, Rule};

use crate::app::{App, empty_hint};
use crate::{theme, widgets};

#[derive(Default)]
pub struct RenameView {
    pub files: Vec<PathBuf>,
    pub rules: Vec<Rule>,
    previews: Vec<Preview>,
    /// The rules the previews are of.
    shown: Vec<Rule>,
    dirty: bool,
    /// The last rename, to undo it; read from disk when the view first shows.
    journal: Option<Journal>,
    journal_read: bool,
}

impl RenameView {
    /// Takes these files instead of the ones listed.
    pub fn load(&mut self, files: Vec<PathBuf>) {
        self.files = files;
        self.dirty = true;
    }

    pub fn previews(&mut self) -> &[Preview] {
        if self.dirty || self.shown != self.rules {
            self.previews = rename::preview(&self.files, &self.rules);
            self.shown = self.rules.clone();
            self.dirty = false;
        }
        &self.previews
    }
}

fn rule_title(rule: &Rule) -> &'static str {
    match rule {
        Rule::Replace { .. } => "Replace text",
        Rule::Insert { .. } => "Insert text",
        Rule::Remove { .. } => "Remove characters",
        Rule::Case(_) => "Change case",
        Rule::Number { .. } => "Number the files",
        Rule::Date { .. } => "Add the file's date",
        Rule::Tidy => "Tidy up",
        Rule::Extension { .. } => "Change the extension",
        Rule::ExtensionCase { .. } => "Extension's case",
    }
}

fn new_rules() -> [Rule; 9] {
    [
        Rule::Replace {
            find: String::new(),
            with: String::new(),
            regex: false,
            case_sensitive: false,
        },
        Rule::Insert {
            text: String::new(),
            at: Place::Start,
        },
        Rule::Remove {
            from: 0,
            count: 1,
            from_end: false,
        },
        Rule::Case(Case::Lower),
        Rule::Number {
            start: 1,
            step: 1,
            digits: 3,
            at: Place::End,
            separator: " ".into(),
        },
        Rule::Date {
            format: "%Y-%m-%d".into(),
            at: Place::Start,
            separator: " ".into(),
        },
        Rule::Tidy,
        Rule::Extension { to: String::new() },
        Rule::ExtensionCase { upper: false },
    ]
}

fn place_ui(ui: &mut Ui, id: usize, at: &mut Place) {
    egui::ComboBox::from_id_salt(("place", id))
        .width(90.0)
        .selected_text(match at {
            Place::Start => "at the start",
            Place::End => "at the end",
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(at, Place::Start, "at the start");
            ui.selectable_value(at, Place::End, "at the end");
        });
}

fn text(ui: &mut Ui, value: &mut String, hint: &str, width: f32) {
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .desired_width(width),
    );
}

fn rule_ui(ui: &mut Ui, id: usize, rule: &mut Rule) {
    match rule {
        Rule::Replace {
            find,
            with,
            regex,
            case_sensitive,
        } => {
            text(ui, find, "find", 150.0);
            ui.label("→");
            text(ui, with, "replace with", 150.0);
            ui.checkbox(case_sensitive, "Match case");
            ui.checkbox(regex, "Regular expression");
        }
        Rule::Insert { text: value, at } => {
            text(ui, value, "text", 200.0);
            place_ui(ui, id, at);
        }
        Rule::Remove {
            from,
            count,
            from_end,
        } => {
            ui.add(egui::DragValue::new(count).range(0..=255));
            ui.label("characters, after skipping");
            ui.add(egui::DragValue::new(from).range(0..=255));
            ui.checkbox(from_end, "Counted from the end");
        }
        Rule::Case(case) => {
            for (value, title) in [
                (Case::Lower, "lower case"),
                (Case::Upper, "UPPER CASE"),
                (Case::Title, "Every Word"),
                (Case::Sentence, "First letter"),
            ] {
                ui.radio_value(case, value, title);
            }
        }
        Rule::Number {
            start,
            step,
            digits,
            at,
            separator,
        } => {
            ui.label("from");
            ui.add(egui::DragValue::new(start));
            ui.label("in steps of");
            ui.add(egui::DragValue::new(step).range(1..=1000));
            ui.label("digits");
            ui.add(egui::DragValue::new(digits).range(1..=10));
            place_ui(ui, id, at);
            ui.label("separated by");
            text(ui, separator, "", 40.0);
        }
        Rule::Date {
            format,
            at,
            separator,
        } => {
            text(ui, format, "%Y-%m-%d", 110.0);
            ui.weak("(%Y year, %m month, %d day, %H hour, %M minute)");
            place_ui(ui, id, at);
            ui.label("separated by");
            text(ui, separator, "", 40.0);
        }
        Rule::Tidy => {
            ui.weak("Removes “ (2)”, “ - Copy” and doubled spaces.");
        }
        Rule::Extension { to } => {
            ui.label("to");
            text(ui, to, "jpg", 80.0);
        }
        Rule::ExtensionCase { upper } => {
            ui.radio_value(upper, false, "lower case (.jpg)");
            ui.radio_value(upper, true, "upper case (.JPG)");
        }
    }
}

impl App {
    pub fn rename_ui(&mut self, ui: &mut Ui) {
        widgets::heading(
            ui,
            "Rename",
            "Rename many files in one go. The steps apply from top to bottom; the list shows \
             every new name first.",
        );
        if !self.rename.journal_read {
            self.rename.journal_read = true;
            self.rename.journal = self
                .paths
                .as_ref()
                .and_then(|p| std::fs::read_to_string(p.rename_journal()).ok())
                .and_then(|text| serde_json::from_str(&text).ok());
        }
        let busy = self.busy();
        let (mut apply, mut undo) = (false, false);
        theme::card(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.strong("Files");
                if ui.button("Add files…").clicked() {
                    let start = self
                        .rename
                        .files
                        .last()
                        .and_then(|f| f.parent())
                        .map(PathBuf::from)
                        .unwrap_or_default();
                    for file in widgets::pick_files(&start) {
                        if !self.rename.files.contains(&file) {
                            self.rename.files.push(file);
                        }
                    }
                    self.rename.dirty = true;
                }
                if ui.button("Add a folder's files…").clicked()
                    && let Some(folder) = widgets::pick_folder(std::path::Path::new(""))
                    && let Ok(reader) = std::fs::read_dir(folder)
                {
                    let mut files: Vec<PathBuf> = reader
                        .flatten()
                        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                        .map(|e| e.path())
                        .collect();
                    files.sort();
                    self.rename.files.extend(files);
                    self.rename.dirty = true;
                }
                if ui.button("Sort by name").clicked() {
                    self.rename.files.sort();
                    self.rename.dirty = true;
                }
                if ui.button("Sort by date").clicked() {
                    self.rename
                        .files
                        .sort_by_key(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok());
                    self.rename.dirty = true;
                }
                if ui.button("Clear").clicked() {
                    self.rename.files.clear();
                    self.rename.dirty = true;
                }
            });
            ui.add_space(6.0);
            ui.strong("Steps");
            let (mut remove, mut up) = (None, None);
            let count = self.rename.rules.len();
            for (index, rule) in self.rename.rules.iter_mut().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!("{}. {}", index + 1, rule_title(rule))).strong(),
                    );
                    rule_ui(ui, index, rule);
                    if index > 0 && ui.small_button("Up").clicked() {
                        up = Some(index);
                    }
                    if index + 1 < count && ui.small_button("Down").clicked() {
                        up = Some(index + 1);
                    }
                    if ui.small_button("Remove").clicked() {
                        remove = Some(index);
                    }
                });
            }
            if let Some(index) = up {
                self.rename.rules.swap(index - 1, index);
            }
            if let Some(index) = remove {
                self.rename.rules.remove(index);
            }
            ui.menu_button("Add a step…", |ui| {
                for rule in new_rules() {
                    if ui.button(rule_title(&rule)).clicked() {
                        self.rename.rules.push(rule);
                        ui.close();
                    }
                }
            });
        });
        ui.add_space(8.0);

        let journal_len = self.rename.journal.as_ref().map(|j| j.renames.len());
        let previews = self.rename.previews();
        let changes = previews.iter().filter(|p| p.changed()).count();
        let problems = previews.iter().filter(|p| p.problem.is_some()).count();
        ui.horizontal(|ui| {
            apply = widgets::primary(
                ui,
                changes > 0 && problems == 0 && !busy,
                &format!("Rename {}", widgets::count(changes, "file")),
            )
            .clicked();
            if problems > 0 {
                ui.colored_label(
                    theme::DANGER,
                    format!("{problems} names can't be given; see the list."),
                );
            }
            if let Some(len) = journal_len {
                undo = ui
                    .add_enabled(!busy, egui::Button::new("Undo the last rename"))
                    .on_hover_text(format!("{len} renames are taken back"))
                    .clicked();
            }
        });
        ui.add_space(4.0);
        if previews.is_empty() {
            empty_hint(
                ui,
                "Add the files to rename, or send them here from Browse or Similar names.",
            );
        } else {
            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::initial(300.0).clip(true))
                .column(Column::initial(300.0).clip(true))
                .column(Column::remainder().clip(true))
                .header(24.0, |mut header| {
                    for title in ["Name", "New name", "Folder"] {
                        header.col(|ui| {
                            ui.strong(title);
                        });
                    }
                })
                .body(|body| {
                    body.rows(24.0, previews.len(), |mut row| {
                        let p = &previews[row.index()];
                        row.col(|ui| {
                            ui.add(egui::Label::new(&p.old).truncate());
                        });
                        row.col(|ui| match &p.problem {
                            Some(problem) => {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!("{} — {problem}", p.new))
                                            .color(theme::DANGER),
                                    )
                                    .truncate(),
                                );
                            }
                            None if p.changed() => {
                                ui.add(egui::Label::new(RichText::new(&p.new).strong()).truncate());
                            }
                            None => {
                                ui.weak("unchanged");
                            }
                        });
                        row.col(|ui| {
                            let folder = p
                                .path
                                .parent()
                                .map(|f| f.to_string_lossy())
                                .unwrap_or_default();
                            ui.add(egui::Label::new(RichText::new(folder).weak()).truncate());
                        });
                    });
                });
        }

        if apply {
            match rename::apply(&self.rename.previews) {
                Ok(journal) => {
                    self.rename.files = self
                        .rename
                        .previews
                        .iter()
                        .map(|p| p.path.with_file_name(&p.new))
                        .collect();
                    self.rename.rules.clear();
                    self.rename.dirty = true;
                    self.info(format!("Renamed {}.", widgets::count(changes, "file")));
                    self.keep_journal(Some(journal));
                    self.browse.refresh();
                }
                Err(e) => self.error(format!("Nothing was renamed: {e:#}")),
            }
        }
        if undo && let Some(journal) = self.rename.journal.take() {
            let left = rename::undo(&journal);
            if left == 0 {
                self.info("The last rename is undone.");
            } else {
                self.error(format!(
                    "{left} files couldn't get their old name back: they're gone, or the name is taken."
                ));
            }
            // The files have their names from before.
            let back: std::collections::HashMap<&PathBuf, &PathBuf> = journal
                .renames
                .iter()
                .map(|(from, to)| (to, from))
                .collect();
            for file in &mut self.rename.files {
                let mut path = &*file;
                while let Some(before) = back.get(path) {
                    path = before;
                }
                *file = path.clone();
            }
            self.rename.dirty = true;
            self.keep_journal(None);
            self.browse.refresh();
        }
    }

    fn keep_journal(&mut self, journal: Option<Journal>) {
        if let Some(paths) = &self.paths {
            let file = paths.rename_journal();
            let result = match &journal {
                Some(journal) => std::fs::create_dir_all(&paths.data_dir).and_then(|()| {
                    std::fs::write(&file, serde_json::to_vec(journal).unwrap_or_default())
                }),
                None => std::fs::remove_file(&file).or(Ok(())),
            };
            if let Err(e) = result {
                tracing::warn!("can't keep the rename journal: {e}");
            }
        }
        self.rename.journal = journal;
    }
}
