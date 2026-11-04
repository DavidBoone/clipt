//! Repainting only what changed: the editor reports damage in logical
//! coordinates, and each host turns it into a few pixel rectangles per
//! screen.

/// Screen-pixel rectangle.
#[derive(Clone, Copy, Debug)]
pub struct IRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl IRect {
    fn right(&self) -> i32 {
        self.x + self.w
    }
    fn bottom(&self) -> i32 {
        self.y + self.h
    }
    fn union(&self, o: &IRect) -> IRect {
        let (x, y) = (self.x.min(o.x), self.y.min(o.y));
        IRect { x, y, w: self.right().max(o.right()) - x, h: self.bottom().max(o.bottom()) - y }
    }
    fn area(&self) -> i64 {
        self.w as i64 * self.h as i64
    }
}

/// Merge rectangles where one box costs little more than the two separately,
/// so nearby fragments become one repaint but a horizontal and a vertical
/// line (the crosshair) don't turn into the whole screen. Overlap is fine:
/// repainting a pixel twice gives the same result.
pub fn merge(mut v: Vec<IRect>) -> Vec<IRect> {
    // Slack for small fragments: a 64x64 block is cheaper to merge than to
    // issue separately.
    const SLACK: i64 = 64 * 64;
    let mut i = 0;
    while i < v.len() {
        let mut j = i + 1;
        let mut merged = false;
        while j < v.len() {
            let u = v[i].union(&v[j]);
            if u.area() * 4 <= (v[i].area() + v[j].area()) * 5 + SLACK {
                v.swap_remove(j);
                v[i] = u;
                merged = true;
            } else {
                j += 1;
            }
        }
        if !merged {
            i += 1;
        }
    }
    v
}

/// Logical damage rectangles -> pixel rectangles inside one screen's buffer.
pub fn screen_rects(scr: &crate::editor::Screen, rs: &[tiny_skia::Rect]) -> Vec<IRect> {
    let (bw, bh) = (scr.bg.width() as i32, scr.bg.height() as i32);
    let o = &scr.output;
    let k = scr.scale;
    rs.iter()
        .filter_map(|r| {
            let x0 = (((r.left() - o.x as f32) * k).floor() as i32).clamp(0, bw);
            let y0 = (((r.top() - o.y as f32) * k).floor() as i32).clamp(0, bh);
            let x1 = (((r.right() - o.x as f32) * k).ceil() as i32).clamp(0, bw);
            let y1 = (((r.bottom() - o.y as f32) * k).ceil() as i32).clamp(0, bh);
            (x1 > x0 && y1 > y0).then_some(IRect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
        })
        .collect()
}

/// Repaint `r` of screen `idx`'s `w`×`h` bitmap `buf`. It is rendered with
/// a margin and only `r` copied back: tiny-skia antialiases a line that a
/// pixmap's edge cuts through differently from one it doesn't, so a damaged
/// rectangle whose edge crossed a line (the loupe's frame, an outline) left
/// a faint seam there.
pub fn repaint(ed: &super::Editor, idx: usize, buf: &mut [u8], w: u32, h: u32, r: IRect) {
    const MARGIN: i32 = 2;
    let (x0, y0) = ((r.x - MARGIN).max(0), (r.y - MARGIN).max(0));
    let (x1, y1) = ((r.right() + MARGIN).min(w as i32), (r.bottom() + MARGIN).min(h as i32));
    let (rx1, ry1) = (r.right().min(w as i32), r.bottom().min(h as i32));
    if rx1 <= r.x || ry1 <= r.y {
        return;
    }
    let Some(mut tmp) = tiny_skia::Pixmap::new((x1 - x0) as u32, (y1 - y0) as u32) else { return };
    super::ui::render(ed, idx, &mut tmp.as_mut(), (x0, y0));
    let (tw, n) = ((x1 - x0) as usize, (rx1 - r.x) as usize * 4);
    for y in r.y..ry1 {
        let src = ((y - y0) as usize * tw + (r.x - x0) as usize) * 4;
        let dst = (y as usize * w as usize + r.x as usize) * 4;
        buf[dst..dst + n].copy_from_slice(&tmp.data()[src..src + n]);
    }
}

#[cfg(test)]
mod tests {
    use super::{IRect, merge};

    #[test]
    fn crosshair_stays_two_strips() {
        let h = IRect { x: 0, y: 1000, w: 3840, h: 28 };
        let v = IRect { x: 1900, y: 0, w: 28, h: 2160 };
        let out = merge(vec![h, v]);
        assert_eq!(out.len(), 2);
        assert!(out.iter().map(|r| r.area()).sum::<i64>() < 3840 * 2160 / 20);
    }

    #[test]
    fn neighbours_and_overlaps_merge() {
        let out =
            merge(vec![IRect { x: 0, y: 0, w: 100, h: 100 }, IRect { x: 50, y: 50, w: 100, h: 100 }, IRect { x: 152, y: 0, w: 10, h: 10 }]);
        assert_eq!(out.len(), 1);
    }
}
