// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where each page sits: in one scrolling column, one at a time, or in pairs.
//!
//! Worked out from the page sizes alone, so every position the view relies on
//! can be tested without a window. Sizes are in logical pixels; page sizes
//! come from the PDF in points, a point being 1/72 of an inch.

/// Space above the first page, below the last, and either side.
pub const MARGIN: f64 = 24.0;
/// Space between one page and the next.
pub const SPACING: f64 = 16.0;
/// Logical pixels per point at 100%: a logical pixel is 1/96 of an inch.
pub const ACTUAL: f64 = 96.0 / 72.0;
/// Zoom limits, as a multiple of actual size.
pub const MIN_ZOOM: f64 = 0.1;
pub const MAX_ZOOM: f64 = 8.0;
/// The most pixels one page is ever rendered with. An A4 page at 800% on a
/// HiDPI screen would otherwise be a few hundred megabytes for a single page.
pub const MAX_PIXELS: f64 = 16_000_000.0;

/// How far the pages are turned, in quarter turns clockwise. A way of looking,
/// not a change to the file: nothing about it is ever written back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rotation(u8);

impl Rotation {
    /// Turned a further `quarters` quarter turns; negative turns the other way.
    pub fn turned(self, quarters: i32) -> Self {
        Rotation((i32::from(self.0) + quarters).rem_euclid(4) as u8)
    }

    /// Quarter turns clockwise, 0 to 3.
    pub fn quarters(self) -> u8 {
        self.0
    }

    pub fn radians(self) -> f64 {
        f64::from(self.0) * std::f64::consts::FRAC_PI_2
    }

    /// A page's size once turned: a quarter turn swaps width and height.
    pub fn size(self, width: f64, height: f64) -> (f64, f64) {
        if self.0 % 2 == 1 { (height, width) } else { (width, height) }
    }

    /// Where a point on the page, of size `width` by `height` before turning,
    /// ends up once the page is turned. Top-left origin, y down.
    pub fn apply(self, x: f64, y: f64, width: f64, height: f64) -> (f64, f64) {
        match self.0 {
            1 => (height - y, x),
            2 => (width - x, height - y),
            3 => (y, width - x),
            _ => (x, y),
        }
    }

    /// The inverse of `apply`: from a point on the turned page back to the
    /// page as the PDF describes it.
    pub fn undo(self, x: f64, y: f64, width: f64, height: f64) -> (f64, f64) {
        match self.0 {
            1 => (y, height - x),
            2 => (width - x, height - y),
            3 => (width - y, x),
            _ => (x, y),
        }
    }
}

/// Keep a scale inside the zoom limits.
pub fn clamp_scale(scale: f64) -> f64 {
    scale.clamp(ACTUAL * MIN_ZOOM, ACTUAL * MAX_ZOOM)
}

/// How pages are arranged: one long column, one page at a time, or side by
/// side in pairs, as a book lies open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Continuous,
    Single,
    Double,
}

impl Mode {
    /// How many pages share a row.
    fn across(self) -> usize {
        if self == Mode::Double { 2 } else { 1 }
    }
}

/// The scale at which the widest page exactly fills the viewport's width.
pub fn fit_width(pages: &[(f64, f64)], viewport_width: f64) -> f64 {
    let widest = pages.iter().map(|p| p.0).fold(0.0, f64::max);
    if widest <= 0.0 {
        return ACTUAL;
    }
    clamp_scale((viewport_width - 2.0 * MARGIN).max(1.0) / widest)
}

