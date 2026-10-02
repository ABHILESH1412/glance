// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Live Text on the picture: the lines of text found on it, which of their
//! characters are selected, and what that selection says.
//!
//! Everything is in the picture's own pixels, so it follows the view through
//! any zoom, rotation or flip without being recomputed. A place in the text
//! is a line and a gap between two of its characters: gap 0 is before the
//! first, gap `n` after the last.

pub type Point = [f64; 2];

/// One line of text on the picture.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLine {
    pub chars: Vec<char>,
    /// Top left, top right, bottom right, bottom left.
    pub quad: [Point; 4],
    /// Where each gap falls along the line, 0 at its start and 1 at its end.
    pub cuts: Vec<f64>,
}

impl TextLine {
    /// From what was read: characters, corners, and gap positions. When the
    /// gaps do not match the characters one for one, they are spread evenly.
    pub fn new(text: &str, quad: [Point; 4], cuts: Vec<f64>) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let cuts = if cuts.len() == chars.len() + 1 && cuts.windows(2).all(|w| w[0] <= w[1]) {
            cuts
        } else {
            (0..=chars.len()).map(|i| i as f64 / chars.len().max(1) as f64).collect()
        };
        TextLine { chars, quad, cuts }
    }

    /// The point `t` of the way along the line, on its top and bottom edges.
    pub fn at(&self, t: f64) -> (Point, Point) {
        let lerp = |a: Point, b: Point| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        (lerp(self.quad[0], self.quad[1]), lerp(self.quad[3], self.quad[2]))
    }

    /// The part of the line between gaps `from` and `to`, as a quadrilateral.
    pub fn span(&self, from: usize, to: usize) -> [Point; 4] {
        let (a, d) = self.at(self.cuts[from.min(self.cuts.len() - 1)]);
        let (b, c) = self.at(self.cuts[to.min(self.cuts.len() - 1)]);
        [a, b, c, d]
    }

    /// Where `p` falls along the line (0 to 1, not clamped), and how far it
    /// is from the line's middle, in line heights (under 0.5 is on it).
    fn locate(&self, p: Point) -> (f64, f64) {
        let mid = |a: Point, b: Point| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let (start, end) = (mid(self.quad[0], self.quad[3]), mid(self.quad[1], self.quad[2]));
        let (dx, dy) = (end[0] - start[0], end[1] - start[1]);
        let length2 = (dx * dx + dy * dy).max(1e-9);
        let t = ((p[0] - start[0]) * dx + (p[1] - start[1]) * dy) / length2;
        let height = {
            let h = |a: Point, b: Point| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
            ((h(self.quad[0], self.quad[3]) + h(self.quad[1], self.quad[2])) / 2.0).max(1.0)
        };
        let across = ((p[0] - start[0]) * -dy + (p[1] - start[1]) * dx) / length2.sqrt();
        (t, (across / height).abs())
    }

    /// How far `p` is from the line, in the picture's own units: nothing on
    /// it, and from its nearest edge or end off it.
    fn distance(&self, p: Point) -> f64 {
        let (t, off) = self.locate(p);
        let span = |a: Point, b: Point| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
        let mid = |a: Point, b: Point| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let length = span(mid(self.quad[0], self.quad[3]), mid(self.quad[1], self.quad[2]));
        let height = ((span(self.quad[0], self.quad[3]) + span(self.quad[1], self.quad[2])) / 2.0).max(1.0);
        let along = if t < 0.0 { -t * length } else { (t - 1.0).max(0.0) * length };
        let across = (off * height - height / 2.0).max(0.0);
        along.hypot(across)
    }

    /// The gap nearest to `t` of the way along.
    fn gap_at(&self, t: f64) -> usize {
        let mut best = 0;
        for (i, cut) in self.cuts.iter().enumerate() {
            if (cut - t).abs() < (self.cuts[best] - t).abs() {
                best = i;
            }
        }
        best
    }
}

/// A place in the text: a line, and a gap in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Place {
    pub line: usize,
    pub gap: usize,
}

