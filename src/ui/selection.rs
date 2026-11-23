//! Region selection rules, free of any toolkit.
use crate::geometry::Rect;

pub const HANDLE: i32 = 8;
pub const MIN_SIZE: i32 = 5;

/// Which edges a drag moves: -1 left/top, 1 right/bottom, 0 none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edges {
    pub x: i8,
    pub y: i8,
}

#[derive(Debug, Clone, Copy)]
enum Drag {
    New((i32, i32)),
    Resize(Rect, Edges),
}

pub struct Selection {
    screen: Rect,
    pub region: Option<Rect>,
    pub locked: bool,
    drag: Option<Drag>,
}

impl Selection {
    pub fn new(screen: Rect, locked: Option<Rect>) -> Self {
        Self {
            screen,
            region: locked,
            locked: locked.is_some(),
            drag: None,
        }
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The edge or corner under `p`, if it can be dragged.
    pub fn edges_at(&self, p: (i32, i32)) -> Option<Edges> {
        let r = self.region.filter(|_| !self.locked)?;
        let side = |v: i32, lo: i32, hi: i32| {
            let (a, b) = ((v - lo).abs(), (v - hi).abs());
            match a.min(b) <= HANDLE {
                true if a < b => -1,
                true => 1,
                false => 0,
            }
        };
        let near = p.0 >= r.x - HANDLE
            && p.0 <= r.right() + HANDLE
            && p.1 >= r.y - HANDLE
            && p.1 <= r.bottom() + HANDLE;
        let e = Edges {
            x: side(p.0, r.x, r.right()),
            y: side(p.1, r.y, r.bottom()),
        };
        (near && (e.x != 0 || e.y != 0)).then_some(e)
    }

    /// Returns false when the region is locked.
    pub fn begin(&mut self, p: (i32, i32)) -> bool {
        if self.locked {
            return false;
        }
        self.drag = Some(match (self.region, self.edges_at(p)) {
            (Some(r), Some(e)) => Drag::Resize(r, e),
            _ => {
                self.region = None;
                Drag::New(self.clamp(p))
            }
        });
        true
    }

    /// `p` is the current pointer position.
    pub fn update(&mut self, p: (i32, i32)) {
        let p = self.clamp(p);
        self.region = match self.drag {
            None => return,
            Some(Drag::New(start)) => Some(Rect::from_points(start, p)),
            Some(Drag::Resize(r, e)) => {
                let axis = |edge: i8, lo: i32, hi: i32, v: i32| match edge {
                    -1 => (hi, v),
                    1 => (lo, v),
                    _ => (lo, hi),
                };
                let (x0, x1) = axis(e.x, r.x, r.right(), p.0);
                let (y0, y1) = axis(e.y, r.y, r.bottom(), p.1);
                Some(Rect::from_points((x0, y0), (x1, y1)))
            }
        };
    }

    pub fn end(&mut self) {
        self.drag = None;
        self.region = self.region.filter(|r| r.w >= MIN_SIZE && r.h >= MIN_SIZE);
    }

    fn clamp(&self, p: (i32, i32)) -> (i32, i32) {
        (
            p.0.clamp(self.screen.x, self.screen.right()),
            p.1.clamp(self.screen.y, self.screen.bottom()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect::new(0, 0, 1000, 800);

    fn drag(s: &mut Selection, a: (i32, i32), b: (i32, i32)) {
        if s.begin(a) {
            s.update(b);
            s.end();
        }
    }

    #[test]
    fn select_reselect_and_click_clears() {
        let mut s = Selection::new(SCREEN, None);
        drag(&mut s, (100, 100), (300, 200));
        assert_eq!(s.region, Some(Rect::new(100, 100, 200, 100)));
        drag(&mut s, (200, 150), (1500, -20));
        assert_eq!(s.region, Some(Rect::new(200, 0, 800, 150)));
        drag(&mut s, (600, 400), (601, 401));
        assert_eq!(s.region, None);
    }

    #[test]
    fn edges_resize_and_can_cross_over() {
        let mut s = Selection::new(SCREEN, None);
        drag(&mut s, (100, 100), (300, 200));
        drag(&mut s, (300, 200), (400, 250));
        assert_eq!(s.region, Some(Rect::new(100, 100, 300, 150)));
        drag(&mut s, (100, 170), (50, 600));
        assert_eq!(s.region, Some(Rect::new(50, 100, 350, 150)));
        drag(&mut s, (50, 170), (500, 170));
        assert_eq!(s.region, Some(Rect::new(400, 100, 100, 150)));
    }

    #[test]
    fn locked_region_is_immutable() {
        let r = Rect::new(10, 10, 50, 50);
        let mut s = Selection::new(SCREEN, Some(r));
        drag(&mut s, (200, 200), (400, 400));
        drag(&mut s, (10, 10), (0, 0));
        assert_eq!(s.region, Some(r));
        s.locked = false;
        drag(&mut s, (10, 10), (0, 0));
        assert_eq!(s.region, Some(Rect::new(0, 0, 60, 60)));
    }
}
