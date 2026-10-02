// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Keyboard Shortcuts window: every key Glance answers to, in one place,
//! instead of beside every menu item.
//!
//! Three pages — General, Pictures, PDFs — each in a few small groups, with a
//! search box over them all. The list is written out here, and a test checks
//! that every key it shows is one Glance really has, so it cannot quietly
//! drift out of date.

use adw::prelude::*;
use gtk::glib;

pub struct Shortcut {
    pub what: &'static str,
    /// GTK accelerators; more than one, space-separated, for alternatives.
    pub keys: &'static str,
    /// Handled by the window's own key table, and so checked against it. The
    /// rest belong to one box: the search bar, the note editor.
    #[cfg_attr(not(test), allow(dead_code))]
    pub window_key: bool,
}

pub struct Group {
    pub title: &'static str,
    pub shortcuts: &'static [Shortcut],
}

pub struct Page {
    pub name: &'static str,
    pub title: &'static str,
    pub icon: &'static str,
    /// Which set of keys is live on this page: `None` for both.
    #[cfg_attr(not(test), allow(dead_code))]
    pub pdf: Option<bool>,
    pub groups: &'static [Group],
}

const fn key(what: &'static str, keys: &'static str) -> Shortcut {
    Shortcut { what, keys, window_key: true }
}

const fn local(what: &'static str, keys: &'static str) -> Shortcut {
    Shortcut { what, keys, window_key: false }
}

pub const PAGES: &[Page] = &[
    Page {
        name: "general",
        title: "General",
        icon: "preferences-system-symbolic",
        pdf: None,
        groups: &[
            Group {
                title: "Files and Windows",
                shortcuts: &[
                    key("Open a file", "<Primary>o"),
                    key("Close the window", "<Primary>w"),
                    key("Quit", "<Primary>q"),
                    key("Print", "<Primary>p"),
                    key("Keyboard shortcuts", "<Primary>question"),
                ],
            },
            Group {
                title: "View",
                shortcuts: &[
                    key("Zoom in", "plus <Primary>plus"),
                    key("Zoom out", "minus <Primary>minus"),
                    key("Fit to the window", "0 <Primary>0"),
                    key("Actual size", "1 <Primary>1"),
                    key("Rotate left", "bracketleft"),
                    key("Rotate right", "bracketright"),
                    key("Fullscreen", "F11"),
                    key("Leave fullscreen, or close a panel", "Escape"),
                ],
            },
            Group {
                title: "Combining into a PDF",
                shortcuts: &[
                    local("Add files", "<Primary>o"),
                    local("Turn the selected pages", "bracketleft bracketright"),
                    local("Take out the selected pages", "Delete"),
                    local("Move the selected pages along", "<Primary>Left <Primary>Right"),
                    local("Look at a page across the window", "Return"),
                    local("Back to all the pages", "Escape"),
                    local("Undo", "<Primary>z"),
                    local("Redo", "<Primary><Shift>z <Primary>y"),
                    local("Save as PDF", "<Primary>s"),
                ],
            },
        ],
    },
    Page {
        name: "images",
        title: "Pictures",
        icon: "image-x-generic-symbolic",
        pdf: Some(false),
        groups: &[
            Group {
                title: "Moving Around",
                shortcuts: &[
                    key("Next picture", "Right Page_Down space"),
                    key("Previous picture", "Left Page_Up BackSpace"),
                ],
            },
            Group {
                title: "Looking",
                shortcuts: &[
                    key("Slideshow", "F5"),
                    local("Pause or carry on the slideshow", "space"),
                    key("Image info", "<Primary>i <Alt>Return"),
                    key("Frames of an animation", "F9"),
                    key("Previous frame", "comma"),
                    key("Next frame", "period"),
                    key("Play or pause an animation", "k"),
                ],
            },
            Group {
                title: "Editing",
                shortcuts: &[
                    key("Edit", "<Primary>e"),
                    key("Rotate and flip", "<Primary>t"),
                    key("Resize", "<Primary>r"),
                    key("Reset the rotation", "<Primary><Shift>r"),
                    key("Flip horizontally", "<Primary>h"),
                    key("Flip vertically", "<Primary>j"),
                    key("Apply the crop", "Return"),
                    key("Undo", "<Primary>z"),
                    key("Redo", "<Primary><Shift>z <Primary>y"),
                ],
            },
            Group {
                title: "File",
                shortcuts: &[
                    key("Save", "<Primary>s"),
                    key("Export a copy", "<Primary><Shift>s"),
                    key("Copy the picture", "<Primary>c"),
                    key("Delete the picture", "Delete"),
                ],
            },
        ],
    },
    Page {
        name: "pdfs",
        title: "PDFs",
        icon: "x-office-document-symbolic",
        pdf: Some(true),
        groups: &[
            Group {
                title: "Moving Around",
                shortcuts: &[
                    key("Next screen", "Page_Down space"),
                    key("Previous screen", "Page_Up BackSpace"),
                    key("Scroll down a little", "Down"),
                    key("Scroll up a little", "Up"),
                    key("Scroll sideways", "Left Right"),
                    key("First page", "Home"),
                    key("Last page", "End"),
                    key("Sidebar: pages, contents and bookmarks", "F9"),
                    key("Bookmark this page", "<Primary>d"),
                ],
            },
            Group {
                title: "Finding Text",
                shortcuts: &[
                    key("Find", "<Primary>f"),
                    key("Next match", "F3 <Primary>g"),
                    key("Previous match", "<Shift>F3 <Primary><Shift>g"),
                    local("Next or previous match, in the search box", "Return <Shift>Return"),
                ],
            },
            Group {
                title: "Marking Up",
                shortcuts: &[
                    key("Copy selected text", "<Primary>c"),
                    key("Highlight", "<Primary>h"),
                    key("Underline", "<Primary>u"),
                    key("Strike through", "<Primary><Shift>x"),
                    key("Mark for redaction", "<Primary><Shift>r"),
                    key("Edit: draw, write and redact", "<Primary>e"),
                    local("Finish a note", "<Primary>Return"),
                    key("Undo", "<Primary>z"),
                    key("Redo", "<Primary><Shift>z <Primary>y"),
                ],
            },
            Group {
                title: "Document",
                shortcuts: &[key("Document info", "<Primary>i")],
            },
        ],
    },
];

