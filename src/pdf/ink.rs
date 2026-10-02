// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Drawing on a page: the image editor's pen, highlighter, line, arrow,
//! rectangle and ellipse, saved as the PDF's own annotations, so every reader
//! draws them: ink for the lines, and a square or circle annotation for a
//! rectangle or ellipse, which is the only kind of shape a PDF lets be filled.
//!
//! What is drawn is the image editor's own `Mark`, in page points rather than
//! pixels: the same tools, drawn the same way while the pointer moves.

use gtk::glib;

use crate::images::edit::draw::{self, Mark, Tool};
use crate::images::edit::shape::{self, Outline};

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
    /// What a rectangle or ellipse is filled with, if anything.
    pub fill: Option<Rgb>,
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
            fill: mark.inside().map(|fill| Rgb::from_rgba(&fill)),
        })
    }

    /// The box a rectangle or ellipse is drawn in, x1, y1, x2, y2: the
    /// middle of its line all round. None for anything else.
    pub fn shape(&self) -> Option<[f64; 4]> {
        if !self.tool.fillable() {
            return None;
        }
        let [x, y, w, h] = shape::frame_of(self.strokes.iter().flatten())?;
        Some([x, y, x + w, y + h])
    }

    /// The kind of annotation this is saved as.
    fn kind(&self) -> poppler::ffi::PopplerAnnotType {
        match self.tool {
            Tool::Rectangle => poppler::ffi::POPPLER_ANNOT_SQUARE,
            Tool::Ellipse => poppler::ffi::POPPLER_ANNOT_CIRCLE,
            _ => poppler::ffi::POPPLER_ANNOT_INK,
        }
    }

    /// Everything it covers, the line's thickness included: x1, y1, x2, y2.
    /// Poppler works out an ink annotation's box itself, as the points' with
    /// a whole line's width all round, so this is that same box, which is how
    /// the drawing is found again.
    pub fn area(&self) -> [f64; 4] {
        // A square or circle annotation's line lies inside its box, so the
        // box is the shape's, half a line out.
        if let Some([x1, y1, x2, y2]) = self.shape() {
            let half = self.width / 2.0;
            return [x1 - half, y1 - half, x2 + half, y2 + half];
        }
        let points = self.strokes.iter().flatten();
        let (mut x1, mut y1, mut x2, mut y2) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for &(x, y) in points {
            (x1, y1, x2, y2) = (x1.min(x), y1.min(y), x2.max(x), y2.max(y));
        }
        let margin = self.width;
        [x1 - margin, y1 - margin, x2 + margin, y2 + margin]
    }

    /// The ends of a line or arrow; None for any other drawing.
    fn ends(&self) -> Option<((f64, f64), (f64, f64))> {
        match (self.tool, self.strokes.as_slice()) {
            (Tool::Line, [shaft]) | (Tool::Arrow, [shaft, _]) if shaft.len() == 2 => Some((shaft[0], shaft[1])),
            _ => None,
        }
    }

    /// How the drawing is held once picked up: a line or arrow by its ends,
    /// anything else by the box round its points.
    pub fn outline(&self) -> Option<Outline> {
        match self.ends() {
            Some((a, b)) => Some(Outline::Ends(a, b)),
            None => shape::frame_of(self.strokes.iter().flatten()).map(Outline::Frame),
        }
    }

    /// The drawing with its outline dragged to `to`. A line or arrow is
    /// drawn again between its new ends, so an arrow's head keeps its shape;
    /// anything else has its points stretched to the new box.
    pub fn reshaped(&self, to: &Outline) -> Drawing {
        let strokes = match (self.outline(), to) {
            (Some(Outline::Ends(..)), Outline::Ends(a, b)) => self.redrawn(vec![*a, *b], self.width),
            (Some(Outline::Frame(from)), Outline::Frame(to)) => self
                .strokes
                .iter()
                .map(|stroke| stroke.iter().map(|&p| shape::refit(p, from, *to)).collect())
                .collect(),
            _ => self.strokes.clone(),
        };
        Drawing { strokes, ..self.clone() }
    }

    /// The drawing in another colour or thickness. An arrow's head is sized
    /// from its line, so it is drawn again.
    pub fn restyled(&self, colour: Rgb, width: f64, fill: Option<Rgb>) -> Drawing {
        let strokes = match self.ends() {
            Some((a, b)) => self.redrawn(vec![a, b], width),
            None => self.strokes.clone(),
        };
        let fill = if self.tool.fillable() { fill } else { None };
        Drawing { strokes, colour, width, fill, ..self.clone() }
    }

    /// A line or arrow's strokes, between the ends given.
    fn redrawn(&self, ends: Vec<(f64, f64)>, width: f64) -> Vec<Vec<(f64, f64)>> {
        Mark { tool: self.tool, points: ends, colour: self.colour.to_rgba(), width, fill: None, sequence: 0 }.strokes()
    }

    /// Whether a press at `p` lands on the drawing's ink, give or take
    /// `slack`.
    pub fn is_at(&self, p: (f64, f64), slack: f64) -> bool {
        shape::touches(&self.strokes, self.width, p, slack) || (self.fill.is_some() && shape::encloses(&self.strokes, p))
    }

    /// The same drawing, as near as the file keeps it: the points come back
    /// a hair off what was written.
    fn same_strokes(&self, strokes: &[Vec<(f64, f64)>]) -> bool {
        const CLOSE: f64 = 0.05;
        self.strokes.len() == strokes.len()
            && self.strokes.iter().zip(strokes).all(|(a, b)| {
                a.len() == b.len() && a.iter().zip(b).all(|(p, q)| (p.0 - q.0).abs() < CLOSE && (p.1 - q.1).abs() < CLOSE)
            })
    }
}

