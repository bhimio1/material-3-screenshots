//! The editor's document: the captured image stays untouched underneath a
//! list of annotation layers and an optional crop, all in image pixels.
//! [`render`] flattens it (tiny-skia for shapes, ab_glyph for text); the
//! editor's preview and every export use the same renderer, so what you see
//! is what gets saved.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use image::RgbaImage;
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

static FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/Inter-Medium.otf");

pub fn font() -> FontRef<'static> {
    FontRef::try_from_slice(FONT_BYTES).expect("bundled font parses")
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct P {
    pub x: f32,
    pub y: f32,
}

impl P {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    fn dist(self, o: P) -> f32 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
}

/// An axis-aligned box, always normalized (min ≤ max).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Area {
    pub min: P,
    pub max: P,
}

impl Area {
    pub fn from_points(a: P, b: P) -> Self {
        Self { min: P::new(a.x.min(b.x), a.y.min(b.y)), max: P::new(a.x.max(b.x), a.y.max(b.y)) }
    }

    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }

    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    pub fn contains(&self, p: P, slack: f32) -> bool {
        p.x >= self.min.x - slack && p.x <= self.max.x + slack && p.y >= self.min.y - slack && p.y <= self.max.y + slack
    }

    pub fn expand(&self, by: f32) -> Area {
        Area { min: P::new(self.min.x - by, self.min.y - by), max: P::new(self.max.x + by, self.max.y + by) }
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        self.min.x += dx;
        self.min.y += dy;
        self.max.x += dx;
        self.max.y += dy;
    }

    /// Clamp to a `w`×`h` image, in whole pixels.
    pub fn pixels(&self, w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
        let x0 = self.min.x.round().clamp(0.0, w as f32) as u32;
        let y0 = self.min.y.round().clamp(0.0, h as f32) as u32;
        let x1 = self.max.x.round().clamp(0.0, w as f32) as u32;
        let y1 = self.max.y.round().clamp(0.0, h as f32) as u32;
        (x1 > x0 && y1 > y0).then_some((x0, y0, x1 - x0, y1 - y0))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Arrow { from: P, to: P },
    Line { from: P, to: P },
    Rect { area: Area },
    Ellipse { area: Area },
    Pen { points: Vec<P> },
    Highlighter { points: Vec<P> },
    Text { at: P, text: String },
    Step { at: P, number: u32 },
    Blur { area: Area },
    Pixelate { area: Area },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub shape: Shape,
    /// Straight (not premultiplied) RGBA.
    pub color: [u8; 4],
    /// Stroke width; text size, step size and blur strength scale with it.
    pub width: f32,
    /// Rectangles and ellipses filled; text on a colored label.
    pub filled: bool,
    pub hidden: bool,
}

impl Layer {
    pub fn new(shape: Shape, color: [u8; 4], width: f32, filled: bool) -> Self {
        Self { shape, color, width, filled, hidden: false }
    }

    pub fn name(&self) -> String {
        match &self.shape {
            Shape::Arrow { .. } => "Arrow".into(),
            Shape::Line { .. } => "Line".into(),
            Shape::Rect { .. } => "Rectangle".into(),
            Shape::Ellipse { .. } => "Ellipse".into(),
            Shape::Pen { .. } => "Pen".into(),
            Shape::Highlighter { .. } => "Highlighter".into(),
            Shape::Text { text, .. } => {
                let first: String = text.lines().next().unwrap_or("").chars().take(24).collect();
                format!("Text “{first}”")
            }
            Shape::Step { number, .. } => format!("Step {number}"),
            Shape::Blur { .. } => "Blur".into(),
            Shape::Pixelate { .. } => "Pixelate".into(),
        }
    }

