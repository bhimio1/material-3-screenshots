//! Rectangles in Hyprland's logical layout coordinates, and the mapping from
//! them to pixels of a `grim` capture of the whole layout.
//!
//! Hyprland (and slurp, and `grim -g`) talk in logical pixels: a 2560×1600
//! panel at scale 1.6 is 1600×1000 in the layout. `grim` with no `-g` writes
//! the whole layout at the highest output scale, so a frozen capture has to
//! be cropped at `(rect - layout origin) * scale`.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    /// `grim -g` / slurp's default output format: `x,y wxh`.
    pub fn to_grim(self) -> String {
        format!("{},{} {}x{}", self.x, self.y, self.w, self.h)
    }

    /// gpu-screen-recorder's `-region`: `WxH+X+Y`.
    pub fn to_wxh_xy(self) -> String {
        format!("{}x{}+{}+{}", self.w, self.h, self.x, self.y)
    }

    /// Parse slurp's `x,y wxh` (a trailing label is ignored).
    pub fn parse(s: &str) -> Option<Rect> {
        let mut parts = s.split_whitespace();
        let (x, y) = parts.next()?.split_once(',')?;
        let (w, h) = parts.next()?.split_once('x')?;
        let r = Rect::new(x.parse().ok()?, y.parse().ok()?, w.parse().ok()?, h.parse().ok()?);
        (!r.is_empty()).then_some(r)
    }

    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let r = Rect::new(x, y, self.right().min(other.right()) - x, self.bottom().min(other.bottom()) - y);
        (!r.is_empty()).then_some(r)
    }

    pub fn union(&self, other: &Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect::new(x, y, self.right().max(other.right()) - x, self.bottom().max(other.bottom()) - y)
    }
}

/// One output, as Hyprland lays it out.
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    pub name: String,
    /// Mode size in physical pixels.
    pub width: i32,
    pub height: i32,
    pub x: i32,
    pub y: i32,
    pub scale: f32,
    /// wl_output transform: odd values are rotated 90°/270°.
    pub transform: i32,
    pub focused: bool,
    pub active_workspace: i64,
    pub special_workspace: i64,
}

impl Monitor {
    /// The output's rectangle in the logical layout.
    pub fn logical(&self) -> Rect {
        let (w, h) = if self.transform % 2 == 1 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        };
        let s = if self.scale > 0.0 { self.scale } else { 1.0 };
        Rect::new(self.x, self.y, (w as f32 / s).round() as i32, (h as f32 / s).round() as i32)
    }
}

/// The union of all outputs.
pub fn layout_bounds(monitors: &[Monitor]) -> Option<Rect> {
    monitors.iter().map(Monitor::logical).reduce(|a, b| a.union(&b))
}

/// The pixel rectangle of `rect` inside a capture of the whole `bounds`
/// that came out `image_w` pixels wide. The scale is measured from the
/// image rather than assumed, so it holds whatever grim chose. The result
/// is clamped to the image.
pub fn crop_in_capture(rect: &Rect, bounds: &Rect, image_w: u32, image_h: u32) -> Option<(u32, u32, u32, u32)> {
    if bounds.is_empty() {
        return None;
    }
    let sx = image_w as f64 / bounds.w as f64;
    let sy = image_h as f64 / bounds.h as f64;
    let x0 = (((rect.x - bounds.x) as f64) * sx).round().max(0.0);
    let y0 = (((rect.y - bounds.y) as f64) * sy).round().max(0.0);
    let x1 = (((rect.right() - bounds.x) as f64) * sx).round().min(image_w as f64);
    let y1 = (((rect.bottom() - bounds.y) as f64) * sy).round().min(image_h as f64);
    (x1 > x0 && y1 > y0).then_some((x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(name: &str, x: i32, y: i32, w: i32, h: i32, scale: f32, transform: i32) -> Monitor {
        Monitor {
            name: name.into(),
            width: w,
            height: h,
            x,
            y,
            scale,
            transform,
            focused: false,
            active_workspace: 1,
            special_workspace: 0,
        }
    }

    #[test]
    fn parse_and_format() {
        let r = Rect::parse("10,-20 300x200").unwrap();
        assert_eq!(r, Rect::new(10, -20, 300, 200));
        assert_eq!(r.to_grim(), "10,-20 300x200");
        assert_eq!(r.to_wxh_xy(), "300x200+10+-20");
        assert_eq!(Rect::parse("1,2 3x4 kitty\n"), Some(Rect::new(1, 2, 3, 4)));
        assert_eq!(Rect::parse("1,2 0x4"), None);
        assert_eq!(Rect::parse("garbage"), None);
        assert_eq!(Rect::parse(""), None);
    }

    #[test]
    fn intersection_and_union() {
        let a = Rect::new(0, 0, 100, 100);
        let b = Rect::new(50, 50, 100, 100);
        assert_eq!(a.intersect(&b), Some(Rect::new(50, 50, 50, 50)));
        assert_eq!(a.intersect(&Rect::new(200, 0, 10, 10)), None);
        assert_eq!(a.union(&b), Rect::new(0, 0, 150, 150));
    }

    #[test]
    fn hidpi_and_rotation() {
        assert_eq!(mon("A", 0, 0, 2560, 1600, 1.6, 0).logical(), Rect::new(0, 0, 1600, 1000));
        // Portrait (transform 1 = 90°): width and height swap.
        assert_eq!(mon("B", 1600, 0, 1920, 1080, 1.0, 1).logical(), Rect::new(1600, 0, 1080, 1920));
        assert_eq!(mon("C", 0, 0, 3840, 2160, 2.0, 2).logical(), Rect::new(0, 0, 1920, 1080));
    }

    #[test]
    fn mixed_scale_layout() {
        // A 1.5x laptop left of a 1x monitor, with a negative origin.
        let ms = [mon("eDP-1", -1920, 0, 2880, 1800, 1.5, 0), mon("DP-1", 0, 0, 2560, 1440, 1.0, 0)];
        let b = layout_bounds(&ms).unwrap();
        assert_eq!(b, Rect::new(-1920, 0, 4480, 1440));
        // grim renders the whole layout at 1.5x: 6720×2160.
        let (w, h) = (6720, 2160);
        // The external monitor's top-left 100×100 logical pixels.
        assert_eq!(crop_in_capture(&Rect::new(0, 0, 100, 100), &b, w, h), Some((2880, 0, 150, 150)));
        // The whole laptop panel maps onto its native pixels.
        assert_eq!(crop_in_capture(&ms[0].logical(), &b, w, h), Some((0, 0, 2880, 1800)));
        // Partly off the layout: clamped.
        assert_eq!(crop_in_capture(&Rect::new(2500, 1400, 100, 100), &b, w, h), Some((6630, 2100, 90, 60)));
        assert_eq!(crop_in_capture(&Rect::new(9000, 0, 10, 10), &b, w, h), None);
    }
}
