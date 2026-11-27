use super::{
    ask_path,
    editor::{Editor, Tool},
    preview::Preview,
    selection::Selection,
    settings, show_error,
    toolbar::Toolbar,
};
use crate::{
    capture::{CaptureRequest, Frame, OutputInfo, track, wlr::WlrBackend},
    config::{Action, Behavior, Config, Launch, Lock, Paths},
    geometry::{Rect, place_panel},
    output::{clipboard, save},
    record::{Audio, Recorder},
};
use anyhow::{Context, Result};
use gtk::{cairo, gdk, gio, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::{
    cell::{Cell, OnceCell, RefCell},
    path::PathBuf,
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
.flick-toolbar label.flick-paused { color: #9aa0a6; }
.flick-toolbar entry { background: #303136; color: #ffffff; border: none; outline: none;
  box-shadow: none; border-radius: 8px; min-height: 32px; }
.flick-toolbar entry.error { background: #7a2f2f; }
.flick-toolbar entry text { color: #ffffff; }
.flick-toolbar .flick-field { background: #303136; border-radius: 8px; min-height: 32px; }
.flick-toolbar scale.flick-field { padding: 0 10px; }
.flick-toolbar scale.flick-field trough { margin: 0; min-height: 4px; background: #55575c; }
.flick-toolbar scale.flick-field highlight { background: #1a73e8; min-height: 4px; }
.flick-toolbar scale.flick-field slider { min-width: 14px; min-height: 14px; margin: -5px;
  background: #ffffff; border: none; box-shadow: none; }
.flick-toolbar .flick-field button { min-width: 26px; min-height: 32px; border-radius: 8px; }
.flick-toolbar .flick-field entry { background: none; padding: 0; min-height: 32px; }
.flick-toolbar .flick-field entry text { min-width: 0; padding: 0; }
.flick-toolbar .flick-field button.toggle { min-width: 34px; }
.flick-toolbar button.flick-letter { font-weight: bold; font-size: 1.05em; }
.flick-toolbar menubutton.flick-color > button { min-width: 26px; min-height: 32px; }
.flick-toolbar menubutton.flick-color > button, .flick-toolbar menubutton.flick-color > button:hover,
.flick-toolbar menubutton.flick-color > button:checked { background: none; outline: none; }
.flick-toolbar dropdown.flick-font { min-width: 94px; }
.flick-toolbar dropdown.flick-font > button { background: #303136; min-height: 32px;
  min-width: 78px; padding: 0 8px; border-radius: 8px; }
.flick-toolbar dropdown.flick-font > button:hover { background: #3c4043; }
.flick-toolbar dropdown.flick-font > button:checked { background: #3c4043; }
.flick-toolbar popover arrow { background: #2b2c30; }
.flick-toolbar text selection { background: #1a73e8; color: #ffffff; }
.flick-toolbar popover contents, .flick-toolbar popover listview { background: #2b2c30;
  color: #ffffff; }
.flick-toolbar button.flick-swatch { min-width: 26px; min-height: 26px; }
.flick-toolbar button.flick-text { padding: 0 10px; }
.flick-toolbar label { color: #ffffff; }
.flick-toolbar image { color: #ffffff; }
.flick-toolbar label.flick-error { color: #ff8a80; }
";

enum Mode {
    Select,
    /// The screen is being unfrozen.
    Starting,
    Recording(Box<Recorder>, Clock),
    Stopping,
    Preview(Rc<Preview>),
}

/// Recorded time, without the pauses.
struct Clock {
    before: Duration,
    /// Running since, or paused.
    since: Option<Instant>,
}

impl Clock {
    fn elapsed(&self) -> Duration {
        self.before + self.since.map_or(Duration::ZERO, |t| t.elapsed())
    }

    fn pause(&mut self) {
        self.before = self.elapsed();
        self.since = None;
    }

    fn resume(&mut self) {
        self.since.get_or_insert_with(Instant::now);
    }
}

pub struct Overlay {
    app: gtk::Application,
    picture: gtk::Picture,
    texture: gdk::MemoryTexture,
    mode: RefCell<Mode>,
    layers: gtk::Overlay,
    editor: OnceCell<Rc<Editor>>,
    /// The editor panel replaces the toolbar and the region stays put.
    editing: Cell<bool>,
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
    config: RefCell<Config>,
    paths: Paths,
    launch: Launch,
    /// This session's screenshot file; later saves replace its content.
    saved: RefCell<Option<PathBuf>>,
    /// The region the selection behavior last ran for.
    completed: Cell<Option<Rect>>,
    /// A save dialog is open.
    asking: Cell<bool>,
    /// The canvas cursor; setting one by name makes a new cursor every time.
    cursor: Cell<Option<&'static str>>,
}

pub fn open(
    app: &gtk::Application,
    frame: Frame,
    output: OutputInfo,
    locked: Option<Rect>,
    launch: Launch,
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
        editor: OnceCell::new(),
        editing: Cell::new(false),
        moves: Cell::new(0),
        input: RefCell::new(None),
        keep: RefCell::new(None),
        selection: RefCell::new(Selection::new(screen, locked).with_guides(config.guides)),
        window,
        canvas,
        toolbar,
        frame,
        output,
        screen,
        config: RefCell::new(config),
        paths,
        launch,
        saved: RefCell::default(),
        completed: Cell::new(None),
        asking: Cell::new(false),
        cursor: Cell::new(None),
    });
    *o.keep.borrow_mut() = Some(o.clone());
    let weak = Rc::downgrade(&o);
    let scale = f64::from(o.frame.width) / f64::from(o.screen.w);
    let editor = Editor::new(&o.layers, scale, move |replace| {
        if let Some(o) = weak.upgrade() {
            if replace && o.editing.get() {
                o.refresh();
            }
            o.canvas.queue_draw();
        }
    });
    let _ = o.editor.set(editor);
    connect(&o);
    o.refresh();
    o.window.present();
    // A locked region is selected already.
    if locked.is_some() {
        let weak = Rc::downgrade(&o);
        glib::idle_add_local_once(move || {
            if let Some(o) = weak.upgrade() {
                o.selected();
            }
        });
    }
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
        let Some(o) = weak.upgrade().filter(|o| o.selecting()) else {
            return;
        };
        if o.editing.get() {
            // Takes the keyboard back from the panel's fields.
            gtk::prelude::GtkWindowExt::set_focus(&o.window, None::<&gtk::Widget>);
            if o.in_region((x, y)) {
                o.editor().press((x, y));
            }
            return;
        }
        o.selection.borrow_mut().begin(point(x, y));
        o.update_cursor((x, y));
        o.refresh();
    });
    let weak = Rc::downgrade(o);
    drag.connect_drag_update(move |g, dx, dy| {
        if let (Some(o), Some((x, y))) = (weak.upgrade(), g.start_point()) {
            if o.editing.get() {
                o.editor().drag((x + dx, y + dy));
                return;
            }
            o.drag_to(point(x + dx, y + dy));
        }
    });
    let weak = Rc::downgrade(o);
    drag.connect_drag_end(move |g, dx, dy| {
        if let Some(o) = weak.upgrade() {
            if o.editing.get() {
                o.editor().release();
                return;
            }
            o.selection.borrow_mut().end();
            if let Some((x, y)) = g.start_point() {
                o.update_cursor((x + dx, y + dy));
            }
            o.keep_lock();
            o.refresh();
            if o.selecting() && o.selection.borrow().region.is_some() {
                o.selected();
            }
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
            o.update_cursor((x, y));
        }
    });
    o.canvas.add_controller(motion);

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(o);
    keys.connect_key_pressed(move |_, key, code, state| {
        let Some(o) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        if o.editing.get() {
            return o.editor_key(key, code, state);
        }
        match (key, o.preview()) {
            (gdk::Key::Escape, None) if o.selecting() => o.quit(),
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
    o.toolbar.edit.connect_clicked(move |_| {
        if let Some(o) = weak.upgrade() {
            o.set_editing(true);
        }
    });
    let weak = Rc::downgrade(o);
    o.toolbar.settings.connect_clicked(move |_| {
        if let Some(o) = weak.upgrade() {
            o.open_settings();
        }
    });
    let weak = Rc::downgrade(o);
    o.toolbar.close.connect_clicked(move |_| {
        if let Some(o) = weak.upgrade() {
            o.quit();
        }
    });
    let weak = Rc::downgrade(o);
    o.editor().close.connect_clicked(move |_| {
        if let Some(o) = weak.upgrade() {
            o.set_editing(false);
        }
    });
    for b in [&o.toolbar.copy, &o.editor().copy] {
        let weak = Rc::downgrade(o);
        b.connect_clicked(move |_| {
            if let Some(o) = weak.upgrade() {
                o.deliver(Action::new(true, false), true, false);
            }
        });
    }
    for b in [&o.toolbar.download, &o.editor().save] {
        let weak = Rc::downgrade(o);
        b.connect_clicked(move |_| {
            if let Some(o) = weak.upgrade() {
                // Only this button asks where to save screenshots.
                let ask = o.config.borrow().screenshots.ask;
                o.deliver(Action::new(false, true), true, ask);
            }
        });
    }
    let weak = Rc::downgrade(o);
    o.toolbar.play.connect_clicked(move |_| {
        if let Some(o) = weak.upgrade()
            && !o.stop_recording()
        {
            o.start_recording();
        }
    });
    let weak = Rc::downgrade(o);
    o.toolbar.pause.connect_clicked(move |_| {
        if let Some(o) = weak.upgrade() {
            o.toggle_pause();
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
        let root = match &preview {
            Some(p) => &p.panel,
            None if self.editing.get() => &self.editor().panel,
            None => &self.toolbar.root,
        };
        self.toolbar.show_locked(s.locked);
        // While recording everything stays visible and clear of the capture.
        let recording = self.recording();
        let region = s.region.filter(|_| recording || !s.dragging());
        self.toolbar.grip.set_visible(recording);
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
        cr.save()?;
        cr.rectangle(r.x.into(), r.y.into(), r.w.into(), r.h.into());
        cr.clip();
        self.editor().render(cr)?;
        cr.restore()?;
        cr.set_source_rgb(ar, ag, ab);
        cr.set_line_width(2.0);
        cr.rectangle(
            f64::from(r.x) - 1.0,
            f64::from(r.y) - 1.0,
            f64::from(r.w) + 2.0,
            f64::from(r.h) + 2.0,
        );
        cr.stroke()?;
        if self.editing.get() {
            return Ok(());
        }
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

    fn editor(&self) -> &Rc<Editor> {
        self.editor.get().expect("set right after creation")
    }

    fn in_region(&self, (x, y): (f64, f64)) -> bool {
        let inside = |r: Rect| {
            x >= r.x.into() && y >= r.y.into() && x < r.right().into() && y < r.bottom().into()
        };
        self.selection.borrow().region.is_some_and(inside)
    }

    /// Annotations stay when the editor closes; copying and saving include them.
    fn set_editing(self: &Rc<Self>, on: bool) {
        let leaving = self.editing.get() && !on;
        self.editor().commit_text();
        self.editing.set(on);
        self.toolbar.error.set_visible(false);
        self.editor().error.set_visible(false);
        self.toolbar.root.set_visible(false);
        self.editor().panel.set_visible(false);
        gtk::prelude::GtkWindowExt::set_focus(&self.window, None::<&gtk::Widget>);
        self.refresh();
        // What the selection did then now includes the annotations.
        let b = self.behavior();
        if leaving && b.open && b.action.any() && self.completed.get().is_some() {
            self.deliver(b.action, false, false);
        }
    }

    fn editor_key(
        self: &Rc<Self>,
        key: gdk::Key,
        code: u32,
        state: gdk::ModifierType,
    ) -> glib::Propagation {
        let focus = gtk::prelude::GtkWindowExt::focus(&self.window);
        // Fields have their own undo.
        let in_field = focus.is_some_and(|w| w.is::<gtk::Text>() || w.is::<gtk::TextView>());
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        // Hardware codes, so the shortcuts work in any keyboard layout.
        const Z: u32 = 52;
        const Y: u32 = 29;
        match (key, code) {
            // Finishes the text; undo takes it back.
            (gdk::Key::Escape, _) if self.editor().commit_text() => {}
            (gdk::Key::Escape, _) => self.set_editing(false),
            (_, Z) if ctrl && shift && !in_field => self.editor().redo(),
            (_, Z) if ctrl && !in_field => self.editor().undo(),
            (_, Y) if ctrl && !in_field => self.editor().redo(),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }

    /// An open hand over a region that can be moved, a closed one while moving.
    fn update_cursor(&self, (x, y): (f64, f64)) {
        let s = self.selection.borrow();
        let name = match s.edges_at(point(x, y)).map(|e| (e.x, e.y)) {
            _ if s.moving() => "grabbing",
            _ if self.editing.get() => match self.editor().tool() {
                _ if !self.in_region((x, y)) => "default",
                Tool::Brush => "crosshair",
                Tool::Text => "text",
            },
            _ if s.locked && !s.moves_at(point(x, y)) => "default",
            Some((0, _)) => "ns-resize",
            Some((_, 0)) => "ew-resize",
            Some((x, y)) if x == y => "nwse-resize",
            Some(_) => "nesw-resize",
            None if s.moves_at(point(x, y)) => "grab",
            None => "crosshair",
        };
        self.set_cursor(Some(name));
    }

    fn set_cursor(&self, name: Option<&'static str>) {
        if self.cursor.replace(name) != name {
            self.canvas.set_cursor_from_name(name);
        }
    }

    fn recording(&self) -> bool {
        matches!(*self.mode.borrow(), Mode::Recording(..))
    }

    pub fn selecting(&self) -> bool {
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
            // The whole surface takes input meanwhile, so the canvas shows it too.
            self.toolbar.grip.set_cursor_from_name(Some("grabbing"));
            self.set_cursor(Some("grabbing"));
            // Hyprland sends no motion outside the input region, even to a held
            // button, so the whole surface takes input until the move ends.
            *self.input.borrow_mut() = None;
            if let Some(s) = self.window.surface() {
                s.set_input_region(None);
            }
        }
        self.refresh();
    }

    fn end_move(self: &Rc<Self>) {
        if !self.selection.borrow().dragging() {
            return;
        }
        self.selection.borrow_mut().end();
        self.toolbar.grip.set_cursor_from_name(Some("grab"));
        self.set_cursor(None);
        self.keep_lock();
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
            self.paths.cache.join("clipboard"),
            &self.config.borrow(),
            self.window.upcast_ref(),
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
            cursor: self.config.borrow().cursor,
            interval: Duration::ZERO,
        };
        let backend = Box::new(WlrBackend::connect()?);
        let c = self.config.borrow().clone();
        let audio = Audio {
            system: c.audio,
            microphone: c.microphone,
        };
        let recorder = Recorder::start(backend, req, c.fps, audio, file.clone())
            .inspect_err(|_| drop(std::fs::remove_file(&file)))?;
        let clock = Clock {
            before: Duration::ZERO,
            since: Some(Instant::now()),
        };
        *self.mode.borrow_mut() = Mode::Recording(Box::new(recorder), clock);
        self.refresh();
        self.apply_input();

        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(250), move || {
            let Some(o) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let failed = match &mut *o.mode.borrow_mut() {
                Mode::Recording(r, clock) => {
                    let s = clock.elapsed().as_secs();
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

    fn toggle_pause(&self) {
        let result = match &mut *self.mode.borrow_mut() {
            Mode::Recording(r, clock) if r.paused() => r.resume().map(|()| {
                clock.resume();
                false
            }),
            Mode::Recording(r, clock) => {
                r.pause();
                clock.pause();
                Ok(true)
            }
            _ => return,
        };
        match result {
            Ok(paused) => {
                self.toolbar.error.set_visible(false);
                self.toolbar.show_paused(paused);
            }
            Err(e) => self.toolbar.show_error(&format!("Cannot resume: {e:#}")),
        }
        self.refresh();
        self.apply_input();
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
        self.editor().commit_text();
        let r = self.pixels(region);
        let frame = self.editor().flatten(self.frame.crop(r)?, (r.x, r.y))?;
        save::encode_png(&frame)
    }

    fn behavior(&self) -> Behavior {
        self.config.borrow().behavior(self.launch)
    }

    /// Runs the selection behavior once a region is chosen.
    fn selected(self: &Rc<Self>) {
        let region = self.selection.borrow().region;
        let b = self.behavior();
        if b.open && self.completed.get() == region {
            return;
        }
        self.completed.set(region);
        if !b.open {
            self.deliver(b.action, true, false);
        } else if b.action.any() {
            self.deliver(b.action, false, false);
        }
    }

    /// Esc and the close button do what the settings say on the way out.
    fn quit(self: &Rc<Self>) {
        let action = self.config.borrow().quit;
        if action.any() && self.selection.borrow().region.is_some() {
            self.deliver(action, true, false);
        } else {
            self.window.close();
        }
    }

    /// Copies and saves the region, then closes if asked. `ask` asks where to
    /// save; otherwise, once saved, every later delivery refreshes the same file.
    fn deliver(self: &Rc<Self>, action: Action, close: bool, ask: bool) {
        if self.asking.get() {
            return;
        }
        let png = match self.png() {
            Ok(p) => p,
            Err(e) => return self.failed(e),
        };
        if action.copy
            && let Err(e) = clipboard::copy_png(&png)
        {
            return self.failed(e);
        }
        let existing = self.saved.borrow().clone();
        if !action.save && existing.is_none() {
            if close {
                self.window.close();
            }
            return;
        }
        let c = self.config.borrow().clone();
        let dir = match c.screenshot_dir() {
            Ok(d) => d,
            Err(e) => return self.failed(e),
        };
        if !(ask && action.save) {
            let r = match existing {
                Some(p) => save::write(&p, &png).map(|()| p),
                None => save::write_new(&dir, &c.file_name, "png", &png),
            };
            return self.saved(r, close);
        }
        // Dialogs open underneath the overlay.
        self.asking.set(true);
        self.window.set_visible(false);
        let o = self.clone();
        glib::spawn_future_local(async move {
            let name = format!(
                "{}.png",
                save::stem(&c.file_name).unwrap_or_else(|_| "Flick".into())
            );
            let r = ask_path(&dir, &name).await;
            o.asking.set(false);
            match r {
                Ok(Some(p)) => {
                    let r = save::write(&p, &png).map(|()| p);
                    if !close || r.is_err() {
                        o.window.set_visible(true);
                    }
                    o.saved(r, close);
                }
                // Cancelled: back to the selection.
                Ok(None) => o.window.set_visible(true),
                Err(e) => {
                    o.window.set_visible(true);
                    o.failed(e);
                }
            }
        });
    }

    fn saved(&self, r: Result<PathBuf>, close: bool) {
        match r {
            Ok(p) => {
                println!("{}", p.display());
                *self.saved.borrow_mut() = Some(p);
                if close {
                    self.window.close();
                }
            }
            Err(e) => self.failed(e),
        }
    }

    fn failed(&self, e: anyhow::Error) {
        self.report(Err(e));
        self.refresh();
    }

    /// Hides the overlay while the settings window is open, since windows open
    /// underneath it; the new settings apply when it comes back.
    pub fn open_settings(self: &Rc<Self>) {
        self.window.set_visible(false);
        let weak = Rc::downgrade(self);
        settings::open(&self.app, &self.paths, move || {
            let Some(o) = weak.upgrade() else { return };
            match Config::load(&o.paths.config) {
                Ok(c) => {
                    o.selection.borrow_mut().guides = c.guides;
                    *o.config.borrow_mut() = c;
                }
                Err(e) => eprintln!("flick: keeping the previous settings: {e:#}"),
            }
            o.window.set_visible(true);
            o.refresh();
        });
    }

    /// A moved locked region stays locked at its new place.
    fn keep_lock(&self) {
        let s = self.selection.borrow().clone();
        let Some(region) = s.region.filter(|_| s.locked) else {
            return;
        };
        let lock = Lock {
            output: self.output.clone(),
            region,
        };
        if Lock::load(&self.paths.lock).ok().flatten().as_ref() != Some(&lock) {
            self.report(lock.save(&self.paths.lock));
        }
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

    fn report(&self, r: Result<()>) {
        match r {
            Ok(()) => self.toolbar.error.set_visible(false),
            Err(e) => {
                eprintln!("flick: {e:#}");
                let text = format!("{e:#}");
                if self.editing.get() {
                    self.editor().error.set_text(&text);
                    self.editor().error.set_visible(true);
                } else {
                    self.toolbar.show_error(&text);
                }
            }
        }
    }
}
