//! The annotation editor: a canvas over the capture, tools along the top,
//! layers on the right, zoom along the bottom.

use crate::annotate::{self, Area, Doc, Layer, Shape, P};
use crate::config::{Config, ImageFormat};
use crate::deliver;
use egui::{pos2, vec2, Color32, CornerRadius, Key, Pos2, Rect, Sense, Stroke, StrokeKind, Ui};
use image::RgbaImage;
use material_3_ui::common;
use material_3_ui::icons::icons::*;
use material_3_ui::icons::MaterialIcon;
use material_3_ui::sys::paths;
use material_3_ui::theme::M3Palette;
use material_3_ui::widgets::{self, color_swatch, ButtonKind};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Crop,
    Arrow,
    Line,
    Rect,
    Ellipse,
    Pen,
    Highlighter,
    Text,
    Step,
    Blur,
    Pixelate,
}

const TOOLS: &[(Tool, MaterialIcon, &str, Key)] = &[
    (Tool::Select, ICON_ARROW_SELECTOR_TOOL, "Select and move (V)", Key::V),
    (Tool::Crop, ICON_CROP, "Crop (C)", Key::C),
    (Tool::Arrow, ICON_ARROW_OUTWARD, "Arrow (A)", Key::A),
    (Tool::Line, ICON_HORIZONTAL_RULE, "Line (L)", Key::L),
    (Tool::Rect, ICON_RECTANGLE, "Rectangle (R)", Key::R),
    (Tool::Ellipse, ICON_CIRCLE, "Ellipse (E)", Key::E),
    (Tool::Pen, ICON_DRAW, "Pen (P)", Key::P),
    (Tool::Highlighter, ICON_INK_HIGHLIGHTER, "Highlighter (H)", Key::H),
    (Tool::Text, ICON_TITLE, "Text (T)", Key::T),
    (Tool::Step, ICON_COUNTER_1, "Numbered step (N)", Key::N),
    (Tool::Blur, ICON_BLUR_ON, "Blur (B)", Key::B),
    (Tool::Pixelate, ICON_GRID_ON, "Pixelate (X)", Key::X),
];

/// What the editor asks of the window around it.
#[derive(Clone, Debug, PartialEq)]
pub enum EditorEvent {
    Close,
    Saved(PathBuf),
    Copied,
    Error(String),
}

enum Drag {
    Shape { start: P, now: P },
    Stroke(Vec<P>),
    Move { layer: usize, last: P, moved: bool },
    Crop { start: P, now: P },
    Pan,
}

struct TextEdit {
    at: P,
    text: String,
    /// Editing an existing layer.
    layer: Option<usize>,
    focus: bool,
}

struct SaveAs {
    path: String,
    format: ImageFormat,
}

pub struct Editor {
    base: RgbaImage,
    pub source: Option<PathBuf>,
    doc: Doc,
    undo: Vec<Doc>,
    redo: Vec<Doc>,
    /// The document as last saved or copied.
    clean: Doc,
    tool: Tool,
    color: [u8; 4],
    width: f32,
    filled: bool,
    selected: Option<usize>,
    zoom: f32,
    pan: egui::Vec2,
    fit_pending: bool,
    drag: Option<Drag>,
    text: Option<TextEdit>,
    save_as: Option<SaveAs>,
    texture: Option<egui::TextureHandle>,
    rendered: Option<Doc>,
    last_render: Instant,
    pub saved_path: Option<PathBuf>,
    pub confirm_close: bool,
}

impl Editor {
    pub fn new(base: RgbaImage, source: Option<PathBuf>, palette: &M3Palette) -> Self {
        Self {
            base,
            source: source.clone(),
            doc: Doc::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            clean: Doc::default(),
            tool: Tool::Arrow,
            color: palette.error.to_srgba_unmultiplied(),
            width: 4.0,
            filled: false,
            selected: None,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            fit_pending: true,
            drag: None,
            text: None,
            save_as: None,
            texture: None,
            rendered: None,
            last_render: Instant::now() - Duration::from_secs(1),
            saved_path: source,
            confirm_close: false,
        }
    }

    pub fn dirty(&self) -> bool {
        self.doc != self.clean
    }

    pub fn image_size(&self) -> (u32, u32) {
        self.base.dimensions()
    }

