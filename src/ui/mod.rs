//! The window: the app (Capture, History, Settings) or the editor alone.

mod capture;
mod editor;
mod history;
mod settings;

use crate::cli::Page;
use crate::config::Config;
use crate::record;
use capture::{CaptureEvent, CaptureOptions};
use editor::{Editor, EditorEvent};
use history::{HistoryEvent, HistoryView, Thumbs};
use image::RgbaImage;
use material_3_ui::common;
use material_3_ui::icons::icons::*;
use material_3_ui::sys::{arch, task::Task};
use material_3_ui::theme::ThemeManager;
use material_3_ui::toasts::{Toast, ToastKind, Toasts};
use material_3_ui::widgets::{self, ButtonKind};
use settings::SettingsEvent;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const APP_ID: &str = "material_3_screenshots";

#[derive(Clone)]
enum Action {
    OpenFolder(PathBuf),
}

struct App {
    cfg: Config,
    saved_cfg: Config,
    cfg_changed: Option<Instant>,
    theme: ThemeManager,
    toasts: Toasts<Action>,
    page: Page,
    editor: Option<Editor>,
    /// Opened as `edit FILE` or `--edit`: closing the editor quits.
    standalone: bool,
    capture: CaptureOptions,
    history: HistoryView,
    thumbs: Thumbs,
    installed: Task<HashSet<String>>,
    recording: Option<record::State>,
    recording_checked: Instant,
    allow_close: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext, cfg: Config, page: Page, editor: Option<(RgbaImage, Option<PathBuf>)>) -> Self {
        let theme = ThemeManager::new(&cc.egui_ctx, &cfg.appearance);
        let standalone = editor.is_some();
        let editor = editor.map(|(img, src)| Editor::new(img, src, theme.palette()));
        let mut installed = Task::default();
        installed.spawn(&cc.egui_ctx, arch::installed_packages);
        Self {
            capture: CaptureOptions::from_config(&cfg),
            saved_cfg: cfg.clone(),
            cfg,
            cfg_changed: None,
            theme,
            toasts: Toasts::default(),
            page,
            editor,
            standalone,
            history: HistoryView::default(),
            thumbs: Thumbs::new(&cc.egui_ctx),
            installed,
            recording: record::current(),
            recording_checked: Instant::now(),
            allow_close: false,
        }
    }

    fn persist(&mut self) {
        if self.cfg == self.saved_cfg {
            self.cfg_changed = None;
            return;
        }
        // Debounced, so typing in a field doesn't write on every key.
        let since = *self.cfg_changed.get_or_insert_with(Instant::now);
        if since.elapsed() > Duration::from_millis(400) {
            match self.cfg.save() {
                Ok(()) => self.saved_cfg = self.cfg.clone(),
                Err(e) => self.toasts.error(format!("Couldn't save settings: {e}")),
            }
            self.cfg_changed = None;
        }
    }

    fn open_editor(&mut self, path: PathBuf) {
        match image::open(&path) {
            Ok(img) => self.editor = Some(Editor::new(img.to_rgba8(), Some(path), self.theme.palette())),
            Err(e) => self.toasts.error(format!("Couldn't open {}: {e}", path.display())),
        }
    }

    fn editor_events(&mut self, ctx: &egui::Context, events: Vec<EditorEvent>) {
        for ev in events {
            match ev {
                EditorEvent::Close => self.close_editor(ctx),
                EditorEvent::Saved(path) => {
                    let mut t = Toast::new(format!("Saved {}", material_3_ui::sys::paths::tilde(&path)), ToastKind::Success);
                    if let Some(dir) = path.parent() {
                        t.action = Some(("Show".into(), Action::OpenFolder(dir.to_path_buf())));
                    }
                    self.toasts.push(t);
                    self.history.refresh(ctx, &self.cfg, true);
                }
                EditorEvent::Copied => self.toasts.success("Copied to the clipboard"),
                EditorEvent::Error(e) => self.toasts.error(e),
            }
        }
    }

    fn close_editor(&mut self, ctx: &egui::Context) {
        let Some(ed) = &mut self.editor else {
            return;
        };
        if ed.dirty() {
            ed.confirm_close = true;
            return;
        }
        if self.standalone {
            self.allow_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            self.editor = None;
        }
    }

    fn unsaved_dialog(&mut self, ctx: &egui::Context) {
        let Some(ed) = &mut self.editor else {
            return;
        };
        if !ed.confirm_close {
            return;
        }
        let p = self.theme.palette().clone();
        let mut choice = None;
        common::modal(ctx, &p, "unsaved", ICON_EDIT, "Keep your changes?", |ui| {
            ui.set_width(360.0);
            ui.label("The annotations haven't been saved or copied.");
            common::dialog_buttons(ui, |ui| {
                if widgets::button(ui, ButtonKind::Filled, None, "Save").clicked() {
                    choice = Some(0);
                }
                if widgets::button(ui, ButtonKind::Text, None, "Discard").clicked() {
                    choice = Some(1);
                }
                if widgets::button(ui, ButtonKind::Text, None, "Cancel").clicked() {
                    choice = Some(2);
                }
            });
        });
        let Some(choice) = choice else {
            return;
        };
        ed.confirm_close = false;
        match choice {
            0 => {
                let ev = ed.save(&self.cfg);
                let ok = matches!(ev, EditorEvent::Saved(_));
                self.editor_events(ctx, vec![ev]);
                if ok {
                    self.close_editor(ctx);
                }
            }
            1 => {
                if self.standalone {
                    self.allow_close = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    self.editor = None;
                }
            }
            _ => {}
        }
    }

    fn spawn_cli(&mut self, ctx: &egui::Context, args: Vec<String>) {
        let exe = std::env::current_exe().unwrap_or_else(|_| "m3-screenshots".into());
        // Give the window time to close before anything is captured.
        let script = "sleep 0.35; exec \"$0\" \"$@\"";
        let mut argv: Vec<&str> = vec!["-c", script, exe.to_str().unwrap_or("m3-screenshots")];
        argv.extend(args.iter().map(String::as_str));
        match material_3_ui::sys::cmd::spawn_detached("sh", &argv) {
            Ok(()) => {
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => self.toasts.error(e),
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let p = self.theme.palette().clone();
        egui::Panel::left("nav")
            .default_size(232.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p.surface).inner_margin(egui::Margin::same(12)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    widgets::icon(ui, ICON_SCREENSHOT_REGION, 26.0, p.primary);
                    ui.label(widgets::title_text("Screenshots", 20.0));
                });
                ui.add_space(16.0);
                let rec = self.recording.as_ref().map(|_| "REC");
                for (page, icon, name, trailing) in [
                    (Page::Capture, ICON_SCREENSHOT_REGION, "Capture", rec),
                    (Page::History, ICON_PHOTO_LIBRARY, "History", None),
                    (Page::Settings, ICON_SETTINGS, "Settings", None),
                ] {
                    if widgets::nav_item(ui, icon, name, self.page == page, trailing).clicked() {
                        self.page = page;
                        if page == Page::History {
                            self.history.refresh(ui.ctx(), &self.cfg, true);
                        }
                    }
                }
            });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.theme.poll(&self.cfg.appearance);
        if let Some(e) = self.theme.take_error() {
            self.toasts.error(e);
        }
        self.installed.poll();
        if self.recording_checked.elapsed() > Duration::from_secs(1) {
            self.recording = record::current();
            self.recording_checked = Instant::now();
        }
        ctx.request_repaint_after(Duration::from_secs(1));

        // Closing the window with unsaved annotations asks first.
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close {
            if let Some(ed) = &mut self.editor {
                if ed.dirty() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    ed.confirm_close = true;
                }
            }
        }

        let p = self.theme.palette().clone();
        if let Some(ed) = &mut self.editor {
            let standalone = self.standalone;
            let events = egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(p.surface))
                .show(ui, |ui| ed.show(ui, &self.cfg, &p, standalone))
                .inner;
            self.editor_events(&ctx, events);
            self.unsaved_dialog(&ctx);
        } else {
            self.sidebar(ui);
            let surface = egui::Frame::new().fill(p.surface).inner_margin(egui::Margin { left: 0, right: 12, top: 12, bottom: 12 });
            egui::CentralPanel::default().frame(surface).show(ui, |ui| {
                egui::Frame::new()
                    .fill(p.surface_container_low)
                    .corner_radius(egui::CornerRadius::same(24))
                    .show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        match self.page {
                            Page::Capture => {
                                let events = capture::show(ui, &self.cfg, &mut self.capture, self.recording.as_ref(), &mut self.history, &mut self.thumbs);
                                for ev in events {
                                    match ev {
                                        CaptureEvent::Spawn(args) => self.spawn_cli(&ctx, args),
                                        CaptureEvent::StopRecording => match record::stop() {
                                            Ok(Some(f)) => self.toasts.success(format!("Saved {}", material_3_ui::sys::paths::tilde(&f))),
                                            Ok(None) => {}
                                            Err(e) => self.toasts.error(e),
                                        },
                                        CaptureEvent::Install(packages) => {
                                            let list: Vec<&str> = packages.iter().map(String::as_str).collect();
                                            let script = arch::install_command(&list, arch::AurHelper::Auto);
                                            match material_3_ui::sys::term::run(&self.cfg.terminal, &script) {
                                                Ok(t) => self.toasts.info(format!("Installing in {t}")),
                                                Err(e) => self.toasts.error(e),
                                            }
                                        }
                                        CaptureEvent::OpenHistory => self.page = Page::History,
                                        CaptureEvent::History(h) => self.history_event(h),
                                    }
                                }
                            }
                            Page::History => {
                                let events = self.history.show(ui, &self.cfg, &mut self.thumbs, &p);
                                for ev in events {
                                    self.history_event(ev);
                                }
                            }
                            Page::Settings => {
                                let events = settings::show(ui, &mut self.cfg, &self.theme, self.installed.value.as_ref());
                                for ev in events {
                                    match ev {
                                        SettingsEvent::ThemeChanged => self.theme.apply(&self.cfg.appearance),
                                        SettingsEvent::Toast(t, true) => self.toasts.error(t),
                                        SettingsEvent::Toast(t, false) => self.toasts.info(t),
                                    }
                                }
                            }
                        }
                    });
            });
        }
        self.persist();
        for action in self.toasts.show(&ctx, &p, 20.0) {
            match action {
                Action::OpenFolder(dir) => crate::deliver::open_folder(&dir),
            }
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.theme.palette().surface.to_normalized_gamma_f32()
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.cfg != self.saved_cfg {
            let _ = self.cfg.save();
        }
    }
}

