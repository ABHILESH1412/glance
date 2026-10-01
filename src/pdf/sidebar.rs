// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The page sidebar: every page small, with its number, for finding your way
//! around a long document.
//!
//! It costs nothing until it is opened: no thread, no thumbnails. Once open,
//! only the thumbnails on screen are drawn — a list view only builds rows for
//! what is visible — and they are drawn on a thread of their own, so the pages
//! being read are never kept waiting behind them.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use super::layout::Rotation;
use super::page::Page;
use super::render::{Job, Rendered, Renderer};
use super::view::texture;

/// How wide each thumbnail is drawn, in logical pixels.
const WIDTH: f64 = 120.0;
/// Thumbnails kept once they have scrolled out of sight, so scrolling back is
/// instant. Beyond this only the ones on screen are kept.
const CACHE: usize = 200;

pub struct Sidebar {
    inner: Rc<Inner>,
}

struct Inner {
    root: gtk::ScrolledWindow,
    list: gtk::ListView,
    model: gtk::StringList,
    selection: gtk::SingleSelection,
    pages: RefCell<Vec<(f64, f64)>>,
    rotation: Cell<Rotation>,
    uri: RefCell<Option<String>>,
    renderer: RefCell<Option<Renderer>>,
    /// Bumped per document, so thumbnails drawn for the last one are ignored.
    document: Cell<u64>,
    /// The version of the file being shown; see `Renderer::reload`.
    revision: Cell<u64>,
    cache: RefCell<HashMap<usize, gdk::Texture>>,
    /// Pages whose rows are built right now, and the widget showing each.
    bound: RefCell<HashMap<usize, Page>>,
    asked: Cell<bool>,
    /// Set while the reader moves the selection, so it is not mistaken for a
    /// click asking to go somewhere.
    syncing: Cell<bool>,
    /// On screen. Nothing is drawn until it is.
    active: Cell<bool>,
    night: Cell<bool>,
    on_pick: RefCell<Option<Box<dyn Fn(usize)>>>,
}

impl Sidebar {
    pub fn new() -> Self {
        let model = gtk::StringList::new(&[]);
        let selection = gtk::SingleSelection::new(Some(model.clone()));
        selection.set_can_unselect(false);
        let factory = gtk::SignalListItemFactory::new();
        let list = gtk::ListView::new(Some(selection.clone()), Some(factory.clone()));
        list.add_css_class("navigation-sidebar");
        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&list)
            .build();

        let inner = Rc::new(Inner {
            root,
            list,
            model,
            selection,
            pages: RefCell::default(),
            rotation: Cell::new(Rotation::default()),
            uri: RefCell::default(),
            renderer: RefCell::default(),
            document: Cell::new(0),
            revision: Cell::new(0),
            cache: RefCell::default(),
            bound: RefCell::default(),
            asked: Cell::new(false),
            syncing: Cell::new(false),
            active: Cell::new(false),
            night: Cell::new(false),
            on_pick: RefCell::default(),
        });

