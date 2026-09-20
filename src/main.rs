mod canvas;
mod decoders;
mod format;
mod image_view;
mod loader;
mod playlist;
mod window;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

const APP_ID: &str = "dev.local.SimpleViewer";

/// Highlights the window while a file is hovering over it.
fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        ".drop-active { box-shadow: inset 0 0 0 4px @accent_bg_color; }",
    );
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

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

        load_css();

        app.set_accels_for_action("win.open", &["<Primary>o"]);
        app.set_accels_for_action("win.close", &["<Primary>w"]);
        app.set_accels_for_action("app.quit", &["<Primary>q"]);
        // Both the bare and Ctrl forms: there is no text entry to conflict with,
        // and "+" needs the unshifted "equal" too on most layouts.
        app.set_accels_for_action(
            "win.zoom-in",
            &["<Primary>plus", "<Primary>equal", "plus", "equal"],
        );
        app.set_accels_for_action("win.zoom-out", &["<Primary>minus", "minus"]);
        app.set_accels_for_action("win.zoom-fit", &["<Primary>0", "0"]);
        app.set_accels_for_action("win.zoom-actual", &["<Primary>1", "1"]);
        app.set_accels_for_action("win.rotate-left", &["bracketleft", "<Primary>bracketleft"]);
        app.set_accels_for_action("win.rotate-right", &["bracketright", "<Primary>bracketright", "<Primary>r"]);
        app.set_accels_for_action("win.rotate-reset", &["<Primary><Shift>r"]);
        app.set_accels_for_action("win.next-image", &["Right", "Page_Down", "space"]);
        app.set_accels_for_action("win.previous-image", &["Left", "Page_Up", "BackSpace"]);
        app.set_accels_for_action("win.transform-open", &["<Primary>t"]);
        app.set_accels_for_action("win.transform-close", &["Escape"]);
        app.set_accels_for_action("win.flip-horizontal", &["<Primary>h"]);
        app.set_accels_for_action("win.flip-vertical", &["<Primary>j"]);
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
