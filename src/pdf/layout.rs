// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where each page sits in the scrolling column.
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

/// The scale at which the widest page exactly fills the viewport's width.
pub fn fit_width(pages: &[(f64, f64)], viewport_width: f64) -> f64 {
    let widest = pages.iter().map(|p| p.0).fold(0.0, f64::max);
    if widest <= 0.0 {
        return ACTUAL;
    }
    clamp_scale((viewport_width - 2.0 * MARGIN).max(1.0) / widest)
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
#[derive(Clone, Debug, Default)]
pub struct Layout {
    tops: Vec<f64>,
    sizes: Vec<(f64, f64)>,
    width: f64,
    height: f64,
}

impl Layout {
    pub fn new(pages: &[(f64, f64)], scale: f64) -> Self {
        let mut tops = Vec::with_capacity(pages.len());
        let mut sizes = Vec::with_capacity(pages.len());
        let mut y = MARGIN;
        let mut widest: f64 = 0.0;
        for &(w, h) in pages {
            let size = pixel_size(w, h, scale);
            tops.push(y);
            sizes.push(size);
            y += size.1 + SPACING;
            widest = widest.max(size.0);
        }
        let height = if pages.is_empty() { 0.0 } else { y - SPACING + MARGIN };
        Layout { tops, sizes, width: widest + 2.0 * MARGIN, height }
    }

    pub fn len(&self) -> usize {
        self.tops.len()
    }

    /// Width and height of the whole column, margins included.
    pub fn extent(&self) -> (f64, f64) {
        (self.width, self.height)
    }

    pub fn top(&self, page: usize) -> f64 {
        self.tops[page]
    }

    /// A page's size in whole logical pixels, exactly as the widget is sized.
    pub fn size(&self, page: usize) -> (f64, f64) {
        self.sizes[page]
    }

    /// The page at height `y`. A gap between pages belongs to the page above
    /// it; anything above the first page belongs to the first.
    pub fn page_at(&self, y: f64) -> usize {
        match self.tops.partition_point(|&top| top <= y) {
            0 => 0,
            n => n - 1,
        }
    }

    /// The first and last pages with any part between `top` and `bottom`.
    pub fn visible(&self, top: f64, bottom: f64) -> Option<(usize, usize)> {
        if self.tops.is_empty() {
            return None;
        }
        let mut first = self.page_at(top);
        // `top` in the gap under a page: that page is already off screen.
        if top >= self.tops[first] + self.sizes[first].1 && first + 1 < self.len() {
            first += 1;
        }
        let last = self.page_at(bottom).max(first);
        Some((first, last))
    }

    /// Where `y` falls, as a page and how far down it: 0.0 at its top edge,
    /// 1.0 at its bottom, beyond 1.0 in the gap below it.
    pub fn locate(&self, y: f64) -> (usize, f64) {
        let page = self.page_at(y);
        let height = self.sizes.get(page).map_or(1.0, |s| s.1.max(1.0));
        (page, (y - self.tops.get(page).copied().unwrap_or(0.0)) / height)
    }

    /// The inverse of `locate`.
    pub fn position(&self, page: usize, fraction: f64) -> f64 {
        match (self.tops.get(page), self.sizes.get(page)) {
            (Some(top), Some(size)) => top + fraction * size.1,
            _ => 0.0,
        }
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
        let layout = Layout::new(&[A4, A4, A4], 1.0);
        assert_eq!(layout.top(0), MARGIN);
        assert_eq!(layout.top(1), MARGIN + 842.0 + SPACING);
        assert_eq!(layout.top(2), MARGIN + 2.0 * (842.0 + SPACING));
        let (w, h) = layout.extent();
        assert_eq!(w, 595.0 + 2.0 * MARGIN);
        assert_eq!(h, 3.0 * 842.0 + 2.0 * SPACING + 2.0 * MARGIN);
    }

    #[test]
    fn a_point_in_a_gap_belongs_to_the_page_above() {
        let layout = Layout::new(&[A4, A4], 1.0);
        assert_eq!(layout.page_at(0.0), 0);
        assert_eq!(layout.page_at(MARGIN + 400.0), 0);
        assert_eq!(layout.page_at(MARGIN + 842.0 + 5.0), 0); // in the gap
        assert_eq!(layout.page_at(layout.top(1) + 1.0), 1);
        assert_eq!(layout.page_at(1e9), 1);
    }

    #[test]
    fn visible_pages_are_the_ones_the_viewport_touches() {
        let layout = Layout::new(&[A4, A4, A4, A4], 1.0);
        // Straddling the boundary between the first two pages.
        assert_eq!(layout.visible(700.0, 1000.0), Some((0, 1)));
        // Starting in the gap under page 0: page 0 is already gone.
        let gap = MARGIN + 842.0 + 4.0;
        assert_eq!(layout.visible(gap, gap + 300.0), Some((1, 1)));
        // A tall viewport sees everything.
        assert_eq!(layout.visible(0.0, 1e9), Some((0, 3)));
        assert_eq!(Layout::new(&[], 1.0).visible(0.0, 100.0), None);
    }

    #[test]
    fn locate_and_position_are_inverses() {
        let small = Layout::new(&[A4, A4, A4], 1.0);
        let large = Layout::new(&[A4, A4, A4], 2.0);
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
    fn huge_renders_are_capped_and_small_ones_left_alone() {
        assert_eq!(render_scale(595.0, 842.0, 2.0), 2.0);
        let capped = render_scale(595.0, 842.0, 20.0);
        let pixels = 595.0 * capped * 842.0 * capped;
        assert!(capped < 20.0 && (pixels - MAX_PIXELS).abs() < 1.0, "{capped} {pixels}");
    }
}
