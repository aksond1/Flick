//! Drawing tools shown in place of the toolbar. The overlay owns the canvas and
//! forwards pointer positions, all in logical pixels.
pub mod color;
pub mod document;
mod stepper;

use color::{Color, PRESETS, Picker, hex, paint_swatch, parse};
use document::{Document, Op};
use gtk::{cairo, gdk, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use stepper::Stepper;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Brush,
    Text,
}

pub struct Editor {
    pub panel: gtk::Box,
    pub copy: gtk::Button,
    pub save: gtk::Button,
    pub close: gtk::Button,
    pub error: gtk::Label,
    document: RefCell<Document>,
    tool: Cell<Tool>,
    colors: Cell<(Color, Color)>,
    stroke: RefCell<Vec<(f64, f64)>>,
    /// Where typing goes on; the widget sits on the overlay.
    entry: RefCell<Option<(gtk::TextView, (i32, i32))>>,
    entry_css: gtk::CssProvider,
    layers: gtk::Overlay,
    /// Frame pixels per logical pixel.
    scale: f64,
    brush: gtk::ToggleButton,
    width: Rc<Stepper>,
    slider: gtk::Scale,
    font: gtk::DropDown,
    size: Rc<Stepper>,
    swatch: gtk::DrawingArea,
    picker: Rc<Picker>,
    undo: gtk::Button,
    redo: gtk::Button,
    clear: gtk::Button,
    /// Redraws the canvas and, if asked, places the panel again.
    changed: Box<dyn Fn(bool)>,
}

