//! Annotation shapes and their rendering. Shapes live in logical (global
//! desktop) coordinates; a `View` maps them into a pixmap's pixel space.

use crate::text::{self, Fonts};
use tiny_skia::{Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PixmapMut, Rect, Stroke, Transform};

pub type Pt = (f32, f32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Rect,
    Ellipse,
    Arrow,
    Line,
    Pen,
    Marker,
    Text,
    Pixelate,
    Counter,
}

impl Tool {
    pub const DRAWING: [Tool; 9] =
        [Tool::Rect, Tool::Ellipse, Tool::Arrow, Tool::Line, Tool::Pen, Tool::Marker, Tool::Text, Tool::Pixelate, Tool::Counter];

    pub fn hotkey(self) -> char {
        match self {
            Tool::Select => 'v',
            Tool::Rect => 'r',
            Tool::Ellipse => 'e',
            Tool::Arrow => 'a',
            Tool::Line => 'l',
            Tool::Pen => 'p',
            Tool::Marker => 'm',
            Tool::Text => 't',
            Tool::Pixelate => 'x',
            Tool::Counter => 'n',
        }
    }
}

#[derive(Clone, Debug)]
pub enum Shape {
    Rect { a: Pt, b: Pt, color: Color, width: f32 },
    Ellipse { a: Pt, b: Pt, color: Color, width: f32 },
    Arrow { a: Pt, b: Pt, color: Color, width: f32 },
    Line { a: Pt, b: Pt, color: Color, width: f32 },
    Pen { pts: Vec<Pt>, color: Color, width: f32 },
    Marker { pts: Vec<Pt>, color: Color, width: f32 },
    Text { pos: Pt, text: String, color: Color, size: f32 },
    Pixelate { a: Pt, b: Pt, block: f32 },
    Counter { pos: Pt, n: u32, color: Color, radius: f32 },
}

/// Logical → pixel mapping for one target pixmap.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub ox: f32,
    pub oy: f32,
    pub scale: f32,
    /// Target stores BGRA instead of RGBA.
    pub swap_rb: bool,
    /// Pixel offset of the target within the full output (partial repaints).
    /// Applied after scaling so partial and full frames round identically.
    pub off: (f32, f32),
}

impl View {
    pub fn px(&self, p: Pt) -> Pt {
        ((p.0 - self.ox) * self.scale - self.off.0, (p.1 - self.oy) * self.scale - self.off.1)
    }
    pub fn color(&self, c: Color) -> Color {
        if self.swap_rb { Color::from_rgba(c.blue(), c.green(), c.red(), c.alpha()).unwrap() } else { c }
    }
    pub fn paint(&self, c: Color) -> Paint<'static> {
        let mut p = Paint::default();
        p.set_color(self.color(c));
        p.anti_alias = true;
        p
    }
}

pub fn norm_rect(a: Pt, b: Pt) -> Option<Rect> {
    Rect::from_ltrb(a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
}

fn stroke(width: f32) -> Stroke {
    Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() }
}

fn polyline(view: &View, pts: &[Pt]) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let first = view.px(*pts.first()?);
    pb.move_to(first.0, first.1);
    if pts.len() == 1 {
        pb.line_to(first.0 + 0.01, first.1);
    }
    // Midpoint quadratic smoothing keeps freehand strokes from looking jagged.
    for w in pts.windows(2) {
        let p0 = view.px(w[0]);
        let p1 = view.px(w[1]);
        pb.quad_to(p0.0, p0.1, (p0.0 + p1.0) / 2.0, (p0.1 + p1.1) / 2.0);
    }
    let last = view.px(*pts.last()?);
    pb.line_to(last.0, last.1);
    pb.finish()
}

/// Paints the background for the source-space pixels starting at the given
/// origin into a fresh pixmap, for pixelation that needs more than the target
/// holds.
pub type Fill<'a> = &'a dyn Fn(&mut PixmapMut, (i32, i32));