        factory.connect_setup(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return };
            let cell = gtk::Box::new(gtk::Orientation::Vertical, 6);
            cell.set_margin_top(6);
            cell.set_margin_bottom(6);
            cell.append(&Page::new());
            let number = gtk::Label::new(None);
            number.add_css_class("caption");
            number.add_css_class("numeric");
            cell.append(&number);
            item.set_child(Some(&cell));
        });

        let weak = Rc::downgrade(&inner);
        factory.connect_bind(move |_, item| {
            let Some(inner) = weak.upgrade() else { return };
            let Some((index, page, number)) = parts(item) else { return };
            number.set_text(&(index + 1).to_string());
            let (w, h) = inner.thumb_size(index);
            page.set_page_size(w, h);
            page.set_night(inner.night.get());
            page.set_texture(inner.cache.borrow().get(&index).cloned());
            inner.bound.borrow_mut().insert(index, page);
            inner.ask();
        });

        let weak = Rc::downgrade(&inner);
        factory.connect_unbind(move |_, item| {
            let Some(inner) = weak.upgrade() else { return };
            let Some((index, page, _)) = parts(item) else { return };
            let mut bound = inner.bound.borrow_mut();
            if bound.get(&index) == Some(&page) {
                bound.remove(&index);
            }
            page.set_texture(None);
        });

        let weak = Rc::downgrade(&inner);
        inner.selection.connect_selected_notify(move |selection| {
            let Some(inner) = weak.upgrade() else { return };
            if inner.syncing.get() {
                return;
            }
            let Ok(index) = usize::try_from(selection.selected()) else { return };
            let pick = inner.on_pick.borrow();
            if let Some(pick) = pick.as_ref() {
                pick(index);
            }
        });

        Sidebar { inner }
    }

    pub fn widget(&self) -> &gtk::ScrolledWindow {
        &self.inner.root
    }

    /// Called with a page index when the user picks a thumbnail.
    pub fn connect_pick(&self, pick: impl Fn(usize) + 'static) {
        self.inner.on_pick.replace(Some(Box::new(pick)));
    }

    pub fn show_document(&self, uri: String, pages: Vec<(f64, f64)>) {
        let inner = &self.inner;
        inner.clear();
        let count = pages.len();
        *inner.pages.borrow_mut() = pages;
        inner.uri.replace(Some(uri));
        let numbers: Vec<String> = (1..=count).map(|n| n.to_string()).collect();
        let numbers: Vec<&str> = numbers.iter().map(String::as_str).collect();
        inner.syncing.set(true);
        inner.model.splice(0, 0, &numbers);
        inner.selection.set_selected(0);
        inner.syncing.set(false);
        if inner.active.get() {
            inner.start();
        }
    }

    pub fn clear(&self) {
        self.inner.clear();
    }

    /// Pages turned in the reader are turned here too.
    pub fn set_rotation(&self, rotation: Rotation) {
        let inner = &self.inner;
        if inner.rotation.replace(rotation) == rotation {
            return;
        }
        inner.cache.borrow_mut().clear();
        for (&index, page) in inner.bound.borrow().iter() {
            let (w, h) = inner.thumb_size(index);
            page.set_page_size(w, h);
            page.set_texture(None);
        }
        inner.ask();
    }

    /// Light and dark swapped, as on the pages being read.
    pub fn set_night(&self, night: bool) {
        self.inner.night.set(night);
        for page in self.inner.bound.borrow().values() {
            page.set_night(night);
        }
    }

    /// Shown or hidden. Showing it for the first time is what starts it
    /// drawing anything at all.
    pub fn set_active(&self, active: bool) {
        let inner = &self.inner;
        inner.active.set(active);
        if active {
            inner.start();
            inner.scroll_to_selected();
        }
    }

    /// The file has been marked up: redraw these pages from it.
    pub fn reload(&self, pages: &[usize], revision: u64) {
        let inner = &self.inner;
        inner.revision.set(revision);
        // Not started yet: it opens the file as it is when it does start.
        match inner.renderer.borrow().as_ref() {
            Some(renderer) => renderer.reload(revision),
            None => return,
        }
        let mut cache = inner.cache.borrow_mut();
        for page in pages {
            cache.remove(page);
        }
        drop(cache);
        inner.ask();
    }

    /// Follow the reader: select the page being read, and bring it into view.
    pub fn set_current(&self, index: usize) {
        let inner = &self.inner;
        let Ok(position) = u32::try_from(index) else { return };
        if inner.selection.selected() == position {
            return;
        }
        inner.syncing.set(true);
        inner.selection.set_selected(position);
        inner.syncing.set(false);
        if inner.active.get() {
            inner.scroll_to_selected();
        }
    }
}

/// The page number, thumbnail and label of a list row.
fn parts(item: &glib::Object) -> Option<(usize, Page, gtk::Label)> {
    let item = item.downcast_ref::<gtk::ListItem>()?;
    let cell = item.child()?;
    let page = cell.first_child()?.downcast::<Page>().ok()?;
    let number = cell.last_child()?.downcast::<gtk::Label>().ok()?;
    Some((usize::try_from(item.position()).ok()?, page, number))
}

