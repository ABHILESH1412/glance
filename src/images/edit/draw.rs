// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Drawing and annotation.
//!
//! A mark is kept as what it is — a tool, a few points, a colour and a width —
//! not as pixels, so it can be undone and the picture underneath is untouched
//! until something bakes it in. Preview and bake go through the same path and
//! the same renderer, for the same reason the text does: two implementations
//! of "what this looks like" will drift apart.

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

use crate::images::edit::text::Patch;

/// How see-through a highlighter is. Low enough to read what is under it.
pub(crate) const HIGHLIGHT_ALPHA: f32 = 0.35;
/// Arrow heads are sized from the line, with a floor so a thin arrow still
/// has a head worth seeing.
const HEAD_OF_WIDTH: f64 = 3.5;
const HEAD_MINIMUM: f64 = 10.0;
/// Half the angle the head opens to.
const HEAD_SPREAD: f64 = 0.44;
/// The circle-to-bezier constant: four curves of this reach are an ellipse to
/// within a fraction of a pixel.
const KAPPA: f64 = 0.552_284_749_83;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Pen,
    Highlighter,
    Line,
    Arrow,
    Rectangle,
    Ellipse,
    /// Black out an area for good. Shown see-through until it is applied,
    /// so what is under it can be checked first. Not in `TOOLS`: it has a
    /// section of its own in the editing panels, apart from the pens.
    Redact,
}

pub const TOOLS: &[Tool] = &[
    Tool::Pen,
    Tool::Highlighter,
    Tool::Line,
    Tool::Arrow,
    Tool::Rectangle,
    Tool::Ellipse,
];

/// How a marked-but-not-yet-applied redaction looks: dark enough to see it
/// is marked, light enough to read what it covers, edged in red.
pub const REDACTION_FILL: gdk::RGBA = gdk::RGBA::new(0.0, 0.0, 0.0, 0.55);
pub const REDACTION_EDGE: gdk::RGBA = gdk::RGBA::new(0.88, 0.11, 0.14, 1.0);

/// Draw a marked redaction over `area`.
pub fn append_redaction(snapshot: &gtk::Snapshot, area: &graphene::Rect) {
    snapshot.append_color(&REDACTION_FILL, area);
    snapshot.append_border(&gsk::RoundedRect::from_rect(*area, 0.0), &[1.5; 4], &[REDACTION_EDGE; 4]);
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Pen => "Pen",
            Tool::Highlighter => "Highlighter",
            Tool::Line => "Line",
            Tool::Arrow => "Arrow",
            Tool::Rectangle => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Redact => "Redact",
        }
    }

    /// Freehand tools keep every point the pointer visited. The rest need only
    /// where the drag started and where it has got to.
    pub fn freehand(self) -> bool {
        matches!(self, Tool::Pen | Tool::Highlighter)
    }
}

#[derive(Clone, Debug)]
pub struct Mark {
    pub tool: Tool,
    /// In display space: the image's own pixels after any flip and rotation,
    /// the frame the pointer is in.
    pub points: Vec<(f64, f64)>,
    pub colour: gdk::RGBA,
    pub width: f64,
    /// When this was drawn, so marks and text stack in the order they were
    /// made rather than by which list they live in.
    pub sequence: u64,
}

impl Mark {
    /// What colour actually goes down. A highlighter is the same colour, let
    /// through.
    pub fn paint(&self) -> gdk::RGBA {
        if self.tool == Tool::Highlighter {
            gdk::RGBA::new(
                self.colour.red(),
                self.colour.green(),
                self.colour.blue(),
                self.colour.alpha() * HIGHLIGHT_ALPHA,
            )
        } else {
            self.colour
        }
    }

    pub fn stroke(&self) -> gsk::Stroke {
        let stroke = gsk::Stroke::new(self.width as f32);
        // Round throughout: a freehand line made of segments would otherwise
        // show a notch at every change of direction.
        stroke.set_line_cap(gsk::LineCap::Round);
        stroke.set_line_join(gsk::LineJoin::Round);
        stroke
    }

