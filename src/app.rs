use crate::{
    capture::{CaptureBackend, CaptureRequest, OutputInfo, wlr::WlrBackend},
    config::{Action, Config, Launch, Lock, Paths},
    hypr,
    output::{clipboard, save},
    ui::{
        self,
        overlay::{Frozen, Group},
        settings,
    },
};
use anyhow::Result;
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
    let overlay: Rc<RefCell<Weak<Group>>> = Rc::default();
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

fn handle(app: &gtk::Application, launch: Launch, overlay: &RefCell<Weak<Group>>) -> Result<()> {
    // The keys being typed into the settings must not start anything.
    if settings::is_capturing() {
        return Ok(());
    }
    let current = overlay
        .borrow()
        .upgrade()
        .map(|g| g.members())
        .unwrap_or_default();
    let paths = Paths::new()?;
    match launch {
        Launch::Settings => match current.iter().find(|o| o.selecting()) {
            Some(o) => o.open_settings(),
            None => settings::open(app, &paths, || {}),
        },
        // A second launch reaches the running instance: it stops a recording,
        // otherwise one overlay is enough. A preview does not block a new one.
        Launch::Main | Launch::Alternative if current.iter().any(|o| !o.previewing()) => {
            for o in current {
                o.stop_recording();
            }
        }
        _ => {
            if let Some(g) = open(app, launch, paths)? {
                *overlay.borrow_mut() = Rc::downgrade(&g);
            }
        }
    }
    Ok(())
}

/// Opens an overlay on every output, or shoots the locked region at once when
/// the behavior does not show the toolbar.
fn open(app: &gtk::Application, launch: Launch, paths: Paths) -> Result<Option<Rc<Group>>> {
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
    anyhow::ensure!(!outputs.is_empty(), "no outputs found");
    // The keyboard starts where the lock is, or on the focused output.
    let focused = match &lock {
        Some(l) => Some(l.output.name.clone()),
        None => hypr::focused_output(),
    };

    // Every frame is taken before any overlay is mapped, or it would be in the picture.
    let frames = outputs
        .iter()
        .map(|o: &OutputInfo| {
            backend.frame(&CaptureRequest {
                output: o.name.clone(),
                region: None,
                cursor: false,
                interval: Duration::ZERO,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let group = Rc::new(Group::default());
    let mut first_error = None;
    for (output, frame) in outputs.into_iter().zip(frames) {
        let locked = lock
            .as_ref()
            .filter(|l| l.output.name == output.name)
            .map(|l| l.region);
        let name = output.name.clone();
        let frozen = Frozen {
            frame,
            output,
            locked,
        };
        let opened = ui::overlay::open(app, frozen, launch, config.clone(), paths.clone(), &group);
        match opened {
            Ok(o) if Some(&name) == focused.as_ref() => o.take_keyboard(),
            Ok(_) => {}
            Err(e) => {
                eprintln!("flick: skipping output {name}: {e:#}");
                first_error.get_or_insert(e);
            }
        }
    }
    match (group.members().first(), first_error) {
        (Some(_), _) => Ok(Some(group)),
        (None, Some(e)) => Err(e),
        (None, None) => anyhow::bail!("no outputs found"),
    }
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
