use crate::{
    capture::{CaptureBackend, CaptureRequest, OutputInfo, wlr::WlrBackend},
    config::{Action, Config, Launch, Lock, Paths},
    hypr,
    output::{clipboard, save},
    ui::{self, overlay::Overlay, settings},
};
use anyhow::{Context, Result};
use gtk::{gio, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    time::Duration,
};

/// Passed to the running instance; plain words, which GApplication does not parse.
fn token(launch: Launch) -> &'static str {
    match launch {
        Launch::Main => "main",
        Launch::Alternative => "alt",
        Launch::Settings => "settings",
    }
}

pub fn run(launch: Launch) -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("dev.flick.Flick")
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    let failed = Rc::new(Cell::new(false));
    let flag = failed.clone();
    let overlay: Rc<RefCell<Weak<Overlay>>> = Rc::default();
    app.connect_command_line(move |app, cmd| {
        let word = cmd
            .arguments()
            .get(1)
            .map(|a| a.to_string_lossy().into_owned());
        let launch = [Launch::Alternative, Launch::Settings]
            .into_iter()
            .find(|l| word.as_deref() == Some(token(*l)))
            .unwrap_or(Launch::Main);
        if let Err(e) = handle(app, launch, &overlay) {
            ui::show_error(app, &format!("{e:#}"));
            flag.set(true);
        }
        glib::ExitCode::SUCCESS
    });
    let code = app.run_with_args(&["flick", token(launch)]);
    if failed.get() {
        glib::ExitCode::FAILURE
    } else {
        code
    }
}

fn handle(app: &gtk::Application, launch: Launch, overlay: &RefCell<Weak<Overlay>>) -> Result<()> {
    // The keys being typed into the settings must not start anything.
    if settings::is_capturing() {
        return Ok(());
    }
    let current = overlay.borrow().upgrade();
    let paths = Paths::new()?;
    match launch {
        Launch::Settings => match current.filter(|o| o.selecting()) {
            Some(o) => o.open_settings(),
            None => settings::open(app, &paths, || {}),
        },
        // A second launch reaches the running instance: it stops a recording,
        // otherwise one overlay is enough. A preview does not block a new one.
        Launch::Main | Launch::Alternative if current.as_ref().is_some_and(|o| !o.previewing()) => {
            if let Some(o) = current {
                o.stop_recording();
            }
        }
        _ => {
            if let Some(o) = open(app, launch, paths)? {
                *overlay.borrow_mut() = Rc::downgrade(&o);
            }
        }
    }
    Ok(())
}

/// Opens the overlay, or shoots the locked region at once when the behavior
/// does not show the toolbar.
fn open(app: &gtk::Application, launch: Launch, paths: Paths) -> Result<Option<Rc<Overlay>>> {
    anyhow::ensure!(
        gtk4_layer_shell::is_supported(),
        "the compositor does not support layer-shell"
    );
    let config = Config::load(&paths.config)?;
    let mut backend = WlrBackend::connect()?;
    let outputs = backend.outputs()?;

    let lock = Lock::load(&paths.lock).unwrap_or_else(|e| {
        eprintln!("flick: ignoring the saved region: {e:#}");
        None
    });
    let lock = lock.filter(|l| l.is_valid(&outputs));
    let behavior = config.behavior(launch);
    if let Some(l) = lock.as_ref().filter(|_| !behavior.open) {
        // Screenshots ask where to save only on the Download button.
        shoot(&mut backend, l, behavior.action, &config)?;
        return Ok(None);
    }
    let wanted = match &lock {
        Some(l) => Some(l.output.name.clone()),
        None => hypr::focused_output(),
    };
    let output: OutputInfo = outputs
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
    let locked = lock.map(|l| l.region);
    ui::overlay::open(app, frame, output, locked, launch, config, paths).map(Some)
}

/// Copies and saves the locked region at once, showing nothing.
fn shoot(backend: &mut WlrBackend, lock: &Lock, action: Action, config: &Config) -> Result<()> {
    let frame = backend.frame(&CaptureRequest {
        output: lock.output.name.clone(),
        region: Some(lock.region),
        cursor: false,
        interval: Duration::ZERO,
    })?;
    let png = save::encode_png(&frame)?;
    if action.copy {
        clipboard::copy_png(&png)?;
    }
    if action.save {
        let path = save::write_new(&config.screenshot_dir()?, &config.file_name, "png", &png)?;
        println!("{}", path.display());
    }
    Ok(())
}
