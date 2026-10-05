// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Picking a drawn mark up again: finding it under the pointer, the grips
//! round it, and what dragging one of them does to it.
//!
//! Shared by pictures and PDFs, so a shape behaves the same on either. It
//! knows nothing of pixels or points: whatever units the strokes are in, the
//! pointer is in too.

use gtk::prelude::*;
use gtk::{gdk, graphene, gsk};

pub type Point = (f64, f64);

/// How a picked shape is held.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outline {
    /// A line or arrow, by its two ends.
    Ends(Point, Point),
    /// Anything else, by the box round its points: x, y, width, height. The
    /// width or height may come out negative while dragging, when a side has
    /// been pulled past the one opposite, which turns the shape over.
    Frame([f64; 4]),
}

/// What part of a picked shape a press took hold of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grip {
    /// The shape itself, to move it.
    Body,
    /// One end of a line or arrow.
    Start,
    End,
    /// A corner or the middle of a side, by which way it lies from the
    /// middle: -1, 0 or 1 across, then down.
    Side(i8, i8),
}

/// The eight grips round a frame, corners first.
const SIDES: [(i8, i8); 8] = [(-1, -1), (1, -1), (1, 1), (-1, 1), (0, -1), (1, 0), (0, 1), (-1, 0)];

impl Outline {
    /// The box round the outline, x, y, width and height, never negative.
    pub fn bounds(&self) -> [f64; 4] {
        match *self {
            Outline::Ends(a, b) => [a.0.min(b.0), a.1.min(b.1), (a.0 - b.0).abs(), (a.1 - b.1).abs()],
            Outline::Frame([x, y, w, h]) => [x.min(x + w), y.min(y + h), w.abs(), h.abs()],
        }
    }

    /// Whether a press at `p` lands within a picked shape's box, give or take
    /// `slack`: anywhere there takes hold of it to move, gaps between its
    /// strokes included. A line or arrow has no box to speak of.
    pub fn holds(&self, p: Point, slack: f64) -> bool {
        match self {
            Outline::Ends(..) => false,
            Outline::Frame(_) => {
                let [x, y, w, h] = self.bounds();
                p.0 >= x - slack && p.0 <= x + w + slack && p.1 >= y - slack && p.1 <= y + h + slack
            }
        }
    }

    /// Every grip, and where it is.
    pub fn grips(&self) -> Vec<(Grip, Point)> {
        match *self {
            Outline::Ends(a, b) => vec![(Grip::Start, a), (Grip::End, b)],
            Outline::Frame([x, y, w, h]) => SIDES
                .iter()
                .map(|&(sx, sy)| {
                    let at = |start: f64, size: f64, s: i8| start + size * (f64::from(s) + 1.0) / 2.0;
                    (Grip::Side(sx, sy), (at(x, w, sx), at(y, h, sy)))
                })
                .collect(),
        }
    }

    /// The grip within `reach` of `p`, the nearest if several are. A frame
    /// too small to tell its grips apart offers only its corners.
    pub fn grip_at(&self, p: Point, reach: f64) -> Option<Grip> {
        let [_, _, w, h] = self.bounds();
        let crowded = w < reach * 3.0 || h < reach * 3.0;
        self.grips()
            .into_iter()
            .filter(|(grip, _)| !(crowded && matches!(grip, Grip::Side(x, y) if *x == 0 || *y == 0)))
            .map(|(grip, at)| (grip, (at.0 - p.0).hypot(at.1 - p.1)))
            .filter(|&(_, distance)| distance <= reach)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(grip, _)| grip)
    }

    /// The outline once `grip`, taken hold of at `from`, has been dragged to
    /// `to`. With `keep_aspect`, a corner scales the shape evenly.
    pub fn dragged(&self, grip: Grip, from: Point, to: Point, keep_aspect: bool) -> Outline {
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let shift = |p: Point| (p.0 + dx, p.1 + dy);
        match (*self, grip) {
            (Outline::Ends(a, b), Grip::Body) => Outline::Ends(shift(a), shift(b)),
            (Outline::Ends(a, b), Grip::Start) => Outline::Ends(shift(a), b),
            (Outline::Ends(a, b), Grip::End) => Outline::Ends(a, shift(b)),
            (Outline::Ends(..), Grip::Side(..)) => *self,
            (Outline::Frame([x, y, w, h]), Grip::Body) => Outline::Frame([x + dx, y + dy, w, h]),
            (Outline::Frame(_), Grip::Start | Grip::End) => *self,
            (Outline::Frame([x, y, w, h]), Grip::Side(sx, sy)) => {
                // Each side pulled moves on its own; the one opposite stays.
                let (mut left, mut right, mut top, mut bottom) = (x, x + w, y, y + h);
                match sx {
                    -1 => left += dx,
                    1 => right += dx,
                    _ => {}
                }
                match sy {
                    -1 => top += dy,
                    1 => bottom += dy,
                    _ => {}
                }
                if keep_aspect && sx != 0 && sy != 0 && w.abs() > 1e-9 && h.abs() > 1e-9 {
                    // Whichever way was pulled further sets the scale.
                    let (fx, fy) = ((right - left) / w, (bottom - top) / h);
                    let scale = fx.abs().max(fy.abs());
                    let (nw, nh) = (w * scale * fx.signum(), h * scale * fy.signum());
                    if sx < 0 {
                        left = right - nw;
                    } else {
                        right = left + nw;
                    }
                    if sy < 0 {
                        top = bottom - nh;
                    } else {
                        bottom = top + nh;
                    }
                }
                Outline::Frame([left, top, right - left, bottom - top])
            }
        }
    }
}

