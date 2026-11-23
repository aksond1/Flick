//! A compact number field: minus, value, plus. Scrolling changes it too.
use gtk::{glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

pub struct Stepper {
    pub root: gtk::Box,
    entry: gtk::Entry,
    range: (i32, i32),
    value: Cell<i32>,
    on_change: RefCell<Option<Box<dyn Fn()>>>,
}

impl Stepper {
    pub fn new(min: i32, max: i32, value: i32, tip: &str) -> Rc<Self> {
        let button = |text: &str| {
            let b = gtk::Button::with_label(text);
            b.set_focusable(false);
            b
        };
        let (less, more) = (button("−"), button("+"));
        let entry = gtk::Entry::new();
        EntryExt::set_alignment(&entry, 0.5);
        entry.set_max_length(3);
        entry.set_width_chars(3);
        entry.set_max_width_chars(3);
        entry.set_hexpand(true);
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.set_size_request(94, -1);
        // Otherwise the entry would stretch the whole row.
        root.set_hexpand(false);
        root.add_css_class("flick-field");
        root.set_tooltip_text(Some(tip));
        root.append(&less);
        root.append(&entry);
        root.append(&more);
        let s = Rc::new(Self {
            root,
            entry,
            range: (min, max),
            value: Cell::new(value),
            on_change: RefCell::default(),
        });
        s.set(value);
        for (b, step) in [(less, -1), (more, 1)] {
            let weak = Rc::downgrade(&s);
            b.connect_clicked(move |_| {
                if let Some(s) = weak.upgrade() {
                    s.set(s.typed() + step);
                }
            });
        }
        let weak = Rc::downgrade(&s);
        s.entry.connect_changed(move |_| {
            if let Some(s) = weak.upgrade() {
                // Applies while typing, but leaves the text alone.
                s.apply(s.typed());
            }
        });
        let weak = Rc::downgrade(&s);
        s.entry.connect_activate(move |_| {
            if let Some(s) = weak.upgrade() {
                s.set(s.typed());
            }
        });
        let focus = gtk::EventControllerFocus::new();
        let weak = Rc::downgrade(&s);
        focus.connect_leave(move |_| {
            if let Some(s) = weak.upgrade() {
                s.set(s.typed());
            }
        });
        s.entry.add_controller(focus);
        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
        );
        let weak = Rc::downgrade(&s);
        scroll.connect_scroll(move |_, _, dy| {
            if let Some(s) = weak.upgrade() {
                s.set(s.typed() - dy.signum() as i32);
            }
            glib::Propagation::Stop
        });
        s.root.add_controller(scroll);
        s
    }

    pub fn value(&self) -> f64 {
        self.value.get().into()
    }

    pub fn connect_changed(&self, f: impl Fn() + 'static) {
        *self.on_change.borrow_mut() = Some(Box::new(f));
    }

    /// What is typed, or the current value if that is not a number.
    fn typed(&self) -> i32 {
        let text = self.entry.text();
        text.trim().parse().unwrap_or(self.value.get())
    }

    fn apply(&self, v: i32) {
        let v = v.clamp(self.range.0, self.range.1);
        if self.value.replace(v) != v
            && let Some(f) = &*self.on_change.borrow()
        {
            f();
        }
    }

    pub fn set(&self, v: i32) {
        self.apply(v);
        let text = self.value.get().to_string();
        if self.entry.text() != text {
            self.entry.set_text(&text);
        }
    }
}
