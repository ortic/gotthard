//! Safety copies: what synchronizations, backups, merges and deletions
//! replaced or removed, to restore or to clean up.

use std::path::PathBuf;

use egui::{RichText, Ui};
use gotthard_core::fsx::{Displacer, Removal, format_bytes};
use gotthard_core::vault::{self, Session};

use crate::app::{App, Confirm};
use crate::icons::{self, Icon};
use crate::theme::{self, bold};
use crate::widgets;

#[derive(Default)]
pub struct VaultView {
    pub root: PathBuf,
    /// The runs of `loaded`.
    pub sessions: Vec<Session>,
    loaded: PathBuf,
    /// The folder changed on disk: read it again.
    stale: bool,
}

impl VaultView {
    /// Reads the folder's safety copies again.
    pub fn reload(&mut self) {
        self.stale = true;
    }
}

impl App {
    pub fn vault_ui(&mut self, ui: &mut Ui) {
        widgets::page(
            ui,
            Icon::Shield,
            "Safety copies",
            "Files that Gotthard replaced or removed are kept in the folder they were in, until \
             you clean them up.",
        );
        let roots = self.settings.known_roots();
        if self.vault.root.as_os_str().is_empty()
            && let Some(first) = roots.first()
        {
            self.vault.root = first.clone();
        }
        theme::card(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(bold("Folder"));
                egui::ComboBox::from_id_salt("vault-root")
                    .width(150.0)
                    .selected_text("Known folders")
                    .show_ui(ui, |ui| {
                        for root in &roots {
                            let label = egui::Button::image_and_text(
                                Icon::Folder.image(16.0, icons::kind::FOLDER),
                                root.to_string_lossy(),
                            )
                            .frame_when_inactive(false);
                            if ui.add(label).clicked() {
                                self.vault.root = root.clone();
                                ui.close();
                            }
                        }
                        if roots.is_empty() {
                            ui.weak("Synchronized folders and backups appear here.");
                        }
                    });
                widgets::folder_field(ui, &mut self.vault.root, "A folder Gotthard worked in");
            });
        });
        ui.add_space(10.0);
        if self.vault.root != self.vault.loaded || self.vault.stale {
            if self.vault.root.as_os_str().is_empty() {
                self.vault.sessions.clear();
                self.vault.loaded = PathBuf::new();
                self.vault.stale = false;
            } else if !self.busy() {
                let root = self.vault.root.clone();
                self.vault.loaded = root.clone();
                self.vault.sessions.clear();
                self.vault.stale = false;
                self.spawn(ui.ctx(), "Reading the safety copies", move |_| {
                    let result = vault::list(&root);
                    Box::new(move |app| match result {
                        Ok(sessions) => app.vault.sessions = sessions,
                        Err(e) => app.error(format!("{e:#}")),
                    })
                });
            }
        }
        let busy = self.busy();
        let sessions = std::mem::take(&mut self.vault.sessions);
        self.vault_sessions(ui, &sessions, busy);
        self.vault.sessions = sessions;
    }

    fn vault_sessions(&mut self, ui: &mut Ui, sessions: &[Session], busy: bool) {
        if sessions.is_empty() {
            if !busy {
                widgets::empty(
                    ui,
                    Icon::Shield,
                    "No safety copies here",
                    "When a synchronization, a backup or a deletion replaces or removes a file \
                     in this folder, the old file appears here.",
                );
            }
            return;
        }
        let root = self.vault.root.clone();
        let days = self.settings.keep_safety_copies_days;
        let total: u64 = sessions.iter().map(|s| s.bytes).sum();
        let files: usize = sessions.iter().map(|s| s.files.len()).sum();
        ui.horizontal(|ui| {
            widgets::badge(ui, Some(Icon::File), &widgets::count(files, "file"), theme::ACCENT);
            widgets::badge(ui, Some(Icon::Clock), &widgets::count(sessions.len(), "run"), theme::ACCENT);
            widgets::badge(ui, Some(Icon::Drive), &format_bytes(total), theme::ACCENT);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::button(
                    ui,
                    !busy,
                    Icon::Trash,
                    &format!("Clean up copies older than {days} days…"),
                )
                .clicked()
                {
                    let root = root.clone();
                    self.confirm = Some(Confirm {
                        title: "Clean up safety copies?".into(),
                        text: format!(
                            "The safety copies of runs more than {days} days ago are deleted for good."
                        ),
                        action: "Delete them".into(),
                        run: Box::new(move |app, _| {
                            match vault::purge_older(&root, days) {
                                Ok((runs, bytes)) => app.info(format!(
                                    "Deleted the safety copies of {}, {}.",
                                    widgets::count(runs, "run"),
                                    format_bytes(bytes)
                                )),
                                Err(e) => app.error(format!("{e:#}")),
                            }
                            app.vault.reload();
                        }),
                    });
                }
            });
        });
        ui.add_space(6.0);

        let mut restore: Option<(String, Vec<String>)> = None;
        let mut delete = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                for (number, session) in sessions.iter().enumerate() {
                    let when = session.time().map_or(session.name.clone(), |t| {
                        t.format("%A, %-d %B %Y, %H:%M").to_string()
                    });
                    theme::card(ui.style()).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.add(Icon::Clock.image(18.0, theme::ACCENT));
                            ui.label(bold(when));
                            ui.weak(format!(
                                "{} · {}",
                                widgets::count(session.files.len(), "file"),
                                format_bytes(session.bytes)
                            ));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if widgets::button(ui, !busy, Icon::Trash, "Delete for good…")
                                        .clicked()
                                    {
                                        delete = Some(session.name.clone());
                                    }
                                    if widgets::button(ui, !busy, Icon::Undo, "Restore all")
                                        .clicked()
                                    {
                                        restore = Some((
                                            session.name.clone(),
                                            session.files.iter().map(|f| f.rel.clone()).collect(),
                                        ));
                                    }
                                },
                            );
                        });
                        egui::CollapsingHeader::new(RichText::new("Files").weak())
                            .id_salt(&session.name)
                            .default_open(number == 0 && session.files.len() <= 30)
                            .show(ui, |ui| {
                                for file in session.files.iter().take(1000) {
                                    ui.horizontal(|ui| {
                                        let name = file.rel.rsplit('/').next().unwrap_or(&file.rel);
                                        if widgets::icon_button(
                                            ui,
                                            !busy,
                                            Icon::Undo,
                                            "Put this file back where it was",
                                        )
                                        .clicked()
                                        {
                                            restore = Some((
                                                session.name.clone(),
                                                vec![file.rel.clone()],
                                            ));
                                        }
                                        let (icon, color) = icons::for_file(name);
                                        ui.add(icon.image(16.0, color));
                                        ui.label(&file.rel);
                                        ui.weak(format_bytes(file.size));
                                    });
                                }
                                if session.files.len() > 1000 {
                                    ui.weak(format!("… and {} more", session.files.len() - 1000));
                                }
                            });
                    });
                    ui.add_space(6.0);
                }
            });
        if let Some((session, files)) = restore {
            // What a restored file replaces is kept in turn.
            let displacer = Displacer::new(Removal::Vault);
            let mut failed = Vec::new();
            for rel in &files {
                if let Err(e) = vault::restore(&root, &session, rel, &displacer) {
                    failed.push(format!("{rel}: {e:#}"));
                }
            }
            if failed.is_empty() {
                self.info(format!(
                    "Restored {} to where they were.",
                    widgets::count(files.len(), "file")
                ));
            } else {
                self.error(format!("Not restored: {}", failed.join("; ")));
            }
            self.vault.reload();
        }
        if let Some(session) = delete {
            self.confirm = Some(Confirm {
                title: "Delete these safety copies?".into(),
                text: format!("The safety copies of the run {session} are deleted for good."),
                action: "Delete them".into(),
                run: Box::new(move |app, _| {
                    if let Err(e) = vault::delete_session(&root, &session) {
                        app.error(format!("{e:#}"));
                    }
                    app.vault.reload();
                }),
            });
        }
    }
}