/// The scale that fits the document to the viewport in a mode: the widest row
/// across it, or for one page at a time, the largest page whole.
pub fn fit(pages: &[(f64, f64)], mode: Mode, viewport: (f64, f64)) -> f64 {
    let (width, height) = viewport;
    match mode {
        Mode::Continuous => fit_width(pages, width),
        Mode::Single => {
            let widest = pages.iter().map(|p| p.0).fold(0.0, f64::max);
            let tallest = pages.iter().map(|p| p.1).fold(0.0, f64::max);
            if widest <= 0.0 || tallest <= 0.0 {
                return ACTUAL;
            }
            let across = (width - 2.0 * MARGIN).max(1.0) / widest;
            let down = (height - 2.0 * MARGIN).max(1.0) / tallest;
            clamp_scale(across.min(down))
        }
        Mode::Double => {
            // Each row needs its pages' widths plus the gap between them.
            let scale = pages
                .chunks(2)
                .map(|row| {
                    let points: f64 = row.iter().map(|p| p.0).sum();
                    let gaps = SPACING * (row.len() - 1) as f64;
                    (width - 2.0 * MARGIN - gaps).max(1.0) / points.max(1.0)
                })
                .fold(f64::MAX, f64::min);
            if scale == f64::MAX { ACTUAL } else { clamp_scale(scale) }
        }
    }
}

/// The scale to actually render a page at: what was asked for, unless that
/// would exceed `MAX_PIXELS`, in which case as close as the limit allows.
pub fn render_scale(width_pt: f64, height_pt: f64, wanted: f64) -> f64 {
    let pixels = width_pt * wanted * height_pt * wanted;
    if pixels > MAX_PIXELS {
        wanted * (MAX_PIXELS / pixels).sqrt()
    } else {
        wanted
    }
}

/// Every page's position and size at one scale.
///
/// Pages are laid out in rows: one page each, or two for a book's spread. One
/// page at a time lays out only that page; asking about any other gives the
/// answer for the one shown, so nothing has to check first.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    count: usize,
    /// The first and last pages laid out.
    first: usize,
    last: usize,
    /// Per laid-out page: its row, and where it sits.
    row_of: Vec<usize>,
    tops: Vec<f64>,
    lefts: Vec<f64>,
    sizes: Vec<(f64, f64)>,
    /// Per row: its top, its height, and its first page.
    rows: Vec<(f64, f64, usize)>,
    width: f64,
    height: f64,
}

impl Layout {
    /// Lay out `pages` at `scale`. `shown` is the one page shown when the mode
    /// shows one at a time, and is ignored otherwise.
    pub fn new(pages: &[(f64, f64)], scale: f64, mode: Mode, shown: usize) -> Self {
        let count = pages.len();
        if count == 0 {
            return Layout::default();
        }
        let (first, last) = if mode == Mode::Single {
            let page = shown.min(count - 1);
            (page, page)
        } else {
            (0, count - 1)
        };
        let sizes: Vec<(f64, f64)> = pages[first..=last].iter().map(|&(w, h)| pixel_size(w, h, scale)).collect();

        // Rows first, to find the widest, which every row is centred against.
        let rows: Vec<std::ops::Range<usize>> = (0..sizes.len())
            .step_by(mode.across())
            .map(|start| start..(start + mode.across()).min(sizes.len()))
            .collect();
        let row_width = |row: &std::ops::Range<usize>| {
            sizes[row.clone()].iter().map(|s| s.0).sum::<f64>() + SPACING * (row.len() - 1) as f64
        };
        let widest = rows.iter().map(row_width).fold(0.0, f64::max);

        let mut layout = Layout {
            count,
            first,
            last,
            row_of: vec![0; sizes.len()],
            tops: vec![0.0; sizes.len()],
            lefts: vec![0.0; sizes.len()],
            sizes: sizes.clone(),
            rows: Vec::with_capacity(rows.len()),
            width: widest + 2.0 * MARGIN,
            height: 0.0,
        };
        let mut y = MARGIN;
        for (r, row) in rows.iter().enumerate() {
            let height = sizes[row.clone()].iter().map(|s| s.1).fold(0.0, f64::max);
            let mut x = MARGIN + ((widest - row_width(row)) / 2.0).round();
            for i in row.clone() {
                layout.row_of[i] = r;
                layout.lefts[i] = x;
                // Pages of different heights side by side line up on their
                // middles, as an open book's do.
                layout.tops[i] = y + ((height - sizes[i].1) / 2.0).round();
                x += sizes[i].0 + SPACING;
            }
            layout.rows.push((y, height, first + row.start));
            y += height + SPACING;
        }
        layout.height = y - SPACING + MARGIN;
        layout
    }

