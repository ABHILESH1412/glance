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

use std::sync::Arc;

use crate::images::edit::shape::{self, Outline};
use crate::images::edit::signature::Signature;
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
    /// An arrow with a head at each end.
    DoubleArrow,
    Rectangle,
    /// A rectangle with its corners rounded off.
    RoundedRectangle,
    Ellipse,
    /// A regular polygon of this many sides, in the box dragged out.
    Polygon(u8),
    /// A star of this many points.
    Star(u8),
    /// A speech bubble's outline: a rounded box with a tail below.
    Bubble,
    /// A tick, for checking things off.
    Tick,
    /// A cross, for crossing them out.
    Cross,
    /// A signature from the signature pad, stretched over its box. Not in
    /// `TOOLS`: signatures are put down whole, not drawn.
    Signature,
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
    Tool::DoubleArrow,
    Tool::Rectangle,
    Tool::RoundedRectangle,
    Tool::Ellipse,
    Tool::Polygon(DEFAULT_SIDES),
    Tool::Star(DEFAULT_POINTS),
    Tool::Bubble,
    Tool::Tick,
    Tool::Cross,
];

/// A new polygon's sides and a new star's points, until changed.
pub const DEFAULT_SIDES: u8 = 6;
pub const DEFAULT_POINTS: u8 = 5;
/// The fewest and most corners a polygon or star can have.
pub const FEWEST_CORNERS: u8 = 3;
pub const MOST_CORNERS: u8 = 24;
/// How far in a star's inner corners sit, as a share of the outer ones.
const STAR_INNER: f64 = 0.42;
/// How round a rounded rectangle's corners are, as a share of its shorter
/// side, and how many straight steps a quarter circle is drawn in.
const ROUNDING: f64 = 0.18;
const ARC_STEPS: usize = 16;
/// The share of a speech bubble's box its body takes; the tail fills the rest.
const BUBBLE_BODY: f64 = 0.78;

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
            Tool::DoubleArrow => "Double Arrow",
            Tool::Rectangle => "Rectangle",
            Tool::RoundedRectangle => "Rounded",
            Tool::Ellipse => "Ellipse",
            Tool::Polygon(_) => "Polygon",
            Tool::Star(_) => "Star",
            Tool::Bubble => "Bubble",
            Tool::Tick => "Tick",
            Tool::Cross => "Cross",
            Tool::Redact => "Redact",
            Tool::Signature => "Signature",
        }
    }

    /// The name in full, for a tooltip, where the button's is cut short.
    pub fn description(self) -> &'static str {
        match self {
            Tool::RoundedRectangle => "Rounded rectangle",
            Tool::Polygon(_) => "Polygon — set how many sides below",
            Tool::Star(_) => "Star — set how many points below",
            Tool::Bubble => "Speech bubble",
            Tool::Tick => "Tick, for checking things off",
            Tool::Cross => "Cross, for crossing things out",
            Tool::DoubleArrow => "Arrow with a head at each end",
            other => other.label(),
        }
    }

    /// The same kind of tool, whatever its corners: a hexagon and a
    /// pentagon are both the polygon tool.
    pub fn same_kind(self, other: Tool) -> bool {
        std::mem::discriminant(&self) == std::mem::discriminant(&other)
    }

    /// A polygon's sides or a star's points; None for anything else.
    pub fn corners(self) -> Option<u8> {
        match self {
            Tool::Polygon(n) | Tool::Star(n) => Some(n),
            _ => None,
        }
    }

    /// The same tool with another number of corners, if it has corners.
    pub fn with_corners(self, n: u8) -> Tool {
        let n = n.clamp(FEWEST_CORNERS, MOST_CORNERS);
        match self {
            Tool::Polygon(_) => Tool::Polygon(n),
            Tool::Star(_) => Tool::Star(n),
            other => other,
        }
    }

    /// Lines held by their two ends rather than by a box.
    pub fn has_ends(self) -> bool {
        matches!(self, Tool::Line | Tool::Arrow | Tool::DoubleArrow)
    }

    /// Closed outlines drawn as one ring of straight steps, the same on
    /// screen, in a picture and in a PDF.
    fn ringed(self) -> bool {
        matches!(self, Tool::RoundedRectangle | Tool::Polygon(_) | Tool::Star(_) | Tool::Bubble)
    }

    /// Freehand tools keep every point the pointer visited. The rest need only
    /// where the drag started and where it has got to.
    pub fn freehand(self) -> bool {
        matches!(self, Tool::Pen | Tool::Highlighter)
    }

    /// Shapes with an inside, which can be filled.
    pub fn fillable(self) -> bool {
        matches!(self, Tool::Rectangle | Tool::Ellipse) || self.ringed()
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
    /// What a rectangle or ellipse is filled with, under its outline. None
    /// for an outline only, and always for anything else.
    pub fill: Option<gdk::RGBA>,
    /// When this was drawn, so marks and text stack in the order they were
    /// made rather than by which list they live in.
    pub sequence: u64,
    /// What a signature mark shows; None for every other kind.
    pub signature: Option<Arc<Signature>>,
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

    /// The fill that actually goes down: only a shape with an inside has one,
    /// and a see-through one is none.
    pub fn inside(&self) -> Option<gdk::RGBA> {
        self.fill.filter(|fill| self.tool.fillable() && fill.alpha() > 0.0)
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
            Tool::Line | Tool::Arrow | Tool::DoubleArrow => {
                let (from, to) = self.ends()?;
                builder.move_to(from.0 as f32, from.1 as f32);
                builder.line_to(to.0 as f32, to.1 as f32);
                let mut tips = Vec::new();
                if matches!(self.tool, Tool::Arrow | Tool::DoubleArrow) {
                    tips.push((from, to));
                }
                if self.tool == Tool::DoubleArrow {
                    tips.push((to, from));
                }
                for (from, to) in tips {
                    let (left, right) = self.head(from, to);
                    for wing in [left, right] {
                        builder.move_to(to.0 as f32, to.1 as f32);
                        builder.line_to(wing.0 as f32, wing.1 as f32);
                    }
                }
            }
            // Drawn as the very steps a PDF is given, so all three agree.
            _ if self.tool.ringed() || matches!(self.tool, Tool::Tick | Tool::Cross | Tool::Signature) => {
                let strokes = self.strokes();
                if strokes.is_empty() {
                    return None;
                }
                for stroke in &strokes {
                    let (first, rest) = stroke.split_first()?;
                    builder.move_to(first.0 as f32, first.1 as f32);
                    if rest.is_empty() {
                        // The dot over an i.
                        builder.line_to(first.0 as f32, first.1 as f32);
                    }
                    for point in rest {
                        builder.line_to(point.0 as f32, point.1 as f32);
                    }
                    if self.tool.ringed() {
                        builder.close();
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
            Tool::Line | Tool::Arrow | Tool::DoubleArrow => {
                let Some((from, to)) = self.ends() else { return Vec::new() };
                let mut strokes = vec![vec![from, to]];
                if matches!(self.tool, Tool::Arrow | Tool::DoubleArrow) {
                    let (left, right) = self.head(from, to);
                    strokes.push(vec![left, to, right]);
                }
                if self.tool == Tool::DoubleArrow {
                    let (left, right) = self.head(to, from);
                    strokes.push(vec![left, from, right]);
                }
                strokes
            }
            _ if self.tool.ringed() => {
                let Some(mut ring) = self.ring() else { return Vec::new() };
                if let Some(&first) = ring.first() {
                    ring.push(first);
                }
                vec![ring]
            }
            Tool::Tick => {
                let Some((x, y, w, h)) = self.rect() else { return Vec::new() };
                vec![vec![(x, y + h * 0.55), (x + w * 0.38, y + h), (x + w, y)]]
            }
            Tool::Cross => {
                let Some((x, y, w, h)) = self.rect() else { return Vec::new() };
                vec![vec![(x, y), (x + w, y + h)], vec![(x + w, y), (x, y + h)]]
            }
            Tool::Signature => match (&self.signature, self.rect()) {
                (Some(signature), Some((x, y, w, h))) => signature.fitted([x, y, w, h]),
                _ => Vec::new(),
            },
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

    /// The corners of a closed outline, in order round it, not closed up.
    fn ring(&self) -> Option<Vec<(f64, f64)>> {
        let (x, y, w, h) = self.rect()?;
        let (cx, cy, rx, ry) = (x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
        let around = |n: usize, radius: &dyn Fn(usize) -> f64| -> Vec<(f64, f64)> {
            (0..n)
                .map(|i| {
                    // From the top, clockwise, so a polygon or star stands
                    // on its base.
                    let angle = -std::f64::consts::FRAC_PI_2 + i as f64 * std::f64::consts::TAU / n as f64;
                    let r = radius(i);
                    (cx + rx * r * angle.cos(), cy + ry * r * angle.sin())
                })
                .collect()
        };
        Some(match self.tool {
            Tool::Polygon(sides) => around(usize::from(sides.max(FEWEST_CORNERS)), &|_| 1.0),
            Tool::Star(points) => {
                around(usize::from(points.max(FEWEST_CORNERS)) * 2, &|i| if i % 2 == 0 { 1.0 } else { STAR_INNER })
            }
            Tool::RoundedRectangle => rounded(x, y, w, h, w.min(h) * ROUNDING, None),
            Tool::Bubble => {
                let body = h * BUBBLE_BODY;
                // The tail leaves the bottom a fifth of the way in and points
                // down and to the left, as a speaker's would.
                let tail = [(x + w * 0.36, y + body), (x + w * 0.12, y + h), (x + w * 0.2, y + body)];
                rounded(x, y, w, body, w.min(body) * 0.25, Some(tail))
            }
            _ => return None,
        })
    }

    /// The two wing tips of an arrow head, behind the point.
    fn head(&self, from: (f64, f64), to: (f64, f64)) -> ((f64, f64), (f64, f64)) {
        let angle = (to.1 - from.1).atan2(to.0 - from.0);
        // Never more than a little under half the arrow, so a short one is
        // not all head, and a double one's heads do not meet.
        let shaft = (to.0 - from.0).hypot(to.1 - from.1);
        let length = (self.width * HEAD_OF_WIDTH).max(HEAD_MINIMUM).min(shaft * 0.4);
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
        if matches!(self.tool, Tool::Arrow | Tool::DoubleArrow) {
            if let Some((from, to)) = self.ends() {
                let mut wings = vec![self.head(from, to)];
                if self.tool == Tool::DoubleArrow {
                    wings.push(self.head(to, from));
                }
                for (left, right) in wings {
                    for wing in [left, right] {
                        xs.push(wing.0);
                        ys.push(wing.1);
                    }
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
        if let Some(fill) = self.inside() {
            snapshot.append_fill(&path, gsk::FillRule::Winding, &fill);
        }
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
    /// How the mark is held once picked up: a line or arrow by its ends,
    /// anything else by the box round its points.
    pub fn outline(&self) -> Option<Outline> {
        match (self.tool, self.points.as_slice()) {
            (tool, [a, .., b]) if tool.has_ends() => Some(Outline::Ends(*a, *b)),
            _ => shape::frame_of(&self.points).map(Outline::Frame),
        }
    }

    /// The mark with its outline dragged to `to`. Only its points change: a
    /// line stays as thick, and an arrow's head as big.
    pub fn reshaped(&self, to: &Outline) -> Mark {
        let points = match (self.outline(), to) {
            (Some(Outline::Ends(..)), Outline::Ends(a, b)) => vec![*a, *b],
            (Some(Outline::Frame(from)), Outline::Frame(to)) => {
                self.points.iter().map(|&p| shape::refit(p, from, *to)).collect()
            }
            _ => self.points.clone(),
        };
        Mark { points, ..self.clone() }
    }

    /// Whether a press at `p` picks the mark up: on its ink, give or take
    /// `slack`, or anywhere inside a rectangle or ellipse when `inside`.
    pub fn is_at(&self, p: (f64, f64), slack: f64, inside: bool) -> bool {
        // A signature is a scribble with gaps everywhere: anywhere in its box.
        if self.tool == Tool::Signature {
            let Some((x, y, w, h)) = self.rect() else { return false };
            return p.0 >= x - slack && p.0 <= x + w + slack && p.1 >= y - slack && p.1 <= y + h + slack;
        }
        let strokes = self.strokes();
        let closed = matches!(self.tool, Tool::Rectangle | Tool::Ellipse | Tool::Redact) || self.tool.ringed();
        // A filled shape is picked up by its inside wherever it is.
        let inside = inside || self.inside().is_some();
        shape::touches(&strokes, self.width, p, slack) || (inside && closed && shape::encloses(&strokes, p))
    }

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
            fill: None,
            sequence: 0,
            signature: None,
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

    #[test]
    fn a_picked_mark_moves_and_resizes_by_its_outline() {
        // A line is held by its ends, and only the end dragged moves.
        let line = mark(Tool::Arrow, &[(0.0, 0.0), (100.0, 0.0)]);
        let Some(Outline::Ends(a, b)) = line.outline() else { panic!("an arrow is held by its ends") };
        let longer = line.reshaped(&Outline::Ends(a, (b.0 + 50.0, b.1 + 20.0)));
        assert_eq!(longer.points, vec![(0.0, 0.0), (150.0, 20.0)]);
        assert_eq!(longer.width, line.width, "the line stays as thick");
        // Anything else is held by its box, and every point keeps its place
        // in it.
        let pen = mark(Tool::Pen, &[(10.0, 10.0), (20.0, 30.0), (30.0, 10.0)]);
        let Some(Outline::Frame(frame)) = pen.outline() else { panic!("a pen stroke is held by its box") };
        assert_eq!(frame, [10.0, 10.0, 20.0, 20.0]);
        let doubled = pen.reshaped(&Outline::Frame([10.0, 10.0, 40.0, 40.0]));
        assert_eq!(doubled.points, vec![(10.0, 10.0), (30.0, 50.0), (50.0, 10.0)]);
        assert_eq!(doubled.sequence, pen.sequence, "still the same mark, where it was in the stack");
    }

    #[test]
    fn a_mark_is_picked_up_by_its_ink_or_inside_a_shape() {
        let rectangle = mark(Tool::Rectangle, &[(0.0, 0.0), (100.0, 50.0)]);
        assert!(rectangle.is_at((100.0, 25.0), 1.0, false), "on the edge");
        assert!(!rectangle.is_at((50.0, 25.0), 1.0, false), "the middle is not ink");
        assert!(rectangle.is_at((50.0, 25.0), 1.0, true), "but it is inside");
        // A line has no inside.
        let line = mark(Tool::Line, &[(0.0, 0.0), (100.0, 0.0)]);
        assert!(!line.is_at((50.0, 20.0), 1.0, true));
        assert!(line.is_at((50.0, 4.0), 1.0, true), "within half its width");
    }

    #[test]
    fn only_a_shape_with_an_inside_is_filled() {
        let red = gdk::RGBA::new(1.0, 0.0, 0.0, 1.0);
        let filled = |tool| Mark { fill: Some(red), ..mark(tool, &[(0.0, 0.0), (40.0, 20.0)]) };
        assert_eq!(filled(Tool::Rectangle).inside(), Some(red));
        assert_eq!(filled(Tool::Ellipse).inside(), Some(red));
        assert_eq!(filled(Tool::Arrow).inside(), None, "a line has no inside");
        let clear = Mark { fill: Some(gdk::RGBA::new(1.0, 0.0, 0.0, 0.0)), ..mark(Tool::Rectangle, &[(0.0, 0.0), (40.0, 20.0)]) };
        assert_eq!(clear.inside(), None, "see-through is no fill");
        // Filled, it is picked up by its middle too.
        assert!(filled(Tool::Rectangle).is_at((20.0, 10.0), 0.0, false));
    }

    #[test]
    fn every_new_shape_stays_in_the_box_it_was_dragged_out_in() {
        let (x0, y0, x1, y1) = (10.0, 20.0, 210.0, 120.0);
        for tool in [Tool::RoundedRectangle, Tool::Polygon(6), Tool::Star(5), Tool::Bubble, Tool::Tick, Tool::Cross] {
            let shape = mark(tool, &[(x0, y0), (x1, y1)]);
            let strokes = shape.strokes();
            assert!(!strokes.is_empty(), "{tool:?} drew nothing");
            for &(x, y) in strokes.iter().flatten() {
                assert!((x0 - 1e-9..=x1 + 1e-9).contains(&x) && (y0 - 1e-9..=y1 + 1e-9).contains(&y), "{tool:?} left its box at {x}, {y}");
            }
            assert!(shape.path().is_some(), "{tool:?} has no path to draw");
        }
    }

    #[test]
    fn polygons_and_stars_have_the_corners_asked_for() {
        let corners = |tool| mark(tool, &[(0.0, 0.0), (100.0, 100.0)]).strokes()[0].len() - 1;
        assert_eq!(corners(Tool::Polygon(3)), 3);
        assert_eq!(corners(Tool::Polygon(8)), 8);
        assert_eq!(corners(Tool::Star(5)), 10, "five points, five corners between them");
        // The first corner is at the top, so a triangle stands on its base.
        let triangle = mark(Tool::Polygon(3), &[(0.0, 0.0), (100.0, 100.0)]).strokes();
        assert!((triangle[0][0].0 - 50.0).abs() < 1e-9 && triangle[0][0].1.abs() < 1e-9);
        // Too few or too many is held to what makes sense.
        assert_eq!(Tool::Polygon(6).with_corners(1), Tool::Polygon(FEWEST_CORNERS));
        assert_eq!(Tool::Star(5).with_corners(200), Tool::Star(MOST_CORNERS));
        assert!(Tool::Star(5).same_kind(Tool::Star(7)) && !Tool::Star(5).same_kind(Tool::Polygon(5)));
    }

    #[test]
    fn closed_shapes_are_picked_up_inside_and_open_ones_only_on_their_line() {
        let star = mark(Tool::Star(5), &[(0.0, 0.0), (100.0, 100.0)]);
        assert!(star.is_at((50.0, 50.0), 1.0, true), "the middle of a star");
        assert!(!star.is_at((2.0, 2.0), 1.0, true), "outside it, in the box's corner");
        let bubble = mark(Tool::Bubble, &[(0.0, 0.0), (100.0, 100.0)]);
        assert!(bubble.is_at((50.0, 40.0), 1.0, true));
        assert!(bubble.is_at((13.0, 97.0), 4.0, false), "its tail's tip");
        let tick = mark(Tool::Tick, &[(0.0, 0.0), (100.0, 100.0)]);
        assert!(!tick.is_at((70.0, 80.0), 1.0, true), "a tick has no inside");
        assert!(Tool::Star(5).fillable() && Tool::Bubble.fillable() && !Tool::Tick.fillable());
    }

    #[test]
    fn a_double_arrow_has_a_head_at_each_end_and_is_held_by_them() {
        let both = mark(Tool::DoubleArrow, &[(0.0, 50.0), (100.0, 50.0)]);
        let strokes = both.strokes();
        assert_eq!(strokes.len(), 3);
        assert_eq!(strokes[1][1], (100.0, 50.0));
        assert_eq!(strokes[2][1], (0.0, 50.0));
        assert!(strokes[2][0].0 > 0.0, "the second head points back the other way");
        assert!(matches!(both.outline(), Some(Outline::Ends(..))));
        let (x, _, w, _) = both.bounds().unwrap();
        assert!(x < strokes[2][0].0.min(strokes[2][2].0) && x + w > 100.0);
    }

    #[test]
    fn a_reshaped_star_keeps_its_points_and_fills_its_new_box() {
        let star = mark(Tool::Star(6), &[(0.0, 0.0), (100.0, 100.0)]);
        let wider = star.reshaped(&Outline::Frame([0.0, 0.0, 200.0, 100.0]));
        assert_eq!(wider.tool, Tool::Star(6));
        let xs: Vec<f64> = wider.strokes()[0].iter().map(|p| p.0).collect();
        assert!(xs.iter().cloned().fold(f64::MIN, f64::max) > 180.0, "it stretched to the new box");
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

/// A rectangle's outline with its corners rounded to `radius`, clockwise
/// from the top-left. A tail, if given, is let into the bottom edge: the
/// point it leaves at on the right, its tip, and where it comes back on the
/// left.
fn rounded(x: f64, y: f64, w: f64, h: f64, radius: f64, tail: Option<[(f64, f64); 3]>) -> Vec<(f64, f64)> {
    let r = radius.clamp(0.0, w.min(h) / 2.0);
    let arc = |ring: &mut Vec<(f64, f64)>, cx: f64, cy: f64, from: f64| {
        for step in 0..=ARC_STEPS {
            let angle = (from + 90.0 * step as f64 / ARC_STEPS as f64).to_radians();
            ring.push((cx + r * angle.cos(), cy + r * angle.sin()));
        }
    };
    let mut ring = Vec::with_capacity(ARC_STEPS * 4 + 8);
    arc(&mut ring, x + w - r, y + r, -90.0);
    arc(&mut ring, x + w - r, y + h - r, 0.0);
    if let Some(tail) = tail {
        ring.extend(tail);
    }
    arc(&mut ring, x + r, y + h - r, 90.0);
    arc(&mut ring, x + r, y + r, 180.0);
    ring
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
            /// The Select tool's pointer, instead of a mark.
            pub pointer: Cell<bool>,
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
                let widget = self.obj();
                let (w, h) = (widget.width() as f64, widget.height() as f64);
                if w < 2.0 || h < 2.0 {
                    return;
                }
                // Follow the label's colour, so the icon dims and highlights
                // with the button like a real symbolic icon.
                let ink = widget.color();
                if self.pointer.get() {
                    snapshot.append_fill(&super::pointer(w, h), gsk::FillRule::Winding, &ink);
                    return;
                }
                let Some(tool) = self.tool.get() else {
                    return;
                };
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

        /// The Select tool's button face: a pointer, and its name.
        pub fn select_face() -> gtk::Box {
            let icon: Self = glib::Object::new();
            icon.imp().pointer.set(true);
            icon.set_size_request(18, 18);
            icon.set_valign(gtk::Align::Center);
            let face = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            face.set_halign(gtk::Align::Center);
            face.append(&icon);
            face.append(&gtk::Label::new(Some("Select")));
            face
        }
    }

    /// An arrow pointer, filling the height it is given.
    fn pointer(w: f64, h: f64) -> gsk::Path {
        let size = h.min(w) - 2.0;
        let (x0, y0) = ((w - size * 0.62) / 2.0, 1.0);
        let at = |x: f64, y: f64| ((x0 + x * size) as f32, (y0 + y * size) as f32);
        let builder = gsk::PathBuilder::new();
        let outline = [(0.0, 0.0), (0.0, 0.86), (0.22, 0.66), (0.37, 1.0), (0.5, 0.94), (0.36, 0.61), (0.62, 0.61)];
        let (x, y) = at(outline[0].0, outline[0].1);
        builder.move_to(x, y);
        for &(px, py) in &outline[1..] {
            let (x, y) = at(px, py);
            builder.line_to(x, y);
        }
        builder.close();
        builder.to_path()
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
            Tool::Line | Tool::Arrow | Tool::DoubleArrow => vec![(x0, y1), (x1, y0)],
            // A star or polygon fills the square it is drawn in.
            Tool::Polygon(_) | Tool::Star(_) => vec![(x0 - 1.0, y0 - 1.0), (x1 + 1.0, y1 + 1.0)],
            Tool::Rectangle
            | Tool::RoundedRectangle
            | Tool::Ellipse
            | Tool::Bubble
            | Tool::Tick
            | Tool::Cross
            | Tool::Redact
            | Tool::Signature => {
                vec![(x0, y0), (x1, y1)]
            }
        };
        Mark {
            tool,
            points,
            colour: gdk::RGBA::BLACK,
            width: if tool == Tool::Highlighter { (h - pad * 2.0).max(2.0) } else { 1.6 },
            fill: None,
            sequence: 0,
            signature: None,
        }
    }
}

pub use icon::ToolIcon;
