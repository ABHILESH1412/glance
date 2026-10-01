// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a password-protected PDF shows until it is unlocked: a padlock, a
//! password box and an Unlock button, in the window rather than a dialog, so
//! nothing has to be dismissed to do something else instead.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

pub struct LockedPage {
    pub root: adw::StatusPage,
    entry: gtk::PasswordEntry,
    on_unlock: Rc<RefCell<Option<Box<dyn Fn(String)>>>>,
}

impl LockedPage {
    pub fn new() -> Self {
        let entry = gtk::PasswordEntry::builder()
            .show_peek_icon(true)
            .placeholder_text("Password")
            .activates_default(false)
            .width_request(260)
            .build();
        let unlock = gtk::Button::builder()
            .label("_Unlock")
            .use_underline(true)
            .css_classes(["pill", "suggested-action"])
            .halign(gtk::Align::Center)
            .sensitive(false)
            .build();
        let column = gtk::Box::new(gtk::Orientation::Vertical, 18);
        column.set_halign(gtk::Align::Center);
        column.append(&entry);
        column.append(&unlock);
        let root = adw::StatusPage::builder()
            .icon_name("system-lock-screen-symbolic")
            .title("This Document Is Locked")
            .child(&column)
            .build();

        let on_unlock: Rc<RefCell<Option<Box<dyn Fn(String)>>>> = Rc::default();
        let submit = {
            let on_unlock = on_unlock.clone();
            move |entry: &gtk::PasswordEntry| {
                let password = entry.text().to_string();
                if password.is_empty() {
                    return;
                }
                if let Some(callback) = on_unlock.borrow().as_ref() {
                    callback(password);
                }
            }
        };
        entry.connect_changed(glib::clone!(
            #[weak]
            unlock,
            move |entry| {
                unlock.set_sensitive(!entry.text().is_empty());
                entry.remove_css_class("error");
            }
        ));
        let on_activate = submit.clone();
        entry.connect_activate(on_activate);
        unlock.connect_clicked(glib::clone!(
            #[weak]
            entry,
            move |_| submit(&entry)
        ));

        LockedPage { root, entry, on_unlock }
    }

    /// Called with the password typed, when the reader asks to unlock.
    pub fn connect_unlock(&self, f: impl Fn(String) + 'static) {
        self.on_unlock.replace(Some(Box::new(f)));
    }

    /// Ask for the password to `name`; `tried` after one that did not work.
    pub fn show(&self, name: &str, tried: bool) {
        self.root.set_description(Some(&if tried {
            "That password did not open it. Check it and try again.".to_string()
        } else {
            format!("“{}” needs a password to open.", glib::markup_escape_text(name))
        }));
        if tried {
            self.entry.add_css_class("error");
            // The wrong one stays, selected, so it can be fixed or typed over.
            self.entry.select_region(0, -1);
        } else {
            self.entry.set_text("");
        }
        self.entry.grab_focus();
    }
}
