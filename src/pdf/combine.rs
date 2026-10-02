// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Combining pages from PDFs and pictures into one new PDF.
//!
//! A PDF's page is copied as it is, with qpdf: its text stays text, its links
//! and notes come along, and nothing is redrawn. A picture becomes a page of
//! its own, on paper of the reader's choosing or at its own size. A JPEG goes
//! in exactly as it was, not compressed a second time; anything else is
//! stored without loss, or as a good JPEG if it is a photograph. Turning a
//! page is only a note on it, so nothing is redrawn for that either.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::qpdf::{Object, Protection, Qpdf, Value};
use super::rewrite;

/// A picture is this many dots per inch at its own size.
const PICTURE_DPI: f64 = 150.0;
/// Room left round a picture fitted to paper, in points: about a centimetre.
const MARGIN: f64 = 28.0;

/// Where a page comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum Origin {
    /// A page of a PDF, counting from zero.
    Pdf { path: PathBuf, password: Option<String>, page: usize },
    Picture { path: PathBuf },
}

/// One page of the new document: where it comes from, and how many quarter
/// turns clockwise to add to it.
#[derive(Clone, Debug, PartialEq)]
pub struct Leaf {
    pub origin: Origin,
    pub turn: u8,
}

/// The paper a picture goes on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paper {
    A4,
    Letter,
    /// The picture's own size, at `PICTURE_DPI`.
    Own,
}

impl Paper {
    pub fn name(self) -> &'static str {
        match self {
            Paper::A4 => "a4",
            Paper::Letter => "letter",
            Paper::Own => "own",
        }
    }

    pub fn from_name(name: &str) -> Option<Paper> {
        match name {
            "a4" => Some(Paper::A4),
            "letter" => Some(Paper::Letter),
            "own" => Some(Paper::Own),
            _ => None,
        }
    }

    /// Width and height in points, portrait, or `None` for a picture's own.
    fn size(self) -> Option<(f64, f64)> {
        match self {
            Paper::A4 => Some((595.276, 841.89)),
            Paper::Letter => Some((612.0, 792.0)),
            Paper::Own => None,
        }
    }
}

/// The page a picture of `width` × `height` pixels goes on, and where on it
/// the picture sits: page width and height, then the picture's x, y, width
/// and height, all in points, from the bottom left as PDF has it.
pub fn picture_page(width: f64, height: f64, paper: Paper) -> ((f64, f64), [f64; 4]) {
    let natural = (width * 72.0 / PICTURE_DPI, height * 72.0 / PICTURE_DPI);
    let Some((short, long)) = paper.size() else {
        return (natural, [0.0, 0.0, natural.0, natural.1]);
    };
    // The paper turns to match the picture.
    let (pw, ph) = if width > height { (long, short) } else { (short, long) };
    let (room_w, room_h) = (pw - 2.0 * MARGIN, ph - 2.0 * MARGIN);
    // Fitted, but never blown up past its own size: a small picture stays sharp.
    let scale = (room_w / natural.0).min(room_h / natural.1).min(1.0);
    let (w, h) = (natural.0 * scale, natural.1 * scale);
    ((pw, ph), [(pw - w) / 2.0, (ph - h) / 2.0, w, h])
}

/// Pages chosen as people type them: "1-5, 8, 10-12". Counting from one,
/// given back counting from zero, in the order written. An empty answer is
/// every page.
pub fn parse_pages(text: &str, count: usize) -> Result<Vec<usize>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok((0..count).collect());
    }
    let mut pages = Vec::new();
    for part in text.split([',', ';']).map(str::trim).filter(|p| !p.is_empty()) {
        let number = |s: &str| -> Result<usize, String> {
            let n: usize = s.trim().parse().map_err(|_| format!("“{}” is not a page number.", s.trim()))?;
            if n == 0 || n > count {
                return Err(format!("There is no page {n}; it has {count}."));
            }
            Ok(n - 1)
        };
        // "3-", "–", and "to" all read as ranges.
        let part = part.replace(['–', '—'], "-").replace(" to ", "-");
        match part.split_once('-') {
            Some((from, to)) => {
                let from = if from.trim().is_empty() { 0 } else { number(from)? };
                let to = if to.trim().is_empty() { count - 1 } else { number(to)? };
                if from <= to {
                    pages.extend(from..=to);
                } else {
                    pages.extend((to..=from).rev());
                }
            }
            None => pages.push(number(&part)?),
        }
    }
    Ok(pages)
}

