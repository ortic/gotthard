//! The window: navigation at the left, the chosen view beside it, the
//! running operation at the bottom.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

use egui::{Color32, RichText, Ui};
use gotthard_config::{Paths, Settings, Theme};
use gotthard_core::Progress;
use gotthard_core::fsx::format_bytes;

use crate::views::{backup, browse, dupes, merge, rename, similar, sync, vault};
use crate::{theme, widgets};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Sync,
    Backup,
    Vault,
    Browse,
    Merge,
    Duplicates,
    Similar,
    Rename,
    Settings,
}

impl View {
    pub fn title(self) -> &'static str {
        match self {
            View::Sync => "Synchronize",
            View::Backup => "Backup",
            View::Vault => "Safety copies",
            View::Browse => "Browse",
            View::Merge => "Merge folders",
            View::Duplicates => "Duplicates",
            View::Similar => "Similar names",
            View::Rename => "Rename",
            View::Settings => "Settings",
        }
    }
}

/// What a finished operation does with its result.
pub type Apply = Box<dyn FnOnce(&mut App) + Send>;

/// The one long operation that runs; file operations don't run side by
/// side.
pub struct Task {
    pub title: String,
    pub progress: Arc<Progress>,
    result: Receiver<Apply>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Error,
}

/// What a confirmed question does.
pub type Confirmed = Box<dyn FnOnce(&mut App, &egui::Context)>;

/// A question before something that can't be taken back.
pub struct Confirm {
    pub title: String,
    pub text: String,
    pub action: String,
    pub run: Confirmed,
}

