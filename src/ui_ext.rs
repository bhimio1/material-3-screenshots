//! Pieces not (yet) in material-3-ui. Each is a candidate for the shared
//! crate: a palette without an egui context (for slurp), color swatches,
//! big action tiles and thumbnail tiles.

use egui::{vec2, Color32, CornerRadius, Response, Sense, Stroke, StrokeKind, TextureHandle, Ui};
use material_3_ui::icons::MaterialIcon;
use material_3_ui::theme::{self, ColorScheme, M3Palette, ThemeSettings, ThemeSource};
use material_3_ui::widgets;

/// The palette the settings resolve to, without an egui context (the CLI
/// has none). `System` counts as dark: that's what end4-pC users run.
pub fn headless_palette(settings: &ThemeSettings) -> M3Palette {
    let dark = settings.scheme != ColorScheme::Light;
    if settings.source == ThemeSource::Auto {
        if let Some(p) = theme::auto_theme_path().and_then(|p| M3Palette::from_theme_json(&p).ok()) {
            return p;
        }
    }
    M3Palette::from_seed(&settings.seed_color, dark)
}

pub fn rgba_hex(c: Color32, alpha: u8) -> String {
    format!("#{:02x}{:02x}{:02x}{:02x}", c.r(), c.g(), c.b(), alpha)
}

pub struct SlurpColors {
    pub border: String,
    pub background: String,
    pub selection: String,
    pub boxes: String,
}

/// slurp in the desktop's colors: primary border, scrim outside.
pub fn slurp_colors(settings: &ThemeSettings) -> SlurpColors {
    let p = headless_palette(settings);
    SlurpColors {
        border: rgba_hex(p.primary, 0xff),
        background: rgba_hex(p.scrim, 0x66),
        selection: rgba_hex(p.primary, 0x22),
        boxes: rgba_hex(p.primary_container, 0x40),
    }
}

pub fn to_rgba(c: Color32) -> [u8; 4] {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    [r, g, b, a]
}

pub fn from_rgba(c: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

/// A round color swatch with a ring when selected.
pub fn color_swatch(ui: &mut Ui, color: Color32, selected: bool, tooltip: &str) -> Response {
    let p = widgets::palette(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let c = rect.center();
        let painter = ui.painter();
        if selected {
            painter.circle_stroke(c, 13.0, Stroke::new(2.0, p.primary));
        } else if resp.hovered() {
            painter.circle_stroke(c, 13.0, Stroke::new(1.5, p.outline));
        }
        painter.circle_filled(c, 9.5, color);
        painter.circle_stroke(c, 9.5, Stroke::new(1.0, p.outline_variant));
    }
    resp.on_hover_text(tooltip)
}

/// A big tonal tile with an icon and two lines of text, for the Capture
/// page's main actions.
pub fn action_tile(ui: &mut Ui, icon: MaterialIcon, title: &str, subtitle: &str, width: f32, accent: bool) -> Response {
    let p = widgets::palette(ui);
    let size = vec2(width, 112.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let (base, fg, icon_fg) = if accent {
            (p.primary_container, p.on_primary_container, p.on_primary_container)
        } else {
            (p.surface_container_high, p.on_surface, p.primary)
        };
        let fill = widgets::state_layer(base, fg, &resp);
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(20), fill);
        widgets::paint_icon(ui, icon, rect.left_top() + vec2(30.0, 34.0), 28.0, icon_fg);
        painter.text(
            rect.left_bottom() + vec2(16.0, -38.0),
            egui::Align2::LEFT_CENTER,
            title,
            widgets::title_font(16.0),
            fg,
        );
        painter.text(
            rect.left_bottom() + vec2(16.0, -18.0),
            egui::Align2::LEFT_CENTER,
            subtitle,
            egui::FontId::proportional(12.5),
            if accent { fg.gamma_multiply(0.8) } else { p.on_surface_variant },
        );
    }
    resp
}

/// A thumbnail card: picture (or an icon while it loads), title, subtitle.
pub fn thumb_tile(
    ui: &mut Ui,
    texture: Option<&TextureHandle>,
    fallback: MaterialIcon,
    title: &str,
    subtitle: &str,
    width: f32,
    badge: Option<MaterialIcon>,
) -> Response {
    let p = widgets::palette(ui);
    let pic_h = (width * 0.62).round();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, pic_h + 52.0), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let fill = widgets::state_layer(p.surface_container, p.on_surface, &resp);
    ui.painter().rect_filled(rect, CornerRadius::same(16), fill);
    let pic = egui::Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(width - 12.0, pic_h - 6.0));
    ui.painter().rect_filled(pic, CornerRadius::same(12), p.surface_container_highest);
    match texture {
        Some(tex) => {
            // Fit inside, keep the aspect ratio.
            let ts = tex.size_vec2();
            let s = (pic.width() / ts.x).min(pic.height() / ts.y);
            let r = egui::Rect::from_center_size(pic.center(), ts * s);
            egui::Image::new(tex)
                .corner_radius(CornerRadius::same(8))
                .paint_at(ui, r);
        }
        None => widgets::paint_icon(ui, fallback, pic.center(), 32.0, p.on_surface_variant),
    }
    if let Some(b) = badge {
        let c = pic.right_top() + vec2(-18.0, 18.0);
        ui.painter().circle_filled(c, 13.0, p.secondary_container);
        widgets::paint_icon(ui, b, c, 16.0, p.on_secondary_container);
    }
    let text_w = width - 24.0;
    let title_g = widgets::truncated_galley(ui, title, egui::FontId::proportional(13.5), p.on_surface, text_w);
    ui.painter().galley(rect.left_bottom() + vec2(12.0, -42.0), title_g, p.on_surface);
    let sub_g = widgets::truncated_galley(ui, subtitle, egui::FontId::proportional(12.0), p.on_surface_variant, text_w);
    ui.painter().galley(rect.left_bottom() + vec2(12.0, -22.0), sub_g, p.on_surface_variant);
    if resp.hovered() {
        ui.painter().rect_stroke(rect, CornerRadius::same(16), Stroke::new(1.0, p.outline_variant), StrokeKind::Inside);
    }
    resp
}

/// Human file size.
pub fn file_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_and_sizes() {
        assert_eq!(rgba_hex(Color32::from_rgb(255, 0, 16), 0x66), "#ff001066");
        assert_eq!(file_size(512), "512 B");
        assert_eq!(file_size(1536), "1.5 KB");
        assert_eq!(file_size(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn seed_palette_without_a_context() {
        let s = ThemeSettings { source: ThemeSource::Seed, scheme: ColorScheme::Light, ..Default::default() };
        let p = headless_palette(&s);
        assert!(!p.is_dark());
        let c = slurp_colors(&s);
        assert!(c.border.starts_with('#') && c.border.len() == 9);
    }
}