/// Draw `shapes` in order onto `pm`, which already holds the background and
/// whose (0, 0) is source pixel `view.off`. `size` is the whole source's size.
///
/// Pixelation averages what's under it, earlier shapes included, so it needs
/// every pixel of each block it repaints. When `pm` is a partial repaint that
/// cuts through blocks, those blocks are rebuilt in a scratch pixmap first:
/// background from `fill`, then the shapes before this one.
pub fn draw_all(pm: &mut PixmapMut, view: &View, size: (i32, i32), shapes: &[&Shape], fonts: &Fonts, fill: Fill) {
    let origin = (view.off.0 as i32, view.off.1 as i32);
    let here = (origin.0, origin.1, origin.0 + pm.width() as i32, origin.1 + pm.height() as i32);
    for (k, shape) in shapes.iter().enumerate() {
        let Shape::Pixelate { a, b, block } = shape else {
            shape.draw(pm, view, fonts);
            continue;
        };
        let Some(area) = pixelate_area(view, *a, *b, size) else { continue };
        let bs = (block * view.scale).max(2.0).round() as i32;
        let Some(need) = blocks_touching(area, bs, here) else { continue };
        if need.0 >= here.0 && need.1 >= here.1 && need.2 <= here.2 && need.3 <= here.3 {
            pixelate_blocks(pm, origin, area, bs, need);
            continue;
        }
        let Some(mut tmp) = Pixmap::new((need.2 - need.0) as u32, (need.3 - need.1) as u32) else { continue };
        let at = (need.0, need.1);
        fill(&mut tmp.as_mut(), at);
        let tv = View { off: (at.0 as f32, at.1 as f32), ..*view };
        draw_all(&mut tmp.as_mut(), &tv, size, &shapes[..k], fonts, fill);
        pixelate_blocks(&mut tmp.as_mut(), at, area, bs, need);
        // Copy back the part this pixmap holds.
        let (x0, y0, x1, y1) = (need.0.max(here.0), need.1.max(here.1), need.2.min(here.2), need.3.min(here.3));
        let n = (x1 - x0) as usize * 4;
        let (tw, pw) = (tmp.width() as usize * 4, pm.width() as usize * 4);
        let src = tmp.data();
        let dst = pm.data_mut();
        for y in y0..y1 {
            let si = (y - at.1) as usize * tw + (x0 - at.0) as usize * 4;
            let di = (y - origin.1) as usize * pw + (x0 - origin.0) as usize * 4;
            dst[di..di + n].copy_from_slice(&src[si..si + n]);
        }
    }
}

impl Shape {
    /// Draw this shape. Pixelation averages `pm` in place, so `pm` must hold
    /// whole blocks: `draw_all` sees to that.
    pub fn draw(&self, pm: &mut PixmapMut, view: &View, fonts: &Fonts) {
        let s = view.scale;
        let id = Transform::identity();
        match self {
            Shape::Rect { a, b, color, width } => {
                let (a, b) = (view.px(*a), view.px(*b));
                if let Some(r) = norm_rect(a, b) {
                    let path = PathBuilder::from_rect(r);
                    let st = Stroke { line_join: LineJoin::Round, ..stroke(width * s) };
                    pm.stroke_path(&path, &view.paint(*color), &st, id, None);
                }
            }
            Shape::Ellipse { a, b, color, width } => {
                let (a, b) = (view.px(*a), view.px(*b));
                if let Some(path) = norm_rect(a, b).and_then(PathBuilder::from_oval) {
                    pm.stroke_path(&path, &view.paint(*color), &stroke(width * s), id, None);
                }
            }
            Shape::Line { a, b, color, width } => {
                let (a, b) = (view.px(*a), view.px(*b));
                let mut pb = PathBuilder::new();
                pb.move_to(a.0, a.1);
                pb.line_to(b.0, b.1);
                if let Some(path) = pb.finish() {
                    pm.stroke_path(&path, &view.paint(*color), &stroke(width * s), id, None);
                }
            }
            Shape::Arrow { a, b, color, width } => draw_arrow(pm, view, *a, *b, *color, *width),
            Shape::Pen { pts, color, width } => {
                if let Some(path) = polyline(view, pts) {
                    pm.stroke_path(&path, &view.paint(*color), &stroke(width * s), id, None);
                }
            }
            Shape::Marker { pts, color, width } => {
                if let Some(path) = polyline(view, pts) {
                    let mut c = *color;
                    c.set_alpha(0.38);
                    let st = Stroke { line_cap: LineCap::Square, ..stroke(width * s) };
                    pm.stroke_path(&path, &view.paint(c), &st, id, None);
                }
            }
            Shape::Text { pos, text: t, color, size } => {
                let p = view.px(*pos);
                text::draw(pm, fonts.bold(), size * s, p.0, p.1, t, *color, view.swap_rb);
            }
            Shape::Counter { pos, n, color, radius } => {
                let p = view.px(*pos);
                let r = radius * s;
                if let Some(c) = PathBuilder::from_circle(p.0, p.1, r) {
                    pm.fill_path(&c, &view.paint(*color), FillRule::Winding, id, None);
                    let ring = Stroke { width: (1.5 * s).max(1.0), ..Default::default() };
                    pm.stroke_path(&c, &view.paint(Color::from_rgba8(255, 255, 255, 230)), &ring, id, None);
                }
                let label = n.to_string();
                let fs = r * 1.15;
                let w = text::measure(fonts.bold(), fs, &label);
                let h = text::line_height(fonts.bold(), fs);
                let fg = if luminance(*color) > 0.6 { Color::BLACK } else { Color::WHITE };
                text::draw(pm, fonts.bold(), fs, p.0 - w / 2.0, p.1 - h / 2.0, &label, fg, view.swap_rb);
            }
            Shape::Pixelate { a, b, block } => {
                let origin = (view.off.0 as i32, view.off.1 as i32);
                let here = (origin.0, origin.1, origin.0 + pm.width() as i32, origin.1 + pm.height() as i32);
                let bs = (block * s).max(2.0).round() as i32;
                if let Some(area) = pixelate_area(view, *a, *b, (here.2, here.3))
                    && let Some(need) = blocks_touching(area, bs, here)
                {
                    pixelate_blocks(pm, origin, area, bs, need);
                }
            }
        }
    }
}