impl Editor {
    pub fn new(layers: &gtk::Overlay, scale: f64, changed: impl Fn(bool) + 'static) -> Rc<Self> {
        let button = |icon: &str, tip: &str| {
            let b = gtk::Button::from_icon_name(icon);
            b.set_tooltip_text(Some(tip));
            b.set_focusable(false);
            b
        };
        let brush = gtk::ToggleButton::new();
        brush.set_icon_name("document-edit-symbolic");
        brush.set_tooltip_text(Some("Brush"));
        brush.set_active(true);
        let text = gtk::ToggleButton::new();
        text.set_label("T");
        text.add_css_class("flick-letter");
        text.set_tooltip_text(Some("Text"));
        text.set_group(Some(&brush));
        for b in [&brush, &text] {
            b.set_focusable(false);
        }

        let tools = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        tools.add_css_class("flick-field");
        tools.append(&brush);
        tools.append(&text);

        let swatch = gtk::DrawingArea::new();
        swatch.set_content_width(22);
        swatch.set_content_height(22);
        swatch.set_halign(gtk::Align::Center);
        swatch.set_valign(gtk::Align::Center);
        let picker = Picker::new();
        let popover = gtk::Popover::new();
        popover.set_child(Some(&picker.root));
        let color = gtk::MenuButton::new();
        color.set_child(Some(&swatch));
        color.add_css_class("flick-color");
        color.set_tooltip_text(Some("Color"));
        color.set_popover(Some(&popover));

        let width = Stepper::new(1, 100, 4, "Brush width");
        // As wide as the font field, so the panel keeps its size between tools.
        let slider = gtk::Scale::with_range(gtk::Orientation::Horizontal, 1.0, 100.0, 1.0);
        slider.add_css_class("flick-field");
        slider.set_size_request(94, -1);
        slider.set_focusable(false);
        slider.set_tooltip_text(Some("Brush width"));
        slider.set_value(width.value());
        let size = Stepper::new(6, 300, 24, "Text size");
        let mut families: Vec<String> = layers
            .pango_context()
            .list_families()
            .iter()
            .map(|f| f.name().to_string())
            .collect();
        families.sort_by_key(|f| f.to_lowercase());
        let names: Vec<&str> = families.iter().map(String::as_str).collect();
        let font = gtk::DropDown::from_strings(&names);
        font.set_enable_search(true);
        font.set_expression(Some(gtk::PropertyExpression::new(
            gtk::StringObject::static_type(),
            None::<gtk::Expression>,
            "string",
        )));
        font.set_search_match_mode(gtk::StringFilterMatchMode::Substring);
        font.set_tooltip_text(Some("Font"));
        font.add_css_class("flick-font");
        font.set_size_request(94, -1);
        // The closed list shortens long names, so the field keeps its size.
        font.set_list_factory(Some(&name_factory(false)));
        font.set_factory(Some(&name_factory(true)));
        let default = families.iter().position(|f| f == "Sans");
        font.set_selected(default.unwrap_or(0) as u32);

        let undo = button("edit-undo-symbolic", "Undo");
        let redo = button("edit-redo-symbolic", "Redo");
        let clear = button("edit-clear-all-symbolic", "Clear");
        let copy = button("edit-copy-symbolic", "Copy to clipboard");
        let save = button("folder-download-symbolic", "Save");
        let close = button("window-close-symbolic", "Back");
        let row = |widgets: &[&gtk::Widget]| {
            let r = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            for w in widgets {
                w.set_valign(gtk::Align::Center);
                r.append(*w);
            }
            r
        };
        let top = row(&[
            tools.upcast_ref(),
            color.upcast_ref(),
            slider.upcast_ref(),
            width.root.upcast_ref(),
            font.upcast_ref(),
            size.root.upcast_ref(),
        ]);
        top.set_spacing(6);
        let space = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        space.set_hexpand(true);
        let bottom = row(&[
            undo.upcast_ref(),
            redo.upcast_ref(),
            clear.upcast_ref(),
            space.upcast_ref(),
            copy.upcast_ref(),
            save.upcast_ref(),
            close.upcast_ref(),
        ]);
        let error = gtk::Label::new(None);
        error.add_css_class("flick-error");
        error.set_wrap(true);
        error.set_max_width_chars(32);
        error.set_visible(false);
        let panel = gtk::Box::new(gtk::Orientation::Vertical, 4);
        panel.add_css_class("flick-toolbar");
        panel.set_halign(gtk::Align::Start);
        panel.set_valign(gtk::Align::Start);
        panel.set_cursor_from_name(Some("default"));
        panel.set_visible(false);
        panel.append(&top);
        panel.append(&bottom);
        panel.append(&error);
        layers.add_overlay(&panel);

        let entry_css = gtk::CssProvider::new();
        gtk::style_context_add_provider_for_display(
            &layers.display(),
            &entry_css,
            gtk::STYLE_PROVIDER_PRIORITY_USER + 2,
        );
        let red = parse(PRESETS[0]).unwrap();
        let e = Rc::new(Self {
            panel,
            copy,
            save,
            close,
            error,
            document: RefCell::default(),
            tool: Cell::new(Tool::Brush),
            colors: Cell::new((red, red)),
            stroke: RefCell::default(),
            entry: RefCell::new(None),
            entry_css,
            layers: layers.clone(),
            scale,
            brush,
            width,
            slider,
            font,
            size,
            swatch,
            picker,
            undo,
            redo,
            clear,
            changed: Box::new(changed),
        });

        let weak = Rc::downgrade(&e);
        e.swatch.set_draw_func(move |_, cr, w, h| {
            if let Some(e) = weak.upgrade() {
                paint_swatch(cr, w, h, e.color());
            }
        });
        let weak = Rc::downgrade(&e);
        e.picker.connect_changed(move |c| {
            if let Some(e) = weak.upgrade() {
                e.set_color(c);
            }
        });
        let weak = Rc::downgrade(&e);
        popover.connect_show(move |_| {
            if let Some(e) = weak.upgrade() {
                e.show_color();
            }
        });
        let pop = popover.clone();
        e.picker.connect_done(move || pop.popdown());
        let weak = Rc::downgrade(&e);
        e.brush.connect_toggled(move |b| {
            if let Some(e) = weak.upgrade() {
                e.set_tool(if b.is_active() {
                    Tool::Brush
                } else {
                    Tool::Text
                });
            }
        });
        let weak = Rc::downgrade(&e);
        e.slider.connect_change_value(move |_, _, v| {
            if let Some(e) = weak.upgrade() {
                e.width.set(v.round() as i32);
            }
            glib::Propagation::Stop
        });
        let weak = Rc::downgrade(&e);
        e.width.connect_changed(move || {
            if let Some(e) = weak.upgrade() {
                e.slider.set_value(e.width.value());
            }
        });
        let weak = Rc::downgrade(&e);
        e.size.connect_changed(move || {
            if let Some(e) = weak.upgrade() {
                e.style_entry();
            }
        });
        let weak = Rc::downgrade(&e);
        e.font.connect_selected_notify(move |_| {
            if let Some(e) = weak.upgrade() {
                e.style_entry();
                e.focus_entry();
            }
        });
        // The list's own popover: Escape there only empties the search.
        let mut child = e.font.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            let Ok(p) = c.downcast::<gtk::Popover>() else {
                continue;
            };
            let keys = gtk::EventControllerKey::new();
            keys.set_propagation_phase(gtk::PropagationPhase::Capture);
            let pop = p.clone();
            keys.connect_key_pressed(move |_, key, _, _| {
                if key != gdk::Key::Escape {
                    return glib::Propagation::Proceed;
                }
                pop.popdown();
                glib::Propagation::Stop
            });
            p.add_controller(keys);
            e.resume_after(&p);
        }
        e.resume_after(&popover);
        e.on_click(&e.undo, |e| e.undo());
        e.on_click(&e.redo, |e| e.redo());
        e.on_click(&e.clear, |e| {
            e.commit_text();
            e.document.borrow_mut().clear();
            e.update(false);
        });
        e.set_tool(Tool::Brush);
        e
    }

    fn on_click(self: &Rc<Self>, b: &gtk::Button, f: impl Fn(&Rc<Self>) + 'static) {
        let weak = Rc::downgrade(self);
        b.connect_clicked(move |_| {
            if let Some(e) = weak.upgrade() {
                f(&e);
            }
        });
    }

    pub fn tool(&self) -> Tool {
        self.tool.get()
    }

    fn set_tool(&self, tool: Tool) {
        self.commit_text();
        self.tool.set(tool);
        self.width.root.set_visible(tool == Tool::Brush);
        self.slider.set_visible(tool == Tool::Brush);
        self.font.set_visible(tool == Tool::Text);
        self.size.root.set_visible(tool == Tool::Text);
        self.show_color();
        self.update(true);
    }

    fn color(&self) -> Color {
        let (brush, text) = self.colors.get();
        match self.tool.get() {
            Tool::Brush => brush,
            Tool::Text => text,
        }
    }

    fn set_color(&self, c: Color) {
        let (brush, text) = self.colors.get();
        self.colors.set(match self.tool.get() {
            Tool::Brush => (c, text),
            Tool::Text => (brush, c),
        });
        self.swatch.queue_draw();
        self.style_entry();
    }

    fn show_color(&self) {
        self.picker.set(self.color());
        self.swatch.queue_draw();
    }

    fn family(&self) -> String {
        let item = self.font.selected_item();
        let item = item.and_downcast::<gtk::StringObject>();
        item.map_or("Sans".into(), |s| s.string().to_string())
    }

    fn update(&self, replace: bool) {
        {
            let d = self.document.borrow();
            self.undo.set_sensitive(d.can_undo());
            self.redo.set_sensitive(d.can_redo());
            self.clear.set_sensitive(!d.is_blank());
        }
        (self.changed)(replace);
    }

    pub fn undo(&self) {
        // Unfinished text counts as the latest change.
        self.commit_text();
        self.document.borrow_mut().undo();
        self.update(false);
    }

    pub fn redo(&self) {
        self.commit_text();
        self.document.borrow_mut().redo();
        self.update(false);
    }

    fn to_frame(&self, p: (f64, f64)) -> (f64, f64) {
        (p.0 * self.scale, p.1 * self.scale)
    }

    /// A press inside the region.
    pub fn press(self: &Rc<Self>, p: (f64, f64)) {
        match self.tool.get() {
            Tool::Brush => {
                *self.stroke.borrow_mut() = vec![self.to_frame(p)];
                (self.changed)(false);
            }
            // The first press elsewhere only finishes the current text.
            Tool::Text if self.commit_text() => {}
            Tool::Text => self.start_text((p.0.round() as i32, p.1.round() as i32)),
        }
    }

    pub fn drag(&self, p: (f64, f64)) {
        let mut s = self.stroke.borrow_mut();
        if !s.is_empty() {
            s.push(self.to_frame(p));
            drop(s);
            (self.changed)(false);
        }
    }

    pub fn release(&self) {
        let points = self.stroke.take();
        if !points.is_empty() {
            self.document.borrow_mut().push(self.stroke_op(points));
            self.update(false);
        }
    }

    fn stroke_op(&self, points: Vec<(f64, f64)>) -> Op {
        Op::Stroke {
            points,
            width: self.width.value() * self.scale,
            color: self.colors.get().0,
        }
    }

    fn start_text(self: &Rc<Self>, at: (i32, i32)) {
        let view = gtk::TextView::new();
        view.add_css_class("flick-entry");
        view.set_halign(gtk::Align::Start);
        view.set_valign(gtk::Align::Start);
        view.set_margin_start(at.0);
        view.set_margin_top(at.1);
        self.layers.add_overlay(&view);
        *self.entry.borrow_mut() = Some((view.clone(), at));
        let weak = Rc::downgrade(self);
        view.buffer().connect_changed(move |_| {
            if let Some(e) = weak.upgrade() {
                e.size_entry();
            }
        });
        self.style_entry();
        view.grab_focus();
    }

    fn style_entry(&self) {
        // Family names may contain anything but must not end the CSS string.
        let family = self.family().replace(['"', '\\'], "");
        self.entry_css.load_from_string(&format!(
            "textview.flick-entry, textview.flick-entry text {{ background: transparent; \
             color: {c}; caret-color: {c}; font-family: \"{family}\"; font-size: {s}px; \
             font-weight: normal; padding: 0; }}\
             textview.flick-entry {{ outline: 1px dashed {c}; }}",
            c = hex(self.colors.get().1),
            s = self.size.value(),
        ));
        self.size_entry();
    }

    /// A text view does not grow by itself, so it is sized like the final text.
    fn size_entry(&self) {
        let Some(view) = self.entry.borrow().as_ref().map(|(v, _)| v.clone()) else {
            return;
        };
        let b = view.buffer();
        let text = b.text(&b.start_iter(), &b.end_iter(), false);
        let layout = view.create_pango_layout(Some(&text));
        let font = document::font(&self.family(), self.size.value());
        layout.set_font_description(Some(&font));
        let (w, h) = layout.pixel_size();
        // Room for the caret.
        view.set_size_request(w + 8, h);
    }

    fn resume_after(self: &Rc<Self>, p: &gtk::Popover) {
        let weak = Rc::downgrade(self);
        p.connect_closed(move |_| {
            // Once GTK has put the focus where it wants.
            let weak = weak.clone();
            glib::idle_add_local_once(move || {
                if let Some(e) = weak.upgrade() {
                    e.focus_entry();
                }
            });
        });
    }

    /// Back to typing after a choice in the panel.
    fn focus_entry(&self) {
        let view = self.entry.borrow().as_ref().map(|(v, _)| v.clone());
        if let Some(v) = view {
            v.grab_focus();
        }
    }

    /// Turns the text being typed into an operation. False if there was none.
    pub fn commit_text(&self) -> bool {
        let Some((view, at)) = self.entry.take() else {
            return false;
        };
        let b = view.buffer();
        let text = b.text(&b.start_iter(), &b.end_iter(), false);
        self.layers.remove_overlay(&view);
        if !text.trim().is_empty() {
            self.document.borrow_mut().push(Op::Text {
                at: self.to_frame((at.0.into(), at.1.into())),
                text: text.into(),
                family: self.family(),
                size: self.size.value() * self.scale,
                color: self.colors.get().1,
            });
        }
        self.update(false);
        true
    }

    /// Draws in logical pixels, including what is being drawn right now.
    pub fn render(&self, cr: &cairo::Context) -> Result<(), cairo::Error> {
        cr.save()?;
        cr.scale(1.0 / self.scale, 1.0 / self.scale);
        self.document.borrow().render(cr)?;
        let stroke = self.stroke.borrow();
        if !stroke.is_empty() {
            document::draw(cr, &self.stroke_op(stroke.clone()))?;
        }
        cr.restore()
    }

    pub fn flatten(
        &self,
        frame: crate::capture::Frame,
        origin: (i32, i32),
    ) -> anyhow::Result<crate::capture::Frame> {
        self.document.borrow().flatten(frame, origin)
    }
}

fn name_factory(short: bool) -> gtk::SignalListItemFactory {
    let f = gtk::SignalListItemFactory::new();
    f.connect_setup(move |_, item| {
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        if short {
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_width_chars(1);
            label.set_hexpand(true);
        }
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            item.set_child(Some(&label));
        }
    });
    f.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let name = item.item().and_downcast::<gtk::StringObject>();
        let label = item.child().and_downcast::<gtk::Label>();
        if let (Some(name), Some(label)) = (name, label) {
            label.set_text(&name.string());
        }
    });
    f
}
