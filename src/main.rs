mod adjust;
mod canvas;
mod compress;
mod decoders;
mod export;
mod filmstrip;
mod format;
mod image_view;
mod loader;
mod playlist;
mod scene;
mod text;
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
        .image-actions button.resize-action {
            color: @accent_color;
            background: alpha(@accent_bg_color, 0.18);
        }
        .image-actions button.resize-action:hover {
            background: alpha(@accent_bg_color, 0.32);
        }
        .image-actions button.delete-action {
            color: @destructive_color;
            background: alpha(@destructive_bg_color, 0.18);
        }
        .image-actions button.delete-action:hover {
            background: alpha(@destructive_bg_color, 0.32);
        }
        /* The three style toggles show what they do rather than spelling it. */
        button.text-bold label { font-weight: bold; }
        button.text-italic label { font-style: italic; }
        button.text-underline label { text-decoration-line: underline; }
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

/// An action's keys, and the subset of them that is safe while a text box has
/// focus.
///
/// A window takes its accelerators before the focused widget sees the key, so a
/// bare `0` never reaches a size entry and a bare `Delete` would remove the file
/// instead of a digit. While something editable has focus, the bare keys are
/// withdrawn and only the modified forms stay live.
struct Accel {
    action: &'static str,
    idle: &'static [&'static str],
    typing: &'static [&'static str],
}

const ACCELS: &[Accel] = &[
    Accel { action: "win.open", idle: &["<Primary>o"], typing: &["<Primary>o"] },
    Accel { action: "win.close", idle: &["<Primary>w"], typing: &["<Primary>w"] },
    Accel { action: "app.quit", idle: &["<Primary>q"], typing: &["<Primary>q"] },
    Accel { action: "win.zoom-in",
            idle: &["<Primary>plus", "<Primary>equal", "plus", "equal"],
            typing: &["<Primary>plus", "<Primary>equal"] },
    Accel { action: "win.zoom-out",
            idle: &["<Primary>minus", "minus"], typing: &["<Primary>minus"] },
    Accel { action: "win.zoom-fit", idle: &["<Primary>0", "0"], typing: &["<Primary>0"] },
    Accel { action: "win.zoom-actual", idle: &["<Primary>1", "1"], typing: &["<Primary>1"] },
    Accel { action: "win.rotate-left",
            idle: &["bracketleft", "<Primary>bracketleft"], typing: &["<Primary>bracketleft"] },
    Accel { action: "win.rotate-right",
            idle: &["bracketright", "<Primary>bracketright"], typing: &["<Primary>bracketright"] },
    Accel { action: "win.rotate-reset", idle: &["<Primary><Shift>r"], typing: &["<Primary><Shift>r"] },
    Accel { action: "win.next-image", idle: &["Right", "Page_Down", "space"], typing: &[] },
    Accel { action: "win.previous-image", idle: &["Left", "Page_Up", "BackSpace"], typing: &[] },
    Accel { action: "win.transform-open", idle: &["<Primary>t"], typing: &["<Primary>t"] },
    // Escape stays live while typing: it is the way out of the editor, and a
    // text box has nothing else to do with it.
    Accel { action: "win.dismiss", idle: &["Escape"], typing: &["Escape"] },
    Accel { action: "win.fullscreen", idle: &["F11", "<Primary>f"], typing: &["F11"] },
    // The one that would do real harm: Delete in a size box must delete a
    // digit, not the file.
    Accel { action: "win.delete", idle: &["Delete"], typing: &[] },
    Accel { action: "win.copy", idle: &["<Primary>c"], typing: &[] },
    Accel { action: "win.edit", idle: &["<Primary>e"], typing: &["<Primary>e"] },
    Accel { action: "win.resize", idle: &["<Primary>r"], typing: &["<Primary>r"] },
    Accel { action: "win.undo", idle: &["<Primary>z"], typing: &[] },
    Accel { action: "win.redo", idle: &["<Primary><Shift>z", "<Primary>y"], typing: &[] },
    Accel { action: "win.crop-apply", idle: &["Return", "KP_Enter"], typing: &[] },
    Accel { action: "win.save", idle: &["<Primary>s"], typing: &["<Primary>s"] },
    Accel { action: "win.save-as", idle: &["<Primary><Shift>s"], typing: &["<Primary><Shift>s"] },
    Accel { action: "win.flip-horizontal", idle: &["<Primary>h"], typing: &[] },
    Accel { action: "win.flip-vertical", idle: &["<Primary>j"], typing: &[] },
];

/// Swap the whole set over when focus moves into or out of a text box.
pub fn apply_accels(app: &adw::Application, typing: bool) {
    for accel in ACCELS {
        app.set_accels_for_action(accel.action, if typing { accel.typing } else { accel.idle });
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

        apply_accels(app, false);
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
