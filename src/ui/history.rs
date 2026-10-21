//! The History page: every capture and recording, as a grid of thumbnails.

use crate::config::Config;
use crate::deliver;
use crate::history::{self, Entry, Kind};
use egui::Ui;
use material_3_ui::common;
use material_3_ui::format::file_size;
use material_3_ui::icons::icons::*;
use material_3_ui::sys::task::Task;
use material_3_ui::theme::M3Palette;
use material_3_ui::thumbs::{self, Thumbnails};
use material_3_ui::widgets::{self, thumb_tile, ButtonKind};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const THUMB: u32 = 360;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Screenshots,
    Recordings,
}

/// What the page wants the app to do.
pub enum HistoryEvent {
    Edit(PathBuf),
    Toast(String, bool),
}

/// Decode one thumbnail, for [`Thumbnails`]: the picture itself, or a frame
/// from a recording when ffmpegthumbnailer is around.
pub fn load_thumb(path: &Path) -> Option<egui::ColorImage> {
    match history::kind_of(path)? {
        Kind::Screenshot => thumbs::load_image(path, THUMB),
        Kind::Recording => {
            let out = std::process::Command::new("ffmpegthumbnailer")
                .args(["-i", &path.to_string_lossy(), "-o", "-", "-c", "png", "-s", &THUMB.to_string()])
                .stderr(std::process::Stdio::null())
                .output()
                .ok()?;
            thumbs::load_image_bytes(&out.stdout, THUMB)
        }
    }
}

pub struct HistoryView {
    task: Task<Vec<Entry>>,
    pub entries: Vec<Entry>,
    filter: Filter,
    selected: Option<PathBuf>,
    confirm_delete: Option<PathBuf>,
    last_scan: Option<Instant>,
}

impl Default for HistoryView {
    fn default() -> Self {
        Self { task: Task::default(), entries: Vec::new(), filter: Filter::All, selected: None, confirm_delete: None, last_scan: None }
    }
}

impl HistoryView {
    /// Rescan now and then (cheap: two directory listings).
    pub fn refresh(&mut self, ctx: &egui::Context, cfg: &Config, force: bool) {
        if let Some(list) = self.task.take() {
            self.entries = list;
        }
        let due = self.last_scan.is_none_or(|t| t.elapsed() > Duration::from_secs(3));
        if (force || due) && !self.task.running() {
            let cfg = cfg.clone();
            self.task.spawn(ctx, move || history::load(&cfg));
            self.last_scan = Some(Instant::now());
        }
        ctx.request_repaint_after(Duration::from_secs(3));
    }

    pub fn loaded(&self) -> bool {
        self.last_scan.is_some() && !self.task.running() || !self.entries.is_empty()
    }