    fn checkpoint(&mut self) {
        self.undo.push(self.doc.clone());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.doc, prev));
            self.selected = None;
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.doc, next));
            self.selected = None;
        }
    }

    fn push_layer(&mut self, layer: Layer) {
        self.checkpoint();
        self.doc.layers.push(layer);
        self.selected = Some(self.doc.layers.len() - 1);
    }

    fn delete_selected(&mut self) {
        if let Some(i) = self.selected.take() {
            if i < self.doc.layers.len() {
                self.checkpoint();
                self.doc.layers.remove(i);
            }
        }
    }

    /// Change the selected layer's style along with the toolbar.
    fn restyle_selected(&mut self) {
        if let Some(i) = self.selected {
            let (color, width, filled) = (self.color, self.width, self.filled);
            if let Some(l) = self.doc.layers.get(i) {
                if (l.color, l.width, l.filled) != (color, width, filled) {
                    self.checkpoint();
                    let l = &mut self.doc.layers[i];
                    l.color = color;
                    l.width = width;
                    l.filled = filled;
                }
            }
        }
    }

    pub fn export(&self) -> RgbaImage {
        annotate::export(&self.base, &self.doc)
    }

    pub fn copy(&mut self) -> EditorEvent {
        match deliver::copy_image(&self.export()) {
            Ok(()) => {
                self.clean = self.doc.clone();
                EditorEvent::Copied
            }
            Err(e) => EditorEvent::Error(e),
        }
    }

    /// Save next to the source (`name-edited.ext`), or into the screenshots
    /// folder for a capture that was never saved.
    pub fn save(&mut self, cfg: &Config) -> EditorEvent {
        let img = self.export();
        let result = match &self.source {
            Some(src) => {
                let dir = src.parent().map(PathBuf::from).unwrap_or_else(|| cfg.screenshots_dir());
                let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let stem = if stem.ends_with("-edited") { stem } else { format!("{stem}-edited") };
                let ext = ImageFormat::from_path(src).unwrap_or(cfg.format).extension();
                // Saving again overwrites the same edited copy.
                let path = match &self.saved_path {
                    Some(p) if p.file_stem().is_some_and(|s| s.to_string_lossy() == stem) => p.clone(),
                    _ => crate::naming::unique_path(&dir, &stem, ext),
                };
                deliver::write_file(&img, &path, cfg.jpeg_quality).map(|_| path)
            }
            None => match &self.saved_path {
                Some(p) => deliver::write_file(&img, p, cfg.jpeg_quality).map(|_| p.clone()),
                None => deliver::save(&img, cfg, "edited", None),
            },
        };
        self.finish_save(result)
    }

    fn save_to(&mut self, path: PathBuf, cfg: &Config) -> EditorEvent {
        let result = deliver::write_file(&self.export(), &path, cfg.jpeg_quality).map(|_| path);
        self.finish_save(result)
    }

    fn finish_save(&mut self, result: Result<PathBuf, String>) -> EditorEvent {
        match result {
            Ok(p) => {
                self.clean = self.doc.clone();
                self.saved_path = Some(p.clone());
                EditorEvent::Saved(p)
            }
            Err(e) => EditorEvent::Error(e),
        }
    }

    fn open_save_as(&mut self, cfg: &Config) {
        let dir = self
            .saved_path
            .as_ref()
            .and_then(|p| p.parent().map(PathBuf::from))
            .unwrap_or_else(|| cfg.screenshots_dir());
        let stem = crate::naming::expand(&cfg.filename_pattern, chrono::Local::now(), "edited", None);
        let path = dir.join(format!("{stem}.{}", cfg.format.extension()));
        self.save_as = Some(SaveAs { path: paths::tilde(&path), format: cfg.format });
    }

    // ---- view ----------------------------------------------------------

    fn origin(&self, canvas: Rect) -> Pos2 {
        let (w, h) = self.image_size();
        canvas.center() + self.pan - vec2(w as f32, h as f32) * self.zoom / 2.0
    }

    fn to_screen(&self, canvas: Rect, p: P) -> Pos2 {
        self.origin(canvas) + vec2(p.x, p.y) * self.zoom
    }

    fn to_image(&self, canvas: Rect, s: Pos2) -> P {
        let v = (s - self.origin(canvas)) / self.zoom;
        P::new(v.x, v.y)
    }

    fn area_rect(&self, canvas: Rect, a: &Area) -> Rect {
        Rect::from_two_pos(self.to_screen(canvas, a.min), self.to_screen(canvas, a.max))
    }

    fn fit(&mut self, canvas: Rect) {
        let (w, h) = self.image_size();
        let z = ((canvas.width() - 48.0) / w as f32).min((canvas.height() - 48.0) / h as f32);
        self.zoom = z.clamp(0.02, 1.0);
        self.pan = egui::Vec2::ZERO;
    }

    fn zoom_at(&mut self, canvas: Rect, factor: f32, at: Pos2) {
        let before = self.to_image(canvas, at);
        self.zoom = (self.zoom * factor).clamp(0.02, 16.0);
        let after = self.to_screen(canvas, before);
        self.pan += at - after;
    }

    fn ensure_texture(&mut self, ctx: &egui::Context, dragging: bool) {
        if self.rendered.as_ref() == Some(&self.doc) && self.texture.is_some() {
            return;
        }
        // While moving a layer, re-render a few times a second at most.
        if dragging && self.texture.is_some() && self.last_render.elapsed() < Duration::from_millis(60) {
            ctx.request_repaint_after(Duration::from_millis(60));
            return;
        }
        let img = annotate::render(&self.base, &self.doc);
        let color = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
        let options = egui::TextureOptions {
            magnification: egui::TextureFilter::Nearest,
            minification: egui::TextureFilter::Linear,
            ..Default::default()
        };
        match &mut self.texture {
            Some(t) => t.set(color, options),
            None => self.texture = Some(ctx.load_texture("m3s-editor", color, options)),
        }
        self.rendered = Some(self.doc.clone());
        self.last_render = Instant::now();
    }

    // ---- ui ------------------------------------------------------------

    /// Draw the editor into `ui`. `standalone` shows Close instead of Back.
    pub fn show(&mut self, ui: &mut Ui, cfg: &Config, p: &M3Palette, standalone: bool) -> Vec<EditorEvent> {
        let mut events = Vec::new();
        self.keyboard(ui.ctx(), cfg, &mut events);

        egui::Panel::top("editor-tools")
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p.surface).inner_margin(egui::Margin::symmetric(12, 10)))
            .show(ui, |ui| self.toolbar(ui, cfg, p, standalone, &mut events));
        egui::Panel::bottom("editor-status")
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p.surface).inner_margin(egui::Margin::symmetric(16, 6)))
            .show(ui, |ui| self.status_bar(ui, p));
        egui::Panel::right("editor-layers")
            .default_size(250.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p.surface).inner_margin(egui::Margin { left: 0, right: 12, top: 0, bottom: 0 }))
            .show(ui, |ui| self.layers_panel(ui, p));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.surface).inner_margin(egui::Margin { left: 12, right: 12, top: 0, bottom: 0 }))
            .show(ui, |ui| self.canvas(ui, p));

        self.save_as_dialog(ui.ctx(), cfg, p, &mut events);
        events
    }

    fn keyboard(&mut self, ctx: &egui::Context, cfg: &Config, events: &mut Vec<EditorEvent>) {
        if self.text.is_some() || self.save_as.is_some() || ctx.egui_wants_keyboard_input() {
            return;
        }
        let (cmd, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
        let pressed = |k: Key| ctx.input_mut(|i| i.consume_key(i.modifiers, k));
        if cmd {
            if pressed(Key::Z) {
                if shift {
                    self.redo();
                } else {
                    self.undo();
                }
            } else if pressed(Key::Y) {
                self.redo();
            } else if pressed(Key::S) {
                if shift {
                    self.open_save_as(cfg);
                } else {
                    events.push(self.save(cfg));
                }
            } else if pressed(Key::C) {
                events.push(self.copy());
            } else if pressed(Key::Num0) {
                self.fit_pending = true;
            }
            return;
        }
        if pressed(Key::Delete) || pressed(Key::Backspace) {
            self.delete_selected();
        }
        if pressed(Key::Escape) {
            if self.drag.is_some() {
                self.drag = None;
            } else if self.selected.is_some() {
                self.selected = None;
            } else if self.tool == Tool::Crop {
                self.tool = Tool::Select;
            }
        }
        if pressed(Key::Enter) && self.tool == Tool::Crop {
            self.tool = Tool::Select;
        }
        for (tool, _, _, key) in TOOLS {
            if pressed(*key) {
                self.set_tool(*tool);
            }
        }
        if pressed(Key::Num1) {
            self.zoom = 1.0;
            self.pan = egui::Vec2::ZERO;
        }
        if pressed(Key::Num0) {
            self.fit_pending = true;
        }
    }

    fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        self.drag = None;
        if tool != Tool::Select {
            self.selected = None;
        }
    }

    fn toolbar(&mut self, ui: &mut Ui, cfg: &Config, p: &M3Palette, standalone: bool, events: &mut Vec<EditorEvent>) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let (icon, tip) = if standalone { (ICON_CLOSE, "Close") } else { (ICON_ARROW_BACK, "Back") };
            if widgets::icon_button(ui, icon, tip, true).clicked() {
                events.push(EditorEvent::Close);
            }
            ui.add_space(8.0);
            egui::Frame::new()
                .fill(p.surface_container)
                .corner_radius(CornerRadius::same(24))
                .inner_margin(egui::Margin::symmetric(6, 4))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for (tool, icon, tip, _) in TOOLS {
                            if widgets::icon_button_sized(ui, *icon, tip, true, 36.0, self.tool == *tool).clicked() {
                                self.set_tool(*tool);
                            }
                        }
                    });
                });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if widgets::button(ui, ButtonKind::Filled, Some(ICON_SAVE), "Save").on_hover_text("Ctrl+S").clicked() {
                    events.push(self.save(cfg));
                }
                if widgets::button(ui, ButtonKind::Outlined, None, "Save as…").on_hover_text("Ctrl+Shift+S").clicked() {
                    self.open_save_as(cfg);
                }
                if widgets::button(ui, ButtonKind::Tonal, Some(ICON_CONTENT_COPY), "Copy").on_hover_text("Ctrl+C").clicked() {
                    events.push(self.copy());
                }
                ui.add_space(4.0);
                if widgets::icon_button(ui, ICON_REDO, "Redo (Ctrl+Shift+Z)", !self.redo.is_empty()).clicked() {
                    self.redo();
                }
                if widgets::icon_button(ui, ICON_UNDO, "Undo (Ctrl+Z)", !self.undo.is_empty()).clicked() {
                    self.undo();
                }
            });
        });
    }

    fn style_card(&mut self, ui: &mut Ui, p: &M3Palette) {
        let swatches: [(Color32, &str); 10] = [
            (p.error, "Error (from your theme)"),
            (p.primary, "Primary (from your theme)"),
            (p.tertiary, "Tertiary (from your theme)"),
            (Color32::from_rgb(0xe5, 0x39, 0x35), "Red"),
            (Color32::from_rgb(0xff, 0xb3, 0x00), "Amber"),
            (Color32::from_rgb(0x43, 0xa0, 0x47), "Green"),
            (Color32::from_rgb(0x1e, 0x88, 0xe5), "Blue"),
            (Color32::from_rgb(0xd8, 0x1b, 0x60), "Pink"),
            (Color32::WHITE, "White"),
            (Color32::BLACK, "Black"),
        ];
        let before = (self.color, self.width, self.filled);
        egui::Frame::new()
            .fill(p.surface_container_low)
            .corner_radius(CornerRadius::same(20))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    widgets::icon(ui, ICON_PALETTE, 20.0, p.primary);
                    ui.label(widgets::title_text("Style", 15.0));
                });
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
                    for (c, name) in swatches {
                        if color_swatch(ui, c, c.to_srgba_unmultiplied() == self.color, name).clicked() {
                            self.color = c.to_srgba_unmultiplied();
                        }
                    }
                    let mut custom = Color32::from_rgba_unmultiplied(self.color[0], self.color[1], self.color[2], self.color[3]);
                    if ui.color_edit_button_srgba(&mut custom).on_hover_text("Custom color").changed() {
                        self.color = custom.to_srgba_unmultiplied();
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    widgets::icon(ui, ICON_LINE_WEIGHT, 20.0, p.on_surface_variant).on_hover_text("Size: stroke width, text size, blur strength");
                    ui.spacing_mut().slider_width = ui.available_width() - 80.0;
                    ui.add(egui::Slider::new(&mut self.width, 1.0..=16.0).step_by(1.0).show_value(false));
                    ui.label(egui::RichText::new(format!("{:.0}", self.width)).color(p.on_surface_variant));
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    widgets::switch(ui, &mut self.filled);
                    ui.label(egui::RichText::new("Fill shapes, label text").color(p.on_surface));
                });
            });
        if (self.color, self.width, self.filled) != before {
            self.restyle_selected();
        }
    }

    fn status_bar(&mut self, ui: &mut Ui, p: &M3Palette) {
        ui.horizontal(|ui| {
            let (w, h) = self.image_size();
            let mut text = format!("{w} × {h}");
            if let Some((_, _, cw, ch)) = self.doc.crop.and_then(|c| c.pixels(w, h)) {
                text = format!("{text}  →  cropped to {cw} × {ch}");
            }
            ui.label(egui::RichText::new(text).size(12.5).color(p.on_surface_variant));
            if self.doc.crop.is_some() && widgets::button(ui, ButtonKind::Text, Some(ICON_CROP_FREE), "Reset crop").clicked() {
                self.checkpoint();
                self.doc.crop = None;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if widgets::icon_button(ui, ICON_FIT_SCREEN, "Fit (0)", true).clicked() {
                    self.fit_pending = true;
                }
                if widgets::button(ui, ButtonKind::Text, None, "1:1").on_hover_text("Actual size (1)").clicked() {
                    self.zoom = 1.0;
                    self.pan = egui::Vec2::ZERO;
                }
                if widgets::icon_button(ui, ICON_ZOOM_IN, "Zoom in (Ctrl+scroll)", true).clicked() {
                    self.zoom = (self.zoom * 1.25).min(16.0);
                }
                ui.label(egui::RichText::new(format!("{:.0}%", self.zoom * 100.0)).color(p.on_surface));
                if widgets::icon_button(ui, ICON_ZOOM_OUT, "Zoom out", true).clicked() {
                    self.zoom = (self.zoom / 1.25).max(0.02);
                }
            });
        });
    }

    fn layers_panel(&mut self, ui: &mut Ui, p: &M3Palette) {
        self.style_card(ui, p);
        ui.add_space(10.0);
        egui::Frame::new()
            .fill(p.surface_container_low)
            .corner_radius(CornerRadius::same(20))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                ui.horizontal(|ui| {
                    widgets::icon(ui, ICON_LAYERS, 20.0, p.primary);
                    ui.label(widgets::title_text("Layers", 15.0));
                });
                ui.add_space(6.0);
                if self.doc.layers.is_empty() {
                    common::empty_hint(ui, "Annotations appear here. They stay editable until you save.");
                    return;
                }
                let mut action: Option<(usize, u8)> = None;
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let n = self.doc.layers.len();
                    for i in (0..n).rev() {
                        let layer = &self.doc.layers[i];
                        let selected = self.selected == Some(i);
                        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
                        let bg = if selected { p.secondary_container } else { widgets::state_layer(p.surface_container_low, p.on_surface, &resp) };
                        ui.painter().rect_filled(rect, CornerRadius::same(12), bg);
                        ui.painter().circle_filled(rect.left_center() + vec2(16.0, 0.0), 6.0, Color32::from_rgba_unmultiplied(layer.color[0], layer.color[1], layer.color[2], layer.color[3]));
                        let fg = if layer.hidden { p.on_surface_variant.gamma_multiply(0.6) } else { p.on_surface };
                        let g = widgets::truncated_galley(ui, &layer.name(), egui::FontId::proportional(13.0), fg, rect.width() - 110.0);
                        ui.painter().galley(rect.left_center() + vec2(30.0, -g.size().y / 2.0), g, fg);
                        if resp.clicked() {
                            action = Some((i, 0));
                        }
                        let mut child = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(rect.shrink2(vec2(2.0, 2.0)))
                                .layout(egui::Layout::right_to_left(egui::Align::Center)),
                        );
                        child.spacing_mut().item_spacing.x = 0.0;
                        if widgets::icon_button_sized(&mut child, ICON_DELETE, "Delete", true, 28.0, false).clicked() {
                            action = Some((i, 1));
                        }
                        let eye = if layer.hidden { ICON_VISIBILITY_OFF } else { ICON_VISIBILITY };
                        if widgets::icon_button_sized(&mut child, eye, "Show / hide", true, 28.0, false).clicked() {
                            action = Some((i, 2));
                        }
                        if widgets::icon_button_sized(&mut child, ICON_KEYBOARD_ARROW_UP, "Bring forward", i + 1 < n, 28.0, false).clicked() {
                            action = Some((i, 3));
                        }
                    }
                });
                match action {
                    Some((i, 0)) => {
                        self.tool = Tool::Select;
                        self.selected = Some(i);
                        let l = &self.doc.layers[i];
                        (self.color, self.width, self.filled) = (l.color, l.width, l.filled);
                    }
                    Some((i, 1)) => {
                        self.selected = Some(i);
                        self.delete_selected();
                    }
                    Some((i, 2)) => {
                        self.checkpoint();
                        self.doc.layers[i].hidden ^= true;
                    }
                    Some((i, 3)) => {
                        self.checkpoint();
                        self.doc.layers.swap(i, i + 1);
                        self.selected = Some(i + 1);
                    }
                    _ => {}
                }
            });
    }

    fn canvas(&mut self, ui: &mut Ui, p: &M3Palette) {
        let canvas = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(canvas, Sense::click_and_drag());
        ui.painter().rect_filled(canvas, CornerRadius::same(20), p.surface_container_lowest);
        if self.fit_pending && canvas.width() > 64.0 {
            self.fit(canvas);
            self.fit_pending = false;
        }
        let moving = matches!(self.drag, Some(Drag::Move { .. }));
        self.ensure_texture(ui.ctx(), moving);

        self.handle_input(ui, canvas, &resp);

        let painter = ui.painter_at(canvas);
        let (w, h) = self.image_size();
        let img_rect = self.area_rect(canvas, &Area::from_points(P::new(0.0, 0.0), P::new(w as f32, h as f32)));
        painter.rect_filled(img_rect.expand(1.0), CornerRadius::ZERO, p.outline_variant);
        if let Some(tex) = &self.texture {
            painter.image(tex.id(), img_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        self.paint_overlays(&painter, canvas, img_rect, p);
        self.text_editor(ui, canvas, p);
    }

    fn handle_input(&mut self, ui: &mut Ui, canvas: Rect, resp: &egui::Response) {
        let ctx = ui.ctx().clone();
        // Scrolling pans; Ctrl+scroll (or a pinch) zooms at the pointer.
        if resp.hovered() {
            let (scroll, zoom, pointer) = ctx.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
            if (zoom - 1.0).abs() > f32::EPSILON {
                if let Some(at) = pointer {
                    self.zoom_at(canvas, zoom, at);
                }
            } else if scroll != egui::Vec2::ZERO {
                self.pan += scroll;
            }
        }
        let pointer = resp.interact_pointer_pos().or_else(|| ctx.input(|i| i.pointer.hover_pos()));
        let Some(pos) = pointer else {
            return;
        };
        let at = self.to_image(canvas, pos);
        let shift = ctx.input(|i| i.modifiers.shift);
        let space = ctx.input(|i| i.key_down(Key::Space));
        let slack = 6.0 / self.zoom;

        if resp.drag_started() {
            if resp.dragged_by(egui::PointerButton::Middle) || space {
                self.drag = Some(Drag::Pan);
            } else if resp.dragged_by(egui::PointerButton::Primary) {
                self.commit_text();
                self.drag = match self.tool {
                    Tool::Select => match self.doc.hit(at, slack) {
                        Some(i) => {
                            self.selected = Some(i);
                            Some(Drag::Move { layer: i, last: at, moved: false })
                        }
                        None => {
                            self.selected = None;
                            Some(Drag::Pan)
                        }
                    },
                    Tool::Crop => Some(Drag::Crop { start: at, now: at }),
                    Tool::Pen | Tool::Highlighter => Some(Drag::Stroke(vec![at])),
                    Tool::Text | Tool::Step => None,
                    _ => Some(Drag::Shape { start: at, now: at }),
                };
            }
        }
        if resp.dragged() {
            let delta = resp.drag_delta();
            let mut checkpoint = false;
            match &mut self.drag {
                Some(Drag::Pan) => self.pan += delta,
                Some(Drag::Shape { start, now }) | Some(Drag::Crop { start, now }) => {
                    *now = constrain(*start, at, shift, self.tool);
                }
                Some(Drag::Stroke(points)) => {
                    if points.last().is_none_or(|l| (l.x - at.x).abs() + (l.y - at.y).abs() > 0.75 / self.zoom) {
                        points.push(at);
                    }
                }
                Some(Drag::Move { layer, last, moved }) => {
                    let (dx, dy) = (at.x - last.x, at.y - last.y);
                    if dx != 0.0 || dy != 0.0 {
                        if !*moved {
                            *moved = true;
                            checkpoint = true;
                        }
                        let i = *layer;
                        *last = at;
                        if checkpoint {
                            self.checkpoint();
                        }
                        if let Some(l) = self.doc.layers.get_mut(i) {
                            l.translate(dx, dy);
                        }
                    }
                }
                None => {}
            }
        }
        if resp.drag_stopped() {
            if let Some(drag) = self.drag.take() {
                self.finish_drag(drag);
            }
        }
        if resp.clicked() {
            self.commit_text();
            match self.tool {
                Tool::Text => self.text = Some(TextEdit { at, text: String::new(), layer: None, focus: true }),
                Tool::Step => {
                    let n = self.doc.next_step();
                    self.push_layer(Layer::new(Shape::Step { at, number: n }, self.color, self.width, true));
                }
                Tool::Select => self.selected = self.doc.hit(at, slack),
                _ => {}
            }
        }
        if resp.double_clicked() && self.tool == Tool::Select {
            if let Some(i) = self.doc.hit(at, slack) {
                if let Shape::Text { at, text } = &self.doc.layers[i].shape {
                    self.text = Some(TextEdit { at: *at, text: text.clone(), layer: Some(i), focus: true });
                }
            }
        }
        if resp.hovered() {
            let icon = match (&self.drag, self.tool) {
                (Some(Drag::Pan), _) => egui::CursorIcon::Grabbing,
                (_, Tool::Select) if self.doc.hit(at, slack).is_some() => egui::CursorIcon::Move,
                (_, Tool::Select) => egui::CursorIcon::Default,
                (_, Tool::Text) => egui::CursorIcon::Text,
                _ => egui::CursorIcon::Crosshair,
            };
            ctx.set_cursor_icon(icon);
        }
    }

    fn finish_drag(&mut self, drag: Drag) {
        let (color, width, filled) = (self.color, self.width, self.filled);
        match drag {
            Drag::Shape { start, now } => {
                if (start.x - now.x).abs() < 2.0 && (start.y - now.y).abs() < 2.0 {
                    return;
                }
                let area = Area::from_points(start, now);
                let shape = match self.tool {
                    Tool::Arrow => Shape::Arrow { from: start, to: now },
                    Tool::Line => Shape::Line { from: start, to: now },
                    Tool::Rect => Shape::Rect { area },
                    Tool::Ellipse => Shape::Ellipse { area },
                    Tool::Blur => Shape::Blur { area },
                    Tool::Pixelate => Shape::Pixelate { area },
                    _ => return,
                };
                self.push_layer(Layer::new(shape, color, width, filled));
                if matches!(self.tool, Tool::Blur | Tool::Pixelate) {
                    // Keep the toolbar's color for the next shape.
                    self.selected = None;
                }
            }
            Drag::Stroke(points) => {
                let shape = if self.tool == Tool::Highlighter { Shape::Highlighter { points } } else { Shape::Pen { points } };
                self.push_layer(Layer::new(shape, color, width, filled));
            }
            Drag::Crop { start, now } => {
                let (w, h) = self.image_size();
                let area = Area::from_points(start, now);
                if area.pixels(w, h).is_some_and(|(_, _, cw, ch)| cw >= 4 && ch >= 4) {
                    let clamped = Area::from_points(
                        P::new(area.min.x.clamp(0.0, w as f32).round(), area.min.y.clamp(0.0, h as f32).round()),
                        P::new(area.max.x.clamp(0.0, w as f32).round(), area.max.y.clamp(0.0, h as f32).round()),
                    );
                    self.checkpoint();
                    self.doc.crop = Some(clamped);
                }
            }
            Drag::Move { .. } | Drag::Pan => {}
        }
    }

    fn commit_text(&mut self) {
        let Some(edit) = self.text.take() else {
            return;
        };
        let text = edit.text.trim_end().to_string();
        match edit.layer {
            Some(i) if i < self.doc.layers.len() => {
                self.checkpoint();
                if text.is_empty() {
                    self.doc.layers.remove(i);
                    self.selected = None;
                } else if let Shape::Text { text: t, .. } = &mut self.doc.layers[i].shape {
                    *t = text;
                }
            }
            _ if !text.is_empty() => {
                self.push_layer(Layer::new(Shape::Text { at: edit.at, text }, self.color, self.width, self.filled));
            }
            _ => {}
        }
    }

    fn text_editor(&mut self, ui: &mut Ui, canvas: Rect, p: &M3Palette) {
        let origin = self.origin(canvas);
        let Some(edit) = &mut self.text else {
            return;
        };
        let pos = origin + vec2(edit.at.x, edit.at.y) * self.zoom;
        let size = (annotate::text_size(self.width) * self.zoom).clamp(10.0, 72.0);
        let mut commit = false;
        let mut cancel = false;
        egui::Area::new(egui::Id::unique("m3s-text-edit"))
            .fixed_pos(pos - vec2(6.0, 6.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(p.surface_container_high.gamma_multiply(0.92))
                    .stroke(Stroke::new(1.5, p.primary))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(4))
                    .show(ui, |ui| {
                        let te = egui::TextEdit::multiline(&mut edit.text)
                            .font(egui::FontId::proportional(size))
                            .text_color(Color32::from_rgba_unmultiplied(self.color[0], self.color[1], self.color[2], self.color[3]))
                            .desired_rows(1)
                            .desired_width(320.0)
                            .frame(egui::Frame::NONE)
                            .hint_text("Type, Enter to place");
                        let r = ui.add(te);
                        if edit.focus {
                            r.request_focus();
                            edit.focus = false;
                        }
                        let (enter, shift, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.modifiers.shift, i.key_pressed(Key::Escape)));
                        if r.has_focus() && enter && !shift {
                            // Drop the newline the key just typed.
                            if edit.text.ends_with('\n') {
                                edit.text.pop();
                            }
                            commit = true;
                        }
                        if esc {
                            cancel = true;
                        }
                    });
            });
        if cancel {
            self.text = None;
        } else if commit {
            self.commit_text();
        }
    }

    fn paint_overlays(&self, painter: &egui::Painter, canvas: Rect, img_rect: Rect, p: &M3Palette) {
        let color = Color32::from_rgba_unmultiplied(self.color[0], self.color[1], self.color[2], self.color[3]);
        let stroke = Stroke::new((self.width * self.zoom).max(1.0), color);
        match &self.drag {
            Some(Drag::Shape { start, now }) => {
                let (a, b) = (self.to_screen(canvas, *start), self.to_screen(canvas, *now));
                let r = Rect::from_two_pos(a, b);
                match self.tool {
                    Tool::Arrow => painter.arrow(a, b - a, stroke),
                    Tool::Line => {
                        painter.line_segment([a, b], stroke);
                    }
                    Tool::Rect if self.filled => {
                        painter.rect_filled(r, CornerRadius::ZERO, color);
                    }
                    Tool::Rect => {
                        painter.rect_stroke(r, CornerRadius::ZERO, stroke, StrokeKind::Middle);
                    }
                    Tool::Ellipse => {
                        let shape = egui::epaint::EllipseShape {
                            center: r.center(),
                            radius: r.size() / 2.0,
                            fill: if self.filled { color } else { Color32::TRANSPARENT },
                            stroke: if self.filled { Stroke::NONE } else { stroke },
                            angle: 0.0,
                        };
                        painter.add(shape);
                    }
                    Tool::Blur | Tool::Pixelate => {
                        painter.rect_filled(r, CornerRadius::ZERO, p.scrim.gamma_multiply(0.25));
                        painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.5, p.primary), StrokeKind::Middle);
                    }
                    _ => {}
                }
            }
            Some(Drag::Stroke(points)) => {
                let pts: Vec<Pos2> = points.iter().map(|q| self.to_screen(canvas, *q)).collect();
                let s = if self.tool == Tool::Highlighter {
                    Stroke::new(annotate::highlighter_width(self.width) * self.zoom, color.gamma_multiply(0.4))
                } else {
                    stroke
                };
                painter.add(egui::Shape::line(pts, s));
            }
            _ => {}
        }

        // Crop: dim what's outside.
        let crop = match &self.drag {
            Some(Drag::Crop { start, now }) => Some(Area::from_points(*start, *now)),
            _ => self.doc.crop,
        };
        if let Some(c) = crop {
            let cr = self.area_rect(canvas, &c).intersect(img_rect);
            let dim = p.scrim.gamma_multiply(0.55);
            let r = CornerRadius::ZERO;
            painter.rect_filled(Rect::from_min_max(img_rect.min, pos2(img_rect.max.x, cr.min.y)), r, dim);
            painter.rect_filled(Rect::from_min_max(pos2(img_rect.min.x, cr.max.y), img_rect.max), r, dim);
            painter.rect_filled(Rect::from_min_max(pos2(img_rect.min.x, cr.min.y), pos2(cr.min.x, cr.max.y)), r, dim);
            painter.rect_filled(Rect::from_min_max(pos2(cr.max.x, cr.min.y), pos2(img_rect.max.x, cr.max.y)), r, dim);
            painter.rect_stroke(cr, r, Stroke::new(2.0, p.primary), StrokeKind::Outside);
            if self.tool == Tool::Crop {
                let label = format!("{} × {}", c.width().round(), c.height().round());
                painter.text(cr.left_top() + vec2(4.0, -6.0), egui::Align2::LEFT_BOTTOM, label, egui::FontId::proportional(12.0), p.primary);
            }
        }

        if let Some(l) = self.selected.and_then(|i| self.doc.layers.get(i)) {
            let r = self.area_rect(canvas, &l.bounds()).expand(4.0);
            painter.rect_stroke(r, CornerRadius::same(4), Stroke::new(1.5, p.primary), StrokeKind::Outside);
            for c in [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()] {
                painter.circle_filled(c, 4.0, p.primary);
            }
        }
    }

    fn save_as_dialog(&mut self, ctx: &egui::Context, cfg: &Config, p: &M3Palette, events: &mut Vec<EditorEvent>) {
        let Some(state) = &mut self.save_as else {
            return;
        };
        let mut go = false;
        let mut cancel = false;
        common::modal(ctx, p, "save-as", ICON_SAVE_AS, "Save as", |ui| {
            ui.set_width(440.0);
            let before = state.format;
            widgets::segmented(
                ui,
                &mut state.format,
                &[(ImageFormat::Png, ICON_IMAGE, "PNG"), (ImageFormat::Jpeg, ICON_PHOTO_CAMERA, "JPEG"), (ImageFormat::Webp, ICON_WEB_ASSET, "WebP")],
            );
            if state.format != before {
                let path = std::path::Path::new(&state.path).with_extension(state.format.extension());
                state.path = path.to_string_lossy().into_owned();
            }
            ui.add_space(10.0);
            let r = ui.add(egui::TextEdit::singleline(&mut state.path).desired_width(f32::INFINITY));
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                go = true;
            }
            common::dialog_buttons(ui, |ui| {
                if widgets::button(ui, ButtonKind::Filled, None, "Save").clicked() {
                    go = true;
                }
                if widgets::button(ui, ButtonKind::Text, None, "Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if go {
            let mut path = paths::expand(state.path.trim());
            if ImageFormat::from_path(&path).is_none() {
                path.set_extension(state.format.extension());
            }
            self.save_as = None;
            events.push(self.save_to(path, cfg));
        } else if cancel {
            self.save_as = None;
        }
    }
}

/// Shift: lines snap to 45°, boxes become squares.
fn constrain(start: P, at: P, shift: bool, tool: Tool) -> P {
    if !shift {
        return at;
    }
    let (dx, dy) = (at.x - start.x, at.y - start.y);
    match tool {
        Tool::Arrow | Tool::Line => {
            let len = (dx * dx + dy * dy).sqrt();
            let angle = (dy.atan2(dx) / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4;
            P::new(start.x + len * angle.cos(), start.y + len * angle.sin())
        }
        _ => {
            let s = dx.abs().max(dy.abs());
            P::new(start.x + s * dx.signum(), start.y + s * dy.signum())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> Editor {
        let p = M3Palette::from_seed("#4285F4", true);
        Editor::new(RgbaImage::new(100, 50), None, &p)
    }

    #[test]
    fn undo_redo_and_dirty() {
        let mut e = editor();
        assert!(!e.dirty());
        e.push_layer(Layer::new(Shape::Step { at: P::new(5.0, 5.0), number: 1 }, [0, 0, 0, 255], 2.0, true));
        e.push_layer(Layer::new(Shape::Step { at: P::new(9.0, 9.0), number: 2 }, [0, 0, 0, 255], 2.0, true));
        assert!(e.dirty());
        e.undo();
        assert_eq!(e.doc.layers.len(), 1);
        e.redo();
        assert_eq!(e.doc.layers.len(), 2);
        e.undo();
        e.undo();
        assert!(!e.dirty());
        e.redo();
        e.selected = Some(0);
        e.delete_selected();
        assert!(e.doc.layers.is_empty());
        assert!(e.redo.is_empty(), "a new edit clears redo");
    }

    #[test]
    fn restyle_applies_to_the_selection() {
        let mut e = editor();
        e.push_layer(Layer::new(Shape::Line { from: P::new(0.0, 0.0), to: P::new(9.0, 9.0) }, [1, 2, 3, 255], 2.0, false));
        e.color = [9, 9, 9, 255];
        e.restyle_selected();
        assert_eq!(e.doc.layers[0].color, [9, 9, 9, 255]);
        e.undo();
        assert_eq!(e.doc.layers[0].color, [1, 2, 3, 255]);
    }

    #[test]
    fn crop_and_text_commit() {
        let mut e = editor();
        e.tool = Tool::Crop;
        e.finish_drag(Drag::Crop { start: P::new(-10.0, 10.0), now: P::new(40.0, 90.0) });
        assert_eq!(e.doc.crop, Some(Area::from_points(P::new(0.0, 10.0), P::new(40.0, 50.0))));
        assert_eq!(e.export().dimensions(), (40, 40));
        e.text = Some(TextEdit { at: P::new(1.0, 1.0), text: "hello\n".into(), layer: None, focus: false });
        e.commit_text();
        assert!(matches!(&e.doc.layers[0].shape, Shape::Text { text, .. } if text == "hello"));
        e.text = Some(TextEdit { at: P::new(1.0, 1.0), text: "   ".into(), layer: None, focus: false });
        e.commit_text();
        assert_eq!(e.doc.layers.len(), 1);
    }

    #[test]
    fn view_transform_round_trips() {
        let mut e = editor();
        let canvas = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        e.fit(canvas);
        e.zoom_at(canvas, 2.0, pos2(300.0, 200.0));
        let p = e.to_image(canvas, pos2(300.0, 200.0));
        let back = e.to_screen(canvas, p);
        assert!((back.x - 300.0).abs() < 0.01 && (back.y - 200.0).abs() < 0.01);
    }

    #[test]
    fn shift_constraints() {
        let s = P::new(0.0, 0.0);
        let q = constrain(s, P::new(10.0, 3.0), true, Tool::Rect);
        assert_eq!(q, P::new(10.0, 10.0));
        let l = constrain(s, P::new(10.0, 1.0), true, Tool::Line);
        assert!(l.y.abs() < 0.001);
        assert_eq!(constrain(s, P::new(3.0, 4.0), false, Tool::Line), P::new(3.0, 4.0));
    }

    #[test]
    fn saving_next_to_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("shot.png");
        let mut e = editor();
        e.source = Some(src.clone());
        e.saved_path = Some(src.clone());
        let cfg = Config::default();
        let EditorEvent::Saved(first) = e.save(&cfg) else { panic!() };
        assert_eq!(first.file_name().unwrap(), "shot-edited.png");
        let EditorEvent::Saved(again) = e.save(&cfg) else { panic!() };
        assert_eq!(again, first, "saving again overwrites the edited copy");
        assert!(!e.dirty());
    }
}