/// Where a point of a shape held by frame `from` goes when the frame becomes
/// `to`: the same share of the way across, and down. A frame of no width
/// (a straight stroke up the page) is moved across rather than stretched.
pub fn refit(p: Point, from: [f64; 4], to: [f64; 4]) -> Point {
    let along = |v: f64, start: f64, size: f64, new_start: f64, new_size: f64| {
        if size.abs() < 1e-9 {
            v + (new_start + new_size / 2.0) - (start + size / 2.0)
        } else {
            new_start + (v - start) / size * new_size
        }
    };
    (along(p.0, from[0], from[2], to[0], to[2]), along(p.1, from[1], from[3], to[1], to[3]))
}

/// The box round some points, x, y, width, height.
pub fn frame_of<'a>(points: impl IntoIterator<Item = &'a Point>) -> Option<[f64; 4]> {
    let mut points = points.into_iter();
    let &(x, y) = points.next()?;
    let (mut x1, mut y1, mut x2, mut y2) = (x, y, x, y);
    for &(x, y) in points {
        (x1, y1, x2, y2) = (x1.min(x), y1.min(y), x2.max(x), y2.max(y));
    }
    Some([x1, y1, x2 - x1, y2 - y1])
}

/// How far `p` is from the segment `a`-`b`.
fn distance_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length2 = dx * dx + dy * dy;
    let t = if length2 < 1e-12 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length2).clamp(0.0, 1.0) };
    (p.0 - (a.0 + dx * t)).hypot(p.1 - (a.1 + dy * t))
}

/// Whether `p` is on any of the strokes, drawn `width` thick, give or take
/// `slack`.
pub fn touches(strokes: &[Vec<Point>], width: f64, p: Point, slack: f64) -> bool {
    let reach = width / 2.0 + slack;
    strokes.iter().any(|stroke| match stroke.as_slice() {
        [] => false,
        [only] => (only.0 - p.0).hypot(only.1 - p.1) <= reach,
        points => points.windows(2).any(|pair| distance_to_segment(p, pair[0], pair[1]) <= reach),
    })
}

/// Whether `p` is inside a closed stroke, one that ends where it began.
pub fn encloses(strokes: &[Vec<Point>], p: Point) -> bool {
    strokes.iter().any(|stroke| {
        let (Some(first), Some(last)) = (stroke.first(), stroke.last()) else { return false };
        if stroke.len() < 4 || (first.0 - last.0).hypot(first.1 - last.1) > 1e-6 {
            return false;
        }
        // Even-odd: count the edges a line out to the right crosses.
        let mut inside = false;
        for pair in stroke.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if (a.1 > p.1) != (b.1 > p.1) && p.0 < a.0 + (p.1 - a.1) / (b.1 - a.1) * (b.0 - a.0) {
                inside = !inside;
            }
        }
        inside
    })
}