/// Write `leaves` to `out` as one PDF, pictures on `paper`. Runs on a worker
/// thread.
pub fn combine(leaves: &[Leaf], paper: Paper, out: &Path) -> Result<usize, String> {
    let document = Qpdf::empty().map_err(|e| e.to_string())?;
    // Each source is opened once, and kept open until the result is
    // written, which is when the pages copied from it are read.
    let mut opened: HashMap<PathBuf, (Qpdf, Vec<Object>)> = HashMap::new();
    for leaf in leaves {
        let page = match &leaf.origin {
            Origin::Pdf { path, password, page } => {
                if !opened.contains_key(path) {
                    let source = Qpdf::read(path, password.as_deref()).map_err(|e| format!("{}: {e}", name(path)))?;
                    let pages = source.pages();
                    opened.insert(path.clone(), (source, pages));
                }
                let (source, pages) = &opened[path];
                let original = *pages.get(*page).ok_or_else(|| format!("{} has no page {}", name(path), page + 1))?;
                document.add_page(source, original).map_err(|e| e.to_string())?
            }
            Origin::Picture { path } => {
                let page = picture(&document, path, paper)?;
                document.add_page(&document, page).map_err(|e| e.to_string())?
            }
        };
        if leaf.turn % 4 != 0 {
            let now = document.get(page, "Rotate").and_then(|r| document.integer(r)).unwrap_or(0);
            let turned = (now + i32::from(leaf.turn % 4) * 90).rem_euclid(360);
            document.set(page, "Rotate", Value::Integer(i64::from(turned)));
        }
    }
    document.write(out, Protection::Remove).map_err(|e| e.to_string())?;
    drop(opened);
    Ok(leaves.len())
}

fn name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// A page showing one picture.
fn picture(document: &Qpdf, path: &Path, paper: Paper) -> Result<Object, String> {
    let (image, width, height) = embed(document, path)?;
    let ((pw, ph), [x, y, w, h]) = picture_page(f64::from(width), f64::from(height), paper);
    let number = |v: f64| format!("{v:.3}");
    let content = format!("q\n{} 0 0 {} {} {} cm\n/Picture Do\nQ\n", number(w), number(h), number(x), number(y));
    let xobjects = document.new_dictionary(vec![("Picture", Value::Object(image))]);
    let resources = document.new_dictionary(vec![("XObject", Value::Object(xobjects))]);
    let contents = document.new_stream(content.as_bytes(), None, Vec::new());
    Ok(document.new_dictionary(vec![
        ("Type", Value::Name("Page")),
        ("MediaBox", Value::Array(vec![Value::Integer(0), Value::Integer(0), Value::Real(pw), Value::Real(ph)])),
        ("Resources", Value::Object(resources)),
        ("Contents", Value::Object(contents)),
    ]))
}

