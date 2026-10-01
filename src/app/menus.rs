// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The main menu, one for pictures and one for PDFs.
//!
//! Kept short and calm: the most used things as a row of icons at the top,
//! the rest in a few small groups, and no shortcut beside every item — those
//! all live in the Keyboard Shortcuts window instead, where they can be read
//! together.

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::pdf;

/// The colours offered for highlighting, by name.
pub const HIGHLIGHTS: &[(&str, &str)] = &[
    ("Yellow", "#ffe400"),
    ("Green", "#7ee36b"),
    ("Blue", "#6ec6ff"),
    ("Pink", "#ff8ad8"),
    ("Purple", "#c7a3ff"),
    ("Orange", "#ffa94d"),
];

/// A plain menu item, without its shortcut written beside it.
fn item(label: &str, action: &str) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), Some(action));
    // An empty accelerator in place of the one GTK would look up.
    item.set_attribute_value("accel", Some(&"".to_variant()));
    item
}

/// One button in a row of icons; the label becomes its tooltip.
fn icon(label: &str, action: &str, icon_name: &str) -> gio::MenuItem {
    let item = item(label, action);
    item.set_attribute_value("verb-icon", Some(&icon_name.to_variant()));
    item
}

/// Items laid out as a row of icon buttons.
fn row(items: &[gio::MenuItem]) -> gio::Menu {
    let section = gio::Menu::new();
    for item in items {
        section.append_item(item);
    }
    section
}

fn as_row(section: &gio::Menu, label: Option<&str>) -> gio::MenuItem {
    let item = gio::MenuItem::new_section(label, section);
    item.set_attribute_value("display-hint", Some(&"horizontal-buttons".to_variant()));
    item
}

fn section(items: &[gio::MenuItem]) -> gio::Menu {
    row(items)
}

/// A filled circle of a colour, as an icon.
fn swatch(hex: &str) -> Option<glib::Variant> {
    let svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='16' height='16'>\
         <circle cx='8' cy='8' r='7' fill='{hex}' stroke='#00000040' stroke-width='1'/></svg>"
    );
    gio::BytesIcon::new(&glib::Bytes::from_owned(svg.into_bytes())).serialize()
}

fn appearance() -> gio::Menu {
    let themes = gio::Menu::new();
    themes.append_item(&item("Follow _System", "app.theme::system"));
    themes.append_item(&item("_Light", "app.theme::light"));
    themes.append_item(&item("_Dark", "app.theme::dark"));
    themes
}

fn closing() -> gio::Menu {
    section(&[item("_Keyboard Shortcuts", "win.show-shortcuts"), item("_About Glance", "win.about")])
}

pub fn image_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    menu.append_item(&as_row(
        &row(&[
            icon("Zoom Out", "win.zoom-out", "zoom-out-symbolic"),
            icon("Fit to Window", "win.zoom-fit", "zoom-fit-best-symbolic"),
            icon("Zoom In", "win.zoom-in", "zoom-in-symbolic"),
        ]),
        None,
    ));
    menu.append_section(
        None,
        &section(&[item("_Edit…", "win.edit"), item("_Save", "win.save"), item("_Export…", "win.export")]),
    );
    menu.append_section(None, &section(&[item("_Copy Image", "win.copy"), item("_Delete Image…", "win.delete")]));
    let turns = section(&[
        item("Rotate and _Flip…", "win.transform-open"),
        item("Rotate _Left", "win.rotate-left"),
        item("Rotate _Right", "win.rotate-right"),
        item("Flip _Horizontally", "win.flip-horizontal"),
        item("Flip _Vertically", "win.flip-vertical"),
        item("Reset Rotation", "win.rotate-reset"),
    ]);
    let view = section(&[item("_Actual Size", "win.zoom-actual"), item("_Fullscreen", "win.fullscreen")]);
    view.append_submenu(Some("Rotate and _Flip"), &turns);
    view.append_submenu(Some("_Appearance"), &appearance());
    menu.append_section(None, &view);
    menu.append_section(None, &closing());
    menu
}

pub fn pdf_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    // What is reached for most, as icons, so the header can stay clear.
    menu.append_item(&as_row(
        &row(&[
            icon("Zoom Out", "win.zoom-out", "zoom-out-symbolic"),
            icon("Fit", "win.zoom-fit", "zoom-fit-best-symbolic"),
            icon("Zoom In", "win.zoom-in", "zoom-in-symbolic"),
            icon("Rotate Left", "win.rotate-left", "object-rotate-left-symbolic"),
            icon("Rotate Right", "win.rotate-right", "object-rotate-right-symbolic"),
            icon("Fullscreen", "win.fullscreen", "view-fullscreen-symbolic"),
        ]),
        None,
    ));

    // The highlighter's colour, as colours rather than names.
    let colours = gio::Menu::new();
    for &(name, hex) in HIGHLIGHTS {
        let Some(rgb) = pdf::Rgb::from_hex(hex) else { continue };
        let colour = item(name, "win.highlight-colour");
        colour.set_action_and_target_value(Some("win.highlight-colour"), Some(&rgb.hex().to_variant()));
        if let Some(dot) = swatch(hex) {
            colour.set_attribute_value("verb-icon", Some(&dot));
        }
        colours.append_item(&colour);
    }
    colours.append_item(&icon("Other Colour…", "win.pick-highlight-colour", "list-add-symbolic"));
    menu.append_item(&as_row(&colours, Some("Highlight Colour")));

    // Marking the document up, then finding the way round it.
    menu.append_section(
        None,
        &section(&[
            item("_Draw and Write", "win.draw-panel"),
            item("Add _Note", "win.pin::note"),
            item("Add Speech _Bubble", "win.pin::bubble"),
        ]),
    );
    menu.append_section(
        None,
        &section(&[
            item("_Find…", "win.find"),
            item("Show _Sidebar", "win.show-pages"),
            item("Book_mark This Page", "win.bookmark"),
            item("Document _Info", "win.document-info"),
        ]),
    );

    let layouts = gio::Menu::new();
    layouts.append_item(&item("_Continuous Scroll", "win.pdf-layout::continuous"));
    layouts.append_item(&item("_Single Page", "win.pdf-layout::single"));
    layouts.append_item(&item("_Two Pages", "win.pdf-layout::double"));
    let view = section(&[item("_Night Mode", "win.night-mode")]);
    view.append_submenu(Some("Page _Layout"), &layouts);
    view.append_submenu(Some("_Appearance"), &appearance());
    menu.append_section(None, &view);
    menu.append_section(None, &closing());
    menu
}