impl Shape {
    /// Generous logical bounding box (text is measured by the caller).
    pub fn bounds(&self, fonts: &Fonts) -> Option<Rect> {
        let pad = |r: Option<Rect>, m: f32| r.and_then(|r| Rect::from_ltrb(r.left() - m, r.top() - m, r.right() + m, r.bottom() + m));
        let of_pts = |pts: &[Pt]| {
            let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for p in pts {
                (l, t, r, b) = (l.min(p.0), t.min(p.1), r.max(p.0), b.max(p.1));
            }
            Rect::from_ltrb(l, t, r.max(l + 1.0), b.max(t + 1.0))
        };
        match self {
            Shape::Rect { a, b, width, .. } | Shape::Ellipse { a, b, width, .. } | Shape::Line { a, b, width, .. } => {
                pad(of_pts(&[*a, *b]), width + 2.0)
            }
            Shape::Arrow { a, b, width, .. } => pad(of_pts(&[*a, *b]), width * 3.0 + 12.0),
            Shape::Pixelate { a, b, block } => pad(of_pts(&[*a, *b]), block + 2.0),
            Shape::Pen { pts, width, .. } | Shape::Marker { pts, width, .. } => pad(of_pts(pts), width + 2.0),
            Shape::Counter { pos, radius, .. } => pad(of_pts(&[*pos]), radius + 3.0),
            Shape::Text { pos, text: t, size, .. } => {
                let w = t.split('\n').map(|l| text::measure(fonts.bold(), *size, l)).fold(0.0, f32::max);
                let h = text::line_height(fonts.bold(), *size) * (t.matches('\n').count() + 1) as f32;
                pad(Rect::from_xywh(pos.0, pos.1, w.max(1.0) + 4.0, h), 4.0)
            }
        }
    }
}

pub fn luminance(c: Color) -> f32 {
    0.2126 * c.red() + 0.7152 * c.green() + 0.0722 * c.blue()
}