/// Open the window, on the PDFs page if one is being read.
pub fn present(parent: &impl IsA<gtk::Widget>, pdf: bool) {
    let stack = adw::ViewStack::new();
    let search = gtk::SearchEntry::builder().placeholder_text("Search shortcuts").hexpand(true).build();
    // Every row, what it is searched by, its group, and its page.
    let mut rows: Vec<(adw::ActionRow, String, adw::PreferencesGroup, &'static str)> = Vec::new();

    for page in PAGES {
        let column = gtk::Box::new(gtk::Orientation::Vertical, 24);
        column.set_margin_top(12);
        column.set_margin_bottom(24);
        column.set_margin_start(12);
        column.set_margin_end(12);
        for group in page.groups {
            let widget = adw::PreferencesGroup::builder().title(group.title).build();
            for shortcut in group.shortcuts {
                let row = adw::ActionRow::builder().title(shortcut.what).build();
                let label = gtk::ShortcutLabel::new(shortcut.keys);
                label.set_valign(gtk::Align::Center);
                row.add_suffix(&label);
                widget.add(&row);
                let haystack = format!("{} {}", shortcut.what, readable(shortcut.keys)).to_lowercase();
                rows.push((row, haystack, widget.clone(), page.name));
            }
            column.append(&widget);
        }
        let clamp = adw::Clamp::builder().maximum_size(560).child(&column).build();
        let scroller = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&clamp).build();
        stack.add_titled_with_icon(&scroller, Some(page.name), page.title, page.icon);
    }
    stack.set_visible_child_name(if pdf { "pdfs" } else { "images" });

    // Rows that do not match are hidden, and groups left empty with them.
    // If nothing on the page in view matches, the first page that has a
    // match comes forward, so a search never looks empty when it is not.
    search.connect_search_changed(glib::clone!(
        #[weak]
        stack,
        move |entry| {
            let query = entry.text().to_lowercase();
            let words: Vec<&str> = query.split_whitespace().collect();
            let mut shown: Vec<(adw::PreferencesGroup, bool)> = Vec::new();
            let mut pages_with_matches: Vec<&str> = Vec::new();
            for (row, haystack, group, page) in &rows {
                let visible = words.iter().all(|word| haystack.contains(word));
                row.set_visible(visible);
                if visible && !pages_with_matches.contains(page) {
                    pages_with_matches.push(page);
                }
                match shown.iter_mut().find(|(g, _)| g == group) {
                    Some((_, any)) => *any |= visible,
                    None => shown.push((group.clone(), visible)),
                }
            }
            for (group, any) in shown {
                group.set_visible(any);
            }
            let current = stack.visible_child_name().map(|n| n.to_string()).unwrap_or_default();
            if !pages_with_matches.contains(&current.as_str()) {
                if let Some(first) = pages_with_matches.first() {
                    stack.set_visible_child_name(first);
                }
            }
        }
    ));

    let switcher = adw::ViewSwitcher::builder().stack(&stack).policy(adw::ViewSwitcherPolicy::Wide).build();
    let header = adw::HeaderBar::builder().title_widget(&switcher).build();
    let search_row = adw::Clamp::builder().maximum_size(560).child(&search).build();
    search_row.set_margin_start(12);
    search_row.set_margin_end(12);
    search_row.set_margin_bottom(6);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.add_top_bar(&search_row);
    toolbar.set_content(Some(&stack));
    let dialog = adw::Dialog::builder()
        .title("Keyboard Shortcuts")
        .content_width(600)
        .content_height(680)
        .child(&toolbar)
        .build();
    // Escape clears the search first, then closes the window.
    search.connect_stop_search(glib::clone!(
        #[weak]
        dialog,
        move |entry| {
            if entry.text().is_empty() {
                dialog.close();
            } else {
                entry.set_text("");
            }
        }
    ));
    dialog.present(Some(parent));
    search.grab_focus();
}

