// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The levels control: the picture's histogram, with the three handles under
//! it that set the black point, the midtones and the white point.
//!
//! What lies outside black and white is shaded, since it is about to become
//! pure black or pure white.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene};

use crate::images::edit::adjust::{Histogram, Levels};

/// The histogram's height and the strip of handles under it.
const GRAPH: f64 = 100.0;
const HANDLES: f64 = 14.0;
/// How near the pointer must be to pick a handle up.
const REACH: f64 = 12.0;
/// The closest black and white may come.
const GAP: f64 = 2.0 / 255.0;

/// Told the new levels whenever a handle moves.
type Changed = Box<dyn Fn(Levels)>;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Handle {
    Black,
    Mid,
    White,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct LevelsGraph {
        pub histogram: RefCell<Option<Histogram>>,
        /// 0 for all channels, then red, green, blue.
        pub channel: Cell<usize>,
        pub levels: Cell<Levels>,
        pub(super) dragging: Cell<Option<Handle>>,
        pub changed: RefCell<Option<Changed>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LevelsGraph {
        const NAME: &'static str = "GlanceLevelsGraph";
        type Type = super::LevelsGraph;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for LevelsGraph {
        fn constructed(&self) {
            self.parent_constructed();
            let widget = self.obj();
            widget.set_hexpand(true);
            widget.set_cursor_from_name(Some("pointer"));
            widget.update_property(&[gtk::accessible::Property::Label(
                "Histogram, with handles for the black point, midtones and white point",
            )]);
            let drag = gtk::GestureDrag::new();
            drag.connect_drag_begin(glib::clone!(
                #[weak]
                widget,
                move |_, x, _| widget.pick(x)
            ));
            drag.connect_drag_update(glib::clone!(
                #[weak]
                widget,
                move |gesture, dx, _| {
                    if let Some((x, _)) = gesture.start_point() {
                        widget.move_to(x + dx);
                    }
                }
            ));
            drag.connect_drag_end(glib::clone!(
                #[weak]
                widget,
                move |_, _, _| widget.imp().dragging.set(None)
            ));
            widget.add_controller(drag);
        }
    }

    impl WidgetImpl for LevelsGraph {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Horizontal => (160, 260, -1, -1),
                _ => {
                    let height = (GRAPH + HANDLES + 4.0) as i32;
                    (height, height, -1, -1)
                }
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            self.obj().draw(snapshot);
        }
    }
}

glib::wrapper! {
    pub struct LevelsGraph(ObjectSubclass<imp::LevelsGraph>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for LevelsGraph {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl LevelsGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_histogram(&self, histogram: Option<Histogram>) {
        self.imp().histogram.replace(histogram);
        self.queue_draw();
    }

    pub fn histogram(&self) -> Option<Histogram> {
        self.imp().histogram.borrow().clone()
    }

    /// Show this channel, with these levels on its handles.
    pub fn show(&self, channel: usize, levels: Levels) {
        self.imp().channel.set(channel);
        self.imp().levels.set(levels);
        self.queue_draw();
    }

    /// Told whenever a handle is dragged.
    pub fn connect_changed(&self, f: impl Fn(Levels) + 'static) {
        self.imp().changed.replace(Some(Box::new(f)));
    }

    /// The width the values 0..1 are spread over, and where it starts: inset
    /// by half a handle, so the end handles are whole.
    fn track(&self) -> (f64, f64) {
        let inset = HANDLES / 2.0;
        (inset, (f64::from(self.width()) - 2.0 * inset).max(1.0))
    }

    fn x_of(&self, value: f64) -> f64 {
        let (start, width) = self.track();
        start + value * width
    }

    fn value_at(&self, x: f64) -> f64 {
        let (start, width) = self.track();
        ((x - start) / width).clamp(0.0, 1.0)
    }

    fn pick(&self, x: f64) {
        let levels = self.imp().levels.get();
        let handles = [
            (Handle::Black, levels.black),
            (Handle::Mid, levels.midpoint()),
            (Handle::White, levels.white),
        ];
        // The nearest, and on a tie (black and white pushed together) the one
        // that can move the way the pointer is.
        let picked = handles
            .iter()
            .map(|&(handle, value)| (handle, (self.x_of(value) - x).abs()))
            .filter(|&(_, distance)| distance <= REACH)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(handle, _)| handle);
        self.imp().dragging.set(picked);
    }

    fn move_to(&self, x: f64) {
        let Some(handle) = self.imp().dragging.get() else { return };
        let mut levels = self.imp().levels.get();
        let value = self.value_at(x);
        match handle {
            // The midtones keep their gamma, so they ride along between.
            Handle::Black => levels.black = value.min(levels.white - GAP),
            Handle::White => levels.white = value.max(levels.black + GAP),
            Handle::Mid => levels.gamma = levels.gamma_for_midpoint(value),
        }
        if levels != self.imp().levels.get() {
            self.imp().levels.set(levels);
            self.queue_draw();
            if let Some(changed) = self.imp().changed.borrow().as_ref() {
                changed(levels);
            }
        }
    }

    fn draw(&self, snapshot: &gtk::Snapshot) {
        let width = f64::from(self.width());
        let foreground = self.color();
        let levels = self.imp().levels.get();
        let channel = self.imp().channel.get();
        let (start, track) = self.track();

        // The well the histogram sits in.
        let well = graphene::Rect::new(start as f32, 0.0, track as f32, GRAPH as f32);
        let mut faint = foreground;
        faint.set_alpha(0.06);
        snapshot.append_color(&faint, &well);

        let cairo = snapshot.append_cairo(&graphene::Rect::new(0.0, 0.0, width as f32, (GRAPH + HANDLES + 4.0) as f32));
        if let Some(histogram) = self.imp().histogram.borrow().as_ref() {
            // Square roots, so a picture with one huge spike (a white sky,
            // a black border) still shows the shape of everything else.
            // The end bins are left out of the scale for the same reason.
            let shown: Vec<(usize, (f64, f64, f64))> = match channel {
                0 => vec![(0, (0.9, 0.25, 0.25)), (1, (0.3, 0.8, 0.35)), (2, (0.3, 0.5, 0.95))],
                1 => vec![(0, (0.9, 0.25, 0.25))],
                2 => vec![(1, (0.3, 0.8, 0.35))],
                _ => vec![(2, (0.3, 0.5, 0.95))],
            };
            let tallest = shown
                .iter()
                .flat_map(|&(c, _)| histogram.counts[c][1..255].iter())
                .map(|&n| f64::from(n).sqrt())
                .fold(1.0, f64::max);
            let alpha = if shown.len() > 1 { 0.45 } else { 0.7 };
            for (c, (r, g, b)) in shown {
                cairo.move_to(start, GRAPH);
                for (value, &count) in histogram.counts[c].iter().enumerate() {
                    let height = (f64::from(count).sqrt() / tallest).min(1.0) * (GRAPH - 2.0);
                    let x0 = start + value as f64 / 256.0 * track;
                    let x1 = start + (value + 1) as f64 / 256.0 * track;
                    cairo.line_to(x0, GRAPH - height);
                    cairo.line_to(x1, GRAPH - height);
                }
                cairo.line_to(start + track, GRAPH);
                cairo.close_path();
                cairo.set_source_rgba(r, g, b, alpha);
                let _ = cairo.fill();
            }
        }

        // What will clip to black or white.
        cairo.set_source_rgba(0.0, 0.0, 0.0, 0.35);
        cairo.rectangle(start, 0.0, levels.black * track, GRAPH);
        cairo.rectangle(self.x_of(levels.white), 0.0, (1.0 - levels.white) * track, GRAPH);
        let _ = cairo.fill();

        let fg = |alpha: f64| {
            cairo.set_source_rgba(
                f64::from(foreground.red()),
                f64::from(foreground.green()),
                f64::from(foreground.blue()),
                alpha,
            )
        };
        // Where each handle's value is, up through the histogram.
        fg(0.35);
        cairo.set_line_width(1.0);
        for value in [levels.black, levels.midpoint(), levels.white] {
            let x = self.x_of(value).round() + 0.5;
            cairo.move_to(x, 0.0);
            cairo.line_to(x, GRAPH);
        }
        let _ = cairo.stroke();

        // The handles: black, grey and white, each outlined so it shows on
        // either theme.
        for (value, shade) in [(levels.black, 0.0), (levels.midpoint(), 0.5), (levels.white, 1.0)] {
            let x = self.x_of(value);
            let top = GRAPH + 2.0;
            cairo.move_to(x, top);
            cairo.line_to(x + HANDLES / 2.0, top + HANDLES);
            cairo.line_to(x - HANDLES / 2.0, top + HANDLES);
            cairo.close_path();
            cairo.set_source_rgb(shade, shade, shade);
            let _ = cairo.fill_preserve();
            fg(0.8);
            let _ = cairo.stroke();
        }
    }
}

/// The channels levels can work on, in the order `Adjustments::levels` has them.
pub const CHANNELS: [&str; 4] = ["RGB", "Red", "Green", "Blue"];
