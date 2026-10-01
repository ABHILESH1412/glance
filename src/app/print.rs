// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Printing, through GTK's print dialog, which also offers saving as a PDF.
//!
//! A PDF prints page by page with Poppler's own printing path. A picture
//! prints as it is on screen, edits included, at a natural size: 150 dots per
//! inch, so a small picture is not blown up into a blur, and anything larger
//! than the paper is shrunk to fit it. Either way, a page wider than it is
//! tall turns the paper to match.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::cairo;

/// A picture is printed at this many dots per inch, unless it does not fit.
const PICTURE_DPI: f64 = 150.0;
/// Pixels sent to the printer at most along a picture's longer side: about
/// 300 dots per inch across a large sheet. More only slows the spooler.
const MAX_SIDE: u32 = 4800;

/// Where to put something `width` × `height` on a printable area of
/// `area_w` × `area_h`, all in points: scaled down to fit if it must, never
/// up, and centred. Returns the scale and the top-left corner.
pub fn place(width: f64, height: f64, area_w: f64, area_h: f64) -> (f64, f64, f64) {
    let scale = (area_w / width.max(1.0)).min(area_h / height.max(1.0)).min(1.0);
    (scale, (area_w - width * scale) / 2.0, (area_h - height * scale) / 2.0)
}

fn orientation(width: f64, height: f64) -> gtk::PageOrientation {
    if width > height { gtk::PageOrientation::Landscape } else { gtk::PageOrientation::Portrait }
}

/// Report how printing went, once it has.
fn finish(operation: &gtk::PrintOperation, window: &crate::app::window::Window) {
    let window = window.downgrade();
    operation.connect_done(move |operation, result| {
        let Some(window) = window.upgrade() else { return };
        if result == gtk::PrintOperationResult::Error {
            let message = operation.error().map_or_else(|| "the printer did not say why".to_string(), |e| e.message().to_string());
            window.toast(&format!("Could not print: {message}"));
        }
    });
}

/// Print a PDF. `password` opens it if it is protected.
pub fn print_pdf(window: &crate::app::window::Window, uri: &str, password: Option<&str>, name: &str) {
    let document = match poppler::Document::from_file(uri, password) {
        Ok(document) => document,
        Err(error) => {
            window.toast(&format!("Could not print: {}", error.message()));
            return;
        }
    };
    let count = document.n_pages();
    if count <= 0 {
        return;
    }
    let operation = gtk::PrintOperation::new();
    operation.set_job_name(name);
    operation.set_n_pages(count);
    operation.set_unit(gtk::Unit::Points);
    operation.set_embed_page_setup(true);
    operation.set_allow_async(true);
    if let Some(first) = document.page(0) {
        let (w, h) = first.size();
        let setup = gtk::PageSetup::new();
        setup.set_orientation(orientation(w, h));
        operation.set_default_page_setup(Some(&setup));
    }
    let document = Rc::new(document);
    {
        let document = document.clone();
        operation.connect_request_page_setup(move |_, _, number, setup| {
            if let Some(page) = document.page(number) {
                let (w, h) = page.size();
                setup.set_orientation(orientation(w, h));
            }
        });
    }
    operation.connect_draw_page(move |_, context, number| {
        let Some(page) = document.page(number) else { return };
        let cr = context.cairo_context();
        let (w, h) = page.size();
        let (scale, x, y) = place(w, h, context.width(), context.height());
        cr.translate(x, y);
        cr.scale(scale, scale);
        page.render_for_printing(&cr);
    });
    finish(&operation, window);
    run(&operation, window);
}

