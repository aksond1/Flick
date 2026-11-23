//! Color picker: a saturation/value field, a hue strip, a code and presets.
use gtk::{cairo, prelude::*};
use std::{
    cell::{Cell, RefCell},
    f64::consts::TAU,
    rc::Rc,
};

/// Red, green, blue in 0..=1.
pub type Color = (f64, f64, f64);

pub const PRESETS: [&str; 12] = [
    "#e53935", "#fb8c00", "#fdd835", "#43a047", "#00acc1", "#1a73e8", "#8e24aa", "#ec407a",
    "#795548", "#000000", "#9e9e9e", "#ffffff",
];
const WIDTH: i32 = 240;
const COLUMNS: i32 = 6;

type OnChange = Box<dyn Fn(Color)>;

pub struct Picker {
    pub root: gtk::Box,
    /// Kept as HSV, so gray and black do not forget their hue.
    hsv: Cell<(f64, f64, f64)>,
    field: gtk::DrawingArea,
    hue: gtk::DrawingArea,
    code: gtk::Entry,
    /// The code is being set by us rather than typed.
    quiet: Cell<bool>,
    on_change: RefCell<Option<OnChange>>,
    on_done: RefCell<Option<Box<dyn Fn()>>>,
}

impl Picker {
    pub fn new() -> Rc<Self> {
        let area = |height: i32| {
            let a = gtk::DrawingArea::new();
            a.set_content_width(WIDTH);
            a.set_content_height(height);
            a.set_cursor_from_name(Some("crosshair"));
            a
        };
        let (field, hue) = (area(120), area(18));
        let code = gtk::Entry::new();
        code.set_max_length(7);
        let grid = gtk::Grid::new();
        grid.set_row_spacing(4);
        grid.set_column_spacing(4);
        grid.set_column_homogeneous(true);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.append(&field);
        root.append(&hue);
        root.append(&grid);
        root.append(&code);
        let p = Rc::new(Self {
            root,
            hsv: Cell::new((0.0, 1.0, 1.0)),
            field,
            hue,
            code,
            quiet: Cell::new(false),
            on_change: RefCell::default(),
            on_done: RefCell::default(),
        });

        let weak = Rc::downgrade(&p);
        p.field.set_draw_func(move |_, cr, w, h| {
            if let Some(p) = weak.upgrade() {
                let _ = p.draw_field(cr, w.into(), h.into());
            }
        });
        let weak = Rc::downgrade(&p);
        p.hue.set_draw_func(move |_, cr, w, h| {
            if let Some(p) = weak.upgrade() {
                let _ = p.draw_hue(cr, w.into(), h.into());
            }
        });
        p.on_drag(&p.field, |x, y| (None, Some((x, 1.0 - y))));
        p.on_drag(&p.hue, |x, _| (Some(x), None));
        let weak = Rc::downgrade(&p);
        p.code.connect_changed(move |c| {
            let Some(p) = weak.upgrade().filter(|p| !p.quiet.get()) else {
                return;
            };
            c.remove_css_class("error");
            match parse(&c.text()) {
                Some(color) => {
                    p.store(color);
                    p.emit();
                }
                None => c.add_css_class("error"),
            }
        });
        let weak = Rc::downgrade(&p);
        p.code.connect_activate(move |c| {
            if let Some(p) = weak.upgrade()
                && parse(&c.text()).is_some()
            {
                p.done();
            }
        });
        for (i, name) in PRESETS.iter().enumerate() {
            let c = parse(name).unwrap();
            let area = gtk::DrawingArea::new();
            area.set_content_width(22);
            area.set_content_height(22);
            area.set_draw_func(move |_, cr, w, h| paint_swatch(cr, w, h, c));
            let b = gtk::Button::new();
            b.set_child(Some(&area));
            b.set_focusable(false);
            b.add_css_class("flick-swatch");
            let weak = Rc::downgrade(&p);
            b.connect_clicked(move |_| {
                if let Some(p) = weak.upgrade() {
                    p.set(c);
                    p.emit();
                    p.done();
                }
            });
            grid.attach(&b, i as i32 % COLUMNS, i as i32 / COLUMNS, 1, 1);
        }
        p
    }