    /// Add a point as a drag goes on. Shapes keep only the two that matter.
    pub fn extend(&mut self, point: (f64, f64)) {
        if self.tool.freehand() {
            // Skip points that have barely moved: a hundred of them in the
            // same place is a hundred segments to draw for nothing.
            if let Some(last) = self.points.last() {
                if (last.0 - point.0).abs() < 0.5 && (last.1 - point.1).abs() < 0.5 {
                    return;
                }
            }
            self.points.push(point);
        } else if self.points.len() < 2 {
            self.points.push(point);
        } else {
            let last = self.points.len() - 1;
            self.points[last] = point;
        }
    }

    /// Whether this is worth keeping: a click that never moved leaves a dot
    /// for the freehand tools and nothing at all for the shapes.
    pub fn is_worth_keeping(&self) -> bool {
        match self.points.as_slice() {
            [] => false,
            [_] => self.tool.freehand(),
            [first, .., last] => {
                self.tool.freehand()
                    || (first.0 - last.0).abs() > 1.0
                    || (first.1 - last.1).abs() > 1.0
            }
        }
    }

    pub fn path(&self) -> Option<gsk::Path> {
        let builder = gsk::PathBuilder::new();
        match self.tool {
            _ if self.tool.freehand() => {
                let (first, rest) = self.points.split_first()?;
                builder.move_to(first.0 as f32, first.1 as f32);
                if rest.is_empty() {
                    // A dot: a zero-length line with round caps is a circle.
                    builder.line_to(first.0 as f32, first.1 as f32);
                }
                for point in rest {
                    builder.line_to(point.0 as f32, point.1 as f32);
                }
            }
            Tool::Line | Tool::Arrow => {
                let (from, to) = self.ends()?;
                builder.move_to(from.0 as f32, from.1 as f32);
                builder.line_to(to.0 as f32, to.1 as f32);
                if self.tool == Tool::Arrow {
                    let (left, right) = self.head(from, to);
                    for wing in [left, right] {
                        builder.move_to(to.0 as f32, to.1 as f32);
                        builder.line_to(wing.0 as f32, wing.1 as f32);
                    }
                }
            }
            Tool::Rectangle | Tool::Redact => {
                let (x, y, w, h) = self.rect()?;
                builder.add_rect(&graphene::Rect::new(x as f32, y as f32, w as f32, h as f32));
            }
            Tool::Ellipse => {
                let (x, y, w, h) = self.rect()?;
                let (rx, ry) = (w / 2.0, h / 2.0);
                let (cx, cy) = (x + rx, y + ry);
                let (ox, oy) = (rx * KAPPA, ry * KAPPA);
                let p = |a: f64, b: f64| (a as f32, b as f32);
                builder.move_to(cx as f32, y as f32);
                let (a, b, c) = (p(cx + ox, y), p(x + w, cy - oy), p(x + w, cy));
                builder.cubic_to(a.0, a.1, b.0, b.1, c.0, c.1);
                let (a, b, c) = (p(x + w, cy + oy), p(cx + ox, y + h), p(cx, y + h));
                builder.cubic_to(a.0, a.1, b.0, b.1, c.0, c.1);
                let (a, b, c) = (p(cx - ox, y + h), p(x, cy + oy), p(x, cy));
                builder.cubic_to(a.0, a.1, b.0, b.1, c.0, c.1);
                let (a, b, c) = (p(x, cy - oy), p(cx - ox, y), p(cx, y));
                builder.cubic_to(a.0, a.1, b.0, b.1, c.0, c.1);
                builder.close();
            }
            _ => return None,
        }
        Some(builder.to_path())
    }