/// The picture as an image object, and its size in pixels.
fn embed(document: &Qpdf, path: &Path) -> Result<(Object, u32, u32), String> {
    let entries = |width: u32, height: u32, space: &'static str| {
        vec![
            ("Type", Value::Name("XObject")),
            ("Subtype", Value::Name("Image")),
            ("Width", Value::Integer(i64::from(width))),
            ("Height", Value::Integer(i64::from(height))),
            ("ColorSpace", Value::Name(space)),
            ("BitsPerComponent", Value::Integer(8)),
        ]
    };
    // A JPEG PDF can show as it is goes in as it is.
    if let Ok(bytes) = std::fs::read(path) {
        if let Some((width, height, channels)) = plain_jpeg(&bytes).filter(|_| upright(path)) {
            let space = if channels == 1 { "DeviceGray" } else { "DeviceRGB" };
            let image = document.new_stream(&bytes, Some("DCTDecode"), entries(width, height, space));
            return Ok((image, width, height));
        }
    }
    let decoded = crate::images::loader::decode(path).map_err(|e| format!("{}: {e}", name(path)))?;
    let (width, height) = (decoded.width, decoded.height);
    let rgba = image::RgbaImage::from_raw(width, height, decoded.rgba)
        .ok_or_else(|| format!("{}: the picture could not be read", name(path)))?;
    // On white, as paper is: see-through parts show the page.
    let premultiplied = decoded.premultiplied;
    let rgb = image::RgbImage::from_fn(width, height, |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let over = |c: u8| {
            let a = u16::from(a);
            let c = if premultiplied { u16::from(c) } else { u16::from(c) * a / 255 };
            (c + (255 - a)).min(255) as u8
        };
        image::Rgb([over(r), over(g), over(b)])
    });
    let picture = image::DynamicImage::ImageRgb8(rgb);
    // A photograph as a good JPEG; a drawing or screenshot without loss.
    let image = match (!rewrite::looks_drawn(&picture)).then(|| rewrite::encode_jpeg(&picture, 3, 92)).flatten() {
        Some(jpeg) => document.new_stream(&jpeg, Some("DCTDecode"), entries(width, height, "DeviceRGB")),
        None => document.new_stream(picture.as_bytes(), None, entries(width, height, "DeviceRGB")),
    };
    Ok((image, width, height))
}

/// Whether a picture needs no turning to stand the right way up.
fn upright(path: &Path) -> bool {
    use image::ImageDecoder;
    image::ImageReader::open(path)
        .ok()
        .and_then(|reader| reader.with_guessed_format().ok())
        .and_then(|reader| reader.into_decoder().ok())
        .and_then(|mut decoder| decoder.orientation().ok())
        .is_some_and(|orientation| orientation == image::metadata::Orientation::NoTransforms)
}