fn draw_arrow(pm: &mut PixmapMut, view: &View, a: Pt, b: Pt, color: Color, width: f32) {
    let s = view.scale;
    let (a, b) = (view.px(a), view.px(b));
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        return;
    }
    let (ux, uy) = (dx / len, dy / len);
    let head = ((width * 3.0 + 9.0) * s).min(len * 0.6);
    let half = head * 0.55;
    // Shaft stops inside the head so the round cap doesn't poke through the tip.
    let shaft_end = (b.0 - ux * head * 0.8, b.1 - uy * head * 0.8);
    let mut pb = PathBuilder::new();
    pb.move_to(a.0, a.1);
    pb.line_to(shaft_end.0, shaft_end.1);
    let paint = view.paint(color);
    if let Some(p) = pb.finish() {
        pm.stroke_path(&p, &paint, &stroke(width * s), Transform::identity(), None);
    }
    let base = (b.0 - ux * head, b.1 - uy * head);
    let mut pb = PathBuilder::new();
    pb.move_to(b.0, b.1);
    pb.line_to(base.0 - uy * half, base.1 + ux * half);
    pb.line_to(base.0 + uy * half, base.1 - ux * half);
    pb.close();
    if let Some(p) = pb.finish() {
        pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
        let st = Stroke { width: (width * s * 0.5).max(1.0), line_join: LineJoin::Round, ..Default::default() };
        pm.stroke_path(&p, &paint, &st, Transform::identity(), None);
    }
}

/// The source-space pixels a pixelation covers, as (left, top, right,
/// bottom) clipped to a source of `size`. Blocks start at its top left, so
/// every repaint, partial or full, lines them up the same way.
fn pixelate_area(view: &View, a: Pt, b: Pt, size: (i32, i32)) -> Option<(i32, i32, i32, i32)> {
    let r = norm_rect(view.px(a), view.px(b))?;
    let off = (view.off.0 as i32, view.off.1 as i32);
    let x0 = (r.left().floor() as i32 + off.0).clamp(0, size.0);
    let y0 = (r.top().floor() as i32 + off.1).clamp(0, size.1);
    let x1 = (r.right().ceil() as i32 + off.0).clamp(0, size.0);
    let y1 = (r.bottom().ceil() as i32 + off.1).clamp(0, size.1);
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
}

/// The whole blocks of `area` (side `bs`) that touch `region`, all in source
/// pixels.
fn blocks_touching(area: (i32, i32, i32, i32), bs: i32, region: (i32, i32, i32, i32)) -> Option<(i32, i32, i32, i32)> {
    let span = |a0: i32, a1: i32, r0: i32, r1: i32| {
        let (v0, v1) = (a0.max(r0), a1.min(r1));
        (v0 < v1).then(|| (a0 + (v0 - a0) / bs * bs, (a0 + (v1 - a0 + bs - 1) / bs * bs).min(a1)))
    };
    let (x0, x1) = span(area.0, area.2, region.0, region.2)?;
    let (y0, y1) = span(area.1, area.3, region.1, region.3)?;
    Some((x0, y0, x1, y1))
}

/// Replace each block of `area` within `need` (both source pixels, `need`
/// block-aligned and inside `pm`) with its average. `pm`'s (0, 0) is source
/// pixel `origin`.
fn pixelate_blocks(pm: &mut PixmapMut, origin: (i32, i32), area: (i32, i32, i32, i32), bs: i32, need: (i32, i32, i32, i32)) {
    let stride = pm.width() as usize * 4;
    let d = pm.data_mut();
    let mut by = need.1;
    while by < need.3 {
        let ey = (by + bs).min(area.3);
        let mut bx = need.0;
        while bx < need.2 {
            let ex = (bx + bs).min(area.2);
            let rows = || (by - origin.1) as usize..(ey - origin.1) as usize;
            let cols = (bx - origin.0) as usize * 4..(ex - origin.0) as usize * 4;
            let mut acc = [0u32; 4];
            for y in rows() {
                for p in d[y * stride..][cols.clone()].as_chunks::<4>().0 {
                    for c in 0..4 {
                        acc[c] += p[c] as u32;
                    }
                }
            }
            let n = ((ex - bx) * (ey - by)) as u32;
            let px = acc.map(|v| (v / n) as u8);
            for y in rows() {
                for p in d[y * stride..][cols.clone()].as_chunks_mut::<4>().0 {
                    *p = px;
                }
            }
            bx = ex;
        }
        by = ey;
    }
}

pub trait RectExt: Sized {
    fn contains(&self, x: f32, y: f32) -> bool;
    fn translate(&self, dx: f32, dy: f32) -> Option<Self>;
}

impl RectExt for Rect {
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.left() && x < self.right() && y >= self.top() && y < self.bottom()
    }
    fn translate(&self, dx: f32, dy: f32) -> Option<Self> {
        Rect::from_xywh(self.left() + dx, self.top() + dy, self.width(), self.height())
    }
}