    /// Pages in the document, whether laid out or not.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Width and height of the whole column, margins included.
    pub fn extent(&self) -> (f64, f64) {
        (self.width, self.height)
    }

    /// Whether a page is laid out: all of them, unless one is shown at a time.
    pub fn contains(&self, page: usize) -> bool {
        self.count > 0 && (self.first..=self.last).contains(&page)
    }

    fn index(&self, page: usize) -> usize {
        page.clamp(self.first, self.last) - self.first
    }

    pub fn top(&self, page: usize) -> f64 {
        self.tops[self.index(page)]
    }

    pub fn left(&self, page: usize) -> f64 {
        self.lefts[self.index(page)]
    }

    /// A page's size in whole logical pixels, exactly as the widget is sized.
    pub fn size(&self, page: usize) -> (f64, f64) {
        self.sizes[self.index(page)]
    }

    /// The row at height `y`. A gap between rows belongs to the row above it;
    /// anything above the first belongs to the first.
    fn row_at(&self, y: f64) -> usize {
        self.rows.partition_point(|&(top, _, _)| top <= y).saturating_sub(1)
    }

    /// The pages in a row, first and last.
    fn row_pages(&self, row: usize) -> (usize, usize) {
        let start = self.rows[row].2;
        let end = self.rows.get(row + 1).map_or(self.last, |next| next.2 - 1);
        (start, end)
    }

    /// The page at height `y`: the first in its row. A gap between pages
    /// belongs to the page above it.
    pub fn page_at(&self, y: f64) -> usize {
        if self.rows.is_empty() {
            return 0;
        }
        self.rows[self.row_at(y)].2
    }

    /// The page under a point, or the nearest one in the row at that height.
    pub fn page_at_point(&self, x: f64, y: f64) -> usize {
        if self.rows.is_empty() {
            return 0;
        }
        let (start, end) = self.row_pages(self.row_at(y));
        // Past the middle of the gap after a page, the next page is nearer.
        (start..end).find(|&page| x < self.left(page) + self.size(page).0 + SPACING / 2.0).unwrap_or(end)
    }

    /// The first and last pages with any part between `top` and `bottom`.
    pub fn visible(&self, top: f64, bottom: f64) -> Option<(usize, usize)> {
        if self.rows.is_empty() {
            return None;
        }
        let mut first = self.row_at(top);
        // `top` in the gap under a row: that row is already off screen.
        let (row_top, row_height, _) = self.rows[first];
        if top >= row_top + row_height && first + 1 < self.rows.len() {
            first += 1;
        }
        let last = self.row_at(bottom).max(first);
        Some((self.row_pages(first).0, self.row_pages(last).1))
    }

    /// Where `y` falls, as a page and how far down its row: 0.0 at the top,
    /// 1.0 at the bottom, beyond 1.0 in the gap below it.
    pub fn locate(&self, y: f64) -> (usize, f64) {
        if self.rows.is_empty() {
            return (0, 0.0);
        }
        let (top, height, page) = self.rows[self.row_at(y)];
        (page, (y - top) / height.max(1.0))
    }

    /// The inverse of `locate`.
    pub fn position(&self, page: usize, fraction: f64) -> f64 {
        if self.rows.is_empty() {
            return 0.0;
        }
        let (top, height, _) = self.rows[self.row_of[self.index(page)]];
        top + fraction * height
    }
}

