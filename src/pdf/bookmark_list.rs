// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The sidebar's Bookmarks view: the pages marked to come back to, each with
//! a name and its page number. A click goes there; each row's menu, or a
//! right-click, renames or removes it.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use super::bookmarks::Bookmark;

/// What the reader asked of the list.
pub enum Request {
    Go(usize),
    Add,
    Rename(usize, String),
    Remove(usize),
}

pub struct BookmarkList {
    inner: Rc<Inner>,
}

struct Inner {
    root: gtk::Stack,
    list: gtk::ListBox,
    add: gtk::Button,
    /// The rows, by page, in the order shown.
    rows: RefCell<Vec<(usize, gtk::ListBoxRow)>>,
    on_request: RefCell<Option<Rc<dyn Fn(Request)>>>,
}

impl BookmarkList {
    pub fn new() -> Self {
        let list = gtk::ListBox::new();
        list.add_css_class("navigation-sidebar");
        list.set_selection_mode(gtk::SelectionMode::None);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();

        let add = gtk::Button::builder()
            .child(&adw::ButtonContent::builder().icon_name("bookmark-new-symbolic").label("Bookmark This Page").build())
            .tooltip_text("Bookmark This Page (Ctrl+D)")
            .build();
        add.add_css_class("flat");
        let footer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        footer.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        add.set_margin_top(6);
        add.set_margin_bottom(6);
        add.set_margin_start(6);
        add.set_margin_end(6);
        footer.append(&add);
        let filled = gtk::Box::new(gtk::Orientation::Vertical, 0);
        filled.append(&scroller);
        filled.append(&footer);

        let empty_add = gtk::Button::builder()
            .label("Bookmark This Page")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        let empty = adw::StatusPage::builder()
            .icon_name("user-bookmarks-symbolic")
            .title("No Bookmarks")
            .description("Mark pages to come back to.\nCtrl+D marks the page you are on.")
            .child(&empty_add)
            .build();
        empty.add_css_class("compact");

        let root = gtk::Stack::new();
        root.add_named(&filled, Some("list"));
        root.add_named(&empty, Some("empty"));
        root.set_visible_child_name("empty");

        let inner = Rc::new(Inner {
            root,
            list,
            add: add.clone(),
            rows: RefCell::default(),
            on_request: RefCell::default(),
        });

        for button in [&add, &empty_add] {
            let weak = Rc::downgrade(&inner);
            button.connect_clicked(move |_| {
                if let Some(inner) = weak.upgrade() {
                    inner.request(Request::Add);
                }
            });
        }
        let weak = Rc::downgrade(&inner);
        inner.list.connect_row_activated(move |_, row| {
            let Some(inner) = weak.upgrade() else { return };
            if let Some(page) = inner.page_of(row) {
                inner.request(Request::Go(page));
            }
        });
        // Delete removes the bookmark whose row has focus.
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(&inner);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(inner) = weak.upgrade() else { return glib::Propagation::Proceed };
            if key != gdk::Key::Delete && key != gdk::Key::KP_Delete {
                return glib::Propagation::Proceed;
            }
            let focused = inner.list.focus_child().and_downcast::<gtk::ListBoxRow>();
            match focused.and_then(|row| inner.page_of(&row)) {
                Some(page) => {
                    inner.request(Request::Remove(page));
                    glib::Propagation::Stop
                }
                None => glib::Propagation::Proceed,
            }
        });
        inner.list.add_controller(keys);

        BookmarkList { inner }
    }

    pub fn widget(&self) -> &gtk::Stack {
        &self.inner.root
    }

    pub fn connect_request(&self, f: impl Fn(Request) + 'static) {
        self.inner.on_request.replace(Some(Rc::new(f)));
    }

    /// Show these bookmarks, picking out the one for the page being read.
    pub fn show(&self, marks: &[Bookmark], current: usize) {
        let inner = &self.inner;
        // Keep focus where it was when a row is renamed or one is added.
        let focused = inner.list.focus_child().and_downcast::<gtk::ListBoxRow>().and_then(|row| inner.page_of(&row));
        while let Some(child) = inner.list.first_child() {
            inner.list.remove(&child);
        }
        let mut rows = Vec::with_capacity(marks.len());
        for mark in marks {
            let row = inner.row(mark);
            inner.list.append(&row);
            rows.push((mark.page, row));
        }
        if let Some(row) = rows.iter().find(|(page, _)| Some(*page) == focused).map(|(_, row)| row.clone()) {
            row.grab_focus();
        }
        inner.rows.replace(rows);
        inner.root.set_visible_child_name(if marks.is_empty() { "empty" } else { "list" });
        self.set_current(current);
    }

    pub fn set_current(&self, page: usize) {
        let inner = &self.inner;
        let mut marked = false;
        for (at, row) in inner.rows.borrow().iter() {
            if *at == page {
                row.add_css_class("bookmark-current");
                marked = true;
            } else {
                row.remove_css_class("bookmark-current");
            }
        }
        // Nothing to add when the page is already marked.
        inner.add.set_sensitive(!marked);
    }
}