pub struct App {
    pub paths: Option<Paths>,
    pub settings: Settings,
    /// The settings file couldn't be read: nothing is saved over it.
    settings_broken: bool,
    pub view: View,
    pub task: Option<Task>,
    pub notice: Option<(Level, String)>,
    pub confirm: Option<Confirm>,
    pub sync: sync::SyncView,
    pub backup: backup::BackupView,
    pub vault: vault::VaultView,
    pub browse: browse::BrowseView,
    pub merge: merge::MergeView,
    pub dupes: dupes::DupesView,
    pub similar: similar::SimilarView,
    pub rename: rename::RenameView,
    exclude_text: String,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, paths: Option<Paths>) -> Self {
        theme::install(&cc.egui_ctx);
        let (settings, problem) = match paths.as_ref().map(Settings::load) {
            Some(Ok(settings)) => (settings, None),
            Some(Err(e)) => (
                Settings::default(),
                Some(format!(
                    "The settings couldn't be read ({e:#}). Gotthard runs with the defaults and \
                     doesn't save until the file is repaired or removed."
                )),
            ),
            None => (Settings::default(), None),
        };
        let mut app = Self {
            exclude_text: settings.exclude.join("\n"),
            settings_broken: problem.is_some(),
            notice: problem.map(|text| (Level::Error, text)),
            paths,
            settings,
            view: View::Sync,
            task: None,
            confirm: None,
            sync: Default::default(),
            backup: Default::default(),
            vault: Default::default(),
            browse: Default::default(),
            merge: Default::default(),
            dupes: Default::default(),
            similar: Default::default(),
            rename: Default::default(),
        };
        app.apply_theme(&cc.egui_ctx);
        app
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        ctx.set_theme(match self.settings.theme {
            Theme::System => egui::ThemePreference::System,
            Theme::Dark => egui::ThemePreference::Dark,
            Theme::Light => egui::ThemePreference::Light,
        });
    }

    pub fn save_settings(&mut self) {
        if self.settings_broken {
            return;
        }
        if let Some(paths) = &self.paths
            && let Err(e) = self.settings.save(paths)
        {
            self.error(format!("The settings couldn't be saved: {e:#}"));
        }
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.notice = Some((Level::Error, text.into()));
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.notice = Some((Level::Info, text.into()));
    }

    pub fn busy(&self) -> bool {
        self.task.is_some()
    }

    /// Runs `work` on a thread of its own; what it returns is applied to
    /// the app when it's done.
    pub fn spawn<F>(&mut self, ctx: &egui::Context, title: &str, work: F)
    where
        F: FnOnce(&Progress) -> Apply + Send + 'static,
    {
        if self.busy() {
            return;
        }
        self.notice = None;
        let progress = Arc::new(Progress::default());
        let (tx, rx) = channel();
        let (ctx, shared) = (ctx.clone(), progress.clone());
        std::thread::spawn(move || {
            let apply = work(&shared);
            let _ = tx.send(apply);
            ctx.request_repaint();
        });
        self.task = Some(Task {
            title: title.to_owned(),
            progress,
            result: rx,
        });
    }

    fn poll_task(&mut self, ctx: &egui::Context) {
        let Some(task) = &self.task else { return };
        match task.result.try_recv() {
            Ok(apply) => {
                self.task = None;
                apply(self);
            }
            Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
            Err(TryRecvError::Disconnected) => {
                let title = task.title.clone();
                self.task = None;
                self.error(format!(
                    "{title} stopped unexpectedly. The log has the details."
                ));
            }
        }
    }

    fn navigation(&mut self, ui: &mut Ui) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.add(
                egui::Image::new(egui::include_image!("../../../assets/icon/gotthard-64.png"))
                    .fit_to_exact_size(egui::vec2(28.0, 28.0)),
            );
            ui.label(RichText::new("Gotthard").size(18.0).strong());
        });
        ui.add_space(10.0);
        let mut entry = |ui: &mut Ui, view: View| {
            let selected = self.view == view;
            let text = RichText::new(format!("   {}", view.title()));
            let button = egui::Button::new(if selected { text.strong() } else { text })
                .selected(selected)
                .frame_when_inactive(false)
                .min_size(egui::vec2(ui.available_width(), 30.0));
            let response = ui.add(button);
            if response.clicked() {
                self.view = view;
            }
        };
        ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
            ui.spacing_mut().button_padding = egui::vec2(8.0, 4.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new("  MASTER COPY").small().weak());
            for view in [View::Sync, View::Backup, View::Vault] {
                entry(ui, view);
            }
            ui.add_space(12.0);
            ui.label(RichText::new("  FILES").small().weak());
            for view in [
                View::Browse,
                View::Merge,
                View::Duplicates,
                View::Similar,
                View::Rename,
            ] {
                entry(ui, view);
            }
            ui.add_space(12.0);
            entry(ui, View::Settings);
        });
    }

    fn status(&mut self, ui: &mut Ui) {
        let Some(task) = &self.task else { return };
        let snapshot = task.progress.snapshot();
        ui.horizontal(|ui| {
            let phase = if snapshot.phase.is_empty() {
                task.title.clone()
            } else {
                snapshot.phase.clone()
            };
            ui.label(RichText::new(phase).strong());
            let cancelled = task.progress.cancelled();
            if ui
                .add_enabled(!cancelled, egui::Button::new("Cancel"))
                .clicked()
            {
                task.progress.cancel();
            }
            let mut bar = match snapshot.fraction() {
                Some(fraction) => egui::ProgressBar::new(fraction).show_percentage(),
                None => egui::ProgressBar::new(0.0).animate(true),
            };
            bar = bar.desired_width(220.0);
            ui.add(bar);
            let mut detail = Vec::new();
            if snapshot.items_total > 0 {
                detail.push(format!(
                    "{} of {}",
                    snapshot.items_done, snapshot.items_total
                ));
            } else if snapshot.items_done > 0 {
                detail.push(format!("{} files", snapshot.items_done));
            }
            if snapshot.bytes_total > 0 {
                detail.push(format!(
                    "{} of {}",
                    format_bytes(snapshot.bytes_done),
                    format_bytes(snapshot.bytes_total)
                ));
            }
            if cancelled {
                detail.push("cancelling…".into());
            }
            detail.push(snapshot.current);
            ui.add(egui::Label::new(RichText::new(detail.join("  ·  ")).weak()).truncate());
        });
    }

    fn notice_ui(&mut self, ui: &mut Ui) {
        let Some((level, text)) = self.notice.clone() else {
            return;
        };
        let color = match level {
            Level::Info => theme::SUCCESS,
            Level::Error => theme::DANGER,
        };
        theme::banner(ui.style(), color).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Dismiss").clicked() {
                        self.notice = None;
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(egui::Label::new(text).wrap());
                    });
                });
            });
        });
        ui.add_space(6.0);
    }

    fn confirm_ui(&mut self, ctx: &egui::Context) {
        let Some(confirm) = &self.confirm else { return };
        let mut choice = None;
        let modal = egui::Modal::new(egui::Id::new("confirm")).show(ctx, |ui| {
            ui.set_max_width(460.0);
            ui.heading(&confirm.title);
            ui.add_space(6.0);
            ui.label(&confirm.text);
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if widgets::danger(ui, true, &confirm.action).clicked() {
                    choice = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(false);
                }
            });
        });
        if modal.should_close() {
            choice = choice.or(Some(false));
        }
        if let Some(run) = choice {
            let confirm = self.confirm.take().expect("checked above");
            if run {
                (confirm.run)(self, ctx);
            }
        }
    }

    fn settings_ui(&mut self, ui: &mut Ui) {
        widgets::heading(ui, "Settings", "How Gotthard treats your files.");
        let before = self.settings.clone();
        theme::card(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.strong("Safety");
            ui.checkbox(
                &mut self.settings.verify_copies,
                "Verify every copy: read it back and compare it with the original",
            );
            ui.add_space(4.0);
            ui.label("Files that are replaced or deleted go");
            use gotthard_core::fsx::Removal;
            for (value, text) in [
                (
                    Removal::Vault,
                    "to the folder's safety copies, from where they can be restored (recommended)",
                ),
                (Removal::RecycleBin, "to the Recycle Bin"),
                (Removal::Permanent, "nowhere: they are deleted for good"),
            ] {
                ui.radio_value(&mut self.settings.removal, value, text);
            }
            ui.horizontal(|ui| {
                ui.label("Safety copies may be cleaned up after");
                ui.add(
                    egui::DragValue::new(&mut self.settings.keep_safety_copies_days)
                        .range(1..=3650)
                        .suffix(" days"),
                );
            });
        });
        ui.add_space(8.0);
        theme::card(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.strong("Files no operation touches");
            ui.weak("One pattern a line, e.g. *.tmp, Thumbs.db or projects/*/target.");
            if ui
                .add(
                    egui::TextEdit::multiline(&mut self.exclude_text)
                        .desired_rows(6)
                        .desired_width(420.0),
                )
                .changed()
            {
                self.settings.exclude = self
                    .exclude_text
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
        });
        ui.add_space(8.0);
        theme::card(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.strong("Appearance");
            ui.horizontal(|ui| {
                for (value, text) in [
                    (Theme::System, "As Windows"),
                    (Theme::Dark, "Dark"),
                    (Theme::Light, "Light"),
                ] {
                    ui.radio_value(&mut self.settings.theme, value, text);
                }
            });
        });
        ui.add_space(8.0);
        ui.weak(format!(
            "Gotthard {} · your files stay on your disks; nothing is sent anywhere.",
            env!("CARGO_PKG_VERSION")
        ));
        if self.settings != before {
            if self.settings.theme != before.theme {
                self.apply_theme(ui.ctx());
            }
            self.save_settings();
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        egui_extras::install_image_loaders(&ctx);
        self.poll_task(&ctx);

        let nav = egui::Frame::new()
            .fill(theme::base(ui.visuals()))
            .inner_margin(egui::Margin::symmetric(8, 4));
        egui::Panel::left("navigation")
            .exact_size(190.0)
            .resizable(false)
            .frame(nav)
            .show(ui, |ui| self.navigation(ui));
        if self.busy() {
            let frame = egui::Frame::new()
                .fill(ui.visuals().window_fill)
                .stroke(egui::Stroke::new(1.0, theme::border(ui.visuals())))
                .inner_margin(egui::Margin::symmetric(12, 8));
            egui::Panel::bottom("status")
                .frame(frame)
                .show(ui, |ui| self.status(ui));
        }
        let frame = egui::Frame::new()
            .fill(ui.visuals().panel_fill)
            .inner_margin(egui::Margin::same(16));
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            self.notice_ui(ui);
            match self.view {
                View::Sync => self.sync_ui(ui),
                View::Backup => self.backup_ui(ui),
                View::Vault => self.vault_ui(ui),
                View::Browse => self.browse_ui(ui),
                View::Merge => self.merge_ui(ui),
                View::Duplicates => self.dupes_ui(ui),
                View::Similar => self.similar_ui(ui),
                View::Rename => self.rename_ui(ui),
                View::Settings => {
                    egui::ScrollArea::vertical().show(ui, |ui| self.settings_ui(ui));
                }
            }
        });
        self.confirm_ui(&ctx);
    }
}

/// A weak hint in the middle of an empty view.
pub fn empty_hint(ui: &mut Ui, text: &str) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(text)
                .color(ui.visuals().weak_text_color())
                .size(15.0),
        );
    });
}

/// A small colored dot with a text: a status.
pub fn status_dot(ui: &mut Ui, color: Color32, text: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 4.5, color);
        ui.label(text);
    });
}
