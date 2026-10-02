// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! One page in the Combine into PDF grid, or in its full-window view: the
//! page's picture, on white, turned as it will be saved, with a stripe in
//! the colour of the file it came from.
//!
//! Turning is done here, as the page is drawn, so turning a page is instant
//! and its picture never has to be made again.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

/// How thick the stripe is, in pixels.
const STRIPE: f32 = 4.0;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct PageTile {
        pub texture: RefCell<Option<gdk::Texture>>,
        /// The page's own width and height, before turning.
        pub shape: Cell<(f64, f64)>,
        pub turn: Cell<u8>,
        pub stripe: Cell<Option<gdk::RGBA>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PageTile {
        const NAME: &'static str = "GlanceCombineTile";
        type Type = super::PageTile;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for PageTile {}

    impl WidgetImpl for PageTile {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (bw, bh) = (widget.width() as f32, widget.height() as f32);
            let (w, h) = self.shape.get();
            if bw < 2.0 || bh < 2.0 || w <= 0.0 || h <= 0.0 {
                return;
            }
            let turn = self.turn.get() % 4;
            // The page as it will stand, fitted into the widget.
            let (tw, th) = if turn % 2 == 1 { (h, w) } else { (w, h) };
            let scale = (f64::from(bw) / tw).min(f64::from(bh) / th);
            let (sw, sh) = ((tw * scale) as f32, (th * scale) as f32);
            let (x, y) = ((bw - sw) / 2.0, (bh - sh) / 2.0);
            let shown = graphene::Rect::new(x, y, sw, sh);
            snapshot.append_outset_shadow(
                &gsk::RoundedRect::from_rect(shown, 2.0),
                &gdk::RGBA::new(0.0, 0.0, 0.0, 0.3),
                0.0,
                1.0,
                0.0,
                4.0,
            );
            snapshot.append_color(&gdk::RGBA::WHITE, &shown);
            if let Some(texture) = self.texture.borrow().as_ref() {
                // Into the middle, turn, and draw the upright picture there.
                snapshot.save();
                snapshot.translate(&graphene::Point::new(x + sw / 2.0, y + sh / 2.0));
                snapshot.rotate(f32::from(turn) * 90.0);
                let (uw, uh) = ((w * scale) as f32, (h * scale) as f32);
                snapshot.append_scaled_texture(
                    texture,
                    gsk::ScalingFilter::Trilinear,
                    &graphene::Rect::new(-uw / 2.0, -uh / 2.0, uw, uh),
                );
                snapshot.restore();
            }
            if let Some(colour) = self.stripe.get() {
                snapshot.append_color(&colour, &graphene::Rect::new(x, y, sw, STRIPE.min(sh)));
            }
        }
    }
}

glib::wrapper! {
    pub struct PageTile(ObjectSubclass<imp::PageTile>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PageTile {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn set_shape(&self, width: f64, height: f64) {
        self.imp().shape.set((width, height));
        self.queue_draw();
    }

    pub fn set_turn(&self, turn: u8) {
        if self.imp().turn.replace(turn % 4) != turn % 4 {
            self.queue_draw();
        }
    }

    pub fn set_texture(&self, texture: Option<gdk::Texture>) {
        self.imp().texture.replace(texture);
        self.queue_draw();
    }

    pub fn set_stripe(&self, colour: Option<gdk::RGBA>) {
        self.imp().stripe.set(colour);
        self.queue_draw();
    }
}

impl Default for PageTile {
    fn default() -> Self {
        Self::new()
    }
}
