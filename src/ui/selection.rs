//! Region selection rules, free of any toolkit.
use crate::geometry::Rect;

pub const HANDLE: i32 = 8;
/// Grab distance for the guide lines; small, so they rarely block a new selection.
pub const LINE_GRAB: f64 = 4.0;
pub const MIN_SIZE: i32 = 5;

/// Which edges a drag moves: -1 left/top, 1 right/bottom, 0 none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edges {
    pub x: i8,
    pub y: i8,
}

/// Clockwise from the top left corner.
const ALL_EDGES: [Edges; 8] = [
    Edges { x: -1, y: -1 },
    Edges { x: 0, y: -1 },
    Edges { x: 1, y: -1 },
    Edges { x: 1, y: 0 },
    Edges { x: 1, y: 1 },
    Edges { x: 0, y: 1 },
    Edges { x: -1, y: 1 },
    Edges { x: -1, y: 0 },
];

/// The corner or side midpoint of `r` that `e` names.
fn anchor(r: Rect, e: Edges) -> (i32, i32) {
    let at = |edge: i8, lo: i32, hi: i32| match edge {
        -1 => lo,
        1 => hi,
        _ => lo + (hi - lo) / 2,
    };
    (at(e.x, r.x, r.right()), at(e.y, r.y, r.bottom()))
}

/// Distance from a point to a segment.
fn distance(p: (i32, i32), [a, b]: [(i32, i32); 2]) -> f64 {
    let v = |p: (i32, i32), q: (i32, i32)| (f64::from(q.0 - p.0), f64::from(q.1 - p.1));
    let (ab, ap) = (v(a, b), v(a, p));
    let len2 = ab.0 * ab.0 + ab.1 * ab.1;
    let t = if len2 == 0.0 {
        0.0
    } else {
        ((ap.0 * ab.0 + ap.1 * ab.1) / len2).clamp(0.0, 1.0)
    };
    (ap.0 - t * ab.0).hypot(ap.1 - t * ab.1)
}

#[derive(Debug, Clone, Copy)]
enum Drag {
    New((i32, i32)),
    /// Original region and the grab point.
    Move(Rect, (i32, i32)),
    /// Original region, moved edges and the grab point. Edges follow the pointer's
    /// displacement, so grabbing a guide far away does not make the region jump.
    Resize(Rect, Edges, (i32, i32)),
}

#[derive(Clone)]
pub struct Selection {
    screen: Rect,
    pub region: Option<Rect>,
    pub locked: bool,
    /// Without guides there is nothing to draw or grab beyond the handles.
    pub guides: bool,
    drag: Option<Drag>,
}

impl Selection {
    pub fn new(screen: Rect, locked: Option<Rect>) -> Self {
        Self {
            screen,
            region: locked,
            locked: locked.is_some(),
            guides: true,
            drag: None,
        }
    }