/// The pointer for a grip: which way it pulls, judged by where it lies from
/// the middle of the shape as it is on screen, so a turned page still gets
/// the right one.
pub fn cursor(grip: Grip, offset: Point) -> &'static str {
    const STRAIGHT: f64 = 0.4;
    match grip {
        Grip::Body => "move",
        Grip::Start | Grip::End => "crosshair",
        Grip::Side(..) => {
            let (dx, dy) = offset;
            let length = dx.hypot(dy).max(1e-9);
            if (dy / length).abs() < STRAIGHT {
                "ew-resize"
            } else if (dx / length).abs() < STRAIGHT {
                "ns-resize"
            } else if dx * dy > 0.0 {
                "nwse-resize"
            } else {
                "nesw-resize"
            }
        }
    }
}

/// Size of a grip on screen, and how near the pointer has to come to take
/// hold of one, in the screen's logical pixels.
pub const GRIP_SIZE: f64 = 8.0;
pub const GRIP_REACH: f64 = 9.0;

/// The marks of a picked shape, in screen pixels: a dashed box round it, if
/// it has one, and its grips.
pub fn append_picked(snapshot: &gtk::Snapshot, frame: Option<[f64; 4]>, grips: &[Point]) {
    let shadow = gdk::RGBA::new(0.0, 0.0, 0.0, 0.45);
    let accent = gdk::RGBA::new(0.208, 0.518, 0.894, 1.0);
    if let Some([x, y, w, h]) = frame {
        // A little outside the shape, so it does not hide the ink it is round.
        let (x, y, w, h) = (x - 3.0, y - 3.0, w + 6.0, h + 6.0);
        let outline = gsk::PathBuilder::new();
        outline.add_rect(&graphene::Rect::new(x as f32, y as f32, w as f32, h as f32));
        let path = outline.to_path();
        snapshot.append_stroke(&path, &gsk::Stroke::new(3.0), &shadow);
        let dashed = gsk::Stroke::new(1.5);
        dashed.set_dash(&[6.0, 4.0]);
        snapshot.append_stroke(&path, &dashed, &gdk::RGBA::new(1.0, 1.0, 1.0, 0.95));
    }
    let half = (GRIP_SIZE / 2.0) as f32;
    for &(gx, gy) in grips {
        let square = graphene::Rect::new(gx as f32 - half, gy as f32 - half, half * 2.0, half * 2.0);
        let rounded = gsk::RoundedRect::from_rect(square, 2.0);
        snapshot.append_outset_shadow(&rounded, &shadow, 0.0, 0.0, 0.0, 1.5);
        snapshot.push_rounded_clip(&rounded);
        snapshot.append_color(&gdk::RGBA::WHITE, &square);
        snapshot.pop();
        snapshot.append_border(&rounded, &[1.5; 4], &[accent; 4]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picked_box_is_held_anywhere_inside_and_a_line_only_on_itself() {
        let frame = Outline::Frame([10.0, 10.0, 100.0, 40.0]);
        assert!(frame.holds((60.0, 30.0), 0.0), "in a gap between strokes");
        assert!(frame.holds((112.0, 30.0), 3.0), "just outside, within reach");
        assert!(!frame.holds((120.0, 30.0), 3.0));
        // Turned over by a drag, still the same box.
        assert!(Outline::Frame([110.0, 50.0, -100.0, -40.0]).holds((60.0, 30.0), 0.0));
        assert!(!Outline::Ends((0.0, 0.0), (100.0, 100.0)).holds((50.0, 50.0), 5.0));
    }

    #[test]
    fn a_frame_has_eight_grips_and_a_line_two() {
        let frame = Outline::Frame([10.0, 20.0, 100.0, 50.0]);
        let grips = frame.grips();
        assert_eq!(grips.len(), 8);
        assert!(grips.contains(&(Grip::Side(-1, -1), (10.0, 20.0))));
        assert!(grips.contains(&(Grip::Side(1, 1), (110.0, 70.0))));
        assert!(grips.contains(&(Grip::Side(0, -1), (60.0, 20.0))));
        assert_eq!(frame.grip_at((108.0, 69.0), 5.0), Some(Grip::Side(1, 1)));
        assert_eq!(frame.grip_at((60.0, 45.0), 5.0), None, "the middle is not a grip");
        let line = Outline::Ends((0.0, 0.0), (50.0, 50.0));
        assert_eq!(line.grip_at((49.0, 52.0), 5.0), Some(Grip::End));
    }

    #[test]
    fn a_small_frame_offers_only_its_corners() {
        let tiny = Outline::Frame([0.0, 0.0, 10.0, 10.0]);
        // The top middle is nearest, but a corner is what it gets.
        assert!(matches!(tiny.grip_at((5.0, 0.0), 6.0), Some(Grip::Side(-1 | 1, -1))));
    }

    #[test]
    fn dragging_a_side_moves_only_that_side() {
        let frame = Outline::Frame([10.0, 10.0, 100.0, 50.0]);
        assert_eq!(frame.dragged(Grip::Side(1, 0), (110.0, 35.0), (130.0, 99.0), false), Outline::Frame([10.0, 10.0, 120.0, 50.0]));
        assert_eq!(frame.dragged(Grip::Side(-1, -1), (10.0, 10.0), (0.0, 5.0), false), Outline::Frame([0.0, 5.0, 110.0, 55.0]));
        assert_eq!(frame.dragged(Grip::Body, (50.0, 50.0), (60.0, 40.0), false), Outline::Frame([20.0, 0.0, 100.0, 50.0]));
        // Past the other side, the shape turns over.
        let Outline::Frame([_, _, w, _]) = frame.dragged(Grip::Side(1, 0), (110.0, 0.0), (0.0, 0.0), false) else { panic!() };
        assert!(w < 0.0);
    }

    #[test]
    fn a_corner_can_keep_the_shape_in_proportion() {
        let frame = Outline::Frame([0.0, 0.0, 100.0, 50.0]);
        // Pulled twice as wide but no taller: both double.
        assert_eq!(frame.dragged(Grip::Side(1, 1), (100.0, 50.0), (200.0, 50.0), true), Outline::Frame([0.0, 0.0, 200.0, 100.0]));
        // From the top left, the bottom right stays put, and the side
        // pulled in least sets the size.
        assert_eq!(frame.dragged(Grip::Side(-1, -1), (0.0, 0.0), (50.0, 10.0), true), Outline::Frame([20.0, 10.0, 80.0, 40.0]));
    }

    #[test]
    fn the_ends_of_a_line_move_on_their_own() {
        let line = Outline::Ends((0.0, 0.0), (10.0, 0.0));
        assert_eq!(line.dragged(Grip::End, (10.0, 0.0), (10.0, 10.0), false), Outline::Ends((0.0, 0.0), (10.0, 10.0)));
        assert_eq!(line.dragged(Grip::Body, (5.0, 0.0), (6.0, 1.0), false), Outline::Ends((1.0, 1.0), (11.0, 1.0)));
    }

    #[test]
    fn points_keep_their_place_in_a_refitted_frame() {
        let (from, to) = ([0.0, 0.0, 10.0, 10.0], [100.0, 100.0, 20.0, 40.0]);
        assert_eq!(refit((5.0, 5.0), from, to), (110.0, 120.0));
        assert_eq!(refit((10.0, 0.0), from, to), (120.0, 100.0));
        // Turned over.
        assert_eq!(refit((0.0, 0.0), from, [10.0, 0.0, -10.0, 10.0]), (10.0, 0.0));
        // A straight stroke up the page moves across without stretching.
        assert_eq!(refit((5.0, 3.0), [5.0, 0.0, 0.0, 10.0], [7.0, 0.0, 4.0, 10.0]), (9.0, 3.0));
    }

    #[test]
    fn a_stroke_is_found_by_its_ink_and_a_closed_one_by_its_inside() {
        let square = vec![vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (0.0, 0.0)]];
        assert!(touches(&square, 2.0, (10.5, 5.0), 0.0));
        assert!(!touches(&square, 2.0, (5.0, 5.0), 0.0));
        assert!(encloses(&square, (5.0, 5.0)));
        assert!(!encloses(&square, (15.0, 5.0)));
        let open = vec![vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]];
        assert!(!encloses(&open, (8.0, 2.0)));
        assert!(touches(&[vec![(3.0, 3.0)]], 4.0, (4.0, 4.0), 0.0), "a dot");
    }

    #[test]
    fn a_grip_points_the_way_it_pulls() {
        assert_eq!(cursor(Grip::Side(1, 0), (40.0, 0.0)), "ew-resize");
        assert_eq!(cursor(Grip::Side(0, 1), (0.0, -30.0)), "ns-resize");
        assert_eq!(cursor(Grip::Side(1, 1), (40.0, 30.0)), "nwse-resize");
        assert_eq!(cursor(Grip::Side(1, -1), (40.0, -30.0)), "nesw-resize");
    }
}
