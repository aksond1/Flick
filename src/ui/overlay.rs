use super::{preview::Preview, selection::Selection, show_error, toolbar::Toolbar};
use crate::{
    capture::{CaptureRequest, Frame, OutputInfo, track, wlr::WlrBackend},
    config::{Config, Lock, Paths},
    geometry::{Rect, place_panel},
    output::{clipboard, save},
    record::Recorder,
};
use anyhow::{Context, Result};
use gtk::{cairo, gdk, gio, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

const GAP: i32 = 8;
/// Time for the compositor to show the unfrozen screen before recording starts.
const UNFREEZE: Duration = Duration::from_millis(200);
/// Time for the compositor to show the moved border before video resumes.
const SETTLE: Duration = Duration::from_millis(100);
/// Accent color (blue, #1a73e8), shared with the CSS below.
const ACCENT: (f64, f64, f64) = (0.1, 0.45, 0.91);
const CSS: &str = "
window.flick-overlay { background: transparent; }
.flick-toolbar { background: #202124; border-radius: 10px; padding: 6px; }
.flick-toolbar button { min-width: 36px; min-height: 36px; padding: 0; color: #ffffff;
  background: transparent; border: none; box-shadow: none; border-radius: 7px; }
.flick-toolbar button:hover { background: #3c4043; }
.flick-toolbar button:checked { background: #1a73e8; }
.flick-toolbar button:disabled { opacity: 0.35; }
.flick-time { color: #ffffff; padding: 0 8px; font-feature-settings: 'tnum'; }
.flick-toolbar button.flick-text { padding: 0 10px; }
.flick-toolbar label { color: #ffffff; }
.flick-toolbar image { color: #ffffff; }
.flick-toolbar label.flick-error { color: #ff8a80; }
";

enum Mode {
    Select,
    /// The screen is being unfrozen.
    Starting,
    Recording(Recorder, Instant),
    Stopping,
    Preview(Rc<Preview>),
}

pub struct Overlay {
    app: gtk::Application,
    picture: gtk::Picture,
    texture: gdk::MemoryTexture,
    mode: RefCell<Mode>,
    layers: gtk::Overlay,
    /// Counts moves during recording, so a new one outdates a pending resume.
    moves: Cell<u32>,
    /// Clickable areas outside selection mode; `None` is the whole surface.
    input: RefCell<Option<Vec<Rect>>>,
    /// Keeps itself alive until the window closes.
    keep: RefCell<Option<Rc<Overlay>>>,
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
) -> Result<Rc<Overlay>> {
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
        app: app.clone(),
        picture,
        texture,
        mode: RefCell::new(Mode::Select),
        layers,
        moves: Cell::new(0),
        input: RefCell::new(None),
        keep: RefCell::new(None),
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
    *o.keep.borrow_mut() = Some(o.clone());
    connect(&o);
    o.refresh();
    o.window.present();
    Ok(o)
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
        if let Some(o) = weak.upgrade().filter(|o| o.selecting()) {
            o.selection.borrow_mut().begin(point(x, y));
            o.refresh();
        }
    });
    let weak = Rc::downgrade(o);
    drag.connect_drag_update(move |g, dx, dy| {
        if let (Some(o), Some((x, y))) = (weak.upgrade(), g.start_point()) {
            o.drag_to(point(x + dx, y + dy));
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

    // The grip travels with the panel, so positions are taken on the overlay.
    let drag = gtk::GestureDrag::new();
    let at = |o: &Overlay, x: f64, y: f64| {
        let p = gtk::graphene::Point::new(x as f32, y as f32);
        let p = o.toolbar.grip.compute_point(&o.layers, &p)?;
        Some(point(p.x().into(), p.y().into()))
    };
    let weak = Rc::downgrade(o);
    drag.connect_drag_begin(move |_, x, y| {
        if let Some(o) = weak.upgrade()
            && let Some(p) = at(&o, x, y)
        {
            o.begin_move(p);
        }
    });
    let weak = Rc::downgrade(o);
    drag.connect_drag_update(move |g, dx, dy| {
        if let Some(o) = weak.upgrade()
            && let Some((x, y)) = g.start_point()
            && let Some(p) = at(&o, x + dx, y + dy)
        {
            o.drag_to(p);
        }
    });
    let weak = Rc::downgrade(o);
    drag.connect_drag_end(move |_, _, _| {
        if let Some(o) = weak.upgrade() {
            o.end_move();
        }
    });
    o.toolbar.grip.add_controller(drag);

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
                None if s.moves_at(point(x, y)) => "move",
                None => "crosshair",
            };
            o.canvas.set_cursor_from_name(Some(name));
        }
    });
    o.canvas.add_controller(motion);

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(o);
    keys.connect_key_pressed(move |_, key, _, _| {
        let Some(o) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        match (key, o.preview()) {
            (gdk::Key::Escape, _) => o.window.close(),
            (gdk::Key::space, Some(p)) => p.toggle(),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
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
    let weak = Rc::downgrade(o);
    o.toolbar.download.connect_clicked(move |_| {
        if let Some(o) = weak.upgrade() {
            o.close_after(o.download());
        }
    });
    let weak = Rc::downgrade(o);
    o.toolbar.play.connect_clicked(move |_| {
        if let Some(o) = weak.upgrade()
            && !o.stop_recording()
        {
            o.start_recording();
        }
    });
    let weak = Rc::downgrade(o);
    o.window.connect_close_request(move |_| {
        let Some(o) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        if let Some(p) = o.preview() {
            p.request_close();
            return glib::Propagation::Stop;
        }
        if o.stop_recording() {
            return glib::Propagation::Stop;
        }
        // Drop after this emission so callbacks still on the stack stay valid.
        let o = o.keep.borrow_mut().take();
        glib::idle_add_local_once(move || drop(o));
        glib::Propagation::Proceed
    });
    let weak = Rc::downgrade(o);
    o.window.connect_realize(move |w| {
        let Some(surface) = w.surface() else { return };
        // GTK may set its own input region on layout, so ours is reapplied.
        let weak = weak.clone();
        surface.connect_layout(move |_, _, _| {
            if let Some(o) = weak.upgrade() {
                o.apply_input();
            }
        });
    });
}

fn point(x: f64, y: f64) -> (i32, i32) {
    (x.round() as i32, y.round() as i32)
}

impl Overlay {
    /// Moves and shows the toolbar to match the selection.
    fn refresh(&self) {
        // A copy: hiding a widget can end a drag, whose handler needs the selection.
        let s = self.selection.borrow().clone();
        let preview = self.preview();
        let root = preview.as_ref().map_or(&self.toolbar.root, |p| &p.panel);
        self.toolbar.show_locked(s.locked);
        // While recording everything stays visible and clear of the capture.
        let recording = self.recording();
        let region = s.region.filter(|_| recording || !s.dragging());
        self.toolbar.grip.set_visible(recording && !s.locked);
        // Moving the panel along would relayout every frame, and it could get
        // recorded; it disappears and comes back at the new place.
        let moving = recording && s.dragging();
        root.set_opacity(if moving { 0.0 } else { 1.0 });
        if moving {
            self.canvas.queue_draw();
            return;
        }
        match region {
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
                let outside = place_panel(region, self.screen, size, GAP);
                if !self.selecting() && preview.is_none() {
                    root.set_visible(outside.is_some());
                }
                let p = outside.unwrap_or_else(|| {
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
                *self.input.borrow_mut() = if self.selecting() {
                    None
                } else if preview.is_some() {
                    Some(vec![region, p])
                } else {
                    Some(outside.into_iter().collect())
                };
            }
        }
        self.canvas.queue_draw();
    }

    fn draw(&self, cr: &cairo::Context, w: i32, h: i32) -> Result<(), cairo::Error> {
        let s = self.selection.borrow();
        if !self.selecting() {
            // Only a border, kept clear of the recorded pixels.
            if let Some(r) = s.region {
                let (red, green, blue) = if self.previewing() {
                    ACCENT
                } else {
                    let (r, g, b) = track::COLOR;
                    let c = |v: u8| f64::from(v) / 255.0;
                    (c(r), c(g), c(b))
                };
                cr.set_source_rgb(red, green, blue);
                let width = f64::from(track::WIDTH);
                let off = f64::from(track::SPACE) + width / 2.0;
                cr.set_line_width(width);
                cr.rectangle(
                    f64::from(r.x) - off,
                    f64::from(r.y) - off,
                    f64::from(r.w) + 2.0 * off,
                    f64::from(r.h) + 2.0 * off,
                );
                cr.stroke()?;
                if let Mode::Recording(rec, _) = &*self.mode.borrow() {
                    rec.border_drawn(r);
                }
            }
            return Ok(());
        }
        let (ar, ag, ab) = ACCENT;
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.45);
        cr.rectangle(0.0, 0.0, w.into(), h.into());
        if let Some(r) = s.region {
            cr.rectangle(r.x.into(), r.y.into(), r.w.into(), r.h.into());
            cr.set_fill_rule(cairo::FillRule::EvenOdd);
        }
        cr.fill()?;
        let Some(r) = s.region else { return Ok(()) };
        cr.set_source_rgb(ar, ag, ab);
        cr.set_line_width(2.0);
        cr.rectangle(
            f64::from(r.x) - 1.0,
            f64::from(r.y) - 1.0,
            f64::from(r.w) + 2.0,
            f64::from(r.h) + 2.0,
        );
        cr.stroke()?;
        cr.set_line_width(1.0);
        cr.set_dash(&[6.0, 5.0], 0.0);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.7);
        for ([a, b], _) in s.guides() {
            cr.move_to(f64::from(a.0) + 0.5, f64::from(a.1) + 0.5);
            cr.line_to(f64::from(b.0) + 0.5, f64::from(b.1) + 0.5);
        }
        cr.stroke()?;
        cr.set_dash(&[], 0.0);
        for ((x, y), _) in s.handles() {
            cr.arc(x.into(), y.into(), 5.0, 0.0, std::f64::consts::TAU);
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.fill_preserve()?;
            cr.set_source_rgb(ar, ag, ab);
            cr.set_line_width(2.0);
            cr.stroke()?;
        }
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

    fn recording(&self) -> bool {
        matches!(*self.mode.borrow(), Mode::Recording(..))
    }

    fn selecting(&self) -> bool {
        matches!(*self.mode.borrow(), Mode::Select)
    }

    fn preview(&self) -> Option<Rc<Preview>> {
        match &*self.mode.borrow() {
            Mode::Preview(p) => Some(p.clone()),
            _ => None,
        }
    }

    pub fn previewing(&self) -> bool {
        self.preview().is_some()
    }

    fn drag_to(&self, p: (i32, i32)) {
        self.selection.borrow_mut().update(p);
        self.canvas.queue_draw();
    }

    /// The border follows the pointer freely; the capture finds it on screen.
    fn begin_move(&self, p: (i32, i32)) {
        if let Mode::Recording(r, _) = &*self.mode.borrow()
            && self.selection.borrow_mut().begin_move(p)
        {
            r.track_video(self.screen);
            self.moves.set(self.moves.get() + 1);
        }
        self.refresh();
    }

    fn end_move(self: &Rc<Self>) {
        if !self.selection.borrow().dragging() {
            return;
        }
        self.selection.borrow_mut().end();
        self.refresh();
        self.apply_input();
        let (weak, this) = (Rc::downgrade(self), self.moves.get());
        glib::timeout_add_local_once(SETTLE, move || {
            let Some(o) = weak.upgrade().filter(|o| o.moves.get() == this) else {
                return;
            };
            if let Mode::Recording(r, _) = &*o.mode.borrow()
                && let Some(region) = o.selection.borrow().region
            {
                r.resume_video(region);
            }
        });
    }

    /// Plays the recording inside the region.
    fn show_preview(self: &Rc<Self>, file: std::path::PathBuf) {
        let Some(r) = self.selection.borrow().region else {
            return;
        };
        let resize = Rc::downgrade(self);
        let close = Rc::downgrade(self);
        let p = Preview::new(
            file,
            &self.config,
            move || {
                if let Some(o) = resize.upgrade() {
                    o.refresh();
                    o.apply_input();
                }
            },
            move || {
                if let Some(o) = close.upgrade() {
                    o.shut();
                }
            },
        );
        // Margins rather than a size, so the video cannot outgrow the region.
        p.picture.set_margin_start(r.x);
        p.picture.set_margin_top(r.y);
        p.picture.set_margin_end(self.screen.w - r.right());
        p.picture.set_margin_bottom(self.screen.h - r.bottom());
        self.layers.add_overlay(&p.picture);
        self.layers.add_overlay(&p.panel);
        self.toolbar.root.set_visible(false);
        *self.mode.borrow_mut() = Mode::Preview(p);
        // Take the keyboard once for Space and Esc, then let other windows have it.
        self.window.set_keyboard_mode(KeyboardMode::Exclusive);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(300), move || {
            if let Some(o) = weak.upgrade() {
                o.window.set_keyboard_mode(KeyboardMode::OnDemand);
            }
        });
        self.refresh();
        self.apply_input();
    }

    /// Destroys the window without asking.
    fn shut(&self) {
        // Drop later, so callbacks still on the stack stay valid.
        let keep = self.keep.borrow_mut().take();
        glib::idle_add_local_once(move || drop(keep));
        self.window.destroy();
    }

    fn apply_input(&self) {
        if let (Some(s), Some(rects)) = (self.window.surface(), &*self.input.borrow()) {
            // An empty region lets every click through.
            let rects: Vec<_> = rects
                .iter()
                .map(|r| cairo::RectangleInt::new(r.x, r.y, r.w, r.h))
                .collect();
            s.set_input_region(Some(&cairo::Region::create_rectangles(&rects)));
        }
    }

    /// Switches the overlay between the frozen and the see-through look.
    fn set_live(&self, live: bool) {
        self.toolbar.show_recording(live);
        if live {
            self.picture.set_paintable(None::<&gdk::Paintable>);
            self.window.set_keyboard_mode(KeyboardMode::None);
        } else {
            self.picture.set_paintable(Some(&self.texture));
            self.window.set_keyboard_mode(KeyboardMode::Exclusive);
            if let Some(s) = self.window.surface() {
                s.set_input_region(None);
            }
        }
        self.refresh();
        self.apply_input();
    }

    fn start_recording(self: &Rc<Self>) {
        if !self.selecting() || self.selection.borrow().region.is_none() {
            return;
        }
        *self.mode.borrow_mut() = Mode::Starting;
        self.toolbar.error.set_visible(false);
        self.toolbar.play.set_sensitive(false);
        self.set_live(true);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(UNFREEZE, move || {
            let Some(o) = weak.upgrade() else { return };
            let result = o.launch_recorder();
            o.toolbar.play.set_sensitive(true);
            if let Err(e) = result {
                *o.mode.borrow_mut() = Mode::Select;
                o.report(Err(e));
                o.set_live(false);
            }
        });
    }

    fn launch_recorder(self: &Rc<Self>) -> Result<()> {
        let region = self.selection.borrow().region.context("nothing selected")?;
        let stem = glib::DateTime::now_local()?.format("rec_%Y-%m-%d_%H-%M-%S")?;
        let file = save::unique_path(&self.paths.cache, &stem, "mp4")?;
        let req = CaptureRequest {
            output: self.output.name.clone(),
            region: Some(region),
            cursor: self.config.cursor,
            interval: Duration::ZERO,
        };
        let backend = Box::new(WlrBackend::connect()?);
        let c = &self.config;
        let recorder = Recorder::start(backend, req, c.fps, c.audio, file.clone())
            .inspect_err(|_| drop(std::fs::remove_file(&file)))?;
        *self.mode.borrow_mut() = Mode::Recording(recorder, Instant::now());
        self.refresh();
        self.apply_input();

        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(250), move || {
            let Some(o) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let failed = match &mut *o.mode.borrow_mut() {
                Mode::Recording(r, start) => {
                    let s = start.elapsed().as_secs();
                    let text = format!("{:02}:{:02}", s / 60, s % 60);
                    o.toolbar.time.set_text(&text);
                    r.has_failed()
                }
                _ => return glib::ControlFlow::Break,
            };
            if failed {
                o.stop_recording();
            }
            glib::ControlFlow::Continue
        });
        Ok(())
    }

    /// Finishes a recording and shows the preview. False if none was running.
    pub fn stop_recording(self: &Rc<Self>) -> bool {
        let recorder = {
            let mut mode = self.mode.borrow_mut();
            match std::mem::replace(&mut *mode, Mode::Stopping) {
                Mode::Recording(r, _) => r,
                other => {
                    let busy = !matches!(other, Mode::Select | Mode::Preview(_));
                    *mode = other;
                    // Starting and stopping are too short to act on.
                    return busy;
                }
            }
        };
        self.toolbar.play.set_sensitive(false);
        self.selection.borrow_mut().end();
        self.refresh();
        self.apply_input();
        let hold = self.app.hold();
        let o = self.clone();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || recorder.stop()).await;
            let result = result.unwrap_or_else(|_| Err(anyhow::anyhow!("recorder panicked")));
            match result {
                Ok(file) => o.show_preview(file),
                Err(e) => {
                    show_error(&o.app, &format!("{e:#}"));
                    o.shut();
                }
            }
            drop(hold);
        });
        true
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
