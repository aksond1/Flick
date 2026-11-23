use super::{selection::Selection, toolbar::Toolbar};
use crate::{
    capture::{Frame, OutputInfo},
    config::{Config, Lock, Paths},
    geometry::{Rect, place_panel},
    output::{clipboard, save},
};
use anyhow::{Context, Result};
use gtk::{cairo, gdk, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::{cell::RefCell, rc::Rc};

const GAP: i32 = 8;
const CSS: &str = "
window.flick-overlay { background: transparent; }
.flick-toolbar { background: #202124; border-radius: 10px; padding: 6px; }
.flick-toolbar button { min-width: 36px; min-height: 36px; padding: 0; color: #ffffff;
  background: transparent; border: none; box-shadow: none; border-radius: 7px; }
.flick-toolbar button:hover { background: #3c4043; }
.flick-toolbar button:checked { background: #1a73e8; }
.flick-toolbar button:disabled { opacity: 0.35; }
.flick-error { color: #ff8a80; }
";

struct Overlay {
    window: gtk::ApplicationWindow,
    canvas: gtk::DrawingArea,
    toolbar: Toolbar,
    selection: RefCell<Selection>,
    frame: Frame,
    output: OutputInfo,
    /// Logical size of the output.
    screen: Rect,
    config: Config,
    paths: Paths,
}

pub fn open(
    app: &gtk::Application,
    frame: Frame,
    output: OutputInfo,
    locked: Option<Rect>,
    config: Config,
    paths: Paths,
) -> Result<()> {
    let display = gdk::Display::default().context("no display")?;
    let monitor = display
        .monitors()
        .iter::<gdk::Monitor>()
        .flatten()
        .find(|m| m.connector().as_deref() == Some(output.name.as_str()))
        .with_context(|| format!("GTK does not know output {}", output.name))?;
    let g = monitor.geometry();
    let screen = Rect::new(0, 0, g.width(), g.height());

    let css = gtk::CssProvider::new();
    css.load_from_string(CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_USER + 1,
    );

    let window = gtk::ApplicationWindow::new(app);
    window.add_css_class("flick-overlay");
    window.init_layer_shell();
    window.set_namespace(Some("flick"));
    window.set_monitor(Some(&monitor));
    window.set_layer(Layer::Overlay);
    window.set_exclusive_zone(-1);
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    for e in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
        window.set_anchor(e, true);
    }

    let texture = gdk::MemoryTexture::new(
        frame.width as i32,
        frame.height as i32,
        gdk::MemoryFormat::B8g8r8x8,
        &glib::Bytes::from(&frame.data),
        frame.width as usize * 4,
    );
    let picture = gtk::Picture::for_paintable(&texture);
    picture.set_content_fit(gtk::ContentFit::Fill);
    let canvas = gtk::DrawingArea::new();
    let toolbar = Toolbar::new();
    let layers = gtk::Overlay::new();
    layers.set_child(Some(&picture));
    layers.add_overlay(&canvas);
    layers.add_overlay(&toolbar.root);
    window.set_child(Some(&layers));

    let o = Rc::new(Overlay {
        selection: RefCell::new(Selection::new(screen, locked)),
        window,
        canvas,
        toolbar,
        frame,
        output,
        screen,
        config,
        paths,
    });
    connect(&o);
    o.refresh();
    o.window.present();
    Ok(())
}

fn connect(o: &Rc<Overlay>) {
    // Widgets outlive closures only through `o`; weak references avoid a cycle.
    let weak = Rc::downgrade(o);
    o.canvas.set_draw_func(move |_, cr, w, h| {
        if let Some(o) = weak.upgrade() {
            let _ = o.draw(cr, w, h);
        }
    });

    let drag = gtk::GestureDrag::new();
    drag.set_button(gdk::BUTTON_PRIMARY);
    let weak = Rc::downgrade(o);
    drag.connect_drag_begin(move |_, x, y| {
        if let Some(o) = weak.upgrade() {
            o.selection.borrow_mut().begin(point(x, y));
            o.refresh();
        }
    });
    let weak = Rc::downgrade(o);
    drag.connect_drag_update(move |g, dx, dy| {
        if let (Some(o), Some((x, y))) = (weak.upgrade(), g.start_point()) {
            o.selection.borrow_mut().update(point(x + dx, y + dy));
            o.canvas.queue_draw();
        }
    });
    let weak = Rc::downgrade(o);
    drag.connect_drag_end(move |_, _, _| {
        if let Some(o) = weak.upgrade() {
            o.selection.borrow_mut().end();
            o.refresh();
        }
    });
    o.canvas.add_controller(drag);

    let motion = gtk::EventControllerMotion::new();
    let weak = Rc::downgrade(o);
    motion.connect_motion(move |_, x, y| {
        if let Some(o) = weak.upgrade() {
            let s = o.selection.borrow();
            let name = match s.edges_at(point(x, y)).map(|e| (e.x, e.y)) {
                _ if s.locked => "default",
                Some((0, _)) => "ns-resize",
                Some((_, 0)) => "ew-resize",
                Some((x, y)) if x == y => "nwse-resize",
                Some(_) => "nesw-resize",
                None => "crosshair",
            };
            o.canvas.set_cursor_from_name(Some(name));
        }
    });
    o.canvas.add_controller(motion);

    let keys = gtk::EventControllerKey::new();
    let weak = Rc::downgrade(o);
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape
            && let Some(o) = weak.upgrade()
        {
            o.window.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    o.window.add_controller(keys);

    let weak = Rc::downgrade(o);
    o.toolbar.lock.connect_clicked(move |_| {
        if let Some(o) = weak.upgrade() {
            o.report(o.toggle_lock());
            o.refresh();
        }
    });
    let weak = Rc::downgrade(o);
    o.toolbar.copy.connect_clicked(move |_| {
        if let Some(o) = weak.upgrade() {
            o.close_after(o.copy());
        }
    });
    // The close handler holds the only lasting strong reference.
    let weak = Rc::downgrade(o);
    o.toolbar.download.connect_clicked(move |_| {
        if let Some(o) = weak.upgrade() {
            o.close_after(o.download());
        }
    });
    let keep = RefCell::new(Some(o.clone()));
    o.window.connect_close_request(move |_| {
        // Drop after this emission so callbacks still on the stack stay valid.
        let o = keep.borrow_mut().take();
        glib::idle_add_local_once(move || drop(o));
        glib::Propagation::Proceed
    });
}

fn point(x: f64, y: f64) -> (i32, i32) {
    (x.round() as i32, y.round() as i32)
}

impl Overlay {
    /// Moves and shows the toolbar to match the selection.
    fn refresh(&self) {
        let s = self.selection.borrow();
        let root = &self.toolbar.root;
        self.toolbar.show_locked(s.locked);
        match s.region.filter(|_| !s.dragging()) {
            None => {
                self.toolbar.error.set_visible(false);
                root.set_visible(false);
            }
            Some(region) => {
                root.set_visible(true);
                // Measurement includes margins, so clear the previous position.
                root.set_margin_start(0);
                root.set_margin_top(0);
                let (_, size) = root.preferred_size();
                let size = (size.width(), size.height());
                let p = place_panel(region, self.screen, size, GAP).unwrap_or_else(|| {
                    // No room outside: sit inside, at the bottom of the region.
                    Rect::new(
                        (region.x + (region.w - size.0) / 2)
                            .clamp(0, (self.screen.w - size.0).max(0)),
                        (region.bottom() - size.1 - GAP).max(0),
                        size.0,
                        size.1,
                    )
                });
                root.set_margin_start(p.x);
                root.set_margin_top(p.y);
            }
        }
        self.canvas.queue_draw();
    }

    fn draw(&self, cr: &cairo::Context, w: i32, h: i32) -> Result<(), cairo::Error> {
        let s = self.selection.borrow();
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.45);
        cr.rectangle(0.0, 0.0, w.into(), h.into());
        if let Some(r) = s.region {
            cr.rectangle(r.x.into(), r.y.into(), r.w.into(), r.h.into());
            cr.set_fill_rule(cairo::FillRule::EvenOdd);
        }
        cr.fill()?;
        let Some(r) = s.region else { return Ok(()) };
        cr.set_source_rgb(0.1, 0.45, 0.91);
        cr.set_line_width(2.0);
        cr.rectangle(
            f64::from(r.x) - 1.0,
            f64::from(r.y) - 1.0,
            f64::from(r.w) + 2.0,
            f64::from(r.h) + 2.0,
        );
        cr.stroke()?;
        if s.dragging() {
            let p = self.pixels(r);
            let text = format!("{} × {}", p.w, p.h);
            cr.set_font_size(14.0);
            let y = if r.y > 24 { r.y - 8 } else { r.y + 20 };
            cr.move_to(f64::from(r.x) + 4.0, y.into());
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.show_text(&text)?;
        }
        Ok(())
    }

    fn pixels(&self, r: Rect) -> Rect {
        r.scaled(
            (self.screen.w, self.screen.h),
            (self.frame.width as i32, self.frame.height as i32),
        )
    }

    fn png(&self) -> Result<Vec<u8>> {
        let region = self.selection.borrow().region.context("nothing selected")?;
        save::encode_png(&self.frame.crop(self.pixels(region))?)
    }

    fn copy(&self) -> Result<()> {
        clipboard::copy_png(&self.png()?)
    }

    fn download(&self) -> Result<()> {
        let png = self.png()?;
        let stem = glib::DateTime::now_local()?.format("Flick_%Y-%m-%d_%H-%M-%S")?;
        let path = save::unique_path(&self.config.save_dir()?, &stem, "png")?;
        if let Err(e) = std::fs::write(&path, png) {
            let _ = std::fs::remove_file(&path);
            return Err(e).with_context(|| format!("cannot write {}", path.display()));
        }
        println!("{}", path.display());
        Ok(())
    }

    fn toggle_lock(&self) -> Result<()> {
        let mut s = self.selection.borrow_mut();
        let region = s.region.context("nothing selected")?;
        if s.locked {
            Lock::clear(&self.paths.lock)?;
        } else {
            Lock {
                output: self.output.clone(),
                region,
            }
            .save(&self.paths.lock)?;
        }
        s.locked = !s.locked;
        Ok(())
    }

    fn close_after(&self, r: Result<()>) {
        if r.is_ok() {
            self.window.close();
        } else {
            self.report(r);
            self.refresh();
        }
    }

    fn report(&self, r: Result<()>) {
        match r {
            Ok(()) => self.toolbar.error.set_visible(false),
            Err(e) => {
                eprintln!("flick: {e:#}");
                self.toolbar.show_error(&format!("{e:#}"));
            }
        }
    }
}
