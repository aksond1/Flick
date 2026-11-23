pub mod editor;
pub mod overlay;
pub mod preview;
pub mod selection;
pub mod toolbar;

use gtk::{gio, glib, prelude::*};

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