    pub fn with_guides(mut self, on: bool) -> Self {
        self.guides = on;
        self
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The whole region is being dragged.
    pub fn moving(&self) -> bool {
        matches!(self.drag, Some(Drag::Move(..)))
    }

    /// The eight resize handles with their edges; empty while locked.
    pub fn handles(&self) -> Vec<((i32, i32), Edges)> {
        let Some(r) = self.region.filter(|_| !self.locked) else {
            return Vec::new();
        };
        ALL_EDGES.map(|e| (anchor(r, e), e)).to_vec()
    }

    /// One line per handle: corners run to the matching screen corner,
    /// side handles straight to the screen edge.
    pub fn guides(&self) -> Vec<([(i32, i32); 2], Edges)> {
        if !self.guides {
            return Vec::new();
        }
        self.handles()
            .into_iter()
            .map(|(from, e)| {
                let to = anchor(self.screen, e);
                let x = if e.x == 0 { from.0 } else { to.0 };
                let y = if e.y == 0 { from.1 } else { to.1 };
                ([from, (x, y)], e)
            })
            .collect()
    }

    /// The edge, corner or guide line under `p`, if it can be dragged.
    pub fn edges_at(&self, p: (i32, i32)) -> Option<Edges> {
        let r = self.region.filter(|_| !self.locked)?;
        let side = |v: i32, lo: i32, hi: i32| {
            let (a, b) = ((v - lo).abs(), (v - hi).abs());
            // Inside a small region the handles shrink, leaving a middle to move it by.
            let inside = v > lo && v < hi;
            let reach = if inside {
                HANDLE.min((hi - lo) / 4)
            } else {
                HANDLE
            };
            match a.min(b) <= reach {
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
        if near && (e.x != 0 || e.y != 0) {
            return Some(e);
        }
        // The middle moves the region; guides only count outside.
        if p.0 > r.x && p.0 < r.right() && p.1 > r.y && p.1 < r.bottom() {
            return None;
        }
        self.guides()
            .into_iter()
            .map(|(line, e)| (distance(p, line), e))
            .filter(|(d, _)| *d <= LINE_GRAB)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, e)| e)
    }

    /// Whether dragging from `p` moves the whole region; a locked one too.
    pub fn moves_at(&self, p: (i32, i32)) -> bool {
        let inside = |r: Rect| p.0 > r.x && p.0 < r.right() && p.1 > r.y && p.1 < r.bottom();
        self.region.is_some_and(inside) && self.edges_at(p).is_none()
    }

    /// A locked region can only be moved; false when nothing starts.
    pub fn begin(&mut self, p: (i32, i32)) -> bool {
        if self.locked {
            let Some(r) = self.region.filter(|_| self.moves_at(p)) else {
                return false;
            };
            self.drag = Some(Drag::Move(r, p));
            return true;
        }
        self.drag = Some(match (self.region, self.edges_at(p)) {
            (Some(r), Some(e)) => Drag::Resize(r, e, self.clamp(p)),
            (Some(r), None) if self.moves_at(p) => Drag::Move(r, p),
            _ => {
                self.region = None;
                Drag::New(self.clamp(p))
            }
        });
        true
    }

    /// Starts moving the region from a point outside it.
    pub fn begin_move(&mut self, p: (i32, i32)) -> bool {
        let Some(r) = self.region else {
            return false;
        };
        self.drag = Some(Drag::Move(r, p));
        true
    }

    /// `p` is the current pointer position.
    pub fn update(&mut self, p: (i32, i32)) {
        let p = self.clamp(p);
        self.region = match self.drag {
            None => return,
            Some(Drag::New(start)) => Some(Rect::from_points(start, p)),
            Some(Drag::Move(r, start)) => {
                let s = self.screen;
                Some(Rect {
                    x: (r.x + p.0 - start.0).clamp(s.x, s.right() - r.w),
                    y: (r.y + p.1 - start.1).clamp(s.y, s.bottom() - r.h),
                    ..r
                })
            }
            Some(Drag::Resize(r, e, start)) => {
                let (dx, dy) = (p.0 - start.0, p.1 - start.1);
                let axis = |edge: i8, lo: i32, hi: i32, d: i32, min: i32, max: i32| match edge {
                    -1 => ((lo + d).clamp(min, max), hi),
                    1 => (lo, (hi + d).clamp(min, max)),
                    _ => (lo, hi),
                };
                let s = self.screen;
                let (x0, x1) = axis(e.x, r.x, r.right(), dx, s.x, s.right());
                let (y0, y1) = axis(e.y, r.y, r.bottom(), dy, s.y, s.bottom());
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
        drag(&mut s, (350, 120), (1500, -20));
        assert_eq!(s.region, Some(Rect::new(350, 0, 650, 120)));
        drag(&mut s, (700, 500), (701, 501));
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
    fn every_handle_and_guide_grabs_its_own_edges() {
        let s = Selection {
            region: Some(Rect::new(300, 200, 300, 200)),
            ..Selection::new(SCREEN, None)
        };
        assert_eq!(s.handles().len(), 8);
        for (p, e) in s.handles() {
            assert_eq!(s.edges_at(p), Some(e));
        }
        assert_eq!(s.guides().len(), 8);
        for ([a, b], e) in s.guides() {
            assert_eq!(s.edges_at(((a.0 + b.0) / 2, (a.1 + b.1) / 2)), Some(e));
        }
        assert_eq!(s.edges_at((700, 250)), None);
    }

    #[test]
    fn hidden_guides_cannot_be_grabbed() {
        let s = Selection {
            region: Some(Rect::new(300, 200, 300, 200)),
            ..Selection::new(SCREEN, None).with_guides(false)
        };
        assert!(s.guides().is_empty());
        assert_eq!(s.handles().len(), 8);
        // The middle of the top guide, and the handles still work.
        assert_eq!(s.edges_at((450, 100)), None);
        assert_eq!(s.edges_at((300, 200)), Some(Edges { x: -1, y: -1 }));
    }

    #[test]
    fn guides_end_at_the_matching_corner_or_edge() {
        let s = Selection {
            region: Some(Rect::new(300, 200, 300, 200)),
            ..Selection::new(SCREEN, None)
        };
        let ends: Vec<_> = s.guides().iter().map(|(l, _)| l[1]).collect();
        assert_eq!(
            ends,
            [
                (0, 0),
                (450, 0),
                (1000, 0),
                (1000, 300),
                (1000, 800),
                (450, 800),
                (0, 800),
                (0, 300)
            ]
        );
    }

    #[test]
    fn dragging_a_guide_moves_by_displacement() {
        let mut s = Selection::new(SCREEN, None);
        drag(&mut s, (300, 200), (600, 400));
        // Right side guide, grabbed far from the region.
        drag(&mut s, (900, 300), (950, 320));
        assert_eq!(s.region, Some(Rect::new(300, 200, 350, 200)));
        // Top right diagonal, grabbed halfway: both edges follow.
        drag(&mut s, (825, 100), (845, 70));
        assert_eq!(s.region, Some(Rect::new(300, 170, 370, 230)));
        s.locked = true;
        assert!(s.guides().is_empty() && s.handles().is_empty());
    }

    #[test]
    fn dragging_inside_moves_and_stays_on_screen() {
        let mut s = Selection::new(SCREEN, None);
        drag(&mut s, (100, 100), (300, 200));
        assert!(s.moves_at((150, 130)) && !s.moves_at((100, 100)));
        drag(&mut s, (150, 130), (200, 110));
        assert_eq!(s.region, Some(Rect::new(150, 80, 200, 100)));
        drag(&mut s, (200, 110), (5000, 5000));
        assert_eq!(s.region, Some(Rect::new(800, 700, 200, 100)));
        // A plain click inside keeps the region.
        drag(&mut s, (850, 730), (850, 730));
        assert_eq!(s.region, Some(Rect::new(800, 700, 200, 100)));
        s.locked = true;
        assert!(s.moves_at((850, 730)));
        s.locked = false;
        // Even a tiny region can be moved by its middle.
        s.region = Some(Rect::new(100, 100, 8, 8));
        assert!(s.moves_at((104, 104)));
    }

    #[test]
    fn the_grip_moves_without_resizing() {
        let mut s = Selection::new(SCREEN, None);
        drag(&mut s, (100, 100), (300, 200));
        assert!(s.begin_move((400, 250)));
        s.update((450, 260));
        s.end();
        assert_eq!(s.region, Some(Rect::new(150, 110, 200, 100)));
        s.locked = true;
        assert!(s.begin_move((0, 0)));
        s.region = None;
        assert!(!s.begin_move((0, 0)));
    }

    #[test]
    fn locked_region_only_moves() {
        let r = Rect::new(10, 10, 50, 50);
        let mut s = Selection::new(SCREEN, Some(r));
        drag(&mut s, (200, 200), (400, 400));
        drag(&mut s, (10, 10), (0, 0));
        assert_eq!(s.region, Some(r));
        drag(&mut s, (30, 30), (40, 50));
        assert_eq!(s.region, Some(Rect::new(20, 30, 50, 50)));
        s.region = Some(r);
        s.locked = false;
        drag(&mut s, (10, 10), (0, 0));
        assert_eq!(s.region, Some(Rect::new(0, 0, 60, 60)));
    }
}