    /// The mark as polylines: what a PDF's ink annotation is made of. A
    /// curve is followed closely enough that the corners do not show.
    pub fn strokes(&self) -> Vec<Vec<(f64, f64)>> {
        const ELLIPSE_STEPS: usize = 72;
        match self.tool {
            _ if self.tool.freehand() => match self.points.as_slice() {
                [] => Vec::new(),
                // A dot: a line of no length, which round caps make round.
                [only] => vec![vec![*only, *only]],
                points => vec![points.to_vec()],
            },
            Tool::Line | Tool::Arrow => {
                let Some((from, to)) = self.ends() else { return Vec::new() };
                let mut strokes = vec![vec![from, to]];
                if self.tool == Tool::Arrow {
                    let (left, right) = self.head(from, to);
                    strokes.push(vec![left, to, right]);
                }
                strokes
            }
            Tool::Rectangle | Tool::Redact => {
                let Some((x, y, w, h)) = self.rect() else { return Vec::new() };
                vec![vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h), (x, y)]]
            }
            Tool::Ellipse => {
                let Some((x, y, w, h)) = self.rect() else { return Vec::new() };
                let (rx, ry) = (w / 2.0, h / 2.0);
                let around = (0..=ELLIPSE_STEPS).map(|i| {
                    let t = i as f64 / ELLIPSE_STEPS as f64 * std::f64::consts::TAU;
                    (x + rx + rx * t.cos(), y + ry + ry * t.sin())
                });
                vec![around.collect()]
            }
            _ => Vec::new(),
        }
    }

    fn ends(&self) -> Option<((f64, f64), (f64, f64))> {
        Some((*self.points.first()?, *self.points.last()?))
    }

    /// The two wing tips of an arrow head, behind the point.
    fn head(&self, from: (f64, f64), to: (f64, f64)) -> ((f64, f64), (f64, f64)) {
        let angle = (to.1 - from.1).atan2(to.0 - from.0);
        let length = (self.width * HEAD_OF_WIDTH).max(HEAD_MINIMUM);
        let wing = |offset: f64| {
            (
                to.0 - length * (angle + offset).cos(),
                to.1 - length * (angle + offset).sin(),
            )
        };
        (wing(-HEAD_SPREAD), wing(HEAD_SPREAD))
    }

    /// The rectangle a two-point shape spans, however it was dragged.
    pub fn rect(&self) -> Option<(f64, f64, f64, f64)> {
        let (from, to) = self.ends()?;
        Some((
            from.0.min(to.0),
            from.1.min(to.1),
            (to.0 - from.0).abs(),
            (to.1 - from.1).abs(),
        ))
    }

    /// Everything the mark covers, stroke and arrow head included.
    pub fn bounds(&self) -> Option<(f64, f64, f64, f64)> {
        if self.tool == Tool::Redact {
            return self.rect();
        }
        let mut xs: Vec<f64> = self.points.iter().map(|p| p.0).collect();
        let mut ys: Vec<f64> = self.points.iter().map(|p| p.1).collect();
        if xs.is_empty() {
            return None;
        }
        if self.tool == Tool::Arrow {
            if let Some((from, to)) = self.ends() {
                let (left, right) = self.head(from, to);
                for wing in [left, right] {
                    xs.push(wing.0);
                    ys.push(wing.1);
                }
            }
        }
        let min_x = xs.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_x = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let min_y = ys.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_y = ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        // Half the stroke falls outside the line, plus a pixel for the
        // anti-aliased edge.
        let margin = self.width / 2.0 + 1.0;
        Some((
            min_x - margin,
            min_y - margin,
            (max_x - min_x) + margin * 2.0,
            (max_y - min_y) + margin * 2.0,
        ))
    }

    /// The mark as render nodes, with its own top-left at the origin.
    pub fn to_node(&self) -> Option<gsk::RenderNode> {
        if self.tool == Tool::Redact {
            let (_, _, w, h) = self.rect()?;
            let snapshot = gtk::Snapshot::new();
            append_redaction(&snapshot, &graphene::Rect::new(0.0, 0.0, w as f32, h as f32));
            return snapshot.to_node();
        }
        let (x, y, _, _) = self.bounds()?;
        let path = self.path()?;
        let snapshot = gtk::Snapshot::new();
        snapshot.save();
        snapshot.translate(&graphene::Point::new(-x as f32, -y as f32));
        snapshot.append_stroke(&path, &self.stroke(), &self.paint());
        snapshot.restore();
        snapshot.to_node()
    }

    /// Draw the mark into pixels at image resolution.
    pub fn render(&self, widget: &impl IsA<gtk::Widget>) -> Option<Patch> {
        if self.tool == Tool::Redact {
            return self.redaction_patch();
        }
        let (x, y, width, height) = self.bounds()?;
        let (width, height) = (width.ceil().max(1.0), height.ceil().max(1.0));
        if width > 16384.0 || height > 16384.0 {
            return None;
        }
        let node = self.to_node()?;
        let renderer = widget.as_ref().native()?.renderer()?;
        let texture = renderer.render_texture(
            &node,
            Some(&graphene::Rect::new(0.0, 0.0, width as f32, height as f32)),
        );
        Some(Patch::from_texture(
            &texture,
            x.round() as i64,
            y.round() as i64,
        ))
    }
}