/// What a drawing read back from a file was made with, as near as can be
/// told: a line or an arrow by its strokes, a highlighter by being see-
/// through, and anything else as if by pen. Only lines and arrows are held
/// differently once picked up, and only a highlighter is saved differently.
fn tool_of(strokes: &[Vec<(f64, f64)>], see_through: bool) -> Tool {
    let near = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() < 0.05 && (a.1 - b.1).abs() < 0.05;
    match strokes {
        _ if see_through => Tool::Highlighter,
        [shaft] if shaft.len() == 2 && !near(shaft[0], shaft[1]) => Tool::Line,
        [shaft, head] if shaft.len() == 2 && head.len() == 3 && near(head[1], shaft[1]) => Tool::Arrow,
        _ => Tool::Pen,
    }
}

/// The drawings on a page, read back from the document, including any
/// another program made.
pub fn on_page(document: &poppler::Document, index: usize) -> Vec<Drawing> {
    let Some(page) = annots::page(document, index) else { return Vec::new() };
    let (_, height) = page.size();
    annots::list(&page)
        .into_iter()
        .filter_map(|found| {
            let shaped = match found.kind {
                poppler::ffi::POPPLER_ANNOT_SQUARE => Some(Tool::Rectangle),
                poppler::ffi::POPPLER_ANNOT_CIRCLE => Some(Tool::Ellipse),
                poppler::ffi::POPPLER_ANNOT_INK => None,
                _ => return None,
            };
            if let Some(tool) = shaped {
                return shape_of(&found, tool, index);
            }
            let strokes = strokes_of(&found.annot, height)?;
            let width = width_of(&found.annot);
            // SAFETY: an ink annotation is a markup annotation.
            let opacity = unsafe {
                use glib::translate::ToGlibPtr;
                let raw: *mut poppler::ffi::PopplerAnnot = found.annot.to_glib_none().0;
                poppler::ffi::poppler_annot_markup_get_opacity(raw.cast())
            };
            Some(Drawing {
                page: index,
                tool: tool_of(&strokes, opacity < 0.99),
                strokes,
                colour: found.colour.unwrap_or(Rgb(0, 0, 0)),
                width,
                fill: None,
            })
        })
        .collect()
}

/// A square or circle annotation, as the rectangle or ellipse it draws.
fn shape_of(found: &annots::Found, tool: Tool, page: usize) -> Option<Drawing> {
    use glib::translate::ToGlibPtr;
    let width = width_of(&found.annot);
    let half = width / 2.0;
    let [x1, y1, x2, y2] = found.area;
    let corners = vec![(x1 + half, y1 + half), (x2 - half, y2 - half)];
    let raw: *mut poppler::ffi::PopplerAnnot = found.annot.to_glib_none().0;
    // SAFETY: the annotation is the kind asked for; the colour Poppler hands
    // back is ours to free.
    let fill = unsafe {
        let colour = match tool {
            Tool::Rectangle => poppler::ffi::poppler_annot_square_get_interior_color(raw.cast()),
            _ => poppler::ffi::poppler_annot_circle_get_interior_color(raw.cast()),
        };
        (!colour.is_null()).then(|| {
            let rgb = Rgb::from_ffi(&*colour);
            glib::ffi::g_free(colour.cast());
            rgb
        })
    };
    let colour = found.colour.unwrap_or(Rgb(0, 0, 0));
    let mark = Mark { tool, points: corners, colour: colour.to_rgba(), width, fill: None, sequence: 0 };
    Some(Drawing { page, tool, strokes: mark.strokes(), colour, width, fill })
}