    pub fn show(&mut self, ui: &mut Ui, cfg: &Config, thumbs: &mut Thumbnails, p: &M3Palette) -> Vec<HistoryEvent> {
        let mut events = Vec::new();
        self.refresh(ui.ctx(), cfg, false);
        common::page(ui, ICON_HISTORY, "History", "Your screenshots and recordings, newest first.", |ui| {
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                for (f, label, icon) in [
                    (Filter::All, "All", None),
                    (Filter::Screenshots, "Screenshots", Some(ICON_IMAGE)),
                    (Filter::Recordings, "Recordings", Some(ICON_MOVIE)),
                ] {
                    if widgets::filter_chip(ui, label, self.filter == f, icon).clicked() {
                        self.filter = f;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::icon_button(ui, ICON_FOLDER_OPEN, "Open the screenshots folder", true).clicked() {
                        deliver::open_folder(&cfg.screenshots_dir());
                    }
                    if common::refresh_button(ui, self.task.running()) {
                        self.refresh(ui.ctx(), cfg, true);
                    }
                });
            });
            ui.add_space(8.0);
            if let Some(sel) = self.selected.clone() {
                if let Some(e) = self.entries.iter().find(|e| e.path == sel).cloned() {
                    self.action_bar(ui, &e, cfg, &mut events);
                } else {
                    self.selected = None;
                }
            }
            let filter = self.filter;
            let shown: Vec<Entry> = self
                .entries
                .iter()
                .filter(|e| match filter {
                    Filter::All => true,
                    Filter::Screenshots => e.kind == Kind::Screenshot,
                    Filter::Recordings => e.kind == Kind::Recording,
                })
                .cloned()
                .collect();
            if shown.is_empty() {
                ui.add_space(24.0);
                if self.loaded() {
                    common::empty_hint(ui, "Nothing here yet. Press your screenshot key, or use the Capture page.");
                } else {
                    common::loading(ui, "Looking for captures…");
                }
                return;
            }
            self.grid(ui, &shown, thumbs, &mut events);
        });
        self.delete_dialog(ui.ctx(), thumbs, p, &mut events);
        events
    }

    fn grid(&mut self, ui: &mut Ui, shown: &[Entry], thumbs: &mut Thumbnails, events: &mut Vec<HistoryEvent>) {
        let width = ui.available_width();
        let gap = 12.0;
        let cols = ((width + gap) / (210.0 + gap)).floor().max(1.0) as usize;
        let tile_w = ((width - gap * (cols as f32 - 1.0)) / cols as f32).floor();
        for row in shown.chunks(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for e in row {
                    let tex = thumbs.get(&e.path, e.modified);
                    let name = e.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let when = chrono::DateTime::<chrono::Local>::from(e.modified).format("%b %-d, %H:%M");
                    let sub = if e.unsaved { format!("{when} · not saved") } else { format!("{when} · {}", file_size(e.size)) };
                    let (fallback, badge) = match e.kind {
                        Kind::Screenshot => (ICON_IMAGE, e.unsaved.then_some(ICON_CONTENT_COPY)),
                        Kind::Recording => (ICON_MOVIE, Some(ICON_PLAY_ARROW)),
                    };
                    let selected = self.selected.as_ref() == Some(&e.path);
                    let r = thumb_tile(ui, tex.as_ref(), fallback, &name, &sub, tile_w, badge);
                    if selected {
                        let p = widgets::palette(ui);
                        ui.painter().rect_stroke(
                            r.rect,
                            egui::CornerRadius::same(16),
                            egui::Stroke::new(2.0, p.primary),
                            egui::StrokeKind::Inside,
                        );
                    }
                    if r.double_clicked() {
                        match e.kind {
                            Kind::Screenshot => events.push(HistoryEvent::Edit(e.path.clone())),
                            Kind::Recording => deliver::open_file(&e.path),
                        }
                    } else if r.clicked() {
                        self.selected = if selected { None } else { Some(e.path.clone()) };
                    }
                    r.context_menu(|ui| {
                        if e.kind == Kind::Screenshot && ui.button("Edit").clicked() {
                            events.push(HistoryEvent::Edit(e.path.clone()));
                        }
                        if ui.button("Open").clicked() {
                            deliver::open_file(&e.path);
                        }
                        if ui.button("Show in folder").clicked() {
                            deliver::open_folder(e.path.parent().unwrap_or(Path::new("/")));
                        }
                        if ui.button("Delete…").clicked() {
                            self.confirm_delete = Some(e.path.clone());
                        }
                    });
                }
            });
            ui.add_space(gap);
        }
    }

    fn action_bar(&mut self, ui: &mut Ui, e: &Entry, cfg: &Config, events: &mut Vec<HistoryEvent>) {
        let p = widgets::palette(ui);
        egui::Frame::new()
            .fill(p.secondary_container)
            .corner_radius(egui::CornerRadius::same(20))
            .inner_margin(egui::Margin::symmetric(14, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let name = e.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let g = widgets::truncated_galley(ui, &name, widgets::title_font(14.0), p.on_secondary_container, (ui.available_width() - 470.0).max(80.0));
                    ui.label(g);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::icon_button(ui, ICON_CLOSE, "Deselect", true).clicked() {
                            self.selected = None;
                        }
                        if widgets::icon_button(ui, ICON_DELETE, "Delete", true).clicked() {
                            self.confirm_delete = Some(e.path.clone());
                        }
                        if widgets::icon_button(ui, ICON_FOLDER_OPEN, "Show in folder", true).clicked() {
                            deliver::open_folder(e.path.parent().unwrap_or(Path::new("/")));
                        }
                        if widgets::icon_button(ui, ICON_OPEN_IN_NEW, "Open", true).clicked() {
                            deliver::open_file(&e.path);
                        }
                        if e.kind == Kind::Screenshot {
                            if widgets::button(ui, ButtonKind::Tonal, Some(ICON_CONTENT_COPY), "Copy").clicked() {
                                let r = image::open(&e.path).map_err(|e| e.to_string()).and_then(|i| deliver::copy_image(&i.to_rgba8()));
                                events.push(match r {
                                    Ok(()) => HistoryEvent::Toast("Copied to the clipboard".into(), false),
                                    Err(err) => HistoryEvent::Toast(err, true),
                                });
                            }
                            if e.unsaved && widgets::button(ui, ButtonKind::Tonal, Some(ICON_SAVE), "Save").clicked() {
                                let r = image::open(&e.path)
                                    .map_err(|e| e.to_string())
                                    .and_then(|i| deliver::save(&i.to_rgba8(), cfg, "region", None));
                                events.push(match r {
                                    Ok(path) => {
                                        let _ = std::fs::remove_file(&e.path);
                                        self.selected = Some(path.clone());
                                        HistoryEvent::Toast(format!("Saved {}", material_3_ui::sys::paths::tilde(&path)), false)
                                    }
                                    Err(err) => HistoryEvent::Toast(err, true),
                                });
                                self.refresh(ui.ctx(), cfg, true);
                            }
                            if widgets::button(ui, ButtonKind::Filled, Some(ICON_EDIT), "Edit").clicked() {
                                events.push(HistoryEvent::Edit(e.path.clone()));
                            }
                        } else if widgets::button(ui, ButtonKind::Filled, Some(ICON_PLAY_ARROW), "Play").clicked() {
                            deliver::open_file(&e.path);
                        }
                    });
                });
            });
        ui.add_space(10.0);
    }

    fn delete_dialog(&mut self, ctx: &egui::Context, thumbs: &mut Thumbnails, p: &M3Palette, events: &mut Vec<HistoryEvent>) {
        let Some(path) = self.confirm_delete.clone() else {
            return;
        };
        let mut done = None;
        common::modal(ctx, p, "delete", ICON_DELETE, "Delete this file?", |ui| {
            ui.set_width(380.0);
            ui.label(material_3_ui::sys::paths::tilde(&path));
            common::dialog_buttons(ui, |ui| {
                if widgets::button(ui, ButtonKind::Danger, None, "Delete").clicked() {
                    done = Some(true);
                }
                if widgets::button(ui, ButtonKind::Text, None, "Cancel").clicked() {
                    done = Some(false);
                }
            });
        });
        match done {
            Some(true) => {
                self.confirm_delete = None;
                match std::fs::remove_file(&path) {
                    Ok(()) => {
                        self.entries.retain(|e| e.path != path);
                        thumbs.forget(&path);
                        if self.selected.as_ref() == Some(&path) {
                            self.selected = None;
                        }
                        events.push(HistoryEvent::Toast("Deleted".into(), false));
                    }
                    Err(e) => events.push(HistoryEvent::Toast(format!("Couldn't delete it: {e}"), true)),
                }
            }
            Some(false) => self.confirm_delete = None,
            None => {}
        }
    }
}
