use crate::{
    capture::{CaptureBackend, CaptureRequest, wlr::WlrBackend},
    config::{Config, Lock, Paths},
    hypr,
    ui::{self, overlay::Overlay},
};
use anyhow::{Context, Result};
use gtk::{glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    time::Duration,
};

pub fn run() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("dev.flick.Flick")
        .build();
    let failed = Rc::new(Cell::new(false));
    let flag = failed.clone();
    let overlay: Rc<RefCell<Weak<Overlay>>> = Rc::default();
    app.connect_activate(move |app| {
        // A second launch reaches the running instance: it stops a recording,
        // otherwise one overlay is enough. A preview does not block a new one.
        let current = overlay.borrow().upgrade();
        if let Some(o) = current.filter(|o| !o.previewing()) {
            o.stop_recording();
            return;
        }
        match open(app) {
            Ok(o) => *overlay.borrow_mut() = Rc::downgrade(&o),
            Err(e) => {
                eprintln!("flick: {e:#}");
                flag.set(true);
            }
        }
    });
    let code = app.run_with_args::<&str>(&[]);
    if failed.get() {
        glib::ExitCode::FAILURE
    } else {
        code
    }
}

fn open(app: &gtk::Application) -> Result<Rc<Overlay>> {
    anyhow::ensure!(
        gtk4_layer_shell::is_supported(),
        "the compositor does not support layer-shell"
    );
    let paths = Paths::new()?;
    let config = Config::load(&paths.config)?;
    let mut backend = WlrBackend::connect()?;
    let outputs = backend.outputs()?;

    let lock = Lock::load(&paths.lock).unwrap_or_else(|e| {
        eprintln!("flick: ignoring the saved region: {e:#}");
        None
    });
    let lock = lock.filter(|l| l.is_valid(&outputs));
    let wanted = match &lock {
        Some(l) => Some(l.output.name.clone()),
        None => hypr::focused_output(),
    };
    let output = outputs
        .iter()
        .find(|o| Some(&o.name) == wanted.as_ref())
        .or(outputs.first())
        .context("no outputs found")?
        .clone();

    // Must happen before the overlay is mapped, or it would be in the picture.
    let frame = backend.frame(&CaptureRequest {
        output: output.name.clone(),
        region: None,
        cursor: false,
        interval: Duration::ZERO,
    })?;
    ui::overlay::open(app, frame, output, lock.map(|l| l.region), config, paths)
}