impl Inner {
    fn thumb_size(&self, index: usize) -> (i32, i32) {
        let (w, h) = self.pages.borrow().get(index).copied().unwrap_or((1.0, 1.0));
        let (w, h) = self.rotation.get().size(w, h);
        (WIDTH as i32, (WIDTH * h / w.max(1.0)).round().max(1.0) as i32)
    }

    /// Device pixels per point for a page's thumbnail.
    fn thumb_scale(&self, index: usize) -> f64 {
        let (w, h) = self.pages.borrow().get(index).copied().unwrap_or((1.0, 1.0));
        let (w, _) = self.rotation.get().size(w, h);
        let screen = self
            .root
            .native()
            .and_then(|native| native.surface())
            .map_or_else(|| f64::from(self.root.scale_factor()), |surface| surface.scale());
        WIDTH / w.max(1.0) * screen.max(1.0)
    }

    /// Start the drawing thread, the first time it is needed.
    fn start(self: &Rc<Self>) {
        if self.renderer.borrow().is_some() {
            return;
        }
        let Some(uri) = self.uri.borrow().clone() else { return };
        let (sender, receiver) = async_channel::bounded(8);
        self.renderer.replace(Some(Renderer::start(uri, self.revision.get(), sender)));
        let id = self.document.get();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(rendered) = receiver.recv().await {
                let Some(inner) = weak.upgrade() else { break };
                if inner.document.get() != id {
                    break;
                }
                inner.on_rendered(rendered);
            }
        });
        self.ask();
    }

    /// Ask for the thumbnails on screen that are missing. Rows bind one at a
    /// time as the list scrolls, so this waits until they have all bound and
    /// asks once.
    fn ask(self: &Rc<Self>) {
        if !self.active.get() || self.asked.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            let Some(inner) = weak.upgrade() else { return };
            inner.asked.set(false);
            let renderer = inner.renderer.borrow();
            let Some(renderer) = renderer.as_ref() else { return };
            let cache = inner.cache.borrow();
            let mut wanted: Vec<usize> =
                inner.bound.borrow().keys().copied().filter(|index| !cache.contains_key(index)).collect();
            wanted.sort_unstable();
            let rotation = inner.rotation.get();
            renderer.want(
                wanted
                    .into_iter()
                    .map(|page| Job { page, scale: inner.thumb_scale(page), rotation })
                    .collect(),
            );
        });
    }

    fn on_rendered(&self, rendered: Rendered) {
        if rendered.rotation != self.rotation.get()
            || rendered.requested != self.thumb_scale(rendered.page)
            || rendered.revision != self.revision.get()
        {
            return; // Drawn for a turn, a screen or a file that has since changed.
        }
        let texture = texture(rendered.pixels);
        if let Some(page) = self.bound.borrow().get(&rendered.page) {
            page.set_texture(Some(texture.clone()));
        }
        let mut cache = self.cache.borrow_mut();
        cache.insert(rendered.page, texture);
        if cache.len() > CACHE {
            let bound = self.bound.borrow();
            cache.retain(|index, _| bound.contains_key(index));
        }
    }

    fn scroll_to_selected(&self) {
        let selected = self.selection.selected();
        if selected != gtk::INVALID_LIST_POSITION {
            self.list.scroll_to(selected, gtk::ListScrollFlags::NONE, None);
        }
    }

    fn clear(&self) {
        self.renderer.replace(None);
        self.document.set(self.document.get() + 1);
        self.cache.borrow_mut().clear();
        self.bound.borrow_mut().clear();
        self.pages.borrow_mut().clear();
        self.uri.replace(None);
        self.revision.set(0);
        self.rotation.set(Rotation::default());
        self.syncing.set(true);
        self.model.splice(0, self.model.n_items(), &[]);
        self.syncing.set(false);
    }
}
