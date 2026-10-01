// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Drawing on a page: the image editor's pen, highlighter, line, arrow,
//! rectangle and ellipse, saved as the PDF's own ink annotations, so every
//! reader draws them.
//!
//! What is drawn is the image editor's own `Mark`, in page points rather than
//! pixels: the same tools, drawn the same way while the pointer moves.

use gtk::glib;

use crate::images::edit::draw::{self, Mark, Tool};

use super::annots::{self, Rgb};
use super::newer;

/// A drawing on one page. Positions are points from the page's top-left.
#[derive(Clone, Debug, PartialEq)]
pub struct Drawing {
    pub page: usize,
    pub tool: Tool,
    pub strokes: Vec<Vec<(f64, f64)>>,
    pub colour: Rgb,
    /// Line thickness, in points.
    pub width: f64,
}

impl Drawing {
    /// What a mark drawn on a page is saved as. None for one not worth
    /// keeping, such as a shape that was only clicked.
    pub fn from_mark(page: usize, mark: &Mark) -> Option<Self> {
        if !mark.is_worth_keeping() {
            return None;
        }
        let strokes = mark.strokes();
        (!strokes.is_empty()).then(|| Drawing {
            page,
            tool: mark.tool,
            strokes,
            colour: Rgb::from_rgba(&mark.colour),
            width: mark.width,
        })
    }

    /// Everything it covers, the line's thickness included: x1, y1, x2, y2.
    /// Poppler works out an ink annotation's box itself, as the points' with
    /// a whole line's width all round, so this is that same box, which is how
    /// the drawing is found again.
    pub fn area(&self) -> [f64; 4] {
        let points = self.strokes.iter().flatten();
        let (mut x1, mut y1, mut x2, mut y2) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for &(x, y) in points {
            (x1, y1, x2, y2) = (x1.min(x), y1.min(y), x2.max(x), y2.max(y));
        }
        let margin = self.width;
        [x1 - margin, y1 - margin, x2 + margin, y2 + margin]
    }
}

/// Whether this Poppler can draw at all: ink needs 25.06.
pub fn available() -> bool {
    newer::get().ink.is_some() && newer::get().border.is_some()
}

pub(super) fn key(page: &poppler::Page, drawing: &Drawing) -> (poppler::ffi::PopplerAnnotType, [f64; 4]) {
    let (_, height) = page.size();
    (poppler::ffi::POPPLER_ANNOT_INK, annots::flip(drawing.area(), height))
}

pub(super) fn add(document: &poppler::Document, page: &poppler::Page, drawing: &Drawing) {
    use glib::translate::{from_glib_full, ToGlibPtr};

    let newer = newer::get();
    let (Some(ink), Some(border)) = (&newer.ink, &newer.border) else { return };
    let (_, height) = page.size();
    let mut rect = annots::rectangle(annots::flip(drawing.area(), height));
    // SAFETY: the calls are Poppler's own, found by name with the signatures
    // its headers give; each path is freed once Poppler has copied it.
    unsafe {
        let raw = (ink.new)(document.to_glib_none().0, &mut rect);
        let annot: poppler::Annot = from_glib_full(raw);
        (border.set)(raw, drawing.width);
        if drawing.tool == Tool::Highlighter {
            poppler::ffi::poppler_annot_markup_set_opacity(raw.cast(), f64::from(draw::HIGHLIGHT_ALPHA));
        }
        // On the page before its strokes are given, or Poppler places them
        // without the page's crop and turn.
        annots::attach(page, &annot, Some(drawing.colour), true);
        let mut paths: Vec<*mut std::ffi::c_void> = drawing.strokes.iter().map(|stroke| path(ink, stroke, height)).collect();
        (ink.set_list)(raw, paths.as_mut_ptr(), paths.len());
        for path in paths {
            (ink.path_free)(path);
        }
    }
}

/// One stroke as Poppler's path, with y measured up from the bottom.
///
/// Poppler's notes say a path copies its points, but `poppler_path_free`
/// frees the array it was given, so it may keep them instead. They are made
/// with GLib's allocator either way, and freed here only if Poppler made its
/// own copy.
///
/// # Safety
/// `ink` must hold Poppler's own functions.
unsafe fn path(ink: &newer::Ink, stroke: &[(f64, f64)], height: f64) -> *mut std::ffi::c_void {
    use poppler::ffi::PopplerPoint;
    // SAFETY: room for every point is allocated, filled, then handed over.
    unsafe {
        let points = glib::ffi::g_malloc_n(stroke.len().max(1), std::mem::size_of::<PopplerPoint>()).cast::<PopplerPoint>();
        for (i, &(x, y)) in stroke.iter().enumerate() {
            points.add(i).write(PopplerPoint { x, y: height - y });
        }
        let path = (ink.path_new)(points, stroke.len());
        let mut count = 0;
        if (ink.path_points)(path, &mut count) != points {
            glib::ffi::g_free(points.cast());
        }
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drawing_covers_its_line_as_well_as_its_points() {
        let drawing = Drawing {
            page: 0,
            tool: Tool::Line,
            strokes: vec![vec![(10.0, 20.0), (50.0, 20.0)]],
            colour: Rgb(0, 0, 0),
            width: 4.0,
        };
        assert_eq!(drawing.area(), [6.0, 16.0, 54.0, 24.0]);
    }

    #[test]
    fn a_click_with_a_shape_tool_draws_nothing() {
        let mark = Mark { tool: Tool::Rectangle, points: vec![(5.0, 5.0)], colour: gtk::gdk::RGBA::BLACK, width: 2.0, sequence: 0 };
        assert_eq!(Drawing::from_mark(0, &mark), None);
    }
}
