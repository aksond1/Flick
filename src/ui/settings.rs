//! The settings window. Every valid change is saved at once; Hyprland gets the
//! keybindings through a file of its own that its config loads.
use crate::{
    config::{Action, Behavior, Config, Keybind, Launch, Paths, Saving},
    hypr::{self, HyprConfig},
    output::save,
};
use gtk::{gdk, gio, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    time::Duration,
};

const CSS: &str = "
.flick-settings .flick-heading { font-weight: bold; margin-top: 6px; }
.flick-settings .flick-dim { opacity: 0.65; }
.flick-settings .flick-small { font-size: 0.9em; }
.flick-settings list row { padding: 10px 14px; }
.flick-settings .flick-problem { background: #c01c28; color: #ffffff; padding: 8px 14px; }
.flick-settings .flick-warning { color: #c64600; }
.flick-settings button.flick-key { min-width: 120px; }
.flick-settings button.flick-capturing { background: #1a73e8; color: #ffffff; }
.flick-settings button.suggested-action { background: #1a73e8; color: #ffffff; }
.flick-settings switch:checked { background: #1a73e8; }
";
const FPS: [u32; 4] = [15, 24, 30, 60];

thread_local! {
    static CURRENT: RefCell<Weak<Settings>> = const { RefCell::new(Weak::new()) };
}

struct KeyButton {
    button: gtk::Button,
    clear: gtk::Button,
}

pub struct Settings {
    window: gtk::ApplicationWindow,
    config: RefCell<Config>,
    path: PathBuf,
    problem: gtk::Label,
    keys: Vec<(Launch, KeyButton)>,
    /// The keybinding being typed.
    capturing: Cell<Option<Launch>>,
    hypr_status: gtk::Label,
    hypr_warning: gtk::Label,
    hypr_button: gtk::Button,
    example: gtk::Label,
    on_close: RefCell<Vec<Box<dyn FnOnce()>>>,
    /// Keeps itself alive until the window closes.
    keep: RefCell<Option<Rc<Settings>>>,
}

/// Opens the window, or brings back the open one; `on_close` runs when it closes.
pub fn open(app: &gtk::Application, paths: &Paths, on_close: impl FnOnce() + 'static) {
    if let Some(s) = CURRENT.with(|c| c.borrow().upgrade()) {
        s.on_close.borrow_mut().push(Box::new(on_close));
        s.window.present();
        return;
    }
    let config = Config::load(&paths.config).unwrap_or_else(|e| {
        eprintln!("flick: starting from the default settings: {e:#}");
        Config::default()
    });
    let s = Settings::new(app, config, paths.config.clone());
    s.on_close.borrow_mut().push(Box::new(on_close));
    CURRENT.with(|c| *c.borrow_mut() = Rc::downgrade(&s));
    s.window.present();
}

/// While a keybinding is typed, its keys must not launch Flick.
pub fn is_capturing() -> bool {
    CURRENT.with(|c| {
        c.borrow()
            .upgrade()
            .is_some_and(|s| s.capturing.get().is_some())
    })
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

/// A titled list of rows.
fn group(page: &gtk::Box, title: &str, note: Option<&str>) -> gtk::ListBox {
    page.append(&label(title, &["flick-heading"]));
    if let Some(n) = note {
        page.append(&label(n, &["flick-dim", "flick-small"]));
    }
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.set_show_separators(true);
    let frame = gtk::Frame::new(None);
    frame.set_child(Some(&list));
    page.append(&frame);
    list
}

/// A row with a title, an optional explanation and controls on the right.
fn row(list: &gtk::ListBox, title: &str, subtitle: Option<&str>, controls: &[&gtk::Widget]) {
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    text.append(&label(title, &[]));
    if let Some(s) = subtitle {
        text.append(&label(s, &["flick-dim", "flick-small"]));
    }
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    line.append(&text);
    for c in controls {
        c.set_valign(gtk::Align::Center);
        line.append(*c);
    }
    let r = gtk::ListBoxRow::new();
    r.set_activatable(false);
    r.set_child(Some(&line));
    list.append(&r);
}

fn saving(c: &mut Config, video: bool) -> &mut Saving {
    match video {
        true => &mut c.recordings,
        false => &mut c.screenshots,
    }
}

fn dropdown(choices: &[&str], selected: Option<usize>) -> gtk::DropDown {
    let d = gtk::DropDown::from_strings(choices);
    d.set_selected(selected.map_or(gtk::INVALID_LIST_POSITION, |i| i as u32));
    d
}

fn name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into(),
    )
}

/// `~` for the home directory, to keep paths short.
fn short(path: &Path) -> String {
    let home = directories::BaseDirs::new().map(|d| d.home_dir().to_owned());
    match home.as_deref().and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

impl Settings {
    fn new(app: &gtk::Application, config: Config, path: PathBuf) -> Rc<Self> {
        let window = gtk::ApplicationWindow::new(app);
        window.set_title(Some("Flick Settings"));
        window.set_default_size(620, 780);
        window.add_css_class("flick-settings");
        if let Some(display) = gdk::Display::default() {
            let css = gtk::CssProvider::new();
            css.load_from_string(CSS);
            gtk::style_context_add_provider_for_display(
                &display,
                &css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        let page = gtk::Box::new(gtk::Orientation::Vertical, 8);
        for m in [
            gtk::Widget::set_margin_start,
            gtk::Widget::set_margin_end,
            gtk::Widget::set_margin_top,
            gtk::Widget::set_margin_bottom,
        ] {
            m(page.upcast_ref(), 20);
        }
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroll.set_vexpand(true);
        scroll.set_child(Some(&page));
        let problem = label("", &["flick-problem"]);
        problem.set_visible(false);
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        outer.append(&problem);
        outer.append(&scroll);
        window.set_child(Some(&outer));

        let key_button = || KeyButton {
            button: {
                let b = gtk::Button::new();
                b.add_css_class("flick-key");
                b.set_tooltip_text(Some("Click, then press the new keys"));
                b
            },
            clear: {
                let b = gtk::Button::from_icon_name("edit-clear-symbolic");
                b.set_tooltip_text(Some("Remove the keybinding"));
                b.add_css_class("flat");
                b
            },
        };
        let keys: Vec<_> = [Launch::Main, Launch::Alternative, Launch::Fastshot]
            .into_iter()
            .map(|l| (l, key_button()))
            .collect();
        let hypr_status = label("", &["flick-small"]);
        let hypr_warning = label("", &["flick-small", "flick-warning"]);
        hypr_warning.set_visible(false);
        let hypr_button = gtk::Button::with_label("Load in Hyprland");
        hypr_button.add_css_class("suggested-action");
        let example = label("", &["flick-dim", "flick-small"]);

        let s = Rc::new(Self {
            window,
            config: RefCell::new(config),
            path,
            problem,
            keys,
            capturing: Cell::new(None),
            hypr_status,
            hypr_warning,
            hypr_button,
            example,
            on_close: RefCell::default(),
            keep: RefCell::default(),
        });
        *s.keep.borrow_mut() = Some(s.clone());
        s.build(&page);
        s.connect();
        s.show_keys();
        s.show_hypr(None);
        s.check_conflicts();
        s.show_problem();
        s
    }

    fn key(&self, launch: Launch) -> &KeyButton {
        &self
            .keys
            .iter()
            .find(|(l, _)| *l == launch)
            .expect("all launches")
            .1
    }

    /// Changes the settings and saves them if they are valid.
    fn change(&self, f: impl FnOnce(&mut Config)) {
        f(&mut self.config.borrow_mut());
        self.show_problem();
        let c = self.config.borrow();
        if c.problem().is_none()
            && let Err(e) = c.save(&self.path)
        {
            self.problem
                .set_text(&format!("Cannot save the settings: {e:#}"));
            self.problem.set_visible(true);
        }
    }

    fn show_problem(&self) {
        let c = self.config.borrow();
        let p = c.problem();
        if let Some(p) = &p {
            self.problem
                .set_text(&format!("{p} Settings are saved once this is fixed."));
        }
        self.problem.set_visible(p.is_some());
        match save::stem(&c.file_name) {
            Ok(stem) => self.example.set_text(&format!("For example {stem}.png")),
            Err(_) => self.example.set_text("Not a valid pattern"),
        }
    }

    fn build(self: &Rc<Self>, page: &gtk::Box) {
        let c = self.config.borrow().clone();
        let behaviors: Vec<&str> = Behavior::CHOICES.iter().map(|(_, n)| *n).collect();
        let pick = |b: Behavior| Behavior::CHOICES.iter().position(|(c, _)| *c == b);
        let keys = |l: Launch| {
            let k = self.key(l);
            [k.button.upcast_ref::<gtk::Widget>(), k.clear.upcast_ref()]
        };

        let triggers = [
            (
                Launch::Main,
                "Main",
                "The usual way to start Flick.",
                c.main.behavior(),
            ),
            (
                Launch::Alternative,
                "Alternative",
                "For the times the main behavior is not what you need. One of the two must \
                 show the toolbar, and they must differ.",
                c.alternative.behavior(),
            ),
        ];
        for (launch, title, note, current) in triggers {
            let list = group(page, title, Some(note));
            row(&list, "Keybinding", None, &keys(launch));
            let d = dropdown(&behaviors, pick(current));
            row(&list, "After selecting a region", None, &[d.upcast_ref()]);
            self.on_selected(&d, move |c, i| {
                let t = match launch {
                    Launch::Alternative => &mut c.alternative,
                    _ => &mut c.main,
                };
                t.set_behavior(Behavior::CHOICES[i].0);
            });
        }

        let list = group(
            page,
            "Fastshot",
            Some(
                "Takes the locked region at once, without freezing the screen or showing the \
                 toolbar. With no locked region you select one first.",
            ),
        );
        row(&list, "Keybinding", None, &keys(Launch::Fastshot));
        let names: Vec<&str> = Action::FASTSHOT.iter().map(|(_, n)| *n).collect();
        let at = Action::FASTSHOT
            .iter()
            .position(|(a, _)| *a == c.fastshot.action());
        let d = dropdown(&names, at);
        row(&list, "Action", None, &[d.upcast_ref()]);
        self.on_selected(&d, |c, i| {
            let a = Action::FASTSHOT[i].0;
            (c.fastshot.copy, c.fastshot.save) = (a.copy, a.save);
        });

        let list = group(page, "Hyprland", None);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
        text.set_hexpand(true);
        text.append(&self.hypr_status);
        text.append(&self.hypr_warning);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        line.append(&text);
        self.hypr_button.set_valign(gtk::Align::Center);
        line.append(&self.hypr_button);
        let r = gtk::ListBoxRow::new();
        r.set_activatable(false);
        r.set_child(Some(&line));
        list.append(&r);

        let list = group(page, "Selection", None);
        let sw = gtk::Switch::new();
        sw.set_active(c.guides);
        row(
            &list,
            "Show guide lines",
            Some(
                "Dashed lines from the handles to the screen edges; dragging one resizes the region.",
            ),
            &[sw.upcast_ref()],
        );
        self.on_switch(&sw, |c, on| c.guides = on);

        let list = group(page, "Closing", None);
        let names: Vec<&str> = Action::QUIT.iter().map(|(_, n)| *n).collect();
        let d = dropdown(&names, Action::QUIT.iter().position(|(a, _)| *a == c.quit));
        row(
            &list,
            "Esc and the close button",
            Some(
                "Acts on the screenshot or the recording on screen. Copying a recording saves it too.",
            ),
            &[d.upcast_ref()],
        );
        self.on_selected(&d, |c, i| c.quit = Action::QUIT[i].0);
        let sw = gtk::Switch::new();
        sw.set_active(c.confirm_discard);
        row(
            &list,
            "Ask before discarding a recording",
            Some("When closing a recording that was not saved."),
            &[sw.upcast_ref()],
        );
        self.on_switch(&sw, |c, on| c.confirm_discard = on);

        for (title, video) in [("Screenshots", false), ("Recordings", true)] {
            let list = group(page, title, None);
            let dir = match video {
                true => c.recording_dir(),
                false => c.screenshot_dir(),
            };
            let folder = gtk::Button::new();
            let name = label(&dir.as_deref().map(short).unwrap_or_default(), &[]);
            name.set_wrap(false);
            name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            name.set_max_width_chars(28);
            let inner = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            inner.append(&gtk::Image::from_icon_name("folder-symbolic"));
            inner.append(&name);
            folder.set_child(Some(&inner));
            row(&list, "Folder", None, &[folder.upcast_ref()]);
            let weak = Rc::downgrade(self);
            folder.connect_clicked(move |_| {
                if let Some(s) = weak.upgrade() {
                    s.choose_folder(video, name.clone());
                }
            });
            let sw = gtk::Switch::new();
            sw.set_active(match video {
                true => c.recordings.ask,
                false => c.screenshots.ask,
            });
            let subtitle = match video {
                true => "Instead of saving to the folder.",
                false => "Instead of saving to the folder. Fastshots always use the folder.",
            };
            row(
                &list,
                "Always ask where to save",
                Some(subtitle),
                &[sw.upcast_ref()],
            );
            self.on_switch(&sw, move |c, on| saving(c, video).ask = on);
            if !video {
                continue;
            }
            let mut rates: Vec<u32> = FPS.to_vec();
            if !rates.contains(&c.fps) {
                rates.push(c.fps);
                rates.sort_unstable();
            }
            let names: Vec<String> = rates.iter().map(|r| format!("{r} FPS")).collect();
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            let d = dropdown(&names, rates.iter().position(|r| *r == c.fps));
            row(&list, "Frame rate", None, &[d.upcast_ref()]);
            self.on_selected(&d, move |c, i| c.fps = rates[i]);
            let sw = gtk::Switch::new();
            sw.set_active(c.audio);
            row(&list, "Record system audio", None, &[sw.upcast_ref()]);
            self.on_switch(&sw, |c, on| c.audio = on);
            let sw = gtk::Switch::new();
            sw.set_active(c.microphone);
            row(
                &list,
                "Record the microphone",
                Some("The default input device, mixed with the system audio."),
                &[sw.upcast_ref()],
            );
            self.on_switch(&sw, |c, on| c.microphone = on);
            let sw = gtk::Switch::new();
            sw.set_active(c.cursor);
            row(&list, "Show the pointer", None, &[sw.upcast_ref()]);
            self.on_switch(&sw, |c, on| c.cursor = on);
        }

        let list = group(page, "File names", None);
        let entry = gtk::Entry::new();
        entry.set_text(&c.file_name);
        entry.set_width_chars(26);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.append(&label("Name pattern", &[]));
        text.append(&label(
            "%Y year, %m month, %d day, %H hour, %M minute, %S second.",
            &["flick-dim", "flick-small"],
        ));
        text.append(&self.example);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        line.append(&text);
        entry.set_valign(gtk::Align::Center);
        line.append(&entry);
        let r = gtk::ListBoxRow::new();
        r.set_activatable(false);
        r.set_child(Some(&line));
        list.append(&r);
        let weak = Rc::downgrade(self);
        entry.connect_changed(move |e| {
            if let Some(s) = weak.upgrade() {
                let text = e.text().to_string();
                s.change(|c| c.file_name = text);
            }
        });
    }

    fn on_selected(self: &Rc<Self>, d: &gtk::DropDown, f: impl Fn(&mut Config, usize) + 'static) {
        let weak = Rc::downgrade(self);
        d.connect_selected_notify(move |d| {
            if let Some(s) = weak.upgrade()
                && d.selected() != gtk::INVALID_LIST_POSITION
            {
                s.change(|c| f(c, d.selected() as usize));
            }
        });
    }

    fn on_switch(self: &Rc<Self>, sw: &gtk::Switch, f: impl Fn(&mut Config, bool) + 'static) {
        let weak = Rc::downgrade(self);
        sw.connect_active_notify(move |sw| {
            if let Some(s) = weak.upgrade() {
                s.change(|c| f(c, sw.is_active()));
            }
        });
    }

    fn choose_folder(self: &Rc<Self>, video: bool, name: gtk::Label) {
        let s = self.clone();
        glib::spawn_future_local(async move {
            let dialog = gtk::FileDialog::builder()
                .title("Choose a folder")
                .modal(true)
                .build();
            let current = {
                let c = s.config.borrow();
                match video {
                    true => c.recording_dir(),
                    false => c.screenshot_dir(),
                }
            };
            if let Ok(dir) = current
                && std::fs::create_dir_all(&dir).is_ok()
            {
                dialog.set_initial_folder(Some(&gio::File::for_path(&dir)));
            }
            let Ok(Some(dir)) = dialog
                .select_folder_future(Some(&s.window))
                .await
                .map(|f| f.path())
            else {
                return;
            };
            name.set_text(&short(&dir));
            s.change(|c| match video {
                true => c.recordings.dir = Some(dir),
                false => c.screenshots.dir = Some(dir),
            });
        });
    }

    fn connect(self: &Rc<Self>) {
        for (launch, k) in &self.keys {
            let (weak, launch) = (Rc::downgrade(self), *launch);
            k.button.connect_clicked(move |_| {
                if let Some(s) = weak.upgrade() {
                    s.capture(Some(launch));
                }
            });
            let weak = Rc::downgrade(self);
            k.clear.connect_clicked(move |_| {
                if let Some(s) = weak.upgrade() {
                    s.capture(None);
                    s.set_key(launch, Keybind::default());
                }
            });
        }
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, code, state| match weak.upgrade() {
            Some(s) if s.capturing.get().is_some() => {
                s.captured(key, code, state);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        self.window.add_controller(keys);
        let weak = Rc::downgrade(self);
        self.window.connect_is_active_notify(move |w| {
            if let Some(s) = weak.upgrade()
                && !w.is_active()
            {
                s.capture(None);
            }
        });
        let weak = Rc::downgrade(self);
        self.hypr_button.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                let result = HyprConfig::locate()
                    .ok_or_else(|| anyhow::anyhow!("no Hyprland config found"))
                    .and_then(|h| h.include());
                s.apply_keys(result.err().map(|e| format!("{e:#}")));
            }
        });
        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            if let Some(s) = weak.upgrade() {
                s.capture(None);
                for f in s.on_close.take() {
                    f();
                }
                // Dropped later, so this handler's own references stay valid.
                let keep = s.keep.borrow_mut().take();
                glib::idle_add_local_once(move || drop(keep));
            }
            glib::Propagation::Proceed
        });
    }

    /// Starts typing a keybinding, or stops with `None`. Meanwhile Hyprland
    /// passes its own shortcuts to this window.
    fn capture(&self, launch: Option<Launch>) {
        if self.capturing.replace(launch) == launch {
            return;
        }
        if let Some(t) = self.window.surface().and_downcast::<gdk::Toplevel>() {
            match launch {
                Some(_) => t.inhibit_system_shortcuts(None::<&gdk::Event>),
                None => t.restore_system_shortcuts(),
            }
        }
        self.show_keys();
    }

    fn captured(self: &Rc<Self>, key: gdk::Key, code: u32, state: gdk::ModifierType) {
        let Some(launch) = self.capturing.get() else {
            return;
        };
        let plain = state
            & (gdk::ModifierType::SHIFT_MASK
                | gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::ALT_MASK
                | gdk::ModifierType::SUPER_MASK);
        use gdk::Key as K;
        match key {
            K::Shift_L
            | K::Shift_R
            | K::Control_L
            | K::Control_R
            | K::Alt_L
            | K::Alt_R
            | K::Super_L
            | K::Super_R
            | K::Meta_L
            | K::Meta_R
            | K::ISO_Level3_Shift
            | K::Hyper_L
            | K::Hyper_R
            | K::Caps_Lock
            | K::Num_Lock => return,
            K::Escape if plain.is_empty() => return self.capture(None),
            K::BackSpace | K::Delete if plain.is_empty() => {
                self.capture(None);
                return self.set_key(launch, Keybind::default());
            }
            _ => {}
        }
        // The key without modifiers: Alt+Print would otherwise be Sys_Req.
        let base = self
            .window
            .upcast_ref::<gtk::Widget>()
            .display()
            .translate_key(code, gdk::ModifierType::empty(), 0)
            .map_or(key, |t| t.0);
        let Some(name) = base.name().map(|n| n.to_string()) else {
            return;
        };
        let mut mods = 0;
        for (m, bit) in [
            (gdk::ModifierType::SHIFT_MASK, Keybind::SHIFT),
            (gdk::ModifierType::CONTROL_MASK, Keybind::CTRL),
            (gdk::ModifierType::ALT_MASK, Keybind::ALT),
            (gdk::ModifierType::SUPER_MASK, Keybind::SUPER),
        ] {
            if plain.contains(m) {
                mods |= bit;
            }
        }
        self.capture(None);
        self.set_key(launch, Keybind::new(mods, &name));
    }

    fn set_key(self: &Rc<Self>, launch: Launch, key: Keybind) {
        self.change(|c| match launch {
            Launch::Alternative => c.alternative.key = key,
            Launch::Fastshot => c.fastshot.key = key,
            _ => c.main.key = key,
        });
        self.show_keys();
        self.apply_keys(None);
    }

    fn show_keys(&self) {
        let c = self.config.borrow();
        for (key, launch) in c.keys() {
            let k = self.key(launch);
            let on = self.capturing.get() == Some(launch);
            k.button.set_label(&match on {
                true => "Press keys…".into(),
                false => key.label(),
            });
            match on {
                true => k.button.add_css_class("flick-capturing"),
                false => k.button.remove_css_class("flick-capturing"),
            }
            k.clear.set_sensitive(key.is_bound());
        }
    }

    /// Writes the bindings file and reloads Hyprland if it loads the file.
    fn apply_keys(self: &Rc<Self>, error: Option<String>) {
        let mut error = error;
        let c = self.config.borrow().clone();
        if let (None, Some(h), None) = (&error, HyprConfig::locate(), c.problem()) {
            let result = h
                .write_binds(&c, &hypr::program())
                .and_then(|()| match h.is_included() {
                    true => hypr::reload(),
                    false => Ok(()),
                });
            error = result.err().map(|e| format!("{e:#}"));
        }
        self.show_hypr(error);
        // Hyprland needs a moment to register the new bindings.
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(400), move || {
            if let Some(s) = weak.upgrade() {
                s.check_conflicts();
            }
        });
    }

    fn show_hypr(&self, error: Option<String>) {
        let h = HyprConfig::locate();
        let included = h.as_ref().is_some_and(HyprConfig::is_included);
        let text = match &h {
            None => "No Hyprland config found. Bind “flick”, “flick --alt” and \
                     “flick fastshot” yourself."
                .to_owned(),
            Some(h) if included => format!(
                "Keybindings are active; Flick keeps them in {}.",
                short(&h.binds)
            ),
            Some(h) => format!(
                "Keybindings are not active yet: {main} does not load {binds}. \
                 This adds “{}” to the end of {main}.",
                h.include_line(),
                main = name(&h.main),
                binds = name(&h.binds),
            ),
        };
        self.hypr_status.set_text(&text);
        self.hypr_button.set_visible(h.is_some() && !included);
        if let Some(e) = error {
            self.hypr_warning.set_text(&e);
            self.hypr_warning.set_visible(true);
        }
    }

    fn check_conflicts(&self) {
        if !HyprConfig::locate().is_some_and(|h| h.is_included()) {
            return;
        }
        let c = self.config.borrow();
        // Until the settings are valid, Hyprland still has the old bindings.
        if c.problem().is_some() {
            return;
        }
        let text = match hypr::conflicts(&c) {
            Ok(keys) if keys.is_empty() => None,
            Ok(keys) => Some(format!(
                "Also bound elsewhere in Hyprland: {}. Remove those bindings, or both run.",
                keys.iter()
                    .map(Keybind::label)
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            Err(e) => Some(format!("{e:#}")),
        };
        self.hypr_warning.set_text(text.as_deref().unwrap_or(""));
        self.hypr_warning.set_visible(text.is_some());
    }
}
