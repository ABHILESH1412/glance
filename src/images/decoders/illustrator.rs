// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Adobe Illustrator artwork. Since Illustrator 9 an `.ai` file carries a PDF
//! of the artwork inside it, for other programs to show, so Poppler draws it:
//! the first artboard, on transparency, at a size worth zooming into.
//!
//! Older Illustrator files are PostScript programs, which nothing here runs.

use std::io::Read;
use std::path::Path;

use gtk::cairo;

use crate::images::loader::{open_error, unsupported, LoadedImage};

/// How many pixels the longer side of the artwork is drawn at.
const LONGEST: f64 = 4096.0;

/// Whether a file is Illustrator artwork: by its name, as the PDF inside it
/// would otherwise make it a document.
pub fn is_illustrator(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ai"))
}

pub fn decode(path: &Path) -> Result<LoadedImage, String> {
    let mut head = [0u8; 1024];
    let read = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).map_err(|e| open_error(path, &e))?;
    if !head[..read].windows(5).any(|w| w == b"%PDF-") {
        let name = path.file_name().map_or_else(|| "This file".into(), |n| format!("“{}”", n.to_string_lossy()));
        return Err(format!(
            "{name} was saved by an Illustrator older than version 9, with no PDF inside it, so it cannot be shown."
        ));
    }
    let uri = crate::pdf::document_uri(path);
    let document = poppler::Document::from_file(&uri, None).map_err(|e| unsupported(path, e))?;
    let artboards = document.n_pages();
    let page = document.page(0).ok_or_else(|| unsupported(path, "no artwork in it"))?;
    let (w, h) = page.size();
    if w <= 0.0 || h <= 0.0 {
        return Err(unsupported(path, "artwork with no size"));
    }
    let scale = LONGEST / w.max(h);
    let (width, height) = ((w * scale).round().max(1.0) as i32, (h * scale).round().max(1.0) as i32);
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height).map_err(|e| unsupported(path, e))?;
    {
        let cr = cairo::Context::new(&surface).map_err(|e| unsupported(path, e))?;
        cr.scale(f64::from(width) / w, f64::from(height) / h);
        page.render(&cr);
    }
    surface.flush();
    let stride = usize::try_from(surface.stride()).unwrap_or(0);
    let data = surface.data().map_err(|e| unsupported(path, e))?;
    let (uw, uh) = (width as usize, height as usize);
    let mut rgba = Vec::with_capacity(uw * uh * 4);
    for row in data.chunks(stride).take(uh) {
        for px in row[..uw * 4].as_chunks::<4>().0 {
            // Cairo's native-endian, premultiplied ARGB.
            let [b, g, r, a] = u32::from_ne_bytes(*px).to_le_bytes();
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }
    Ok(LoadedImage {
        width: width as u32,
        height: height as u32,
        rgba,
        premultiplied: true,
        label: if artboards > 1 { format!("Illustrator · first of {artboards} artboards") } else { "Illustrator".to_string() },
        animation: Vec::new(),
        vector: None,
    })
}
