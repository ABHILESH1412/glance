// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Opening a PDF, and turning one page into pixels.
//!
//! Poppler objects are not safe to share between threads, so nothing here holds
//! on to one: every caller opens its own document on the thread that uses it,
//! and only plain data — page sizes, pixels — crosses back to the window.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use gtk::prelude::*;
use gtk::{cairo, gio, glib};

use super::layout::{self, Rotation};

/// What the window needs to lay out a document before any page is drawn.
pub struct Opened {
    pub path: PathBuf,
    /// Poppler opens files by URI.
    pub uri: String,
    /// Every page's size in points, with its rotation already applied.
    pub pages: Vec<(f64, f64)>,
}

/// One rendered page: cairo's ARGB32, premultiplied, in native byte order.
pub struct Pixels {
    pub width: i32,
    pub height: i32,
    pub stride: usize,
    pub data: Vec<u8>,
}

/// Whether a file is a PDF, judged by its content. The PDF header may sit
/// anywhere in the first kilobyte, so that much is read, not the first five
/// bytes.
pub fn is_pdf(path: &Path) -> bool {
    let mut head = [0u8; 1024];
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let n = file.read(&mut head).unwrap_or(0);
    head[..n].windows(5).any(|w| w == b"%PDF-")
}

pub fn uri(path: &Path) -> String {
    gio::File::for_path(path).uri().to_string()
}

/// Open a document for its page sizes. Runs on a worker thread.
pub fn open(path: &Path) -> Result<Opened, String> {
    let uri = uri(path);
    let document = poppler::Document::from_file(&uri, None).map_err(|e| describe(path, &e))?;
    let count = document.n_pages();
    if count <= 0 {
        return Err(format!("“{}” has no pages.", name(path)));
    }
    let pages = (0..count)
        // A page Poppler cannot read is laid out at US Letter rather than
        // dropped, so the page numbers after it stay right.
        .map(|i| document.page(i).map_or((612.0, 792.0), |page| page.size()))
        .collect();
    Ok(Opened { path: path.to_path_buf(), uri, pages })
}

/// Draw one page at `scale` device pixels per point, turned by `rotation`, on
/// white. The scale is lowered if it would make the page larger than
/// `layout::MAX_PIXELS`.
pub fn render_page(
    document: &poppler::Document,
    index: usize,
    scale: f64,
    rotation: Rotation,
) -> Option<Pixels> {
    let page = document.page(i32::try_from(index).ok()?)?;
    let (page_w, page_h) = page.size();
    let (width_pt, height_pt) = rotation.size(page_w, page_h);
    let scale = layout::render_scale(width_pt, height_pt, scale);
    let width = (width_pt * scale).round().max(1.0) as i32;
    let height = (height_pt * scale).round().max(1.0) as i32;

    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height).ok()?;
    {
        let cr = cairo::Context::new(&surface).ok()?;
        // A PDF page has no background of its own; paper is white.
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.paint().ok()?;
        // Scale by the rounded size rather than `scale`, so the page fills the
        // surface exactly instead of leaving a sliver at one edge.
        cr.scale(f64::from(width) / width_pt, f64::from(height) / height_pt);
        // Turn about the page's corner, then move it back into view: the same
        // transform as `Rotation::apply`.
        match rotation.quarters() {
            1 => cr.translate(page_h, 0.0),
            2 => cr.translate(page_w, page_h),
            3 => cr.translate(0.0, page_w),
            _ => {}
        }
        cr.rotate(rotation.radians());
        page.render(&cr);
    }
    surface.flush();
    let stride = usize::try_from(surface.stride()).ok()?;
    let data = surface.data().ok()?.to_vec();
    Some(Pixels { width, height, stride, data })
}

/// A point on a page, in the PDF's own terms: points, top-left origin, before
/// any turning Glance has done for display.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub page: usize,
    pub x: f64,
    pub y: f64,
}

/// How much one press selects: a character at a time while dragging, a word
/// for a double-click, a line for a triple.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Glyph,
    Word,
    Line,
}

impl Unit {
    fn style(self) -> poppler::SelectionStyle {
        match self {
            Unit::Glyph => poppler::SelectionStyle::Glyph,
            Unit::Word => poppler::SelectionStyle::Word,
            Unit::Line => poppler::SelectionStyle::Line,
        }
    }
}

/// Put two spots in reading order: earlier page first, then higher on the
/// page, then further left.
pub fn ordered(a: Spot, b: Spot) -> (Spot, Spot) {
    let key = |s: &Spot| (s.page, s.y, s.x);
    if key(&b) < key(&a) { (b, a) } else { (a, b) }
}