/// An ink annotation's strokes, in points from the page's top-left corner.
fn strokes_of(annot: &poppler::Annot, height: f64) -> Option<Vec<Vec<(f64, f64)>>> {
    use glib::translate::ToGlibPtr;
    let ink = newer::get().ink.as_ref()?;
    let raw: *mut poppler::ffi::PopplerAnnot = annot.to_glib_none().0;
    let mut strokes = Vec::new();
    // SAFETY: Poppler's own calls; the list and its paths are ours to free,
    // the points in them are not.
    unsafe {
        let mut count = 0usize;
        let list = (ink.get_list)(raw, &mut count);
        if list.is_null() {
            return None;
        }
        for i in 0..count {
            let path = *list.add(i);
            if path.is_null() {
                continue;
            }
            let mut n = 0usize;
            let points = (ink.path_points)(path, &mut n);
            if !points.is_null() {
                strokes.push((0..n).map(|j| {
                    let p = *points.add(j);
                    (p.x, height - p.y)
                }).collect::<Vec<_>>());
            }
            (ink.path_free)(path);
        }
        glib::ffi::g_free(list.cast());
    }
    strokes.retain(|stroke| !stroke.is_empty());
    (!strokes.is_empty()).then_some(strokes)
}

/// How thick an annotation's line is, in points: 1 if it does not say, as
/// the PDF has it.
fn width_of(annot: &poppler::Annot) -> f64 {
    use glib::translate::ToGlibPtr;
    let Some(border) = newer::get().border.as_ref() else { return 1.0 };
    let mut width = 1.0;
    // SAFETY: Poppler's own call, writing one number.
    let known = unsafe { (border.get)(annot.to_glib_none().0, &mut width) };
    if known != 0 && width > 0.0 { width } else { 1.0 }
}

/// The drawing's annotation on its page, found by its strokes when its box
/// is not the one Glance would have given it: one made by another program.
pub(super) fn find(page: &poppler::Page, drawing: &Drawing) -> Option<poppler::Annot> {
    let (_, height) = page.size();
    annots::list(page)
        .into_iter()
        .filter(|found| found.kind == poppler::ffi::POPPLER_ANNOT_INK)
        .find(|found| strokes_of(&found.annot, height).is_some_and(|strokes| drawing.same_strokes(&strokes)))
        .map(|found| found.annot)
}

/// Whether this Poppler can draw at all: ink needs 25.06.
pub fn available() -> bool {
    newer::get().ink.is_some() && newer::get().border.is_some()
}

pub(super) fn key(page: &poppler::Page, drawing: &Drawing) -> (poppler::ffi::PopplerAnnotType, [f64; 4]) {
    let (_, height) = page.size();
    (drawing.kind(), annots::flip(drawing.area(), height))
}