#[derive(Clone, Debug, Default)]
pub struct LiveLayer {
    pub lines: Vec<TextLine>,
    /// Where the selection started and where it reaches now.
    pub anchor: Option<Place>,
    pub focus: Option<Place>,
}

impl LiveLayer {
    pub fn new(lines: Vec<TextLine>) -> Self {
        LiveLayer { lines, anchor: None, focus: None }
    }

    /// The text under `p`, if `p` is on a line.
    pub fn hit(&self, p: Point) -> Option<Place> {
        self.lines.iter().enumerate().find_map(|(i, line)| {
            let (t, off) = line.locate(p);
            ((-0.02..=1.02).contains(&t) && off <= 0.6).then(|| Place { line: i, gap: line.gap_at(t) })
        })
    }

    /// Whether `p` is on a line, or within `reach` of one.
    pub fn near(&self, p: Point, reach: f64) -> bool {
        self.lines.iter().any(|line| line.distance(p) <= reach)
    }

    /// The place nearest to `p`, on or off the text: for dragging a
    /// selection out past the end of a line or between lines.
    pub fn nearest(&self, p: Point) -> Option<Place> {
        let mut best: Option<(f64, Place)> = None;
        for (i, line) in self.lines.iter().enumerate() {
            let (t, off) = line.locate(p);
            let outside = if (0.0..=1.0).contains(&t) { 0.0 } else { t.abs().min((t - 1.0).abs()) * 4.0 };
            let score = off + outside;
            if best.is_none_or(|(s, _)| score < s) {
                best = Some((score, Place { line: i, gap: line.gap_at(t.clamp(0.0, 1.0)) }));
            }
        }
        best.map(|(_, place)| place)
    }

    /// The selection, first place to last, if there is anything in it.
    pub fn range(&self) -> Option<(Place, Place)> {
        let (a, b) = (self.anchor?, self.focus?);
        let (from, to) = if a <= b { (a, b) } else { (b, a) };
        (from != to).then_some((from, to))
    }

    /// The selected part of each line, as gap ranges: for drawing.
    pub fn selected_spans(&self) -> Vec<(usize, usize, usize)> {
        let Some((from, to)) = self.range() else { return Vec::new() };
        (from.line..=to.line)
            .filter_map(|i| {
                let line = self.lines.get(i)?;
                let start = if i == from.line { from.gap } else { 0 };
                let end = if i == to.line { to.gap } else { line.chars.len() };
                (end > start).then_some((i, start, end))
            })
            .collect()
    }

    /// What is selected, a line break between lines.
    pub fn selected_text(&self) -> Option<String> {
        let spans = self.selected_spans();
        if spans.is_empty() {
            return None;
        }
        let parts: Vec<String> = spans
            .iter()
            .map(|&(i, start, end)| self.lines[i].chars[start..end].iter().collect::<String>().trim().to_string())
            .collect();
        Some(parts.join("\n"))
    }