impl Mark {
    /// A redaction as pixels: solid, opaque black over every pixel the area
    /// touches and one more all round, built directly rather than drawn, so
    /// no soft edge can let a shade of what was under it through.
    fn redaction_patch(&self) -> Option<Patch> {
        let (x, y, w, h) = self.rect()?;
        let left = x.floor() - 1.0;
        let top = y.floor() - 1.0;
        let width = ((x + w).ceil() + 1.0 - left).max(1.0);
        let height = ((y + h).ceil() + 1.0 - top).max(1.0);
        if width > 65536.0 || height > 65536.0 {
            return None;
        }
        let pixels = image::RgbaImage::from_pixel(width as u32, height as u32, image::Rgba([0, 0, 0, 255]));
        Some(Patch { pixels, x: left as i64, y: top as i64, sequence: self.sequence })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_redaction_bakes_to_solid_black_past_its_edges() {
        let redact = mark(Tool::Redact, &[(10.4, 20.6), (30.2, 25.1)]);
        let patch = redact.redaction_patch().unwrap();
        assert_eq!((patch.x, patch.y), (9, 19), "rounded out, and a pixel more");
        assert_eq!(patch.pixels.dimensions(), (23, 8));
        assert!(patch.pixels.pixels().all(|p| p.0 == [0, 0, 0, 255]));
        // Laid over a picture, nothing of it shows through.
        let mut base = image::RgbaImage::from_pixel(64, 64, image::Rgba([200, 150, 100, 255]));
        crate::images::edit::text::composite(&mut base, &[patch]);
        for y in 19..27 {
            for x in 9..32 {
                assert_eq!(base.get_pixel(x, y).0, [0, 0, 0, 255], "at {x}, {y}");
            }
        }
    }

    fn mark(tool: Tool, points: &[(f64, f64)]) -> Mark {
        Mark {
            tool,
            points: points.to_vec(),
            colour: gdk::RGBA::new(1.0, 0.0, 0.0, 1.0),
            width: 8.0,
            sequence: 0,
        }
    }

    #[test]
    fn every_mark_can_be_traced_as_lines() {
        let line = mark(Tool::Line, &[(0.0, 0.0), (10.0, 0.0)]);
        assert_eq!(line.strokes(), vec![vec![(0.0, 0.0), (10.0, 0.0)]]);
        // An arrow is its shaft and its head.
        let arrow = mark(Tool::Arrow, &[(0.0, 0.0), (100.0, 0.0)]);
        let strokes = arrow.strokes();
        assert_eq!(strokes.len(), 2);
        assert_eq!(strokes[1][1], (100.0, 0.0));
        // A rectangle closes on itself, whichever way it was dragged.
        let rectangle = mark(Tool::Rectangle, &[(10.0, 20.0), (0.0, 0.0)]);
        let ring = &rectangle.strokes()[0];
        assert_eq!((ring[0], ring.len()), ((0.0, 0.0), 5));
        assert_eq!(ring.first(), ring.last());
        // Every point of an ellipse is on it.
        let ellipse = mark(Tool::Ellipse, &[(0.0, 0.0), (20.0, 10.0)]);
        for &(x, y) in &ellipse.strokes()[0] {
            let on = ((x - 10.0) / 10.0).powi(2) + ((y - 5.0) / 5.0).powi(2);
            assert!((on - 1.0).abs() < 1e-9, "({x}, {y})");
        }
        // A click with the pen is a dot.
        assert_eq!(mark(Tool::Pen, &[(3.0, 4.0)]).strokes(), vec![vec![(3.0, 4.0), (3.0, 4.0)]]);
    }

    /// A shape is two points however far the pointer travelled: keeping the
    /// whole drag would make a rectangle out of a scribble.
    #[test]
    fn a_shape_keeps_only_its_two_corners() {
        let mut rectangle = mark(Tool::Rectangle, &[]);
        for step in 0..20 {
            rectangle.extend((f64::from(step) * 5.0, f64::from(step) * 3.0));
        }
        assert_eq!(rectangle.points.len(), 2);
        assert_eq!(rectangle.points[0], (0.0, 0.0));
        assert_eq!(rectangle.points[1], (95.0, 57.0));
    }

    #[test]
    fn freehand_keeps_the_whole_path_but_not_the_still_moments() {
        let mut pen = mark(Tool::Pen, &[]);
        pen.extend((0.0, 0.0));
        pen.extend((0.1, 0.1)); // barely moved: not worth a segment
        pen.extend((10.0, 0.0));
        pen.extend((20.0, 5.0));
        assert_eq!(pen.points, vec![(0.0, 0.0), (10.0, 0.0), (20.0, 5.0)]);
    }

    /// The bounds have to cover the stroke, not just the centre line, or the
    /// baked patch clips its own edges.
    #[test]
    fn bounds_allow_for_the_thickness_of_the_line() {
        let line = mark(Tool::Line, &[(50.0, 50.0), (150.0, 50.0)]);
        let (x, y, w, h) = line.bounds().unwrap();
        assert!(x < 50.0 && y < 50.0, "got {x}, {y}");
        assert!(w > 100.0 && h > 8.0, "got {w} x {h}");
    }

    /// An arrow head sits behind the tip, and the bounds must include it.
    #[test]
    fn an_arrow_head_points_the_way_the_line_goes() {
        let arrow = mark(Tool::Arrow, &[(0.0, 0.0), (100.0, 0.0)]);
        let (left, right) = arrow.head((0.0, 0.0), (100.0, 0.0));
        assert!(left.0 < 100.0 && right.0 < 100.0, "head should trail the tip");
        // One either side of the line, whichever way round they come out.
        assert!(left.1 * right.1 < 0.0, "wings should open either side: {left:?} {right:?}");
        let (_, y, _, h) = arrow.bounds().unwrap();
        let (top, bottom) = (left.1.min(right.1), left.1.max(right.1));
        assert!(y < top, "bounds should cover the head");
        assert!(h > (bottom - top), "and its whole spread");
    }

    /// A click that never moved is a dot with a pen and nothing with a shape.
    #[test]
    fn a_drag_that_never_moved_only_counts_for_freehand() {
        assert!(mark(Tool::Pen, &[(5.0, 5.0)]).is_worth_keeping());
        assert!(!mark(Tool::Rectangle, &[(5.0, 5.0)]).is_worth_keeping());
        assert!(!mark(Tool::Line, &[(5.0, 5.0), (5.2, 5.1)]).is_worth_keeping());
        assert!(mark(Tool::Line, &[(5.0, 5.0), (80.0, 40.0)]).is_worth_keeping());
    }

    /// A highlighter is the chosen colour, let through; everything else goes
    /// down as picked.
    #[test]
    fn only_the_highlighter_is_see_through() {
        assert_eq!(mark(Tool::Pen, &[(0.0, 0.0)]).paint().alpha(), 1.0);
        let through = mark(Tool::Highlighter, &[(0.0, 0.0)]).paint();
        assert!(through.alpha() > 0.0 && through.alpha() < 0.5, "{}", through.alpha());
        assert_eq!(through.red(), 1.0);
    }
}

/// A button icon showing the very shape the tool draws.
///
/// Adwaita has no line, rectangle, ellipse or arrow icons, and a stand-in from
/// somewhere else in the set reads as the wrong thing. Drawing each tool's own
/// mark, with the same code that draws it on the picture, cannot be wrong.
mod icon {
    use super::*;