impl App {
    fn history_event(&mut self, ev: HistoryEvent) {
        match ev {
            HistoryEvent::Edit(path) => self.open_editor(path),
            HistoryEvent::Toast(t, true) => self.toasts.error(t),
            HistoryEvent::Toast(t, false) => self.toasts.success(t),
        }
    }
}

fn options(title: &str, size: [f32; 2]) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_app_id(APP_ID)
            .with_inner_size(size)
            .with_min_inner_size([760.0, 480.0]),
        ..Default::default()
    }
}

fn launch(title: &str, size: [f32; 2], make: impl FnOnce(&eframe::CreationContext) -> App + 'static) -> i32 {
    let result = eframe::run_native(title, options(title, size), Box::new(move |cc| Ok(Box::new(make(cc)))));
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("m3-screenshots: {e}");
            1
        }
    }
}

pub fn run_app(page: Page, cfg: Config) -> i32 {
    launch("Screenshots", [1100.0, 760.0], move |cc| App::new(cc, cfg, page, None))
}

/// The editor in its own window. `source` is the file it came from (or
/// where the capture was just saved).
pub fn run_editor(img: RgbaImage, source: Option<PathBuf>, cfg: Config) -> i32 {
    // Large captures open large, within reason.
    let (w, h) = img.dimensions();
    let size = [(w as f32 + 320.0).clamp(900.0, 1500.0), (h as f32 + 180.0).clamp(600.0, 960.0)];
    launch("Screenshot editor", size, move |cc| App::new(cc, cfg, Page::Capture, Some((img, source))))
}