/// A JPEG a PDF can show as it is: eight bits, grey or colour, not CMYK.
/// Its width, height and channels.
fn plain_jpeg(bytes: &[u8]) -> Option<(u32, u32, u8)> {
    if bytes.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut at = 2;
    while at + 4 <= bytes.len() {
        if bytes[at] != 0xFF {
            return None;
        }
        let marker = bytes[at + 1];
        let length = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
        // The frame header: any start-of-frame but the table markers.
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let frame = bytes.get(at + 4..at + 2 + length)?;
            let (precision, height, width, channels) = (
                frame[0],
                u32::from(u16::from_be_bytes([frame[1], frame[2]])),
                u32::from(u16::from_be_bytes([frame[3], frame[4]])),
                frame[5],
            );
            return (precision == 8 && (channels == 1 || channels == 3) && width > 0 && height > 0)
                .then_some((width, height, channels));
        }
        at += 2 + length;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two A4 pages saying "Alpha" and "Bravo", written by hand.
    fn two_pages(path: &Path) {
        let page = |word: &str| format!("BT /F1 24 Tf 72 700 Td ({word}) Tj ET");
        let (a, b) = (page("Alpha"), page("Bravo"));
        let stream = |c: &str| format!("<< /Length {} >>\nstream\n{c}\nendstream", c.len());
        let objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 595 842] \
             /Resources << /Font << /F1 7 0 R >> >> >>"
                .to_string(),
            "<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>".to_string(),
            stream(&a),
            stream(&b),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
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
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn pdfs_and_pictures_become_one_document() {
        let dir = std::env::temp_dir().join(format!("glance-combine-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (pdf, photo, drawing, out) = (dir.join("two.pdf"), dir.join("photo.jpg"), dir.join("drawing.png"), dir.join("out.pdf"));
        two_pages(&pdf);
        let noise = image::RgbImage::from_fn(400, 300, |x, y| image::Rgb([(x * 7 % 251) as u8, (y * 5 % 241) as u8, ((x ^ y) % 256) as u8]));
        image::DynamicImage::ImageRgb8(noise).save(&photo).unwrap();
        image::RgbaImage::from_fn(200, 100, |x, _| if x < 100 { image::Rgba([0, 0, 255, 255]) } else { image::Rgba([0, 0, 0, 0]) })
            .save(&drawing)
            .unwrap();

        let page = |n| Origin::Pdf { path: pdf.clone(), password: None, page: n };
        let leaves = vec![
            Leaf { origin: page(1), turn: 1 },
            Leaf { origin: Origin::Picture { path: photo.clone() }, turn: 0 },
            Leaf { origin: page(0), turn: 0 },
            Leaf { origin: Origin::Picture { path: drawing.clone() }, turn: 2 },
        ];
        assert_eq!(combine(&leaves, Paper::A4, &out), Ok(4));

        let document = poppler::Document::from_file(&super::super::document::uri(&out), None).unwrap();
        assert_eq!(document.n_pages(), 4);
        let text = |i| document.page(i).unwrap().text().unwrap().to_string();
        assert!(text(0).contains("Bravo") && text(2).contains("Alpha"), "in the order chosen, text still text");
        let size = |i| {
            let (w, h) = document.page(i).unwrap().size();
            (w.round(), h.round())
        };
        assert_eq!(size(0), (842.0, 595.0), "turned a quarter, inherited size and all");
        assert_eq!(size(1), (842.0, 595.0), "a landscape photo on landscape A4");
        assert_eq!(size(2), (595.0, 842.0));
        assert_eq!(size(3), (842.0, 595.0), "turned upside down, still landscape");
        // The JPEG went in as it was, not compressed again.
        let jpeg = std::fs::read(&photo).unwrap();
        let written = std::fs::read(&out).unwrap();
        assert!(written.windows(jpeg.len()).any(|w| w == jpeg.as_slice()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pages_are_read_as_people_write_them() {
        assert_eq!(parse_pages("", 4), Ok(vec![0, 1, 2, 3]), "nothing is everything");
        assert_eq!(parse_pages("1-3, 5", 6), Ok(vec![0, 1, 2, 4]));
        assert_eq!(parse_pages("2 to 4; 6", 6), Ok(vec![1, 2, 3, 5]));
        assert_eq!(parse_pages("5-", 6), Ok(vec![4, 5]), "to the end");
        assert_eq!(parse_pages("3–1", 6), Ok(vec![2, 1, 0]), "backwards, with a dash");
        assert!(parse_pages("0", 6).is_err());
        assert!(parse_pages("7", 6).unwrap_err().contains("no page 7"));
        assert!(parse_pages("two", 6).is_err());
    }

    #[test]
    fn a_picture_fits_its_paper_and_turns_it() {
        // 3000 × 2000 pixels: landscape A4, shrunk to fit inside the margins.
        let ((pw, ph), [x, y, w, h]) = picture_page(3000.0, 2000.0, Paper::A4);
        assert_eq!((pw, ph), (841.89, 595.276));
        assert!((x - MARGIN).abs() < 1e-6 && w <= pw - 2.0 * MARGIN + 1e-6);
        assert!((w / h - 1.5).abs() < 1e-9, "its shape kept");
        assert!((y - (ph - h) / 2.0).abs() < 1e-9);
        // A small one is not blown up.
        let (_, [_, _, w, _]) = picture_page(300.0, 300.0, Paper::Letter);
        assert!((w - 144.0).abs() < 1e-9, "300 px at 150 dpi is two inches");
        // At its own size, the page is the picture.
        assert_eq!(picture_page(1500.0, 750.0, Paper::Own), ((720.0, 360.0), [0.0, 0.0, 720.0, 360.0]));
    }

    #[test]
    fn only_plain_jpegs_go_in_as_they_are() {
        // A minimal frame header: 8 bits, 16 high, 32 wide, then channels.
        let jpeg = |channels: u8| {
            let mut b = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
            b.extend([0xFF, 0xC0, 0x00, 0x0B, 8, 0, 16, 0, 32, channels, 1, 0x11, 0]);
            b
        };
        assert_eq!(plain_jpeg(&jpeg(3)), Some((32, 16, 3)));
        assert_eq!(plain_jpeg(&jpeg(1)), Some((32, 16, 1)));
        assert_eq!(plain_jpeg(&jpeg(4)), None, "CMYK is decoded instead");
        assert_eq!(plain_jpeg(b"\x89PNG"), None);
    }
}