    mod imp {
        use super::*;
        use std::cell::Cell;

        #[derive(Default)]
        pub struct ToolIcon {
            pub tool: Cell<Option<Tool>>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for ToolIcon {
            const NAME: &'static str = "GlanceToolIcon";
            type Type = super::ToolIcon;
            type ParentType = gtk::Widget;
        }

        impl ObjectImpl for ToolIcon {}

        impl WidgetImpl for ToolIcon {
            fn snapshot(&self, snapshot: &gtk::Snapshot) {
                let Some(tool) = self.tool.get() else {
                    return;
                };
                let widget = self.obj();
                let (w, h) = (widget.width() as f64, widget.height() as f64);
                if w < 2.0 || h < 2.0 {
                    return;
                }
                // Follow the label's colour, so the icon dims and highlights
                // with the button like a real symbolic icon.
                let ink = widget.color();
                let sample = super::sample(tool, w, h);
                // A solid block: what a redaction leaves.
                if tool == Tool::Redact {
                    if let Some((x, y, rw, rh)) = sample.rect() {
                        snapshot.append_color(&ink, &graphene::Rect::new(x as f32, y as f32 + 2.0, rw as f32, rh as f32 - 4.0));
                    }
                    return;
                }
                let Some(path) = sample.path() else {
                    return;
                };
                snapshot.append_stroke(&path, &sample.stroke(), &ink);
            }
        }
    }