/// Accelerators as words, for searching: "<Primary>h" as "ctrl+h".
fn readable(keys: &str) -> String {
    keys.split_whitespace()
        .filter_map(|accel| gtk::accelerator_parse(accel).map(|(key, mods)| gtk::accelerator_get_label(key, mods).to_string()))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An accelerator as GTK reads it. The C call needs no display; the Rust
    /// wrapper insists on one, which a test does not have.
    fn parse(accel: &str) -> (u32, u32) {
        let text = std::ffi::CString::new(accel).unwrap();
        let (mut key, mut mods) = (0u32, 0u32);
        // SAFETY: both out-pointers are valid for the call.
        unsafe { gtk::ffi::gtk_accelerator_parse(text.as_ptr(), &mut key, &mut mods) };
        assert!(key != 0, "{accel} does not parse");
        (key, mods)
    }

    /// Every key the window shows is one that is really bound, in the mode
    /// the page is about.
    #[test]
    fn every_shortcut_shown_is_really_there() {
        for page in PAGES {
            let modes: &[bool] = match page.pdf {
                Some(true) => &[true],
                Some(false) => &[false],
                None => &[false, true],
            };
            for group in page.groups {
                for shortcut in group.shortcuts.iter().filter(|s| s.window_key) {
                    for accel in shortcut.keys.split_whitespace() {
                        for &pdf in modes {
                            let bound = crate::bound_keys(pdf).iter().any(|k| parse(k) == parse(accel));
                            assert!(bound, "{accel:?} for “{}” is not bound with pdf = {pdf}", shortcut.what);
                        }
                    }
                }
            }
        }
    }
}
