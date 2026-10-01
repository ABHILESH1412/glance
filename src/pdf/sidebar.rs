// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The sidebar beside a PDF: three ways round the document, one tab each.
//!
//! - **Pages**, every page small;
//! - **Contents**, the author's table of contents;
//! - **Bookmarks**, the pages the reader has marked.
//!
//! Only what is on screen does any work: the thumbnails draw nothing while
//! another tab is showing, or the sidebar is closed.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use gtk::prelude::*;

use super::bookmark_list::{BookmarkList, Request};
use super::bookmarks::Bookmark;
use super::contents::Contents;
use super::layout::Rotation;
use super::outline::Heading;
use super::thumbnails::Thumbnails;

/// Which tab the sidebar shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Pages,
    Contents,
    Bookmarks,
}

impl View {
    pub fn name(self) -> &'static str {
        match self {
            View::Pages => "pages",
            View::Contents => "contents",
            View::Bookmarks => "bookmarks",
        }
    }

    pub fn from_name(name: &str) -> Option<View> {
        match name {
            "pages" => Some(View::Pages),
            "contents" => Some(View::Contents),
            "bookmarks" => Some(View::Bookmarks),
            _ => None,
        }
    }
}

/// What the reader did in the sidebar.
pub enum Event {
    /// Go to a page, and so far down it, in points.
    Go(usize, Option<f64>),
    Bookmarks(Request),
    Switched(View),
}

pub struct Sidebar {
    inner: Rc<Inner>,
}

struct Inner {
    root: gtk::Box,
    stack: adw::ViewStack,
    thumbnails: Thumbnails,
    contents: Contents,
    bookmarks: BookmarkList,
    /// On screen at all.
    shown: Cell<bool>,
    /// Set while the tab is changed from outside, so it is not reported back.
    syncing: Cell<bool>,
    on_event: RefCell<Option<Rc<dyn Fn(Event)>>>,
}

impl Sidebar {
    pub fn new() -> Self {
        let thumbnails = Thumbnails::new();
        let contents = Contents::new();
        let bookmarks = BookmarkList::new();

        let stack = adw::ViewStack::new();
        stack.set_vexpand(true);
        stack.add_titled_with_icon(thumbnails.widget(), Some("pages"), "Pages", "view-paged-symbolic");
        stack.add_titled_with_icon(contents.widget(), Some("contents"), "Contents", "view-list-bullet-symbolic");
        stack.add_titled_with_icon(bookmarks.widget(), Some("bookmarks"), "Bookmarks", "user-bookmarks-symbolic");

        let switcher = adw::ViewSwitcher::builder().stack(&stack).policy(adw::ViewSwitcherPolicy::Narrow).build();
        switcher.add_css_class("sidebar-switcher");
        switcher.set_margin_top(6);
        switcher.set_margin_bottom(6);
        switcher.set_margin_start(6);
        switcher.set_margin_end(6);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&switcher);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        root.append(&stack);

        let inner = Rc::new(Inner {
            root,
            stack,
            thumbnails,
            contents,
            bookmarks,
            shown: Cell::new(false),
            syncing: Cell::new(false),
            on_event: RefCell::default(),
        });

        let weak = Rc::downgrade(&inner);
        inner.thumbnails.connect_pick(move |page| {
            if let Some(inner) = weak.upgrade() {
                inner.emit(Event::Go(page, None));
            }
        });
        let weak = Rc::downgrade(&inner);
        inner.contents.connect_pick(move |page, y| {
            if let Some(inner) = weak.upgrade() {
                inner.emit(Event::Go(page, y));
            }
        });
        let weak = Rc::downgrade(&inner);
        inner.bookmarks.connect_request(move |request| {
            if let Some(inner) = weak.upgrade() {
                inner.emit(Event::Bookmarks(request));
            }
        });
        let weak = Rc::downgrade(&inner);
        inner.stack.connect_visible_child_name_notify(move |_| {
            let Some(inner) = weak.upgrade() else { return };
            inner.wake();
            if !inner.syncing.get() {
                inner.emit(Event::Switched(inner.view()));
            }
        });

        Sidebar { inner }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.inner.root
    }

    pub fn connect_event(&self, f: impl Fn(Event) + 'static) {
        self.inner.on_event.replace(Some(Rc::new(f)));
    }

    pub fn view(&self) -> View {
        self.inner.view()
    }

    pub fn set_view(&self, view: View) {
        let inner = &self.inner;
        inner.syncing.set(true);
        inner.stack.set_visible_child_name(view.name());
        inner.syncing.set(false);
    }

    /// Whether the bookmarks are on screen right now.
    pub fn showing_bookmarks(&self) -> bool {
        self.inner.shown.get() && self.view() == View::Bookmarks
    }

    pub fn show_document(&self, uri: String, pages: Vec<(f64, f64)>, outline: &[Heading]) {
        let inner = &self.inner;
        inner.thumbnails.show_document(uri, pages);
        inner.contents.show(outline);
        inner.bookmarks.show(&[], 0);
        inner.wake();
    }

    pub fn clear(&self) {
        let inner = &self.inner;
        inner.thumbnails.clear();
        inner.contents.clear();
        inner.bookmarks.show(&[], 0);
    }

    pub fn set_bookmarks(&self, marks: &[Bookmark], current: usize) {
        let inner = &self.inner;
        inner.bookmarks.show(marks, current);
        inner.thumbnails.set_bookmarked(marks.iter().map(|m| m.page).collect::<HashSet<_>>());
    }

    /// Shown or hidden. Nothing is drawn while hidden.
    pub fn set_active(&self, active: bool) {
        self.inner.shown.set(active);
        self.inner.wake();
    }

    /// Follow the reader, on every tab.
    pub fn set_current(&self, page: usize) {
        let inner = &self.inner;
        inner.thumbnails.set_current(page);
        inner.contents.set_current(page);
        inner.bookmarks.set_current(page);
    }

    pub fn set_rotation(&self, rotation: Rotation) {
        self.inner.thumbnails.set_rotation(rotation);
    }

    pub fn set_night(&self, night: bool) {
        self.inner.thumbnails.set_night(night);
    }

    /// The file has been marked up: redraw these pages' thumbnails.
    pub fn reload(&self, pages: &[usize], revision: u64) {
        self.inner.thumbnails.reload(pages, revision);
    }
}

impl Inner {
    fn view(&self) -> View {
        self.stack.visible_child_name().and_then(|name| View::from_name(&name)).unwrap_or_default()
    }

    /// The thumbnails draw only while they can be seen.
    fn wake(&self) {
        self.thumbnails.set_active(self.shown.get() && self.view() == View::Pages);
    }

    fn emit(&self, event: Event) {
        let callback = self.on_event.borrow().clone();
        if let Some(callback) = callback {
            callback(event);
        }
    }
}
