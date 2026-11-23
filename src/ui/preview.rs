//! Plays a finished recording inside the region and lets the user keep it.
use crate::{
    config::Config,
    output::{clipboard, save},
    record::export::export,
};
use anyhow::{Context, Result};
use gst::prelude::*;
use gtk::{gdk, gio, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

struct Player {
    pipeline: gst::Element,
    paintable: gdk::Paintable,
    ended: Cell<bool>,
    _watch: RefCell<Option<gst::bus::BusWatchGuard>>,
}

impl Player {
    fn new(file: &Path) -> Result<Self> {
        gst::init()?;
        let sink = gst::ElementFactory::make("gtk4paintablesink")
            .build()
            .context("video output is missing, install gst-plugin-gtk4")?;
        let paintable = sink.property::<gdk::Paintable>("paintable");
        let gl = paintable.property::<Option<gdk::GLContext>>("gl-context");
        let video = match gl {
            Some(_) => gst::ElementFactory::make("glsinkbin")
                .property("sink", &sink)
                .build()?,
            None => sink,
        };
        let pipeline = gst::ElementFactory::make("playbin")
            .property("uri", glib::filename_to_uri(file, None)?.as_str())
            .property("video-sink", &video)
            .build()?;
        pipeline.set_state(gst::State::Paused)?;
        Ok(Self {
            pipeline,
            paintable,
            ended: Cell::new(false),
            _watch: RefCell::new(None),
        })
    }

    fn playing(&self) -> bool {
        self.pipeline.current_state() == gst::State::Playing && !self.ended.get()
    }

    fn toggle(&self) {
        if self.playing() {
            let _ = self.pipeline.set_state(gst::State::Paused);
            return;
        }
        if self.ended.replace(false) {
            self.seek(0.0);
        }
        let _ = self.pipeline.set_state(gst::State::Playing);
    }

    fn seek(&self, seconds: f64) {
        let t = gst::ClockTime::from_mseconds((seconds.max(0.0) * 1000.0) as u64);
        let flags = gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE;
        let _ = self.pipeline.seek_simple(flags, t);
        self.ended.set(false);
    }

    /// Position and length in seconds.
    fn times(&self) -> Option<(f64, f64)> {
        let s = |t: gst::ClockTime| t.mseconds() as f64 / 1000.0;
        let pos = self.pipeline.query_position::<gst::ClockTime>()?;
        let len = self.pipeline.query_duration::<gst::ClockTime>()?;
        Some((s(pos), s(len)))
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

/// Playback picture and controls; the overlay places both on screen.
pub struct Preview {
    pub picture: gtk::Picture,
    pub panel: gtk::Box,
    player: Option<Player>,
    /// Temporary recording, deleted on close.
    temp: PathBuf,
    config: Config,
    saved: RefCell<Option<PathBuf>>,
    busy: Cell<bool>,
    play: gtk::Button,
    seek: gtk::Scale,
    time: gtk::Label,
    status: gtk::Label,
    actions: gtk::Box,
    confirm: gtk::Box,
    on_resize: Box<dyn Fn()>,
    on_close: Box<dyn Fn()>,
}

#[derive(Clone, Copy)]
enum After {
    Stay,
    Copy,
    Close,
}

impl Preview {
    /// `on_resize` runs when the panel changes size, `on_close` when it is done.
    pub fn new(
        temp: PathBuf,
        config: &Config,
        on_resize: impl Fn() + 'static,
        on_close: impl Fn() + 'static,
    ) -> Rc<Self> {
        let (player, problem) = match Player::new(&temp) {
            Ok(p) => (Some(p), None),
            Err(e) => (None, Some(format!("Cannot play the recording: {e:#}"))),
        };
        let picture = gtk::Picture::new();
        picture.set_content_fit(gtk::ContentFit::Fill);
        picture.set_paintable(player.as_ref().map(|p| &p.paintable));

        let button = |icon: &str, tip: &str| {
            let b = gtk::Button::from_icon_name(icon);
            b.set_tooltip_text(Some(tip));
            b.set_focusable(false);
            b
        };
        let play = button("media-playback-start-symbolic", "Play");
        let seek = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.1);
        seek.set_hexpand(true);
        let time = gtk::Label::new(Some("00:00 / 00:00"));
        time.add_css_class("flick-time");
        let speaker = gtk::Image::from_icon_name("audio-volume-high-symbolic");
        let volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.05);
        volume.set_value(1.0);
        volume.set_width_request(80);
        volume.set_tooltip_text(Some("Volume"));
        let save = button("folder-download-symbolic", "Save");
        let copy = button("edit-copy-symbolic", "Save and copy file");
        let discard = button("user-trash-symbolic", "Discard");
        let close = button("window-close-symbolic", "Close");
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        for b in [&save, &copy, &discard, &close] {
            actions.append(b);
        }
        for w in [
            play.upcast_ref::<gtk::Widget>(),
            seek.upcast_ref(),
            volume.upcast_ref(),
        ] {
            w.set_focusable(false);
            w.set_sensitive(player.is_some());
        }
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        bar.append(&play);
        bar.append(&time);
        bar.append(&speaker);
        bar.append(&volume);
        bar.append(&actions);

        // Inline, because a dialog window would open underneath the overlay.
        let confirm = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let question = gtk::Label::new(Some("Save the recording?"));
        question.set_hexpand(true);
        confirm.append(&question);
        let answer = |text: &str| {
            let b = gtk::Button::with_label(text);
            b.add_css_class("flick-text");
            b.set_focusable(false);
            confirm.append(&b);
            b
        };
        let yes = answer("Save");
        let no = answer("Discard");
        let cancel = answer("Cancel");
        confirm.set_visible(false);

        let status = gtk::Label::new(problem.as_deref());
        status.set_wrap(true);
        status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        status.set_max_width_chars(40);
        status.set_visible(problem.is_some());

        let panel = gtk::Box::new(gtk::Orientation::Vertical, 4);
        panel.add_css_class("flick-toolbar");
        panel.set_halign(gtk::Align::Start);
        panel.set_valign(gtk::Align::Start);
        panel.set_cursor_from_name(Some("default"));
        panel.append(&seek);
        panel.append(&bar);
        panel.append(&status);
        panel.append(&confirm);

        let p = Rc::new(Self {
            picture,
            panel,
            player,
            temp,
            config: config.clone(),
            saved: RefCell::new(None),
            busy: Cell::new(false),
            play,
            seek,
            time,
            status,
            actions,
            confirm,
            on_resize: Box::new(on_resize),
            on_close: Box::new(on_close),
        });

        p.on_click(&p.play, |p| p.toggle());
        p.on_click(&save, |p| p.save(After::Stay));
        p.on_click(&copy, |p| p.save(After::Copy));
        p.on_click(&discard, |p| p.finish());
        p.on_click(&close, |p| p.request_close());
        p.on_click(&yes, |p| p.save(After::Close));
        p.on_click(&no, |p| p.finish());
        p.on_click(&cancel, |p| p.ask(false));
        let click = gtk::GestureClick::new();
        let weak = Rc::downgrade(&p);
        click.connect_released(move |_, _, _, _| {
            if let Some(p) = weak.upgrade() {
                p.toggle();
            }
        });
        p.picture.add_controller(click);
        let weak = Rc::downgrade(&p);
        p.seek.connect_change_value(move |_, _, value| {
            if let Some(p) = weak.upgrade().as_ref().and_then(|p| p.player.as_ref()) {
                p.seek(value);
            }
            glib::Propagation::Proceed
        });
        let weak = Rc::downgrade(&p);
        volume.connect_value_changed(move |s| {
            if let Some(p) = weak.upgrade().as_ref().and_then(|p| p.player.as_ref()) {
                // Cubic, so the slider feels even to the ear.
                p.pipeline.set_property("volume", s.value().powi(3));
            }
        });

        p.watch();
        let weak = Rc::downgrade(&p);
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let Some(p) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            p.tick();
            glib::ControlFlow::Continue
        });
        p.toggle();
        p
    }

    fn on_click(self: &Rc<Self>, b: &gtk::Button, f: impl Fn(&Rc<Self>) + 'static) {
        let weak = Rc::downgrade(self);
        b.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                f(&p);
            }
        });
    }

    fn watch(self: &Rc<Self>) {
        let Some(p) = &self.player else { return };
        let Some(bus) = p.pipeline.bus() else { return };
        let weak = Rc::downgrade(self);
        let guard = bus.add_watch_local(move |_, m| {
            let Some(v) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            match m.view() {
                gst::MessageView::Eos(_) => {
                    if let Some(p) = &v.player {
                        p.ended.set(true);
                        let _ = p.pipeline.set_state(gst::State::Paused);
                    }
                }
                gst::MessageView::Error(e) => {
                    v.say(&format!("Cannot play the recording: {}", e.error()));
                }
                _ => {}
            }
            glib::ControlFlow::Continue
        });
        *p._watch.borrow_mut() = guard.ok();
    }

    pub fn toggle(&self) {
        if let Some(p) = &self.player {
            p.toggle();
            self.tick();
        }
    }

    fn tick(&self) {
        let Some(p) = &self.player else { return };
        let (icon, tip) = if p.playing() {
            ("media-playback-pause-symbolic", "Pause")
        } else {
            ("media-playback-start-symbolic", "Play")
        };
        if self.play.icon_name().as_deref() != Some(icon) {
            self.play.set_icon_name(icon);
            self.play.set_tooltip_text(Some(tip));
        }
        if let Some((pos, len)) = p.times() {
            self.seek.set_range(0.0, len.max(0.1));
            self.seek.set_value(pos);
            self.time
                .set_text(&format!("{} / {}", clock(pos), clock(len)));
        }
    }

    fn say(&self, text: &str) {
        self.status.set_text(text);
        self.status.set_visible(true);
        (self.on_resize)();
    }

    fn ask(&self, on: bool) {
        self.confirm.set_visible(on);
        (self.on_resize)();
    }

    /// Exports off the UI thread; an existing copy is reused.
    fn save(self: &Rc<Self>, after: After) {
        if self.busy.replace(true) {
            return;
        }
        self.actions.set_sensitive(false);
        self.confirm.set_visible(false);
        self.say("Saving…");
        let v = self.clone();
        glib::spawn_future_local(async move {
            let existing = v.saved.borrow().clone().filter(|p| p.exists());
            let result = match existing {
                Some(p) => Ok(p),
                None => {
                    let (temp, config) = (v.temp.clone(), v.config.clone());
                    gio::spawn_blocking(move || save_to_library(&temp, &config))
                        .await
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("export panicked")))
                }
            };
            v.busy.set(false);
            v.actions.set_sensitive(true);
            let result = result.and_then(|path| {
                *v.saved.borrow_mut() = Some(path.clone());
                match after {
                    After::Copy => clipboard::copy_file(&path)?,
                    After::Close => v.finish(),
                    After::Stay => {}
                }
                Ok(path)
            });
            match (result, after) {
                (Err(e), _) => v.say(&format!("{e:#}")),
                (Ok(_), After::Close) => {}
                (Ok(p), After::Copy) => v.say(&format!("Copied {}", p.display())),
                (Ok(p), After::Stay) => v.say(&format!("Saved {}", p.display())),
            }
        });
    }

    /// Closes, or asks first when the recording was not saved.
    pub fn request_close(&self) {
        if self.busy.get() {
            return;
        }
        if self.saved.borrow().is_some() {
            self.finish();
        } else {
            // A second request while asking takes the question back.
            self.ask(!self.confirm.is_visible());
        }
    }

    /// Closes for good and removes the temporary file.
    fn finish(&self) {
        if let Some(p) = &self.player {
            let _ = p.pipeline.set_state(gst::State::Null);
        }
        let _ = std::fs::remove_file(&self.temp);
        (self.on_close)();
    }
}

fn save_to_library(temp: &Path, config: &Config) -> Result<PathBuf> {
    let stem = glib::DateTime::now_local()?.format("Flick_%Y-%m-%d_%H-%M-%S")?;
    let path = save::unique_path(&config.save_dir()?, &stem, config.video_format.extension())?;
    if let Err(e) = export(temp, &path, config.video_format) {
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    Ok(path)
}

fn clock(seconds: f64) -> String {
    let s = seconds as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}
