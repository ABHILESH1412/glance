// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The sidebar's Contents view: the document's table of contents as a tree,
//! opened as far as its author left it. A click goes to the heading; the one
//! being read is picked out as the reader moves.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use super::outline::Heading;

/// Titles longer than this, in characters, may not fit in the sidebar.
const LONG_TITLE: usize = 24;

/// A heading in the list, and its place in reading order.
struct Item {
    number: usize,
    heading: Heading,
}

pub struct Contents {
    inner: Rc<Inner>,
}

struct Inner {
    root: gtk::Stack,
    list: gtk::ListView,
    tree: RefCell<Option<gtk::TreeListModel>>,
    /// Every heading's page, in reading order, for finding the current one.
    pages: RefCell<Vec<Option<usize>>>,
    current: Cell<Option<usize>>,
    /// A heading just clicked stays the current one while its page is read,
    /// even if another heading starts on the same page.
    clicked: Cell<Option<(usize, usize)>>,
    /// Rows built right now, by heading.
    bound: RefCell<HashMap<usize, gtk::Widget>>,
    on_pick: RefCell<Option<Box<dyn Fn(usize, Option<f64>)>>>,
}

impl Contents {
    pub fn new() -> Self {
        let factory = gtk::SignalListItemFactory::new();
        let list = gtk::ListView::new(None::<gtk::NoSelection>, Some(factory.clone()));
        list.add_css_class("navigation-sidebar");
        list.set_single_click_activate(true);
        let scroller = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&list).build();

        let empty = adw::StatusPage::builder()
            .icon_name("view-list-bullet-symbolic")
            .title("No Contents")
            .description("This document has no table of contents.")
            .build();
        empty.add_css_class("compact");

        let root = gtk::Stack::new();
        root.add_named(&scroller, Some("list"));
        root.add_named(&empty, Some("empty"));
        root.set_visible_child_name("empty");

        let inner = Rc::new(Inner {
            root,
            list,
            tree: RefCell::default(),
            pages: RefCell::default(),
            current: Cell::new(None),
            clicked: Cell::new(None),
            bound: RefCell::default(),
            on_pick: RefCell::default(),
        });

        factory.connect_setup(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return };
            let title = gtk::Label::builder()
                .xalign(0.0)
                .hexpand(true)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .lines(2)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            title.add_css_class("contents-title");
            let page = gtk::Label::builder().xalign(1.0).valign(gtk::Align::Center).build();
            page.add_css_class("dim-label");
            page.add_css_class("caption");
            page.add_css_class("numeric");
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.append(&title);
            row.append(&page);
            let expander = gtk::TreeExpander::new();
            expander.set_child(Some(&row));
            item.set_child(Some(&expander));
        });

        let weak = Rc::downgrade(&inner);
        factory.connect_bind(move |_, item| {
            let Some(inner) = weak.upgrade() else { return };
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return };
            let Some(row) = item.item().and_downcast::<gtk::TreeListRow>() else { return };
            let Some(expander) = item.child().and_downcast::<gtk::TreeExpander>() else { return };
            expander.set_list_row(Some(&row));
            let Some(boxed) = row.item().and_downcast::<glib::BoxedAnyObject>() else { return };
            let entry = boxed.borrow::<Item>();
            let Some(content) = expander.child() else { return };
            let (Some(title), Some(page)) = (
                content.first_child().and_downcast::<gtk::Label>(),
                content.last_child().and_downcast::<gtk::Label>(),
            ) else {
                return;
            };
            title.set_text(&entry.heading.title);
            // The whole title, for one long enough to be cut short.
            let long = entry.heading.title.chars().count() > LONG_TITLE;
            title.set_tooltip_text(long.then_some(entry.heading.title.as_str()));
            page.set_text(&entry.heading.page().map(|p| (p + 1).to_string()).unwrap_or_default());
            set_current_style(&content, inner.current.get() == Some(entry.number));
            inner.bound.borrow_mut().insert(entry.number, content);
        });

        let weak = Rc::downgrade(&inner);
        factory.connect_unbind(move |_, item| {
            let Some(inner) = weak.upgrade() else { return };
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return };
            let Some(expander) = item.child().and_downcast::<gtk::TreeExpander>() else { return };
            let Some(content) = expander.child() else { return };
            inner.bound.borrow_mut().retain(|_, widget| widget != &content);
            expander.set_list_row(None);
        });

        let weak = Rc::downgrade(&inner);
        inner.list.connect_activate(move |_, position| {
            let Some(inner) = weak.upgrade() else { return };
            inner.activate(position);
        });

        Contents { inner }
    }

    pub fn widget(&self) -> &gtk::Stack {
        &self.inner.root
    }

    /// Called with a page and how far down it, when a heading is clicked.
    pub fn connect_pick(&self, pick: impl Fn(usize, Option<f64>) + 'static) {
        self.inner.on_pick.replace(Some(Box::new(pick)));
    }


    pub fn show(&self, outline: &[Heading]) {
        let inner = &self.inner;
        inner.clear();
        if outline.is_empty() {
            return;
        }
        let mut pages = Vec::new();
        let root = store(outline, 0, &mut pages);
        inner.pages.replace(pages);
        let tree = gtk::TreeListModel::new(root, false, false, |item| {
            let boxed = item.downcast_ref::<glib::BoxedAnyObject>()?;
            let entry = boxed.borrow::<Item>();
            if entry.heading.children.is_empty() {
                return None;
            }
            // Numbered on from the parent, the way `store` numbered them.
            let mut numbers = Vec::new();
            let children = store(&entry.heading.children, entry.number + 1, &mut numbers);
            Some(children.upcast())
        });
        // Open what the author left open.
        let mut i = 0;
        while i < tree.n_items() {
            if let Some(row) = tree.row(i) {
                let open = row
                    .item()
                    .and_downcast::<glib::BoxedAnyObject>()
                    .is_some_and(|boxed| boxed.borrow::<Item>().heading.open);
                if open {
                    row.set_expanded(true);
                }
            }
            i += 1;
        }
        inner.list.set_model(Some(&gtk::NoSelection::new(Some(tree.clone()))));
        inner.tree.replace(Some(tree));
        inner.root.set_visible_child_name("list");
    }

    pub fn clear(&self) {
        self.inner.clear();
    }

    /// Follow the reader: pick out the heading the page being read is under.
    pub fn set_current(&self, page: usize) {
        let inner = &self.inner;
        let pages = inner.pages.borrow();
        let current = match inner.clicked.get() {
            Some((number, at)) if at == page => Some(number),
            _ => {
                inner.clicked.set(None);
                pages.iter().rposition(|p| p.is_some_and(|p| p <= page))
            }
        };
        drop(pages);
        let before = inner.current.replace(current);
        if before == current {
            return;
        }
        let bound = inner.bound.borrow();
        for (number, widget) in bound.iter() {
            set_current_style(widget, Some(*number) == current);
        }
    }
}

