mod canvas;
mod decoders;
mod export;
mod filmstrip;
mod format;
mod image_view;
mod loader;
mod playlist;
mod scene;
mod thumbs;
mod window;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

const APP_ID: &str = "dev.local.SimpleViewer";

/// Where the chosen theme is remembered. A plain file rather than GSettings:
/// that would need a schema compiled and installed system-wide, which is a lot
/// of machinery for one word.
fn theme_file() -> std::path::PathBuf {
    glib::user_config_dir().join("simple-viewer").join("theme")
}

fn load_theme() -> String {
    let saved = std::fs::read_to_string(theme_file()).unwrap_or_default();
    match saved.trim() {
        "light" => "light".to_string(),
        "dark" => "dark".to_string(),
        _ => "system".to_string(),
    }
}

fn save_theme(theme: &str) {
    let path = theme_file();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Losing the preference is not worth bothering the user about.
    let _ = std::fs::write(path, theme);
}

fn apply_theme(theme: &str) {
    let scheme = match theme {
        "light" => adw::ColorScheme::ForceLight,
        "dark" => adw::ColorScheme::ForceDark,
        // Whatever the desktop is set to, and it keeps following it.
        _ => adw::ColorScheme::Default,
    };
    adw::StyleManager::default().set_color_scheme(scheme);
}

fn install_theme_action(app: &adw::Application) {
    let initial = load_theme();
    apply_theme(&initial);

    let action = gio::SimpleAction::new_stateful(
        "theme",
        Some(glib::VariantTy::STRING),
        &initial.to_variant(),
    );
    action.connect_change_state(|action, value| {
        let Some(theme) = value.and_then(|v| v.str().map(str::to_owned)) else {
            return;
        };
        apply_theme(&theme);
        save_theme(&theme);
        // Ticks the matching radio item in the menu.
        action.set_state(&theme.to_variant());
    });
    app.add_action(&action);
}

/// Highlights the window while a file is hovering over it.
fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        "
        .drop-active { box-shadow: inset 0 0 0 4px @accent_bg_color; }
        .filmstrip-slot { padding: 2px; }
        .filmstrip-current {
            outline: 2px solid @accent_bg_color;
            outline-offset: -2px;
            background: alpha(@accent_bg_color, 0.18);
        }
        /* The two file actions carry a tint rather than a plain button face,
           so the destructive one is never mistaken for the reversible one.
           Both are drawn from libadwaita's palette, so they track the theme. */
        .image-actions { padding: 6px 12px; }
        .image-actions button {
            padding-left: 12px;
            padding-right: 12px;
        }
        .image-actions button.edit-action {
            color: @warning_color;
            background: alpha(@warning_bg_color, 0.18);
        }
        .image-actions button.edit-action:hover {
            background: alpha(@warning_bg_color, 0.32);
        }
        .image-actions button.edit-action:checked {
            color: @warning_fg_color;
            background: @warning_bg_color;
        }
        .image-actions button.delete-action {
            color: @destructive_color;
            background: alpha(@destructive_bg_color, 0.18);
        }
        .image-actions button.delete-action:hover {
            background: alpha(@destructive_bg_color, 0.32);
        }
        .image-actions button:disabled {
            color: alpha(@window_fg_color, 0.35);
            background: alpha(@window_fg_color, 0.06);
        }
        ",
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
        install_theme_action(app);

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
        app.set_accels_for_action("win.dismiss", &["Escape"]);
        app.set_accels_for_action("win.fullscreen", &["F11", "<Primary>f"]);
        app.set_accels_for_action("win.delete", &["Delete"]);
        app.set_accels_for_action("win.copy", &["<Primary>c"]);
        app.set_accels_for_action("win.edit", &["<Primary>e"]);
        app.set_accels_for_action("win.undo", &["<Primary>z"]);
        app.set_accels_for_action("win.redo", &["<Primary><Shift>z", "<Primary>y"]);
        app.set_accels_for_action("win.crop-apply", &["Return", "KP_Enter"]);
        app.set_accels_for_action("win.save", &["<Primary>s"]);
        app.set_accels_for_action("win.save-as", &["<Primary><Shift>s"]);
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
