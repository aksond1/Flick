use gtk::prelude::*;

/// The panel shown next to the selected region.
pub struct Toolbar {
    pub root: gtk::Box,
    pub settings: gtk::Button,
    pub lock: gtk::ToggleButton,
    pub play: gtk::Button,
    /// Pauses and resumes a recording.
    pub pause: gtk::Button,
    pub edit: gtk::Button,
    pub copy: gtk::Button,
    pub download: gtk::Button,
    pub close: gtk::Button,
    pub time: gtk::Label,
    /// Moves the region while recording.
    pub grip: gtk::Image,
    pub error: gtk::Label,
}

impl Toolbar {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.add_css_class("flick-toolbar");
        root.set_halign(gtk::Align::Start);
        root.set_valign(gtk::Align::Start);
        root.set_visible(false);
        root.set_cursor_from_name(Some("default"));

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let button = |icon: &str, tip: &str| {
            let b = gtk::Button::from_icon_name(icon);
            b.set_tooltip_text(Some(tip));
            b.set_focusable(false);
            b
        };
        let settings = button("emblem-system-symbolic", "Settings");
        let lock = gtk::ToggleButton::new();
        lock.set_focusable(false);
        let play = button("media-playback-start-symbolic", "Record");
        let pause = button("media-playback-pause-symbolic", "Pause");
        pause.set_visible(false);
        let edit = button("document-edit-symbolic", "Edit");
        let copy = button("edit-copy-symbolic", "Copy to clipboard");
        let download = button("folder-download-symbolic", "Save");
        let close = button("window-close-symbolic", "Close");
        row.append(&settings);
        row.append(&lock);
        for b in [&play, &pause, &edit, &copy, &download, &close] {
            row.append(b);
        }
        let time = gtk::Label::new(Some("00:00"));
        time.add_css_class("flick-time");
        time.set_visible(false);
        row.append(&time);
        let grip = gtk::Image::from_icon_name("list-drag-handle-symbolic");
        grip.set_size_request(36, 36);
        grip.set_tooltip_text(Some("Drag to move the region"));
        grip.set_cursor_from_name(Some("move"));
        grip.set_visible(false);
        row.append(&grip);
        let error = gtk::Label::new(None);
        error.add_css_class("flick-error");
        error.set_wrap(true);
        error.set_max_width_chars(32);
        error.set_visible(false);
        root.append(&row);
        root.append(&error);
        let t = Self {
            root,
            settings,
            lock,
            play,
            pause,
            edit,
            copy,
            download,
            close,
            time,
            grip,
            error,
        };
        t.show_locked(false);
        t
    }

    pub fn show_locked(&self, locked: bool) {
        self.lock.set_active(locked);
        self.lock.set_icon_name(if locked {
            "changes-prevent-symbolic"
        } else {
            "changes-allow-symbolic"
        });
        self.lock.set_tooltip_text(Some(if locked {
            "Unlock region"
        } else {
            "Lock region"
        }));
    }

    /// While recording only stop, pause, the timer and the grip remain.
    pub fn show_recording(&self, on: bool) {
        for b in [
            &self.settings,
            &self.edit,
            &self.copy,
            &self.download,
            &self.close,
        ] {
            b.set_visible(!on);
        }
        self.lock.set_visible(!on);
        self.time.set_text("00:00");
        self.time.set_visible(on);
        self.pause.set_visible(on);
        self.show_paused(false);
        self.play.set_icon_name(if on {
            "media-playback-stop-symbolic"
        } else {
            "media-playback-start-symbolic"
        });
        self.play
            .set_tooltip_text(Some(if on { "Stop" } else { "Record" }));
    }

    pub fn show_paused(&self, paused: bool) {
        let (icon, tip) = if paused {
            ("media-record-symbolic", "Resume")
        } else {
            ("media-playback-pause-symbolic", "Pause")
        };
        self.pause.set_icon_name(icon);
        self.pause.set_tooltip_text(Some(tip));
        if paused {
            self.time.add_css_class("flick-paused");
        } else {
            self.time.remove_css_class("flick-paused");
        }
    }

    pub fn show_error(&self, message: &str) {
        self.error.set_text(message);
        self.error.set_visible(true);
    }
}

impl Default for Toolbar {
    fn default() -> Self {
        Self::new()
    }
}
