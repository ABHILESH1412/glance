mod decoders;
mod format;
mod image_view;
mod loader;
mod window;

use adw::prelude::*;
use gtk::{gio, glib};

const APP_ID: &str = "dev.local.SimpleViewer";

fn main() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        // Without this, a path on the command line is rejected as an unknown option.
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    app.connect_startup(|app| {
        let quit = gio::SimpleAction::new("quit", None);
        let app_weak = app.downgrade();
        quit.connect_activate(move |_, _| {
            if let Some(app) = app_weak.upgrade() {
                app.quit();
            }
        });
        app.add_action(&quit);

        app.set_accels_for_action("win.open", &["<Primary>o"]);
        app.set_accels_for_action("win.close", &["<Primary>w"]);
        app.set_accels_for_action("app.quit", &["<Primary>q"]);
    });

    app.connect_activate(|app| {
        window::Window::new(app).present();
    });

    app.connect_open(|app, files, _hint| {
        let window = window::Window::new(app);
        if let Some(file) = files.first() {
            window.open_file(file);
        }
        window.present();
    });

    app.run()
}
