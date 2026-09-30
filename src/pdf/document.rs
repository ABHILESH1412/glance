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

use super::layout;
use crate::images::loader::LoadedImage;

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
        return has_pdf_extension(path);
    };
    let n = file.read(&mut head).unwrap_or(0);
    head[..n].windows(5).any(|w| w == b"%PDF-")
}

/// The cheap check, for places that must not open every file in a folder.
pub fn has_pdf_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
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

/// Draw one page at `scale` device pixels per point, on white. The scale is
/// lowered if it would make the page larger than `layout::MAX_PIXELS`.
pub fn render_page(document: &poppler::Document, index: usize, scale: f64) -> Option<Pixels> {
    let page = document.page(i32::try_from(index).ok()?)?;
    let (width_pt, height_pt) = page.size();
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
        page.render(&cr);
    }
    surface.flush();
    let stride = usize::try_from(surface.stride()).ok()?;
    let data = surface.data().ok()?.to_vec();
    Some(Pixels { width, height, stride, data })
}

/// The first page, shrunk to fit a filmstrip slot and centred in it. Returned
/// in the same form as an image thumbnail so the strip needs no special case.
pub fn thumbnail(path: &Path, width: u32, height: u32) -> Result<LoadedImage, String> {
    let document = poppler::Document::from_file(&uri(path), None)
        .map_err(|e| e.message().to_string())?;
    let page = document.page(0).ok_or("no first page")?;
    let (page_w, page_h) = page.size();
    let scale = (f64::from(width) / page_w).min(f64::from(height) / page_h);
    let pixels = render_page(&document, 0, scale).ok_or("the first page did not render")?;
    Ok(LoadedImage {
        width,
        height,
        rgba: letterbox(&pixels, width, height),
        premultiplied: true,
        label: "PDF".to_string(),
        animation: Vec::new(),
        vector: None,
    })
}

/// Copy cairo pixels into the middle of a transparent RGBA canvas.
fn letterbox(pixels: &Pixels, width: u32, height: u32) -> Vec<u8> {
    let (width, height) = (width as usize, height as usize);
    let mut rgba = vec![0u8; width * height * 4];
    let (w, h) = (pixels.width as usize, pixels.height as usize);
    let (w, h) = (w.min(width), h.min(height));
    let (left, top) = ((width - w) / 2, (height - h) / 2);
    for y in 0..h {
        for x in 0..w {
            let at = y * pixels.stride + x * 4;
            // One native-endian 0xAARRGGBB word, whatever the byte order.
            let argb = u32::from_ne_bytes(pixels.data[at..at + 4].try_into().unwrap());
            let out = ((top + y) * width + left + x) * 4;
            rgba[out] = (argb >> 16) as u8;
            rgba[out + 1] = (argb >> 8) as u8;
            rgba[out + 2] = argb as u8;
            rgba[out + 3] = (argb >> 24) as u8;
        }
    }
    rgba
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
    fn letterboxing_centres_the_page_and_converts_the_channels() {
        // A 2x1 page: one opaque red pixel, one opaque blue, in cairo's order.
        let red = 0xFFFF_0000u32.to_ne_bytes();
        let blue = 0xFF00_00FFu32.to_ne_bytes();
        let pixels = Pixels { width: 2, height: 1, stride: 8, data: [red, blue].concat() };
        let rgba = letterbox(&pixels, 4, 3);
        let at = |x: usize, y: usize| &rgba[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
        assert_eq!(at(1, 1), [255, 0, 0, 255]);
        assert_eq!(at(2, 1), [0, 0, 255, 255]);
        // Everything around it stays transparent.
        assert_eq!(at(0, 0), [0, 0, 0, 0]);
        assert_eq!(at(3, 2), [0, 0, 0, 0]);
    }

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
        assert!(has_pdf_extension(&png));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