    /// The box the layer covers, in image pixels.
    pub fn bounds(&self) -> Area {
        let pad = self.width / 2.0;
        match &self.shape {
            Shape::Arrow { from, to } => Area::from_points(*from, *to).expand(pad + arrow_head(self.width)),
            Shape::Line { from, to } => Area::from_points(*from, *to).expand(pad),
            Shape::Rect { area } | Shape::Ellipse { area } => area.expand(pad),
            Shape::Blur { area } | Shape::Pixelate { area } => *area,
            Shape::Pen { points } | Shape::Highlighter { points } => {
                let w = if matches!(self.shape, Shape::Highlighter { .. }) { highlighter_width(self.width) } else { self.width };
                points
                    .iter()
                    .fold(None::<Area>, |acc, p| {
                        Some(match acc {
                            None => Area { min: *p, max: *p },
                            Some(a) => Area::from_points(
                                P::new(a.min.x.min(p.x), a.min.y.min(p.y)),
                                P::new(a.max.x.max(p.x), a.max.y.max(p.y)),
                            ),
                        })
                    })
                    .unwrap_or_default()
                    .expand(w / 2.0)
            }
            Shape::Text { at, text } => {
                let (w, h) = text_extent(text, text_size(self.width));
                let pad = if self.filled { label_padding(self.width) } else { 0.0 };
                Area { min: P::new(at.x - pad, at.y - pad), max: P::new(at.x + w + pad, at.y + h + pad) }
            }
            Shape::Step { at, .. } => {
                let r = step_radius(self.width);
                Area { min: P::new(at.x - r, at.y - r), max: P::new(at.x + r, at.y + r) }
            }
        }
    }

    /// Is `p` on the layer (within `slack` image pixels)?
    pub fn hit(&self, p: P, slack: f32) -> bool {
        let near = |a: P, b: P, w: f32| seg_dist(p, a, b) <= w / 2.0 + slack;
        match &self.shape {
            Shape::Arrow { from, to } | Shape::Line { from, to } => near(*from, *to, self.width),
            Shape::Pen { points } | Shape::Highlighter { points } => {
                let w = if matches!(self.shape, Shape::Highlighter { .. }) { highlighter_width(self.width) } else { self.width };
                match points.as_slice() {
                    [one] => one.dist(p) <= w / 2.0 + slack,
                    pts => pts.windows(2).any(|s| near(s[0], s[1], w)),
                }
            }
            Shape::Rect { area } if !self.filled => {
                area.expand(self.width / 2.0).contains(p, slack)
                    && !area.expand(-(self.width / 2.0 + slack)).contains(p, 0.0)
            }
            _ => self.bounds().contains(p, slack),
        }
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        let mv = |p: &mut P| {
            p.x += dx;
            p.y += dy;
        };
        match &mut self.shape {
            Shape::Arrow { from, to } | Shape::Line { from, to } => {
                mv(from);
                mv(to);
            }
            Shape::Rect { area } | Shape::Ellipse { area } | Shape::Blur { area } | Shape::Pixelate { area } => {
                area.translate(dx, dy)
            }
            Shape::Pen { points } | Shape::Highlighter { points } => points.iter_mut().for_each(mv),
            Shape::Text { at, .. } | Shape::Step { at, .. } => mv(at),
        }
    }
}