pub(super) fn add(document: &poppler::Document, page: &poppler::Page, drawing: &Drawing) {
    use glib::translate::{from_glib_full, ToGlibPtr};

    let newer = newer::get();
    let (Some(ink), Some(border)) = (&newer.ink, &newer.border) else { return };
    let (_, height) = page.size();
    let mut rect = annots::rectangle(annots::flip(drawing.area(), height));
    if drawing.shape().is_some() {
        // SAFETY: Poppler's own calls; the colour is copied by Poppler.
        unsafe {
            let raw = match drawing.tool {
                Tool::Rectangle => poppler::ffi::poppler_annot_square_new(document.to_glib_none().0, &mut rect),
                _ => poppler::ffi::poppler_annot_circle_new(document.to_glib_none().0, &mut rect),
            };
            let annot: poppler::Annot = from_glib_full(raw);
            (border.set)(raw, drawing.width);
            let mut fill = drawing.fill.map(Rgb::ffi);
            let fill = fill.as_mut().map_or(std::ptr::null_mut(), std::ptr::from_mut);
            match drawing.tool {
                Tool::Rectangle => poppler::ffi::poppler_annot_square_set_interior_color(raw.cast(), fill),
                _ => poppler::ffi::poppler_annot_circle_set_interior_color(raw.cast(), fill),
            }
            annots::attach(page, &annot, Some(drawing.colour), true);
        }
        return;
    }
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
            fill: None,
        };
        assert_eq!(drawing.area(), [6.0, 16.0, 54.0, 24.0]);
    }

    #[test]
    fn what_a_drawing_was_made_with_is_told_from_its_strokes() {
        assert_eq!(tool_of(&[vec![(0.0, 0.0), (10.0, 5.0)]], false), Tool::Line);
        let arrow = Mark { tool: Tool::Arrow, points: vec![(0.0, 0.0), (100.0, 0.0)], colour: gtk::gdk::RGBA::BLACK, width: 2.0, fill: None, sequence: 0 };
        assert_eq!(tool_of(&arrow.strokes(), false), Tool::Arrow);
        assert_eq!(tool_of(&[vec![(0.0, 0.0), (10.0, 5.0)]], true), Tool::Highlighter);
        assert_eq!(tool_of(&[vec![(0.0, 0.0), (5.0, 5.0), (10.0, 0.0)]], false), Tool::Pen);
        assert_eq!(tool_of(&[vec![(3.0, 3.0), (3.0, 3.0)]], false), Tool::Pen, "a dot is not a line");
    }

    #[test]
    fn an_arrow_keeps_its_head_when_its_end_is_dragged() {
        let mark = Mark { tool: Tool::Arrow, points: vec![(0.0, 0.0), (100.0, 0.0)], colour: gtk::gdk::RGBA::BLACK, width: 2.0, fill: None, sequence: 0 };
        let arrow = Drawing::from_mark(0, &mark).unwrap();
        let Some(Outline::Ends(a, _)) = arrow.outline() else { panic!("held by its ends") };
        let turned = arrow.reshaped(&Outline::Ends(a, (0.0, 100.0)));
        // Pointing down now, the head drawn again at the new tip.
        assert_eq!(turned.strokes[0], vec![(0.0, 0.0), (0.0, 100.0)]);
        assert_eq!(turned.strokes[1][1], (0.0, 100.0));
        assert!(turned.strokes[1][0].1 < 100.0 && turned.strokes[1][2].1 < 100.0, "the head trails the tip");
        // Thicker, the head grows with it.
        let thick = arrow.restyled(Rgb(0, 0, 0), 8.0, None);
        let spread = |d: &Drawing| (d.strokes[1][0].1 - d.strokes[1][2].1).abs();
        assert!(spread(&thick) > spread(&arrow));
    }

    /// Saved, read back as another run would, moved and taken away: the
    /// whole life of a drawing picked up again.
    #[test]
    fn a_drawing_is_read_back_moved_and_removed() {
        use super::super::annots::{self, Annotation};
        use super::super::document;
        if !available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("glance-ink-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.pdf");
        // One blank A4 page.
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>",
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{object}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for offset in offsets {
            out.extend(format!("{offset:010} 00000 n \n").bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
        std::fs::write(&path, out).unwrap();
        let open = || poppler::Document::from_file(&document::uri(&path), None).unwrap();

        let pen = Mark {
            tool: Tool::Highlighter,
            points: vec![(100.0, 100.0), (150.0, 140.0), (200.0, 100.0)],
            colour: gtk::gdk::RGBA::new(1.0, 0.0, 0.0, 1.0),
            width: 6.0,
            fill: None,
            sequence: 0,
        };
        let arrow = Mark { tool: Tool::Arrow, points: vec![(300.0, 300.0), (400.0, 350.0)], ..pen.clone() };
        let arrow = Mark { tool: Tool::Arrow, width: 3.0, ..arrow };
        let drawings = [Drawing::from_mark(0, &pen).unwrap(), Drawing::from_mark(0, &arrow).unwrap()];
        let doc = open();
        for drawing in &drawings {
            Annotation::Ink(drawing.clone()).add(&doc);
        }
        annots::settle(&doc, &[0]);
        annots::save(&doc, &path).unwrap();

        let doc = open();
        let read = on_page(&doc, 0);
        assert_eq!(read.len(), 2);
        for (got, made) in read.iter().zip(&drawings) {
            assert!(got.same_strokes(&made.strokes), "{got:?} is not {made:?}");
            assert_eq!(got.tool, made.tool, "told apart by its strokes and how see-through it is");
            assert!((got.width - made.width).abs() < 0.01);
            assert_eq!(got.colour, made.colour);
        }
        // Found where it is, and moved.
        assert!(read[0].is_at((150.0, 141.0), 0.5));
        let Some(Outline::Frame([x, y, w, h])) = read[0].outline() else { panic!() };
        let moved = read[0].reshaped(&Outline::Frame([x + 50.0, y + 200.0, w, h]));
        assert!(Annotation::Ink(read[0].clone()).remove(&doc), "the drawing read back is found to take away");
        Annotation::Ink(moved.clone()).add(&doc);
        annots::settle(&doc, &[0]);
        annots::save(&doc, &path).unwrap();
        let doc = open();
        let read = on_page(&doc, 0);
        assert_eq!(read.len(), 2);
        assert!(read.iter().any(|d| d.same_strokes(&moved.strokes)), "moved where it was put");
        // And taken away.
        for drawing in &read {
            assert!(Annotation::Ink(drawing.clone()).remove(&doc));
        }
        annots::save(&doc, &path).unwrap();
        assert!(on_page(&open(), 0).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A rectangle and an ellipse are a PDF's own square and circle, filled
    /// or not, and come back from the file as what they were.
    #[test]
    fn shapes_are_saved_as_squares_and_circles_with_their_fill() {
        use super::super::annots::{self, Annotation};
        use super::super::document;
        if !available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("glance-shape-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.pdf");
        std::fs::write(&path, blank_page()).unwrap();
        let open = || poppler::Document::from_file(&document::uri(&path), None).unwrap();

        let yellow = gtk::gdk::RGBA::new(1.0, 0.8, 0.0, 1.0);
        let rectangle = Mark {
            tool: Tool::Rectangle,
            points: vec![(100.0, 100.0), (300.0, 200.0)],
            colour: gtk::gdk::RGBA::new(0.0, 0.0, 1.0, 1.0),
            width: 4.0,
            fill: Some(yellow),
            sequence: 0,
        };
        let ellipse = Mark { tool: Tool::Ellipse, points: vec![(100.0, 400.0), (250.0, 500.0)], fill: None, ..rectangle.clone() };
        let made = [Drawing::from_mark(0, &rectangle).unwrap(), Drawing::from_mark(0, &ellipse).unwrap()];
        assert_eq!(made[0].fill, Some(Rgb::from_rgba(&yellow)));
        let doc = open();
        for drawing in &made {
            Annotation::Ink(drawing.clone()).add(&doc);
        }
        annots::settle(&doc, &[0]);
        annots::save(&doc, &path).unwrap();

        let doc = open();
        let page = annots::page(&doc, 0).unwrap();
        let kinds: Vec<_> = annots::list(&page).iter().map(|f| f.kind).collect();
        assert_eq!(kinds, vec![poppler::ffi::POPPLER_ANNOT_SQUARE, poppler::ffi::POPPLER_ANNOT_CIRCLE]);
        let read = on_page(&doc, 0);
        assert_eq!(read.len(), 2);
        for (got, made) in read.iter().zip(&made) {
            assert_eq!((got.tool, got.fill, got.colour), (made.tool, made.fill, made.colour));
            let (a, b) = (got.shape().unwrap(), made.shape().unwrap());
            assert!(a.iter().zip(b).all(|(p, q)| (p - q).abs() < 0.05), "{a:?} is not {b:?}");
        }
        // The filled one is picked up by its middle, the empty one only by
        // its line.
        assert!(read[0].is_at((200.0, 150.0), 1.0));
        assert!(!read[1].is_at((175.0, 450.0), 1.0));
        assert!(read[1].is_at((100.0, 450.0), 1.0));

        // Emptied and moved, then taken away.
        let Some(Outline::Frame([x, y, w, h])) = read[0].outline() else { panic!() };
        let changed = read[0].restyled(read[0].colour, 2.0, None).reshaped(&Outline::Frame([x + 50.0, y, w, h]));
        assert!(Annotation::Ink(read[0].clone()).remove(&doc));
        Annotation::Ink(changed.clone()).add(&doc);
        annots::settle(&doc, &[0]);
        annots::save(&doc, &path).unwrap();
        let doc = open();
        let read = on_page(&doc, 0);
        let moved = read.iter().find(|d| d.tool == Tool::Rectangle).unwrap();
        assert_eq!(moved.fill, None);
        assert!((moved.width - 2.0).abs() < 0.01);
        assert!((moved.shape().unwrap()[0] - 150.0).abs() < 0.05);
        for drawing in &read {
            assert!(Annotation::Ink(drawing.clone()).remove(&doc), "{drawing:?} not found to remove");
        }
        annots::save(&doc, &path).unwrap();
        assert!(on_page(&open(), 0).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// One blank A4 page.
    fn blank_page() -> Vec<u8> {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>",
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{object}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for offset in offsets {
            out.extend(format!("{offset:010} 00000 n \n").bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
        out
    }

    #[test]
    fn a_click_with_a_shape_tool_draws_nothing() {
        let mark = Mark { tool: Tool::Rectangle, points: vec![(5.0, 5.0)], colour: gtk::gdk::RGBA::BLACK, width: 2.0, fill: None, sequence: 0 };
        assert_eq!(Drawing::from_mark(0, &mark), None);
    }
}
