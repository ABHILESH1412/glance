// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

mod app;
mod images;
mod model3d;
mod pdf;

use app::window::Window;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

pub const APP_ID: &str = "io.github.abhilesh1412.Glance";

/// Where the chosen theme is remembered. A plain file rather than GSettings:
/// that would need a schema compiled and installed system-wide, which is a lot
/// of machinery for one word.
fn theme_file() -> std::path::PathBuf {
    glib::user_config_dir().join("glance").join("theme")
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
    Accel { action: "win.export", idle: &["<Primary><Shift>s"], typing: &["<Primary><Shift>s"] },
    Accel { action: "win.flip-horizontal", idle: &["<Primary>h"], typing: &[] },
    Accel { action: "win.flip-vertical", idle: &["<Primary>j"], typing: &[] },
    // Moving through a PDF. No keys of their own: they borrow them from the
    // folder navigation while a PDF is open, through `PDF_KEYS` below.
    Accel { action: "win.page-down", idle: &[], typing: &[] },
    Accel { action: "win.page-up", idle: &[], typing: &[] },
    Accel { action: "win.line-down", idle: &[], typing: &[] },
    Accel { action: "win.line-up", idle: &[], typing: &[] },
    Accel { action: "win.page-first", idle: &[], typing: &[] },
    Accel { action: "win.page-last", idle: &[], typing: &[] },
    Accel { action: "win.scroll-left", idle: &[], typing: &[] },
    Accel { action: "win.scroll-right", idle: &[], typing: &[] },
    Accel { action: "win.show-pages", idle: &["F9"], typing: &["F9"] },
    // Finding text in a PDF: keys only while one is open, through `PDF_KEYS`;
    // F3 also works from inside the search box.
    Accel { action: "win.find", idle: &[], typing: &[] },
    Accel { action: "win.find-next", idle: &[], typing: &["F3"] },
    Accel { action: "win.find-previous", idle: &[], typing: &["<Shift>F3"] },
    // Marking up a PDF's text, and taking it back: keys only while one is
    // open, through `PDF_KEYS`.
    Accel { action: "win.mark-highlight", idle: &[], typing: &[] },
    Accel { action: "win.mark-underline", idle: &[], typing: &[] },
    Accel { action: "win.mark-strike", idle: &[], typing: &[] },
    Accel { action: "win.undo-mark", idle: &[], typing: &[] },
    Accel { action: "win.redo-mark", idle: &[], typing: &[] },
    Accel { action: "win.document-info", idle: &[], typing: &[] },
    Accel { action: "win.show-shortcuts", idle: &["<Primary>question"], typing: &["<Primary>question"] },
];

/// While a PDF is open, the keys every reader uses to move through a document
/// go to the document rather than the folder. A PDF is read on its own, so
/// nothing moves to another file, and Delete does not remove the document
/// being read. Left and Right scroll sideways: left without a job, GTK would
/// use them to walk keyboard focus into the page box, after which Space and
/// Backspace would type into it instead of turning pages.
const PDF_KEYS: &[(&str, &[&str])] = &[
    ("win.next-image", &[]),
    ("win.previous-image", &[]),
    ("win.delete", &[]),
    ("win.page-down", &["Page_Down", "space"]),
    ("win.page-up", &["Page_Up", "BackSpace", "<Shift>space"]),
    ("win.line-down", &["Down"]),
    ("win.line-up", &["Up"]),
    ("win.page-first", &["Home"]),
    ("win.page-last", &["End"]),
    ("win.scroll-left", &["Left"]),
    ("win.scroll-right", &["Right"]),
    // Ctrl+F finds, as in every reader and browser; F11 still goes fullscreen.
    ("win.fullscreen", &["F11"]),
    ("win.find", &["<Primary>f"]),
    ("win.find-next", &["F3", "<Primary>g"]),
    ("win.find-previous", &["<Shift>F3", "<Primary><Shift>g"]),
    // Ctrl+H highlights, as in Preview, rather than flipping a picture that
    // is not on screen; Ctrl+Z undoes marks, not image edits.
    ("win.flip-horizontal", &[]),
    ("win.flip-vertical", &[]),
    ("win.undo", &[]),
    ("win.redo", &[]),
    ("win.mark-highlight", &["<Primary>h"]),
    ("win.mark-underline", &["<Primary>u"]),
    ("win.mark-strike", &["<Primary><Shift>x"]),
    ("win.undo-mark", &["<Primary>z"]),
    ("win.redo-mark", &["<Primary><Shift>z", "<Primary>y"]),
    ("win.document-info", &["<Primary>i"]),
];

/// An action's keys, while typing or not, with a PDF open or not.
fn keys_for(accel: &Accel, typing: bool, pdf: bool) -> &'static [&'static str] {
    if typing {
        accel.typing
    } else if pdf {
        PDF_KEYS.iter().find(|(action, _)| *action == accel.action).map_or(accel.idle, |(_, keys)| keys)
    } else {
        accel.idle
    }
}

/// Every key bound when nothing is being typed, for a picture or a PDF: what
/// the Keyboard Shortcuts window is checked against.
#[cfg(test)]
pub(crate) fn bound_keys(pdf: bool) -> Vec<&'static str> {
    ACCELS.iter().flat_map(|accel| keys_for(accel, false, pdf).iter().copied()).collect()
}

/// Swap the whole set over when focus moves into or out of a text box, or
/// between an image and a PDF.
pub fn apply_accels(app: &adw::Application, typing: bool, pdf: bool) {
    for accel in ACCELS {
        app.set_accels_for_action(accel.action, keys_for(accel, typing, pdf));
    }
}

fn main() -> glib::ExitCode {
    // The icons, compiled into the binary by build.rs. GtkApplication looks for
    // icons under the resource path it derives from the application ID, so once
    // this is registered the program finds its own icon whether or not it has
    // been installed. Without it, an uninstalled build shows the broken-image
    // placeholder in the About dialog rather than the app's icon.
    gio::resources_register_include!("glance.gresource")
        .expect("the icons are compiled into the binary by build.rs");

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

        apply_accels(app, false, false);
    });

    app.connect_activate(|app| {
        Window::new(app).present();
    });

    app.connect_open(|app, files, _hint| {
        let window = Window::new(app);
        if let Some(file) = files.first() {
            window.open_file(file);
        }
        window.present();
    });

    app.run()
}
