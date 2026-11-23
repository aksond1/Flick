//! Annotations on the frozen frame. Coordinates and sizes are frame pixels, so
//! the screen and the exported image are drawn by the same code.
use super::color::Color;
use crate::capture::Frame;
use anyhow::{Context as _, Result};
use gtk::{cairo, pango};

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Stroke {
        points: Vec<(f64, f64)>,
        width: f64,
        color: Color,
    },
    Text {
        /// Top left corner.
        at: (f64, f64),
        text: String,
        family: String,
        size: f64,
        color: Color,
    },
    /// Hides everything before it; being an operation, it can be undone.
    Clear,
}

#[derive(Debug, Default)]
pub struct Document {
    ops: Vec<Op>,
    undone: Vec<Op>,
}

impl Document {
    pub fn push(&mut self, op: Op) {
        self.ops.push(op);
        self.undone.clear();
    }

    pub fn undo(&mut self) {
        self.undone.extend(self.ops.pop());
    }

    pub fn redo(&mut self) {
        self.ops.extend(self.undone.pop());
    }

    pub fn clear(&mut self) {
        if !self.is_blank() {
            self.push(Op::Clear);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.ops.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    pub fn is_blank(&self) -> bool {
        self.visible().is_empty()
    }

    fn visible(&self) -> &[Op] {
        let start = self.ops.iter().rposition(|o| *o == Op::Clear);
        &self.ops[start.map_or(0, |i| i + 1)..]
    }

    pub fn render(&self, cr: &cairo::Context) -> Result<(), cairo::Error> {
        self.visible().iter().try_for_each(|o| draw(cr, o))
    }

    /// `frame` is a cutout whose top left corner is at `origin`.
    pub fn flatten(&self, frame: Frame, origin: (i32, i32)) -> Result<Frame> {
        if self.is_blank() {
            return Ok(frame);
        }
        let (width, height) = (frame.width, frame.height);
        // Little-endian RGB24 is exactly our BGRX.
        let mut surface = cairo::ImageSurface::create_for_data(
            frame.data,
            cairo::Format::Rgb24,
            width as i32,
            height as i32,
            width as i32 * 4,
        )?;
        {
            let cr = cairo::Context::new(&surface)?;
            cr.translate(-f64::from(origin.0), -f64::from(origin.1));
            self.render(&cr)?;
        }
        surface.flush();
        let data = surface.data().context("image is still in use")?.to_vec();
        Ok(Frame {
            width,
            height,
            data,
        })
    }
}

/// `size` is in pixels.
pub fn font(family: &str, size: f64) -> pango::FontDescription {
    let mut font = pango::FontDescription::new();
    font.set_family(family);
    font.set_absolute_size(size * f64::from(pango::SCALE));
    font
}

pub fn draw(cr: &cairo::Context, op: &Op) -> Result<(), cairo::Error> {
    match op {
        Op::Clear => Ok(()),
        Op::Stroke {
            points,
            width,
            color,
        } => {
            let Some(&(x, y)) = points.first() else {
                return Ok(());
            };
            cr.set_source_rgb(color.0, color.1, color.2);
            cr.set_line_width(*width);
            cr.set_line_cap(cairo::LineCap::Round);
            cr.set_line_join(cairo::LineJoin::Round);
            cr.new_path();
            cr.move_to(x, y);
            // Curves through the midpoints hide the corners between samples.
            let mut from = (x, y);
            for w in points.windows(2) {
                let (a, b) = (w[0], w[1]);
                let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
                let t = |p: f64, q: f64| p + (q - p) * 2.0 / 3.0;
                cr.curve_to(
                    t(from.0, a.0),
                    t(from.1, a.1),
                    t(mid.0, a.0),
                    t(mid.1, a.1),
                    mid.0,
                    mid.1,
                );
                from = mid;
            }
            let end = points[points.len() - 1];
            cr.line_to(end.0, end.1);
            cr.stroke()
        }
        Op::Text {
            at,
            text,
            family,
            size,
            color,
        } => {
            let layout = pangocairo::functions::create_layout(cr);
            layout.set_font_description(Some(&font(family, *size)));
            layout.set_text(text);
            cr.set_source_rgb(color.0, color.1, color.2);
            cr.move_to(at.0, at.1);
            pangocairo::functions::show_layout(cr, &layout);
            cr.status()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dot(x: f64, y: f64) -> Op {
        Op::Stroke {
            points: vec![(x, y)],
            width: 4.0,
            color: (1.0, 0.0, 0.0),
        }
    }

    fn white() -> Frame {
        Frame {
            width: 20,
            height: 10,
            data: vec![255; 800],
        }
    }

    #[test]
    fn undo_redo_and_clear() {
        let mut d = Document::default();
        d.clear();
        assert!(d.is_blank() && !d.can_undo());
        d.push(dot(1.0, 1.0));
        d.push(dot(2.0, 2.0));
        d.clear();
        assert!(d.is_blank() && d.can_undo());
        d.undo();
        assert_eq!(d.visible().len(), 2);
        d.undo();
        assert_eq!(d.visible(), [dot(1.0, 1.0)]);
        d.redo();
        d.redo();
        assert!(d.is_blank() && !d.can_redo());
        d.push(dot(3.0, 3.0));
        assert_eq!(d.visible(), [dot(3.0, 3.0)]);
        d.undo();
        d.undo();
        d.push(dot(4.0, 4.0));
        assert!(!d.can_redo());
    }

    #[test]
    fn flatten_draws_relative_to_the_cutout() {
        let mut d = Document::default();
        assert_eq!(d.flatten(white(), (0, 0)).unwrap(), white());
        d.push(dot(105.0, 55.0));
        let f = d.flatten(white(), (100, 50)).unwrap();
        let at = |x: usize, y: usize| &f.data[(y * 20 + x) * 4..][..3];
        assert_eq!(at(5, 5), [0, 0, 255]);
        assert_eq!(at(15, 5), [255, 255, 255]);
    }

    #[test]
    fn flatten_draws_text() {
        let mut d = Document::default();
        d.push(Op::Text {
            at: (0.0, 0.0),
            text: "Їж".into(),
            family: "Sans".into(),
            size: 10.0,
            color: (0.0, 0.0, 0.0),
        });
        assert_ne!(d.flatten(white(), (0, 0)).unwrap(), white());
    }
}