/// Print a picture, given as its pixels.
pub fn print_picture(window: &crate::app::window::Window, picture: image::DynamicImage, name: &str) {
    let picture = if picture.width().max(picture.height()) > MAX_SIDE {
        picture.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle)
    } else {
        picture
    };
    let Some(surface) = surface(&picture) else {
        window.toast("Could not print: the picture is too large.");
        return;
    };
    // Its size on paper at the natural resolution, from the pixels it had.
    let (w, h) = (f64::from(picture.width()), f64::from(picture.height()));
    let (paper_w, paper_h) = (w * 72.0 / PICTURE_DPI, h * 72.0 / PICTURE_DPI);

    let operation = gtk::PrintOperation::new();
    operation.set_job_name(name);
    operation.set_n_pages(1);
    operation.set_unit(gtk::Unit::Points);
    operation.set_embed_page_setup(true);
    operation.set_allow_async(true);
    let setup = gtk::PageSetup::new();
    setup.set_orientation(orientation(w, h));
    operation.set_default_page_setup(Some(&setup));
    let surface = RefCell::new(surface);
    operation.connect_draw_page(move |_, context, _| {
        let cr = context.cairo_context();
        let (scale, x, y) = place(paper_w, paper_h, context.width(), context.height());
        cr.translate(x, y);
        cr.scale(scale * paper_w / w, scale * paper_h / h);
        let surface = surface.borrow();
        if cr.set_source_surface(&*surface, 0.0, 0.0).is_ok() {
            // Smooth when the printer's dots and the picture's pixels differ.
            cr.source().set_filter(cairo::Filter::Good);
            let _ = cr.paint();
        }
    });
    finish(&operation, window);
    run(&operation, window);
}

fn run(operation: &gtk::PrintOperation, window: &crate::app::window::Window) {
    if let Err(error) = operation.run(gtk::PrintOperationAction::PrintDialog, Some(window)) {
        window.toast(&format!("Could not print: {}", error.message()));
    }
}

/// A picture as a Cairo surface: premultiplied, in Cairo's byte order.
fn surface(picture: &image::DynamicImage) -> Option<cairo::ImageSurface> {
    let rgba = picture.to_rgba8();
    let (width, height) = (i32::try_from(rgba.width()).ok()?, i32::try_from(rgba.height()).ok()?);
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height).ok()?;
    let stride = usize::try_from(surface.stride()).ok()?;
    {
        let mut data = surface.data().ok()?;
        for (y, row) in rgba.rows().enumerate() {
            for (x, pixel) in row.enumerate() {
                let [r, g, b, a] = pixel.0;
                let premultiply = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
                let word = (u32::from(a) << 24)
                    | (u32::from(premultiply(r)) << 16)
                    | (u32::from(premultiply(g)) << 8)
                    | u32::from(premultiply(b));
                let at = y * stride + x * 4;
                data[at..at + 4].copy_from_slice(&word.to_ne_bytes());
            }
        }
    }
    surface.mark_dirty();
    Some(surface)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_that_fits_prints_at_its_own_size_in_the_middle() {
        // A5 on an A4 printable area: no scaling, centred.
        let (scale, x, y) = place(420.0, 595.0, 595.0, 842.0);
        assert_eq!(scale, 1.0);
        assert!((x - 87.5).abs() < 1e-9 && (y - 123.5).abs() < 1e-9);
    }

    #[test]
    fn a_page_too_big_is_shrunk_to_fit_whole() {
        // A4 on a printable area with margins: shrunk by the tighter side.
        let (scale, x, y) = place(595.0, 842.0, 559.0, 783.0);
        assert!((scale - 783.0 / 842.0).abs() < 1e-9);
        assert!(595.0 * scale <= 559.0 && x >= 0.0 && y.abs() < 1e-9);
    }

    #[test]
    fn wide_pages_turn_the_paper() {
        assert_eq!(orientation(842.0, 595.0), gtk::PageOrientation::Landscape);
        assert_eq!(orientation(595.0, 842.0), gtk::PageOrientation::Portrait);
    }

    #[test]
    fn pixels_reach_cairo_premultiplied() {
        let picture = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 128])));
        let mut surface = surface(&picture).unwrap();
        let data = surface.data().unwrap();
        let word = u32::from_ne_bytes([data[0], data[1], data[2], data[3]]);
        assert_eq!(word, 0x8080_0000, "alpha 128, red premultiplied to 128");
    }
}