    /// `f` runs on every change, also while dragging.
    pub fn connect_changed(&self, f: impl Fn(Color) + 'static) {
        *self.on_change.borrow_mut() = Some(Box::new(f));
    }

    /// `f` runs after a choice that needs no refinement.
    pub fn connect_done(&self, f: impl Fn() + 'static) {
        *self.on_done.borrow_mut() = Some(Box::new(f));
    }

    /// Shows a color without reporting a change.
    pub fn set(&self, c: Color) {
        self.store(c);
        self.show_code();
    }

    fn color(&self) -> Color {
        to_rgb(self.hsv.get())
    }

    fn store(&self, c: Color) {
        // The same color keeps its exact hue and saturation.
        if hex(c) != hex(self.color()) {
            self.hsv.set(to_hsv(c, self.hsv.get()));
        }
        self.field.queue_draw();
        self.hue.queue_draw();
    }

    fn show_code(&self) {
        self.quiet.set(true);
        self.code.set_text(&hex(self.color()));
        self.code.remove_css_class("error");
        self.quiet.set(false);
    }

    fn emit(&self) {
        if let Some(f) = &*self.on_change.borrow() {
            f(self.color());
        }
    }

    fn done(&self) {
        if let Some(f) = &*self.on_done.borrow() {
            f();
        }
    }

    /// `f` turns a position in 0..=1 into a new hue or saturation and value.
    fn on_drag(
        self: &Rc<Self>,
        area: &gtk::DrawingArea,
        f: impl Fn(f64, f64) -> (Option<f64>, Option<(f64, f64)>) + 'static,
    ) {
        let weak = Rc::downgrade(self);
        let apply = Rc::new(move |a: &gtk::DrawingArea, x: f64, y: f64| {
            let Some(p) = weak.upgrade() else { return };
            let x = (x / f64::from(a.width().max(1))).clamp(0.0, 1.0);
            let y = (y / f64::from(a.height().max(1))).clamp(0.0, 1.0);
            let old = p.hsv.get();
            let (h, sv) = f(x, y);
            let (s, v) = sv.unwrap_or((old.1, old.2));
            p.hsv.set((h.unwrap_or(old.0), s, v));
            p.field.queue_draw();
            p.hue.queue_draw();
            p.show_code();
            p.emit();
        });
        let drag = gtk::GestureDrag::new();
        let (a, go) = (area.clone(), apply.clone());
        drag.connect_drag_begin(move |_, x, y| go(&a, x, y));
        let a = area.clone();
        drag.connect_drag_update(move |g, dx, dy| {
            if let Some((x, y)) = g.start_point() {
                apply(&a, x + dx, y + dy);
            }
        });
        area.add_controller(drag);
    }

    fn draw_field(&self, cr: &cairo::Context, w: f64, h: f64) -> Result<(), cairo::Error> {
        let (hue, s, v) = self.hsv.get();
        let full = to_rgb((hue, 1.0, 1.0));
        let across = cairo::LinearGradient::new(0.0, 0.0, w, 0.0);
        across.add_color_stop_rgb(0.0, 1.0, 1.0, 1.0);
        across.add_color_stop_rgb(1.0, full.0, full.1, full.2);
        cr.set_source(&across)?;
        cr.paint()?;
        let down = cairo::LinearGradient::new(0.0, 0.0, 0.0, h);
        down.add_color_stop_rgba(0.0, 0.0, 0.0, 0.0, 0.0);
        down.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, 1.0);
        cr.set_source(&down)?;
        cr.paint()?;
        marker(cr, s * w, (1.0 - v) * h, self.color())
    }

    fn draw_hue(&self, cr: &cairo::Context, w: f64, h: f64) -> Result<(), cairo::Error> {
        let g = cairo::LinearGradient::new(0.0, 0.0, w, 0.0);
        for i in 0..=6 {
            let x = f64::from(i) / 6.0;
            let c = to_rgb((x, 1.0, 1.0));
            g.add_color_stop_rgb(x, c.0, c.1, c.2);
        }
        cr.set_source(&g)?;
        cr.paint()?;
        let hue = self.hsv.get().0;
        marker(cr, hue * w, h / 2.0, to_rgb((hue, 1.0, 1.0)))
    }
}