/// What to ask Poppler for on each page a selection covers, as start and end
/// points. The first page runs from where the drag began to its end, the last
/// from its start to where the drag is now, and any between are taken whole.
pub fn spans(from: Spot, to: Spot, sizes: &[(f64, f64)]) -> Vec<(usize, [f64; 4])> {
    let (a, b) = ordered(from, to);
    (a.page..=b.page.min(sizes.len().saturating_sub(1)))
        .map(|page| {
            let (w, h) = sizes[page];
            let (x1, y1) = if page == a.page { (a.x, a.y) } else { (0.0, 0.0) };
            let (x2, y2) = if page == b.page { (b.x, b.y) } else { (w, h) };
            (page, [x1, y1, x2, y2])
        })
        .collect()
}

fn rectangle(span: [f64; 4]) -> poppler::Rectangle {
    let mut r = poppler::Rectangle::new();
    r.set_x1(span[0]);
    r.set_y1(span[1]);
    r.set_x2(span[2]);
    r.set_y2(span[3]);
    r
}

/// The areas to highlight for one page's part of a selection, as x, y, width
/// and height in points. Poppler answers in whole units of whatever scale it
/// is asked at, so it is asked at four times and divided back down: a quarter
/// of a point is fine enough not to show at any zoom.
pub fn highlights(document: &poppler::Document, page: usize, span: [f64; 4], unit: Unit) -> Vec<[f64; 4]> {
    const FINE: f64 = 4.0;
    let Some(page) = i32::try_from(page).ok().and_then(|i| document.page(i)) else {
        return Vec::new();
    };
    let Some(region) = page.selected_region(FINE, unit.style(), &mut rectangle(span)) else {
        return Vec::new();
    };
    (0..region.num_rectangles())
        .map(|i| {
            let r = region.rectangle(i);
            [
                f64::from(r.x()) / FINE,
                f64::from(r.y()) / FINE,
                f64::from(r.width()) / FINE,
                f64::from(r.height()) / FINE,
            ]
        })
        .collect()
}

/// The text of one page's part of a selection.
pub fn selected_text(document: &poppler::Document, page: usize, span: [f64; 4], unit: Unit) -> String {
    i32::try_from(page)
        .ok()
        .and_then(|i| document.page(i))
        .and_then(|page| page.selected_text(unit.style(), &mut rectangle(span)))
        .map(|text| text.to_string())
        .unwrap_or_default()
}

/// A short message for the toast; the detail goes to stderr.
fn describe(path: &Path, error: &glib::Error) -> String {
    eprintln!("glance: {}: {}", path.display(), error.message());
    let name = name(path);
    match error.kind::<poppler::Error>() {
        Some(poppler::Error::Encrypted) => {
            format!("“{name}” is password-protected. Glance cannot open protected PDFs yet.")
        }
        Some(poppler::Error::OpenFile) => format!("Could not open “{name}”."),
        Some(poppler::Error::Damaged | poppler::Error::BadCatalog | poppler::Error::Invalid) => {
            format!("“{name}” is damaged and cannot be opened.")
        }
        _ => format!("“{name}” could not be opened as a PDF."),
    }
}

fn name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_is_found_anywhere_in_the_first_kilobyte() {
        let dir = std::env::temp_dir().join(format!("glance-pdf-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let late = dir.join("late.bin");
        std::fs::write(&late, [vec![b' '; 500], b"%PDF-1.7\n".to_vec()].concat()).unwrap();
        let png = dir.join("fake.pdf");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\n not a pdf at all").unwrap();
        assert!(is_pdf(&late), "a header after leading junk still counts");
        assert!(!is_pdf(&png), "the extension alone does not make a PDF");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    const A4: (f64, f64) = (595.0, 842.0);

    fn spot(page: usize, x: f64, y: f64) -> Spot {
        Spot { page, x, y }
    }

    #[test]
    fn a_selection_on_one_page_is_just_its_two_ends() {
        let got = spans(spot(0, 10.0, 20.0), spot(0, 300.0, 40.0), &[A4, A4]);
        assert_eq!(got, vec![(0, [10.0, 20.0, 300.0, 40.0])]);
    }

    #[test]
    fn dragging_backwards_selects_the_same_text_as_dragging_forwards() {
        let pages = [A4, A4, A4];
        let forward = spans(spot(0, 50.0, 100.0), spot(2, 80.0, 200.0), &pages);
        let backward = spans(spot(2, 80.0, 200.0), spot(0, 50.0, 100.0), &pages);
        assert_eq!(forward, backward);
        // Up a line on the same page, right to left, is still start to end.
        let up = spans(spot(0, 400.0, 300.0), spot(0, 60.0, 280.0), &pages);
        assert_eq!(up, vec![(0, [60.0, 280.0, 400.0, 300.0])]);
    }

    #[test]
    fn a_selection_across_pages_takes_the_middle_ones_whole() {
        let got = spans(spot(0, 50.0, 700.0), spot(2, 80.0, 90.0), &[A4, A4, A4]);
        assert_eq!(
            got,
            vec![
                (0, [50.0, 700.0, 595.0, 842.0]),
                (1, [0.0, 0.0, 595.0, 842.0]),
                (2, [0.0, 0.0, 80.0, 90.0]),
            ]
        );
    }
}