impl Inner {
    fn request(&self, request: Request) {
        let callback = self.on_request.borrow().clone();
        if let Some(callback) = callback {
            callback(request);
        }
    }

    fn page_of(&self, row: &gtk::ListBoxRow) -> Option<usize> {
        self.rows.borrow().iter().find(|(_, r)| r == row).map(|(page, _)| *page)
    }

    fn row(self: &Rc<Self>, mark: &Bookmark) -> gtk::ListBoxRow {
        let name = gtk::Label::builder()
            .label(&mark.name)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .tooltip_text(&mark.name)
            .build();
        let page = gtk::Label::builder().label(format!("Page {}", mark.page + 1)).xalign(0.0).build();
        page.add_css_class("dim-label");
        page.add_css_class("caption");
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        text.append(&name);
        text.append(&page);

        let menu = gio::Menu::new();
        menu.append(Some("_Rename…"), Some("bookmark.rename"));
        menu.append(Some("_Remove"), Some("bookmark.remove"));
        let more = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .menu_model(&menu)
            .valign(gtk::Align::Center)
            .tooltip_text("More")
            .build();
        more.add_css_class("flat");
        more.add_css_class("circular");

        let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        content.append(&gtk::Image::from_icon_name("user-bookmarks-symbolic"));
        content.append(&text);
        content.append(&more);
        let row = gtk::ListBoxRow::builder().child(&content).build();
        row.add_css_class("bookmark-row");

        let actions = gio::SimpleActionGroup::new();
        let rename = gio::SimpleAction::new("rename", None);
        let weak = Rc::downgrade(self);
        let (at, current) = (mark.page, mark.name.clone());
        rename.connect_activate(glib::clone!(
            #[weak]
            row,
            move |_, _| {
                if let Some(inner) = weak.upgrade() {
                    inner.rename(&row, at, &current);
                }
            }
        ));
        actions.add_action(&rename);
        let remove = gio::SimpleAction::new("remove", None);
        let weak = Rc::downgrade(self);
        remove.connect_activate(move |_, _| {
            if let Some(inner) = weak.upgrade() {
                inner.request(Request::Remove(at));
            }
        });
        actions.add_action(&remove);
        row.insert_action_group("bookmark", Some(&actions));

        // A right-click opens the same menu.
        let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
        click.connect_pressed(glib::clone!(
            #[weak]
            more,
            move |gesture, _, _, _| {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                more.popup();
            }
        ));
        row.add_controller(click);
        row
    }

    /// Ask for a new name, starting from the old one.
    fn rename(self: &Rc<Self>, row: &gtk::ListBoxRow, page: usize, current: &str) {
        let entry = gtk::Entry::builder().text(current).activates_default(true).build();
        let dialog = adw::AlertDialog::builder()
            .heading("Rename Bookmark")
            .body(format!("Page {}", page + 1))
            .extra_child(&entry)
            .default_response("rename")
            .close_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", "_Cancel"), ("rename", "_Rename")]);
        dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
        // Nothing to save until there is a name to save.
        entry.connect_changed(glib::clone!(
            #[weak]
            dialog,
            move |entry| dialog.set_response_enabled("rename", !entry.text().trim().is_empty())
        ));
        let weak = Rc::downgrade(self);
        dialog.connect_response(Some("rename"), glib::clone!(
            #[weak]
            entry,
            move |_, _| {
                let name = entry.text().trim().to_string();
                if let (Some(inner), false) = (weak.upgrade(), name.is_empty()) {
                    inner.request(Request::Rename(page, name));
                }
            }
        ));
        dialog.present(Some(row));
        entry.grab_focus();
    }
}