    /// Every line, one to a line.
    pub fn all_text(&self) -> String {
        self.lines.iter().map(|line| line.chars.iter().collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    pub fn select_all(&mut self) {
        if let Some(last) = self.lines.len().checked_sub(1) {
            self.anchor = Some(Place { line: 0, gap: 0 });
            self.focus = Some(Place { line: last, gap: self.lines[last].chars.len() });
        }
    }

    /// The word at `place`: the run of non-spaces around its gap.
    pub fn select_word(&mut self, place: Place) {
        let Some(line) = self.lines.get(place.line) else { return };
        let gap = place.gap.min(line.chars.len());
        let mut start = gap;
        while start > 0 && !line.chars[start - 1].is_whitespace() {
            start -= 1;
        }
        let mut end = gap;
        while end < line.chars.len() && !line.chars[end].is_whitespace() {
            end += 1;
        }
        self.anchor = Some(Place { line: place.line, gap: start });
        self.focus = Some(Place { line: place.line, gap: end });
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
        self.focus = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Hello world" across (100, 100)-(210, 120), one 10-pixel cell a
    /// character; "Second" under it.
    fn layer() -> LiveLayer {
        let line = |text: &str, x: f64, y: f64| {
            let n = text.chars().count();
            let width = 10.0 * n as f64;
            TextLine::new(
                text,
                [[x, y], [x + width, y], [x + width, y + 20.0], [x, y + 20.0]],
                (0..=n).map(|i| i as f64 / n as f64).collect(),
            )
        };
        LiveLayer::new(vec![line("Hello world", 100.0, 100.0), line("Second", 100.0, 140.0)])
    }

    #[test]
    fn a_point_on_a_line_finds_the_gap_under_it() {
        let layer = layer();
        assert_eq!(layer.hit([101.0, 110.0]), Some(Place { line: 0, gap: 0 }));
        assert_eq!(layer.hit([124.0, 110.0]), Some(Place { line: 0, gap: 2 }));
        assert_eq!(layer.hit([209.0, 115.0]), Some(Place { line: 0, gap: 11 }));
        assert_eq!(layer.hit([130.0, 150.0]), Some(Place { line: 1, gap: 3 }));
        assert_eq!(layer.hit([130.0, 131.0]), None, "between the lines");
        assert_eq!(layer.hit([400.0, 110.0]), None, "past the end");
        // Off the text, the nearest place is still found for dragging.
        assert_eq!(layer.nearest([400.0, 110.0]), Some(Place { line: 0, gap: 11 }));
    }

    #[test]
    fn a_selection_reads_in_order_whichever_way_it_was_dragged() {
        let mut layer = layer();
        layer.anchor = Some(Place { line: 1, gap: 3 });
        layer.focus = Some(Place { line: 0, gap: 6 });
        assert_eq!(layer.selected_text().as_deref(), Some("world\nSec"));
        assert_eq!(layer.selected_spans(), vec![(0, 6, 11), (1, 0, 3)]);
        layer.focus = layer.anchor;
        assert_eq!(layer.selected_text(), None, "nothing between a place and itself");
    }

    #[test]
    fn near_the_text_is_on_it_or_a_little_way_off() {
        let layer = layer();
        assert!(layer.near([150.0, 110.0], 0.0), "on it");
        assert!(layer.near([150.0, 123.0], 4.0), "just under it");
        assert!(!layer.near([150.0, 130.0], 4.0), "further under it");
        assert!(layer.near([95.0, 110.0], 6.0), "just before it");
        assert!(!layer.near([300.0, 110.0], 6.0), "well past its end");
    }

    #[test]
    fn words_and_everything() {
        let mut layer = layer();
        layer.select_word(Place { line: 0, gap: 8 });
        assert_eq!(layer.selected_text().as_deref(), Some("world"));
        layer.select_word(Place { line: 0, gap: 2 });
        assert_eq!(layer.selected_text().as_deref(), Some("Hello"));
        layer.select_all();
        assert_eq!(layer.selected_text().as_deref(), Some("Hello world\nSecond"));
        assert_eq!(layer.all_text(), "Hello world\nSecond");
    }

    #[test]
    fn a_slanted_line_is_followed_along_its_slant() {
        // 30 degrees up to the right.
        let (c, s) = (30f64.to_radians().cos(), -30f64.to_radians().sin());
        let at = |a: f64, b: f64| [100.0 + a * c - b * s, 100.0 + a * s + b * c];
        let line = TextLine::new("abcd", [at(0.0, 0.0), at(40.0, 0.0), at(40.0, 10.0), at(0.0, 10.0)], vec![0.0, 0.25, 0.5, 0.75, 1.0]);
        let layer = LiveLayer::new(vec![line]);
        assert_eq!(layer.hit(at(19.0, 5.0)), Some(Place { line: 0, gap: 2 }));
        assert_eq!(layer.hit(at(19.0, 25.0)), None);
    }

    #[test]
    fn mismatched_gaps_are_spread_evenly() {
        let line = TextLine::new("abc", [[0.0, 0.0], [30.0, 0.0], [30.0, 10.0], [0.0, 10.0]], vec![0.0, 1.0]);
        assert_eq!(line.cuts.len(), 4);
        assert!((line.cuts[1] - 1.0 / 3.0).abs() < 1e-9);
    }
}
