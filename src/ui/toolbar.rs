use gtk::prelude::*;

/// The six-button panel shown next to the selected region.
pub struct Toolbar {
    pub root: gtk::Box,
    pub settings: gtk::Button,
    pub lock: gtk::ToggleButton,
    pub play: gtk::Button,
    pub edit: gtk::Button,
    pub copy: gtk::Button,
    pub download: gtk::Button,
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
        let edit = button("document-edit-symbolic", "Edit");
        let copy = button("edit-copy-symbolic", "Copy to clipboard");
        let download = button("folder-download-symbolic", "Save");
        // Arrive with stages 2-4.
        for b in [&settings, &play, &edit] {
            b.set_sensitive(false);
        }
        row.append(&settings);
        row.append(&lock);
        for b in [&play, &edit, &copy, &download] {
            row.append(b);
        }
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
            edit,
            copy,
            download,
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
