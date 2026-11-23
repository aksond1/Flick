//! Recording a region while the user drags it.
//!
//! The screen shows the border a few frames late, so the capture takes a wider
//! window, finds the border in the picture itself and cuts out its inside.
use super::Frame;
use crate::geometry::Rect;

/// Border color as RGB. The overlay draws a solid line of `WIDTH` logical pixels
/// that leaves `SPACE` pixels between itself and the region.
pub const COLOR: (u8, u8, u8) = (230, 51, 51);
pub const WIDTH: i32 = 2;
pub const SPACE: i32 = 1;
/// How far the region may have moved since the window was chosen.
const MARGIN: i32 = 200;
/// Points checked along each side.
const SAMPLES: i32 = 16;

/// Part of the screen to capture around `r`; its size depends only on `r`'s size,
/// so buffers can be reused.
pub fn window(r: Rect, screen: Rect) -> Rect {
    let (w, h) = (
        (r.w + 2 * MARGIN).min(screen.w),
        (r.h + 2 * MARGIN).min(screen.h),
    );
    Rect::new(
        (r.x - MARGIN).clamp(screen.x, screen.right() - w),
        (r.y - MARGIN).clamp(screen.y, screen.bottom() - h),
        w,
        h,
    )
}

/// Cuts out the first of `candidates` whose border is visible in `frame`, which
/// shows `win`.
pub fn cut(frame: &Frame, win: Rect, candidates: &[Rect]) -> Option<Frame> {
    let r = candidates
        .iter()
        .find(|r| win.contains(r) && has_border(frame, win, **r))?;
    let local = Rect::new(r.x - win.x, r.y - win.y, r.w, r.h);
    let size = (frame.width as i32, frame.height as i32);
    frame.crop(local.scaled((win.w, win.h), size)).ok()
}

fn has_border(f: &Frame, win: Rect, r: Rect) -> bool {
    let s = f64::from(f.width) / f64::from(win.w);
    let (x, y) = (f64::from(r.x - win.x), f64::from(r.y - win.y));
    let (w, h) = (f64::from(r.w), f64::from(r.h));
    let (near, far) = (f64::from(SPACE), f64::from(SPACE + WIDTH));
    // Fully covered pixels of a line that spans `a..b`: the first and the last.
    let line = |a: f64, b: f64| [(a * s).ceil() as i32, (b * s).floor() as i32 - 1];
    let red = |x: i32, y: i32| {
        let inside = x >= 0 && y >= 0 && x < f.width as i32 && y < f.height as i32;
        inside.then(|| {
            let p = &f.data[(y as usize * f.width as usize + x as usize) * 4..][..3];
            let close = |v: u8, c: u8| v.abs_diff(c) <= 24;
            close(p[2], COLOR.0) && close(p[1], COLOR.1) && close(p[0], COLOR.2)
        })
    };
    let along = |from: f64, len: f64, i: i32| {
        ((from + len * (f64::from(i) + 0.5) / f64::from(SAMPLES)) * s) as i32
    };
    // Sides outside the picture do not count, but some side must decide.
    let side = |points: &dyn Fn(i32, usize) -> Option<bool>| -> Option<bool> {
        let hits: Vec<bool> = (0..SAMPLES)
            .flat_map(|i| [points(i, 0), points(i, 1)])
            .flatten()
            .collect();
        // The pointer may hide a part of the line.
        (!hits.is_empty()).then(|| hits.iter().filter(|h| **h).count() * 4 >= hits.len() * 3)
    };
    let left = line(x - far, x - near);
    let right = line(x + w + near, x + w + far);
    let top = line(y - far, y - near);
    let bottom = line(y + h + near, y + h + far);
    let sides = [
        side(&|i, k| red(left[k], along(y, h, i))),
        side(&|i, k| red(right[k], along(y, h, i))),
        side(&|i, k| red(along(x, w, i), top[k])),
        side(&|i, k| red(along(x, w, i), bottom[k])),
    ];
    sides.iter().flatten().count() > 0 && sides.iter().flatten().all(|s| *s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect::new(0, 0, 900, 700);

    /// A picture of `win` at `scale`, with a border around `r` and marked insides.
    fn picture(win: Rect, r: Rect, scale: i32) -> Frame {
        let (w, h) = (win.w * scale, win.h * scale);
        let far = SPACE + WIDTH;
        let grow = |by: i32| Rect::new(r.x - by, r.y - by, r.w + 2 * by, r.h + 2 * by);
        let (outer, inner) = (grow(far), grow(SPACE));
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let p = Rect::new(win.x + x / scale, win.y + y / scale, 1, 1);
                let border = outer.contains(&p) && !inner.contains(&p);
                let px = match (border, r.contains(&p)) {
                    (true, _) => [COLOR.2, COLOR.1, COLOR.0, 0],
                    (_, true) => [9, 9, 9, 0],
                    _ => [200, 200, 200, 0],
                };
                data.extend(px);
            }
        }
        Frame {
            width: w as u32,
            height: h as u32,
            data,
        }
    }

    #[test]
    fn finds_the_shown_position_among_close_candidates() {
        for scale in [1, 2] {
            let shown = Rect::new(300, 250, 120, 80);
            let win = window(Rect::new(310, 260, 120, 80), SCREEN);
            let f = picture(win, shown, scale);
            let near = [(303, 250), (301, 251), (302, 250), (300, 250), (299, 250)];
            let near = near.map(|(x, y)| Rect::new(x, y, 120, 80));
            let out = cut(&f, win, &near).unwrap();
            assert_eq!(
                (out.width, out.height),
                (120 * scale as u32, 80 * scale as u32)
            );
            assert!(out.data.iter().all(|v| *v == 9 || *v == 0));
            assert!(cut(&f, win, &near[..2]).is_none());
        }
    }

    #[test]
    fn works_at_screen_edges() {
        for (x, y) in [(0, 0), (780, 0), (0, 620), (780, 620)] {
            let r = Rect::new(x, y, 120, 80);
            let win = window(r, SCREEN);
            assert!(SCREEN.contains(&win));
            let f = picture(win, r, 1);
            assert!(cut(&f, win, &[r]).is_some(), "{r:?}");
        }
        let full = window(SCREEN, SCREEN);
        assert_eq!(full, SCREEN);
    }

    #[test]
    fn window_size_does_not_change_while_moving() {
        let size = |x| {
            let w = window(Rect::new(x, 300, 120, 80), SCREEN);
            (w.w, w.h)
        };
        assert!((0..780).all(|x| size(x) == size(0)));
    }
}
