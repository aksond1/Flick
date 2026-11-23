//! Pure geometry. All values are logical pixels, relative to one output.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

    /// Rectangle spanned by two drag points, in any direction.
    pub fn from_points(a: (i32, i32), b: (i32, i32)) -> Self {
        Self::new(
            a.0.min(b.0),
            a.1.min(b.1),
            (a.0 - b.0).abs(),
            (a.1 - b.1).abs(),
        )
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    /// Smallest rectangle holding both.
    pub fn union(&self, o: &Rect) -> Rect {
        let (x, y) = (self.x.min(o.x), self.y.min(o.y));
        Rect::new(
            x,
            y,
            self.right().max(o.right()) - x,
            self.bottom().max(o.bottom()) - y,
        )
    }

    pub fn contains(&self, o: &Rect) -> bool {
        o.x >= self.x && o.y >= self.y && o.right() <= self.right() && o.bottom() <= self.bottom()
    }

    /// Maps from a space of size `from` to one of size `to` (logical to pixels).
    pub fn scaled(&self, from: (i32, i32), to: (i32, i32)) -> Rect {
        let f =
            |v: i32, a: i32, b: i32| (f64::from(v) * f64::from(b) / f64::from(a)).round() as i32;
        let (x, y) = (f(self.x, from.0, to.0), f(self.y, from.1, to.1));
        let (r, b) = (
            f(self.right(), from.0, to.0),
            f(self.bottom(), from.1, to.1),
        );
        Rect::new(x, y, r - x, b - y)
    }

    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
}

/// Places the toolbar strictly outside `region`: below, above, right, then left.
/// `None` means it fits nowhere and has to be hidden while recording.
pub fn place_panel(region: Rect, screen: Rect, size: (i32, i32), gap: i32) -> Option<Rect> {
    let (w, h) = size;
    if w > screen.w || h > screen.h {
        return None;
    }
    let cx = (region.x + (region.w - w) / 2).clamp(screen.x, screen.right() - w);
    let cy = (region.y + (region.h - h) / 2).clamp(screen.y, screen.bottom() - h);
    [
        Rect::new(cx, region.bottom() + gap, w, h),
        Rect::new(cx, region.y - gap - h, w, h),
        Rect::new(region.right() + gap, cy, w, h),
        Rect::new(region.x - gap - w, cy, w, h),
    ]
    .into_iter()
    .find(|p| screen.contains(p) && !p.intersects(&region))
}

#[cfg(test)]
mod tests {
    use super::*;
    const SCREEN: Rect = Rect::new(0, 0, 1920, 1080);

    #[test]
    fn drag_direction_does_not_matter() {
        let r = Rect::new(10, 20, 30, 40);
        assert_eq!(Rect::from_points((10, 20), (40, 60)), r);
        assert_eq!(Rect::from_points((40, 60), (10, 20)), r);
        assert_eq!(Rect::from_points((40, 20), (10, 60)), r);
    }

    #[test]
    fn scaling_keeps_adjacent_edges_together() {
        let (from, to) = ((1536, 864), (1920, 1080));
        let a = Rect::new(0, 0, 333, 100).scaled(from, to);
        let b = Rect::new(333, 0, 1203, 100).scaled(from, to);
        assert_eq!(a.right(), b.x);
        assert_eq!(b.right(), 1920);
        assert_eq!(
            SCREEN.scaled((1920, 1080), (3840, 2160)),
            Rect::new(0, 0, 3840, 2160)
        );
    }

    #[test]
    fn panel_tries_each_side() {
        let size = (300, 48);
        let below = place_panel(Rect::new(500, 500, 400, 200), SCREEN, size, 8).unwrap();
        assert_eq!(below.y, 708);
        let above = place_panel(Rect::new(500, 500, 400, 580), SCREEN, size, 8).unwrap();
        assert_eq!(above.bottom(), 492);
        let right = place_panel(Rect::new(0, 0, 1000, 1080), SCREEN, size, 8).unwrap();
        assert_eq!(right.x, 1008);
        let left = place_panel(Rect::new(920, 0, 1000, 1080), SCREEN, size, 8).unwrap();
        assert_eq!(left.right(), 912);
        assert_eq!(place_panel(SCREEN, SCREEN, size, 8), None);
    }

    #[test]
    fn panel_is_always_on_screen_and_outside_region() {
        for x in (0..1900).step_by(95) {
            for y in (0..1060).step_by(53) {
                let r = Rect::new(x, y, (1920 - x).min(400), (1080 - y).min(300));
                let p = place_panel(r, SCREEN, (300, 48), 8).unwrap();
                assert!(SCREEN.contains(&p) && !p.intersects(&r), "{r:?} {p:?}");
            }
        }
    }
}
