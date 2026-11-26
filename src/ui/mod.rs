pub mod editor;
pub mod overlay;
pub mod preview;
pub mod selection;
pub mod settings;
pub mod toolbar;

use anyhow::Result;
use gtk::{gio, glib, prelude::*};
use std::path::{Path, PathBuf};

/// Shows an error that has no window of its own to appear in.
pub fn show_error(app: &gtk::Application, message: &str) {
    eprintln!("flick: {message}");
    let hold = app.hold();
    let dialog = gtk::AlertDialog::builder()
        .message("Flick")
        .detail(message)
        .buttons(["OK"])
        .build();
    dialog.choose(
        None::<&gtk::Window>,
        None::<&gio::Cancellable>,
        move |_: Result<i32, glib::Error>| drop(hold),
    );
}

/// Asks where to save, starting in `folder`; `None` when the user cancels.
/// Dialogs open underneath the overlay, so callers hide it first.
pub async fn ask_path(folder: &Path, name: &str) -> Result<Option<PathBuf>> {
    let dialog = gtk::FileDialog::builder()
        .title("Save")
        .modal(true)
        .initial_name(name)
        .build();
    if std::fs::create_dir_all(folder).is_ok() {
        dialog.set_initial_folder(Some(&gio::File::for_path(folder)));
    }
    match dialog.save_future(None::<&gtk::Window>).await {
        Ok(f) => Ok(f.path()),
        Err(e) if e.matches(gtk::DialogError::Dismissed) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