fn seg_dist(p: P, a: P, b: P) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 <= f32::EPSILON {
        return p.dist(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    p.dist(P::new(a.x + t * dx, a.y + t * dy))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Doc {
    pub layers: Vec<Layer>,
    pub crop: Option<Area>,
}

impl Doc {
    pub fn next_step(&self) -> u32 {
        self.layers
            .iter()
            .filter_map(|l| match l.shape {
                Shape::Step { number, .. } => Some(number),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            + 1
    }

    /// The topmost visible layer under `p`.
    pub fn hit(&self, p: P, slack: f32) -> Option<usize> {
        self.layers.iter().rposition(|l| !l.hidden && l.hit(p, slack))
    }
}

pub fn text_size(width: f32) -> f32 {
    12.0 + width * 4.0
}

pub fn step_radius(width: f32) -> f32 {
    10.0 + width * 2.5
}

pub fn highlighter_width(width: f32) -> f32 {
    (width * 5.0).max(12.0)
}

fn arrow_head(width: f32) -> f32 {
    (width * 4.0).max(14.0)
}

fn label_padding(width: f32) -> f32 {
    text_size(width) * 0.3
}

/// Width and height of (multi-line) text at `px`.
pub fn text_extent(text: &str, px: f32) -> (f32, f32) {
    let font = font();
    let f = font.as_scaled(PxScale::from(px));
    let lines: Vec<&str> = if text.is_empty() { vec![""] } else { text.split('\n').collect() };
    let w = lines
        .iter()
        .map(|l| l.chars().map(|c| f.h_advance(f.glyph_id(c))).sum::<f32>())
        .fold(0.0, f32::max);
    let line_h = f.height() + f.line_gap();
    (w, line_h * lines.len() as f32)
}

/// Black or white, whichever reads on `c`.
pub fn contrast(c: [u8; 4]) -> [u8; 4] {
    let l = 0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32;
    if l > 150.0 {
        [0, 0, 0, 255]
    } else {
        [255, 255, 255, 255]
    }
}

fn to_pixmap(img: &RgbaImage) -> Pixmap {
    let mut pm = Pixmap::new(img.width().max(1), img.height().max(1)).expect("non-zero size");
    for (dst, src) in pm.data_mut().chunks_exact_mut(4).zip(img.as_raw().chunks_exact(4)) {
        let a = src[3] as u16;
        dst[0] = ((src[0] as u16 * a + 127) / 255) as u8;
        dst[1] = ((src[1] as u16 * a + 127) / 255) as u8;
        dst[2] = ((src[2] as u16 * a + 127) / 255) as u8;
        dst[3] = src[3];
    }
    pm
}

fn from_pixmap(pm: &Pixmap) -> RgbaImage {
    let mut out = Vec::with_capacity(pm.data().len());
    for px in pm.data().chunks_exact(4) {
        let a = px[3] as u16;
        for c in &px[..3] {
            let v = (*c as u16 * 255 + a / 2).checked_div(a).unwrap_or(0);
            out.push(v.min(255) as u8);
        }
        out.push(px[3]);
    }
    RgbaImage::from_raw(pm.width(), pm.height(), out).expect("matching size")
}

fn paint(color: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(color[0], color[1], color[2], color[3]);
    p.anti_alias = true;
    p
}

fn stroke(width: f32) -> Stroke {
    Stroke { width: width.max(0.5), line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() }
}

/// Flatten the layers onto the image (crop not applied).
pub fn render(base: &RgbaImage, doc: &Doc) -> RgbaImage {
    let mut pm = to_pixmap(base);
    let font = font();
    for layer in doc.layers.iter().filter(|l| !l.hidden) {
        draw(&mut pm, layer, &font);
    }
    from_pixmap(&pm)
}

/// Flatten and crop: what gets saved or copied.
pub fn export(base: &RgbaImage, doc: &Doc) -> RgbaImage {
    let full = render(base, doc);
    match doc.crop.and_then(|c| c.pixels(full.width(), full.height())) {
        Some((x, y, w, h)) => image::imageops::crop_imm(&full, x, y, w, h).to_image(),
        None => full,
    }
}

fn draw(pm: &mut Pixmap, layer: &Layer, font: &FontRef) {
    let id = Transform::identity();
    let color = layer.color;
    match &layer.shape {
        Shape::Line { from, to } => {
            let mut pb = PathBuilder::new();
            pb.move_to(from.x, from.y);
            pb.line_to(to.x, to.y);
            if let Some(path) = pb.finish() {
                pm.stroke_path(&path, &paint(color), &stroke(layer.width), id, None);
            }
        }
        Shape::Arrow { from, to } => {
            let len = from.dist(*to);
            if len < 1.0 {
                return;
            }
            let (ux, uy) = ((to.x - from.x) / len, (to.y - from.y) / len);
            let head = arrow_head(layer.width).min(len);
            let base = P::new(to.x - ux * head, to.y - uy * head);
            let mut pb = PathBuilder::new();
            pb.move_to(from.x, from.y);
            // Stop the shaft inside the head so its round cap doesn't show.
            pb.line_to(base.x + ux * head * 0.3, base.y + uy * head * 0.3);
            if let Some(path) = pb.finish() {
                pm.stroke_path(&path, &paint(color), &stroke(layer.width), id, None);
            }
            let half = head * 0.6;
            let mut pb = PathBuilder::new();
            pb.move_to(to.x, to.y);
            pb.line_to(base.x - uy * half, base.y + ux * half);
            pb.line_to(base.x + uy * half, base.y - ux * half);
            pb.close();
            if let Some(path) = pb.finish() {
                pm.fill_path(&path, &paint(color), FillRule::Winding, id, None);
            }
        }
        Shape::Rect { area } | Shape::Ellipse { area } => {
            let Some(rect) = tiny_skia::Rect::from_ltrb(area.min.x, area.min.y, area.max.x.max(area.min.x + 1.0), area.max.y.max(area.min.y + 1.0)) else {
                return;
            };
            let path = match layer.shape {
                Shape::Rect { .. } => PathBuilder::from_rect(rect),
                _ => match PathBuilder::from_oval(rect) {
                    Some(p) => p,
                    None => return,
                },
            };
            if layer.filled {
                pm.fill_path(&path, &paint(color), FillRule::Winding, id, None);
            } else {
                let mut s = stroke(layer.width);
                if matches!(layer.shape, Shape::Rect { .. }) {
                    s.line_join = LineJoin::Miter;
                }
                pm.stroke_path(&path, &paint(color), &s, id, None);
            }
        }
        Shape::Pen { points } | Shape::Highlighter { points } => {
            let highlighter = matches!(layer.shape, Shape::Highlighter { .. });
            let (width, color) = if highlighter {
                (highlighter_width(layer.width), [color[0], color[1], color[2], (color[3] as f32 * 0.4) as u8])
            } else {
                (layer.width, color)
            };
            let Some(first) = points.first() else {
                return;
            };
            let mut pb = PathBuilder::new();
            pb.move_to(first.x, first.y);
            if points.len() == 1 {
                pb.line_to(first.x + 0.01, first.y);
            }
            for p in &points[1..] {
                pb.line_to(p.x, p.y);
            }
            if let Some(path) = pb.finish() {
                let mut s = stroke(width);
                if highlighter {
                    s.line_cap = LineCap::Square;
                }
                pm.stroke_path(&path, &paint(color), &s, id, None);
            }
        }
        Shape::Text { at, text } => {
            let px = text_size(layer.width);
            let text_color = if layer.filled {
                let (w, h) = text_extent(text, px);
                let pad = label_padding(layer.width);
                if let Some(r) = tiny_skia::Rect::from_xywh(at.x - pad, at.y - pad, w + pad * 2.0, h + pad * 2.0) {
                    let path = rounded_rect(r, pad);
                    pm.fill_path(&path, &paint(color), FillRule::Winding, id, None);
                }
                contrast(color)
            } else {
                color
            };
            draw_text(pm, font, text, *at, px, text_color);
        }
        Shape::Step { at, number } => {
            let r = step_radius(layer.width);
            if let Some(path) = PathBuilder::from_circle(at.x, at.y, r) {
                pm.fill_path(&path, &paint(color), FillRule::Winding, id, None);
            }
            let label = number.to_string();
            let px = r * 1.1;
            let (w, h) = text_extent(&label, px);
            draw_text(pm, font, &label, P::new(at.x - w / 2.0, at.y - h / 2.0), px, contrast(color));
        }
        Shape::Blur { area } => {
            if let Some(px) = area.pixels(pm.width(), pm.height()) {
                blur(pm, px, (6.0 + layer.width * 2.0) as usize);
            }
        }
        Shape::Pixelate { area } => {
            if let Some(px) = area.pixels(pm.width(), pm.height()) {
                pixelate(pm, px, (6.0 + layer.width * 2.0) as u32);
            }
        }
    }
}

fn rounded_rect(r: tiny_skia::Rect, radius: f32) -> tiny_skia::Path {
    let (l, t, rt, b) = (r.left(), r.top(), r.right(), r.bottom());
    let k = radius.min(r.width() / 2.0).min(r.height() / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(l + k, t);
    pb.line_to(rt - k, t);
    pb.quad_to(rt, t, rt, t + k);
    pb.line_to(rt, b - k);
    pb.quad_to(rt, b, rt - k, b);
    pb.line_to(l + k, b);
    pb.quad_to(l, b, l, b - k);
    pb.line_to(l, t + k);
    pb.quad_to(l, t, l + k, t);
    pb.close();
    pb.finish().unwrap_or_else(|| PathBuilder::from_rect(r))
}

/// Draw text with its top-left at `at`, blending glyph coverage straight
/// into the premultiplied pixmap.
fn draw_text(pm: &mut Pixmap, font: &FontRef, text: &str, at: P, px: f32, color: [u8; 4]) {
    let scaled = font.as_scaled(PxScale::from(px));
    let line_h = scaled.height() + scaled.line_gap();
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    for (i, line) in text.split('\n').enumerate() {
        let mut x = at.x;
        let baseline = at.y + scaled.ascent() + line_h * i as f32;
        let mut prev = None;
        for c in line.chars() {
            let gid = scaled.glyph_id(c);
            if let Some(p) = prev {
                x += scaled.kern(p, gid);
            }
            let glyph = gid.with_scale_and_position(PxScale::from(px), ab_glyph::point(x, baseline));
            x += scaled.h_advance(gid);
            prev = Some(gid);
            let Some(outlined) = font.outline_glyph(glyph) else {
                continue;
            };
            let bb = outlined.px_bounds();
            outlined.draw(|gx, gy, cov| {
                let (px_x, px_y) = (bb.min.x as i32 + gx as i32, bb.min.y as i32 + gy as i32);
                if px_x < 0 || px_y < 0 || px_x >= w || px_y >= h {
                    return;
                }
                let a = cov.clamp(0.0, 1.0) * color[3] as f32 / 255.0;
                let i = (px_y * w + px_x) as usize * 4;
                for k in 0..3 {
                    let src = color[k] as f32 * a;
                    data[i + k] = (src + data[i + k] as f32 * (1.0 - a)).round() as u8;
                }
                data[i + 3] = (a * 255.0 + data[i + 3] as f32 * (1.0 - a)).round() as u8;
            });
        }
    }
}

/// Three passes of a box blur ≈ a Gaussian.
fn blur(pm: &mut Pixmap, (x0, y0, w, h): (u32, u32, u32, u32), radius: usize) {
    let stride = pm.width() as usize;
    let (x0, y0, w, h) = (x0 as usize, y0 as usize, w as usize, h as usize);
    let mut buf: Vec<[f32; 4]> = Vec::with_capacity(w * h);
    {
        let data = pm.data();
        for y in 0..h {
            for x in 0..w {
                let i = ((y0 + y) * stride + x0 + x) * 4;
                buf.push([data[i] as f32, data[i + 1] as f32, data[i + 2] as f32, data[i + 3] as f32]);
            }
        }
    }
    let r = radius.max(1);
    let mut line: Vec<[f32; 4]> = Vec::new();
    for _ in 0..3 {
        // Horizontal.
        for y in 0..h {
            line.clear();
            line.extend_from_slice(&buf[y * w..(y + 1) * w]);
            box_pass(&line, &mut buf[y * w..(y + 1) * w], r);
        }
        // Vertical.
        let mut col: Vec<[f32; 4]> = vec![[0.0; 4]; h];
        for x in 0..w {
            line.clear();
            line.extend((0..h).map(|y| buf[y * w + x]));
            box_pass(&line, &mut col, r);
            for y in 0..h {
                buf[y * w + x] = col[y];
            }
        }
    }
    let data = pm.data_mut();
    for y in 0..h {
        for x in 0..w {
            let i = ((y0 + y) * stride + x0 + x) * 4;
            let v = buf[y * w + x];
            for k in 0..4 {
                data[i + k] = v[k].round().clamp(0.0, 255.0) as u8;
            }
            // Premultiplied channels can't exceed alpha.
            for k in 0..3 {
                data[i + k] = data[i + k].min(data[i + 3]);
            }
        }
    }
}

/// A moving average with clamped edges.
fn box_pass(src: &[[f32; 4]], dst: &mut [[f32; 4]], r: usize) {
    let n = src.len();
    if n == 0 {
        return;
    }
    let at = |i: isize| src[i.clamp(0, n as isize - 1) as usize];
    let span = (2 * r + 1) as f32;
    let mut acc = [0.0f32; 4];
    for i in -(r as isize)..=(r as isize) {
        let v = at(i);
        for k in 0..4 {
            acc[k] += v[k];
        }
    }
    for (i, out) in dst.iter_mut().enumerate().take(n) {
        for k in 0..4 {
            out[k] = acc[k] / span;
        }
        let add = at(i as isize + r as isize + 1);
        let sub = at(i as isize - r as isize);
        for k in 0..4 {
            acc[k] += add[k] - sub[k];
        }
    }
}

fn pixelate(pm: &mut Pixmap, (x0, y0, w, h): (u32, u32, u32, u32), block: u32) {
    let stride = pm.width() as usize;
    let block = block.max(2);
    let data = pm.data_mut();
    let mut by = y0;
    while by < y0 + h {
        let bh = block.min(y0 + h - by);
        let mut bx = x0;
        while bx < x0 + w {
            let bw = block.min(x0 + w - bx);
            let mut sum = [0u64; 4];
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let i = (y as usize * stride + x as usize) * 4;
                    for k in 0..4 {
                        sum[k] += data[i + k] as u64;
                    }
                }
            }
            let n = (bw * bh) as u64;
            let avg = sum.map(|s| ((s + n / 2) / n) as u8);
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let i = (y as usize * stride + x as usize) * 4;
                    data[i..i + 4].copy_from_slice(&avg);
                }
            }
            bx += bw;
        }
        by += bh;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [u8; 4] = [220, 30, 30, 255];

    fn white(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]))
    }

    fn layer(shape: Shape) -> Layer {
        Layer::new(shape, RED, 4.0, false)
    }

    #[test]
    fn rendering_leaves_the_base_alone() {
        let base = white(64, 64);
        let doc = Doc { layers: vec![layer(Shape::Line { from: P::new(0.0, 32.0), to: P::new(64.0, 32.0) })], crop: None };
        let out = render(&base, &doc);
        assert_eq!(base.get_pixel(32, 32).0, [255, 255, 255, 255]);
        assert_eq!(out.get_pixel(32, 32).0, RED);
        assert_eq!(out.get_pixel(32, 5).0, [255, 255, 255, 255]);
    }

    #[test]
    fn hidden_layers_and_crop() {
        let base = white(100, 50);
        let mut l = layer(Shape::Rect { area: Area::from_points(P::new(10.0, 10.0), P::new(40.0, 40.0)) });
        l.filled = true;
        let mut doc = Doc { layers: vec![l], crop: Some(Area::from_points(P::new(5.0, 5.0), P::new(25.0, 45.0))) };
        let out = export(&base, &doc);
        assert_eq!(out.dimensions(), (20, 40));
        assert_eq!(out.get_pixel(10, 10).0, RED);
        doc.layers[0].hidden = true;
        assert_eq!(export(&base, &doc).get_pixel(10, 10).0, [255, 255, 255, 255]);
    }

    #[test]
    fn arrows_steps_and_text_draw_something() {
        let base = white(200, 120);
        for shape in [
            Shape::Arrow { from: P::new(10.0, 60.0), to: P::new(190.0, 60.0) },
            Shape::Step { at: P::new(100.0, 60.0), number: 3 },
            Shape::Text { at: P::new(10.0, 10.0), text: "Hi\nthere".into() },
            Shape::Ellipse { area: Area::from_points(P::new(20.0, 20.0), P::new(180.0, 100.0)) },
            Shape::Pen { points: vec![P::new(5.0, 5.0), P::new(50.0, 80.0), P::new(90.0, 20.0)] },
            Shape::Highlighter { points: vec![P::new(5.0, 100.0), P::new(150.0, 100.0)] },
        ] {
            let doc = Doc { layers: vec![layer(shape.clone())], crop: None };
            let out = render(&base, &doc);
            let changed = out.pixels().filter(|p| p.0 != [255, 255, 255, 255]).count();
            assert!(changed > 50, "{shape:?} changed {changed} pixels");
        }
    }

    #[test]
    fn highlighter_is_translucent() {
        let base = RgbaImage::from_pixel(40, 40, image::Rgba([0, 0, 255, 255]));
        let doc = Doc { layers: vec![layer(Shape::Highlighter { points: vec![P::new(0.0, 20.0), P::new(40.0, 20.0)] })], crop: None };
        let px = render(&base, &doc).get_pixel(20, 20).0;
        assert!(px[0] > 50 && px[2] > 100, "{px:?}");
    }

    #[test]
    fn blur_and_pixelate_mix_neighbours() {
        // Left half black, right half white.
        let base = RgbaImage::from_fn(40, 20, |x, _| if x < 20 { image::Rgba([0, 0, 0, 255]) } else { image::Rgba([255, 255, 255, 255]) });
        let area = Area::from_points(P::new(0.0, 0.0), P::new(40.0, 20.0));
        let blurred = render(&base, &Doc { layers: vec![layer(Shape::Blur { area })], crop: None });
        let edge = blurred.get_pixel(20, 10).0[0];
        assert!(edge > 40 && edge < 215, "{edge}");
        assert_eq!(blurred.get_pixel(20, 10).0[3], 255);
        let mut l = layer(Shape::Pixelate { area: Area::from_points(P::new(16.0, 0.0), P::new(24.0, 8.0)) });
        l.width = 1.0; // 8px blocks
        let px = render(&base, &Doc { layers: vec![l], crop: None });
        assert_eq!(px.get_pixel(16, 0), px.get_pixel(23, 7));
        assert_eq!(px.get_pixel(16, 0).0[0], 128);
        assert_eq!(px.get_pixel(30, 15).0, [255, 255, 255, 255]);
    }

    #[test]
    fn hit_testing() {
        let mut doc = Doc::default();
        doc.layers.push(layer(Shape::Line { from: P::new(0.0, 0.0), to: P::new(100.0, 0.0) }));
        doc.layers.push(layer(Shape::Rect { area: Area::from_points(P::new(10.0, 10.0), P::new(90.0, 90.0)) }));
        doc.layers.push(layer(Shape::Step { at: P::new(50.0, 50.0), number: 1 }));
        assert_eq!(doc.hit(P::new(50.0, 1.0), 3.0), Some(0));
        assert_eq!(doc.hit(P::new(10.0, 40.0), 3.0), Some(1));
        // An outlined rectangle is only hit on its edge…
        assert_eq!(doc.hit(P::new(20.0, 30.0), 3.0), None);
        // …the step on top wins in the middle.
        assert_eq!(doc.hit(P::new(50.0, 50.0), 3.0), Some(2));
        assert_eq!(doc.next_step(), 2);
        doc.layers[2].translate(5.0, 0.0);
        assert_eq!(doc.hit(P::new(68.0, 50.0), 0.0), Some(2));
    }

    #[test]
    fn areas() {
        let a = Area::from_points(P::new(10.0, 5.0), P::new(-2.0, 30.0));
        assert_eq!(a.min, P::new(-2.0, 5.0));
        assert_eq!(a.pixels(8, 100), Some((0, 5, 8, 25)));
        assert_eq!(Area::from_points(P::new(50.0, 50.0), P::new(60.0, 60.0)).pixels(10, 10), None);
        assert!(text_extent("ab\ncd", 20.0).1 > text_extent("ab", 20.0).1);
        assert_eq!(contrast([250, 250, 0, 255]), [0, 0, 0, 255]);
    }
}
