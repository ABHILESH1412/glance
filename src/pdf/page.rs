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
/// Where a note being dragged will land.
const GHOST: gdk::RGBA = gdk::RGBA::new(0.208, 0.518, 0.894, 0.2);
const GHOST_EDGE: gdk::RGBA = gdk::RGBA::new(0.208, 0.518, 0.894, 0.9);

/// Night mode: light and dark swapped, but colours kept. Plain inversion turns
/// red ink cyan and a photograph into its negative; this inverts, then turns
/// the hue back half a circle, so white paper goes black, black text goes
/// white, and red stays red.
///
/// GSK multiplies a row of colour by the matrix, so row `i` here is what input
/// channel `i` gives each output channel: the transpose of the usual form.
fn night() -> (graphene::Matrix, graphene::Vec4) {
    #[rustfmt::skip]
    let matrix = graphene::Matrix::from_float([
         0.574, -0.426, -0.426, 0.0,
        -1.430, -0.430, -1.430, 0.0,
        -0.144, -0.144,  0.856, 0.0,
         0.0,    0.0,    0.0,   1.0,
    ]);
    (matrix, graphene::Vec4::new(1.0, 1.0, 1.0, 0.0))
}

/// A drawing picked up with the Select tool, in fractions of the page as it
/// is shown, so it stays put however the page is zoomed.
#[derive(Clone, Debug, PartialEq)]
pub struct Picked {
    /// Where it is being dragged to, drawn over the page until it is
    /// dropped there; empty while it sits still.
    pub strokes: Vec<Vec<(f64, f64)>>,
    pub paint: gdk::RGBA,
    /// Line thickness, as a share of the page's width.
    pub width: f64,
    /// The box round it, x, y, width, height, for anything but a line.
    pub frame: Option<[f64; 4]>,
    pub grips: Vec<(f64, f64)>,
}

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
        /// Where a note or speech bubble being dragged would land.
        pub ghost: RefCell<Option<[f64; 4]>>,
        /// The chosen text box's outline, as fractions of the page.
        pub outline: RefCell<Option<[f64; 4]>>,
        /// A drawing in progress, in the widget's own pixels, until the page
        /// is redrawn with it saved.
        pub sketch: RefCell<Option<crate::images::edit::draw::Mark>>,
        pub night: Cell<bool>,
        /// Areas marked for redaction, as fractions of the page.
        pub redactions: RefCell<Vec<[f64; 4]>>,
        pub picked: RefCell<Option<Picked>>,
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
            let night = self.night.get();
            if night {
                let (matrix, offset) = super::night();
                snapshot.push_color_matrix(&matrix, &offset);
            }
            snapshot.append_color(&gdk::RGBA::WHITE, &bounds);
            if let Some(texture) = self.texture.borrow().as_ref() {
                snapshot.append_scaled_texture(texture, gsk::ScalingFilter::Linear, &bounds);
            }
            // Part of the page it is drawn on, so turned dark with it at night.
            let sketch = self.sketch.borrow();
            let redacting = sketch.as_ref().filter(|mark| mark.tool == crate::images::edit::draw::Tool::Redact);
            if let Some(mark) = sketch.as_ref().filter(|_| redacting.is_none()) {
                if let Some(path) = mark.path() {
                    snapshot.append_stroke(&path, &mark.stroke(), &mark.paint());
                }
            }
            let picked = self.picked.borrow();
            if let Some(picked) = picked.as_ref().filter(|p| !p.strokes.is_empty()) {
                let builder = gsk::PathBuilder::new();
                for stroke in &picked.strokes {
                    let Some((first, rest)) = stroke.split_first() else { continue };
                    builder.move_to((first.0 as f32) * w, (first.1 as f32) * h);
                    if rest.is_empty() {
                        builder.line_to((first.0 as f32) * w, (first.1 as f32) * h);
                    }
                    for p in rest {
                        builder.line_to((p.0 as f32) * w, (p.1 as f32) * h);
                    }
                }
                let stroke = gsk::Stroke::new(((picked.width as f32) * w).max(0.5));
                stroke.set_line_cap(gsk::LineCap::Round);
                stroke.set_line_join(gsk::LineJoin::Round);
                snapshot.append_stroke(&builder.to_path(), &stroke, &picked.paint);
            }
            if night {
                snapshot.pop();
            }
            if let Some(picked) = picked.as_ref() {
                let (fw, fh) = (f64::from(w), f64::from(h));
                let frame = picked.frame.map(|[x, y, rw, rh]| [x * fw, y * fh, rw * fw, rh * fh]);
                let grips: Vec<(f64, f64)> = picked.grips.iter().map(|&(x, y)| (x * fw, y * fh)).collect();
                crate::images::edit::shape::append_picked(snapshot, frame, &grips);
            }
            drop(picked);
            // Marked for redaction, over everything, the same by day and night.
            let scale = |[x, y, rw, rh]: [f64; 4]| {
                graphene::Rect::new((x as f32) * w, (y as f32) * h, (rw as f32) * w, (rh as f32) * h)
            };
            for area in self.redactions.borrow().iter() {
                crate::images::edit::draw::append_redaction(snapshot, &scale(*area));
            }
            if let Some((x, y, rw, rh)) = redacting.and_then(|mark| mark.rect()) {
                let area = graphene::Rect::new(x as f32, y as f32, rw as f32, rh as f32);
                crate::images::edit::draw::append_redaction(snapshot, &area);
            }
            drop(sketch);
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
            if let Some([x, y, rw, rh]) = *self.outline.borrow() {
                let area = graphene::Rect::new((x as f32) * w - 3.0, (y as f32) * h - 3.0, (rw as f32) * w + 6.0, (rh as f32) * h + 6.0);
                snapshot.append_border(&gsk::RoundedRect::from_rect(area, 3.0), &[1.5; 4], &[GHOST_EDGE; 4]);
            }
            if let Some([x, y, rw, rh]) = *self.ghost.borrow() {
                let area = graphene::Rect::new((x as f32) * w, (y as f32) * h, (rw as f32) * w, (rh as f32) * h);
                snapshot.append_color(&GHOST, &area);
                snapshot.append_border(&gsk::RoundedRect::from_rect(area, 2.0), &[1.5; 4], &[GHOST_EDGE; 4]);
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

    /// Swap light and dark, for reading at night.
    pub fn set_night(&self, night: bool) {
        if self.imp().night.replace(night) != night {
            self.queue_draw();
        }
    }

    /// Outline where something being dragged would land, as fractions of the
    /// page; `None` once it is dropped.
    pub fn set_ghost(&self, ghost: Option<[f64; 4]>) {
        if *self.imp().ghost.borrow() != ghost {
            self.imp().ghost.replace(ghost);
            self.queue_draw();
        }
    }

    /// Outline the chosen text box; `None` to take the outline away.
    pub fn set_outline(&self, outline: Option<[f64; 4]>) {
        if *self.imp().outline.borrow() != outline {
            self.imp().outline.replace(outline);
            self.queue_draw();
        }
    }

    /// Show a drawing being made, in this widget's own pixels.
    pub fn set_sketch(&self, sketch: Option<crate::images::edit::draw::Mark>) {
        let had = self.imp().sketch.replace(sketch).is_some();
        if had || self.imp().sketch.borrow().is_some() {
            self.queue_draw();
        }
    }

    /// Show a drawing picked up, or `None` once it is put down.
    pub fn set_picked(&self, picked: Option<Picked>) {
        if *self.imp().picked.borrow() != picked {
            self.imp().picked.replace(picked);
            self.queue_draw();
        }
    }

    /// Areas to show as selected, each x, y, width, height as fractions of
    /// the page.
    pub fn set_redactions(&self, areas: Vec<[f64; 4]>) {
        if *self.imp().redactions.borrow() == areas {
            return;
        }
        self.imp().redactions.replace(areas);
        self.queue_draw();
    }

    pub fn set_highlights(&self, highlights: Vec<[f64; 4]>) {
        let had_any = !self.imp().highlights.borrow().is_empty();
        if had_any || !highlights.is_empty() {
            self.imp().highlights.replace(highlights);
            self.queue_draw();
        }
    }
}