impl Inner {
    fn activate(&self, position: u32) {
        let Some(tree) = self.tree.borrow().clone() else { return };
        let Some(row) = tree.row(position) else { return };
        let Some(boxed) = row.item().and_downcast::<glib::BoxedAnyObject>() else { return };
        let (number, target) = {
            let entry = boxed.borrow::<Item>();
            (entry.number, entry.heading.target)
        };
        match target {
            Some((page, y)) => {
                self.clicked.set(Some((number, page)));
                if let Some(pick) = self.on_pick.borrow().as_ref() {
                    pick(page, y);
                }
            }
            // A heading that leads nowhere is only there to hold others.
            None if row.is_expandable() => row.set_expanded(!row.is_expanded()),
            None => {}
        }
    }

    fn clear(&self) {
        self.list.set_model(None::<&gtk::NoSelection>);
        self.tree.replace(None);
        self.pages.borrow_mut().clear();
        self.bound.borrow_mut().clear();
        self.current.set(None);
        self.clicked.set(None);
        self.root.set_visible_child_name("empty");
    }
}

/// One level of the outline as a list, numbering every heading in reading
/// order from `first`, and noting each one's page.
fn store(headings: &[Heading], first: usize, pages: &mut Vec<Option<usize>>) -> gio::ListStore {
    let list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let mut number = first;
    for heading in headings {
        list.append(&glib::BoxedAnyObject::new(Item { number, heading: heading.clone() }));
        number = numbered(heading, number, pages);
    }
    list
}

/// Note a heading's page and all those under it; the number after them.
fn numbered(heading: &Heading, number: usize, pages: &mut Vec<Option<usize>>) -> usize {
    pages.push(heading.page());
    let mut next = number + 1;
    for child in &heading.children {
        next = numbered(child, next, pages);
    }
    next
}

fn set_current_style(row: &gtk::Widget, current: bool) {
    if current {
        row.add_css_class("contents-current");
    } else {
        row.remove_css_class("contents-current");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heading(title: &str, page: usize, children: Vec<Heading>) -> Heading {
        Heading { title: title.to_string(), target: Some((page, None)), open: true, children }
    }

    /// Children numbered from their parent land on the same numbers as the
    /// whole outline numbered at once, so the current heading is found again
    /// however far the tree is opened.
    #[test]
    fn headings_keep_their_numbers_at_every_level() {
        let outline = vec![
            heading("1", 0, vec![heading("1.1", 1, vec![heading("1.1.1", 2, vec![])]), heading("1.2", 3, vec![])]),
            heading("2", 4, vec![heading("2.1", 5, vec![])]),
        ];
        let mut pages = Vec::new();
        let _ = numbered(&outline[0], 0, &mut pages);
        let _ = numbered(&outline[1], pages.len(), &mut pages);
        assert_eq!(pages, (0..6).map(Some).collect::<Vec<_>>());

        // "1.2" is fifth in reading order: after 1, 1.1, 1.1.1 — number 3.
        let mut ignored = Vec::new();
        let children = store(&outline[0].children, 1, &mut ignored);
        let numbers: Vec<(usize, String)> = (0..children.n_items())
            .filter_map(|i| children.item(i).and_downcast::<glib::BoxedAnyObject>())
            .map(|b| {
                let item = b.borrow::<Item>();
                (item.number, item.heading.title.clone())
            })
            .collect();
        assert_eq!(numbers, vec![(1, "1.1".to_string()), (3, "1.2".to_string())]);
        assert_eq!(pages[3], Some(3), "and number 3 is on page 3, as 1.2 is");
    }
}