/// A page's size at a scale, rounded to whole pixels so every page edge lands
/// on a pixel and the layout agrees with what GTK allocates.
pub fn pixel_size(width_pt: f64, height_pt: f64, scale: f64) -> (f64, f64) {
    ((width_pt * scale).round().max(1.0), (height_pt * scale).round().max(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A4: (f64, f64) = (595.0, 842.0);

    #[test]
    fn fit_width_fills_the_viewport_less_its_margins() {
        let scale = fit_width(&[A4, A4], 643.0);
        assert!((scale - 1.0).abs() < 1e-9, "{scale}");
        // Mixed sizes fit the widest page, so nothing is cut off at the side.
        let scale = fit_width(&[A4, (842.0, 595.0)], 890.0);
        assert!((scale - 1.0).abs() < 1e-9, "{scale}");
    }

    #[test]
    fn fit_width_stays_inside_the_zoom_limits() {
        assert_eq!(fit_width(&[A4], 1.0), ACTUAL * MIN_ZOOM);
        assert_eq!(fit_width(&[(1.0, 1.0)], 100_000.0), ACTUAL * MAX_ZOOM);
        assert_eq!(fit_width(&[], 800.0), ACTUAL);
    }

    #[test]
    fn pages_stack_with_margins_and_spacing() {
        let layout = Layout::new(&[A4, A4, A4], 1.0, Mode::Continuous, 0);
        assert_eq!(layout.top(0), MARGIN);
        assert_eq!(layout.top(1), MARGIN + 842.0 + SPACING);
        assert_eq!(layout.top(2), MARGIN + 2.0 * (842.0 + SPACING));
        let (w, h) = layout.extent();
        assert_eq!(w, 595.0 + 2.0 * MARGIN);
        assert_eq!(h, 3.0 * 842.0 + 2.0 * SPACING + 2.0 * MARGIN);
    }

    #[test]
    fn a_point_in_a_gap_belongs_to_the_page_above() {
        let layout = Layout::new(&[A4, A4], 1.0, Mode::Continuous, 0);
        assert_eq!(layout.page_at(0.0), 0);
        assert_eq!(layout.page_at(MARGIN + 400.0), 0);
        assert_eq!(layout.page_at(MARGIN + 842.0 + 5.0), 0); // in the gap
        assert_eq!(layout.page_at(layout.top(1) + 1.0), 1);
        assert_eq!(layout.page_at(1e9), 1);
    }

    #[test]
    fn visible_pages_are_the_ones_the_viewport_touches() {
        let layout = Layout::new(&[A4, A4, A4, A4], 1.0, Mode::Continuous, 0);
        // Straddling the boundary between the first two pages.
        assert_eq!(layout.visible(700.0, 1000.0), Some((0, 1)));
        // Starting in the gap under page 0: page 0 is already gone.
        let gap = MARGIN + 842.0 + 4.0;
        assert_eq!(layout.visible(gap, gap + 300.0), Some((1, 1)));
        // A tall viewport sees everything.
        assert_eq!(layout.visible(0.0, 1e9), Some((0, 3)));
        assert_eq!(Layout::new(&[], 1.0, Mode::Continuous, 0).visible(0.0, 100.0), None);
    }

    #[test]
    fn locate_and_position_are_inverses() {
        let small = Layout::new(&[A4, A4, A4], 1.0, Mode::Continuous, 0);
        let large = Layout::new(&[A4, A4, A4], 2.0, Mode::Continuous, 0);
        let y = small.top(1) + 421.0; // halfway down page 1
        let (page, fraction) = small.locate(y);
        assert_eq!(page, 1);
        assert!((fraction - 0.5).abs() < 1e-9);
        // The same spot on the page, after zooming in.
        let moved = large.position(page, fraction);
        assert!((moved - (large.top(1) + 842.0)).abs() < 1e-9, "{moved}");
    }

    #[test]
    fn four_quarter_turns_come_back_round() {
        let r = Rotation::default();
        assert_eq!(r.turned(4), r);
        assert_eq!(r.turned(-1), r.turned(3));
        assert_eq!(r.turned(1).size(595.0, 842.0), (842.0, 595.0));
        assert_eq!(r.turned(2).size(595.0, 842.0), (595.0, 842.0));
    }

    #[test]
    fn turning_a_point_and_back_gets_the_same_point() {
        let (w, h) = (595.0, 842.0);
        for quarters in 0..4 {
            let r = Rotation::default().turned(quarters);
            let (rw, rh) = r.size(w, h);
            for &(x, y) in &[(0.0, 0.0), (100.0, 700.0), (595.0, 842.0), (300.0, 20.0)] {
                let (tx, ty) = r.apply(x, y, w, h);
                assert!((0.0..=rw).contains(&tx) && (0.0..=rh).contains(&ty), "{quarters}: ({tx}, {ty}) off the page");
                let (bx, by) = r.undo(tx, ty, w, h);
                assert!((bx - x).abs() < 1e-9 && (by - y).abs() < 1e-9, "{quarters}: ({x}, {y}) came back as ({bx}, {by})");
            }
        }
    }

    #[test]
    fn a_quarter_turn_is_clockwise() {
        // The page's top-left corner ends up top-right, as a page turned
        // clockwise on a desk would have it.
        let r = Rotation::default().turned(1);
        assert_eq!(r.apply(0.0, 0.0, 595.0, 842.0), (842.0, 0.0));
    }

    #[test]
    fn two_pages_share_a_row_and_an_odd_one_out_sits_alone_in_the_middle() {
        let layout = Layout::new(&[A4, A4, A4], 1.0, Mode::Double, 0);
        let (w, h) = layout.extent();
        assert_eq!(w, 2.0 * 595.0 + SPACING + 2.0 * MARGIN);
        assert_eq!(h, 2.0 * 842.0 + SPACING + 2.0 * MARGIN);
        assert_eq!((layout.left(0), layout.top(0)), (MARGIN, MARGIN));
        assert_eq!((layout.left(1), layout.top(1)), (MARGIN + 595.0 + SPACING, MARGIN));
        // The last page alone, centred under the pair.
        assert_eq!(layout.left(2), MARGIN + ((595.0 + SPACING) / 2.0).round());
        assert_eq!(layout.top(2), MARGIN + 842.0 + SPACING);
    }

    #[test]
    fn in_a_spread_the_point_decides_the_page() {
        let layout = Layout::new(&[A4, A4, A4, A4], 1.0, Mode::Double, 0);
        let y = MARGIN + 100.0;
        assert_eq!(layout.page_at_point(MARGIN + 10.0, y), 0);
        assert_eq!(layout.page_at_point(MARGIN + 595.0 + SPACING + 10.0, y), 1);
        let below = layout.top(2) + 50.0;
        assert_eq!(layout.page_at_point(MARGIN + 700.0, below), 3);
        // Both pages of each row are visible, and the row is one step down.
        assert_eq!(layout.visible(0.0, 400.0), Some((0, 1)));
        assert_eq!(layout.page_at(below), 2);
        let (page, fraction) = layout.locate(below);
        assert_eq!(page, 2);
        assert!((layout.position(3, fraction) - below).abs() < 1e-9);
    }

    #[test]
    fn one_page_at_a_time_lays_out_only_that_page() {
        let layout = Layout::new(&[A4, (842.0, 595.0), A4], 1.0, Mode::Single, 1);
        assert_eq!(layout.len(), 3);
        assert_eq!(layout.extent(), (842.0 + 2.0 * MARGIN, 595.0 + 2.0 * MARGIN));
        assert!(layout.contains(1) && !layout.contains(0) && !layout.contains(2));
        assert_eq!(layout.visible(0.0, 1e9), Some((1, 1)));
        assert_eq!(layout.page_at(0.0), 1);
        assert_eq!(layout.page_at_point(0.0, 0.0), 1);
    }

    #[test]
    fn fitting_depends_on_the_mode() {
        // Two A4 pages side by side in 1238 pixels, less margins and the gap.
        let two = fit(&[A4, A4], Mode::Double, (2.0 * 595.0 + SPACING + 2.0 * MARGIN, 500.0));
        assert!((two - 1.0).abs() < 1e-9, "{two}");
        // A whole page in a window shorter than it is wide: height decides.
        let one = fit(&[A4], Mode::Single, (2000.0, 842.0 + 2.0 * MARGIN));
        assert!((one - 1.0).abs() < 1e-9, "{one}");
    }

    #[test]
    fn huge_renders_are_capped_and_small_ones_left_alone() {
        assert_eq!(render_scale(595.0, 842.0, 2.0), 2.0);
        let capped = render_scale(595.0, 842.0, 20.0);
        let pixels = 595.0 * capped * 842.0 * capped;
        assert!(capped < 20.0 && (pixels - MAX_PIXELS).abs() < 1.0, "{capped} {pixels}");
    }
}