fn marker(cr: &cairo::Context, x: f64, y: f64, c: Color) -> Result<(), cairo::Error> {
    cr.arc(x, y, 6.0, 0.0, TAU);
    cr.set_source_rgb(c.0, c.1, c.2);
    cr.fill_preserve()?;
    // Two rings, to stand out on any color.
    cr.set_source_rgb(0.0, 0.0, 0.0);
    cr.set_line_width(4.0);
    cr.stroke_preserve()?;
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.set_line_width(2.0);
    cr.stroke()
}

pub fn paint_swatch(cr: &cairo::Context, w: i32, h: i32, c: Color) {
    let (w, h) = (f64::from(w), f64::from(h));
    cr.arc(w / 2.0, h / 2.0, w.min(h) / 2.0 - 1.0, 0.0, TAU);
    cr.set_source_rgb(c.0, c.1, c.2);
    let _ = cr.fill_preserve();
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.6);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
}

pub fn parse(s: &str) -> Option<Color> {
    let s = s.trim();
    let digits = s.strip_prefix('#').unwrap_or(s);
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let v = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).map(|v| f64::from(v) / 255.0);
    Some((v(0).ok()?, v(2).ok()?, v(4).ok()?))
}

pub fn hex(c: Color) -> String {
    let v = |v: f64| (v * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", v(c.0), v(c.1), v(c.2))
}

/// All of `h`, `s`, `v` in 0..=1.
fn to_rgb((h, s, v): (f64, f64, f64)) -> Color {
    let f = |n: f64| {
        let k = (n + h * 6.0) % 6.0;
        v - v * s * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    (f(5.0), f(3.0), f(1.0))
}

/// What the color does not define, the hue of gray for one, stays as in `old`.
fn to_hsv((r, g, b): Color, old: (f64, f64, f64)) -> (f64, f64, f64) {
    let v = r.max(g).max(b);
    let d = v - r.min(g).min(b);
    if v == 0.0 {
        return (old.0, old.1, 0.0);
    }
    if d == 0.0 {
        return (old.0, 0.0, v);
    }
    let h = if v == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if v == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, d / v, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_and_reject_nonsense() {
        for c in PRESETS {
            assert_eq!(hex(parse(c).unwrap()), c);
        }
        assert_eq!(parse(" 00ff00 "), Some((0.0, 1.0, 0.0)));
        for bad in ["", "#12", "#gggggg", "#її1", "red"] {
            assert_eq!(parse(bad), None);
        }
    }

    #[test]
    fn every_color_survives_hsv() {
        for r in (0..=255).step_by(15) {
            for g in (0..=255).step_by(15) {
                for b in (0..=255).step_by(15) {
                    let code = format!("#{r:02x}{g:02x}{b:02x}");
                    let c = parse(&code).unwrap();
                    assert_eq!(hex(to_rgb(to_hsv(c, (0.3, 0.5, 0.5)))), code);
                }
            }
        }
    }

    #[test]
    fn gray_and_black_keep_the_hue() {
        let old = (0.6, 0.8, 1.0);
        assert_eq!(to_hsv((0.5, 0.5, 0.5), old), (0.6, 0.0, 0.5));
        assert_eq!(to_hsv((0.0, 0.0, 0.0), old), (0.6, 0.8, 0.0));
        // The end of the hue strip is red again.
        assert_eq!(hex(to_rgb((1.0, 1.0, 1.0))), "#ff0000");
    }
}
