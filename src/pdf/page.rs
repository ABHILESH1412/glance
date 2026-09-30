// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! One page on screen, in the reader and in the sidebar.
//!
//! Exactly the size it is told to be, whatever it is showing, with the drawn
//! page stretched to fill it. A `gtk::Picture` would size itself from its
//! texture instead, and a texture drawn for a HiDPI screen is twice the size it
//! should appear.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

/// Selected text: Adwaita's blue, see-through so the text stays readable.
const SELECTION: gdk::RGBA = gdk::RGBA::new(0.208, 0.518, 0.894, 0.35);
/// Every search match: a highlighter yellow.
const MATCH: gdk::RGBA = gdk::RGBA::new(1.0, 0.8, 0.0, 0.35);
/// The match being looked at: stronger, and orange, so it stands out from the
/// rest at a glance.
const CURRENT: gdk::RGBA = gdk::RGBA::new(1.0, 0.45, 0.0, 0.55);

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Page {
        pub size: Cell<(i32, i32)>,
        pub texture: RefCell<Option<gdk::Texture>>,
        /// Selected areas, as fractions of the page's width and height, so
        /// they stay put however the page is zoomed.
        pub highlights: RefCell<Vec<[f64; 4]>>,
        /// Search matches on this page, and the current one if it is here, in
        /// the same fractions.
        pub matches: RefCell<Vec<[f64; 4]>>,
        pub current: RefCell<Vec<[f64; 4]>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Page {
        const NAME: &'static str = "GlancePdfPage";
        type Type = super::Page;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Page {}

    impl WidgetImpl for Page {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let (w, h) = self.size.get();
            let size = if orientation == gtk::Orientation::Horizontal { w } else { h };
            (size, size, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (w, h) = (widget.width() as f32, widget.height() as f32);
            let bounds = graphene::Rect::new(0.0, 0.0, w, h);
            // A soft edge, so a white page still reads as paper on a light window.
            snapshot.append_outset_shadow(
                &gsk::RoundedRect::from_rect(bounds, 0.0),
                &gdk::RGBA::new(0.0, 0.0, 0.0, 0.3),
                0.0,
                1.0,
                0.0,
                4.0,
            );
            snapshot.append_color(&gdk::RGBA::WHITE, &bounds);
            if let Some(texture) = self.texture.borrow().as_ref() {
                snapshot.append_scaled_texture(texture, gsk::ScalingFilter::Linear, &bounds);
            }
            // Matches under the selection, so selecting a found word shows as
            // selected.
            for (areas, colour) in [
                (&self.matches, &MATCH),
                (&self.current, &CURRENT),
                (&self.highlights, &SELECTION),
            ] {
                for [x, y, rw, rh] in areas.borrow().iter() {
                    let area = graphene::Rect::new(
                        (*x as f32) * w,
                        (*y as f32) * h,
                        (*rw as f32) * w,
                        (*rh as f32) * h,
                    );
                    snapshot.append_color(colour, &area);
                }
            }
        }
    }
}

glib::wrapper! {
    pub struct Page(ObjectSubclass<imp::Page>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Page {
    pub fn new() -> Self {
        // Centred, not stretched: a vertical box widens every child to its
        // widest one, which would squash a portrait page to the width of a
        // landscape page beside it.
        glib::Object::builder().property("halign", gtk::Align::Center).build()
    }

    pub fn set_page_size(&self, width: i32, height: i32) {
        if self.imp().size.replace((width, height)) != (width, height) {
            self.queue_resize();
        }
    }

    pub fn set_texture(&self, texture: Option<gdk::Texture>) {
        self.imp().texture.replace(texture);
        self.queue_draw();
    }

    /// Search matches to mark, and the current one if it is on this page,
    /// each x, y, width, height as fractions of the page.
    pub fn set_matches(&self, matches: Vec<[f64; 4]>, current: Vec<[f64; 4]>) {
        let imp = self.imp();
        let had_any = !imp.matches.borrow().is_empty() || !imp.current.borrow().is_empty();
        if had_any || !matches.is_empty() || !current.is_empty() {
            imp.matches.replace(matches);
            imp.current.replace(current);
            self.queue_draw();
        }
    }

    /// Areas to show as selected, each x, y, width, height as fractions of
    /// the page.
    pub fn set_highlights(&self, highlights: Vec<[f64; 4]>) {
        let had_any = !self.imp().highlights.borrow().is_empty();
        if had_any || !highlights.is_empty() {
            self.imp().highlights.replace(highlights);
            self.queue_draw();
        }
    }
}