    glib::wrapper! {
        pub struct ToolIcon(ObjectSubclass<imp::ToolIcon>)
            @extends gtk::Widget,
            @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
    }

    impl ToolIcon {
        pub fn new(tool: Tool) -> Self {
            let icon: Self = glib::Object::new();
            icon.imp().tool.set(Some(tool));
            icon.set_size_request(18, 18);
            icon.set_valign(gtk::Align::Center);
            icon
        }
    }

    /// A miniature of what the tool makes, inset so the stroke has room.
    pub(super) fn sample(tool: Tool, w: f64, h: f64) -> Mark {
        let pad = 3.0;
        let (x0, y0, x1, y1) = (pad, pad, w - pad, h - pad);
        let points = match tool {
            // A squiggle, because that is what a pen leaves.
            Tool::Pen => vec![
                (x0, y1),
                (x0 + (x1 - x0) * 0.3, y0),
                (x0 + (x1 - x0) * 0.6, y1),
                (x1, y0),
            ],
            // One thick sweep, the way a marker goes down.
            Tool::Highlighter => vec![(x0, (y0 + y1) / 2.0), (x1, (y0 + y1) / 2.0)],
            Tool::Line | Tool::Arrow => vec![(x0, y1), (x1, y0)],
            Tool::Rectangle | Tool::Ellipse | Tool::Redact => vec![(x0, y0), (x1, y1)],
        };
        Mark {
            tool,
            points,
            colour: gdk::RGBA::BLACK,
            width: if tool == Tool::Highlighter { (h - pad * 2.0).max(2.0) } else { 1.6 },
            sequence: 0,
        }
    }
}

pub use icon::ToolIcon;
