// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Writing a PDF anew with qpdf: smaller, or with a different password and
//! permissions — and putting the result in place of the original.
//!
//! ## Smaller
//!
//! Every way starts with what changes nothing that shows: qpdf packs the
//! document's objects together, compresses what was left uncompressed, and
//! drops what nothing refers to. Most of a large PDF is usually its photos,
//! though, so the smaller settings also scale down photos that are sharper
//! than their page can show, and save them again as JPEG.
//!
//! Only photos are touched, and only when it is safe: eight-bit grey or
//! colour, no colour-keyed transparency, no unusual decoding. Drawings,
//! diagrams and screenshots stored without loss stay as they are, judged by
//! how few colours they use, since JPEG would blur their edges. A picture
//! that comes out no smaller is left as it was.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::qpdf::{self, Object, Protection, Qpdf};

/// How far to go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Level {
    /// Nothing that shows changes.
    Lossless,
    /// Photos at most `dpi` dots per inch of their page, as JPEG at `quality`.
    Photos { dpi: f64, quality: u8 },
}

pub struct Report {
    pub before: u64,
    pub after: u64,
    /// Photos made smaller.
    pub photos: usize,
}

/// Write a smaller copy of `source` to `out`, protected as the original is.
/// Runs on a worker thread.
pub fn shrink(source: &Path, password: Option<&str>, level: Level, out: &Path) -> Result<Report, String> {
    let before = std::fs::metadata(source).map_err(|e| e.to_string())?.len();
    let document = Qpdf::read(source, password).map_err(|e| e.to_string())?;
    let mut photos = 0;
    if let Level::Photos { dpi, quality } = level {
        for (image, cap) in pictures(&document, dpi) {
            if recompress(&document, image, cap, quality) {
                photos += 1;
            }
        }
    }
    document.write(out, Protection::Keep).map_err(|e| e.to_string())?;
    let after = std::fs::metadata(out).map_err(|e| e.to_string())?.len();
    Ok(Report { before, after, photos })
}

/// Every picture the pages show, once each, with the most pixels along its
/// longer side worth keeping: its largest page's longer side at `dpi`.
fn pictures(document: &Qpdf, dpi: f64) -> Vec<(Object, f64)> {
    let mut found: HashMap<(i32, i32), (Object, f64)> = HashMap::new();
    let mut forms = HashSet::new();
    for page in document.pages() {
        let longest = document
            .get(page, "MediaBox")
            .and_then(|b| document.items(b))
            .and_then(|b| {
                let n: Vec<f64> = b.iter().filter_map(|&v| document.number(v)).collect();
                (n.len() == 4).then(|| (n[2] - n[0]).abs().max((n[3] - n[1]).abs()))
            })
            .filter(|l| *l > 0.0)
            .unwrap_or(842.0);
        let cap = longest / 72.0 * dpi;
        if let Some(resources) = document.get(page, "Resources") {
            visit(document, resources, cap, &mut found, &mut forms, 0);
        }
    }
    found.into_values().collect()
}

fn visit(
    document: &Qpdf,
    resources: Object,
    cap: f64,
    found: &mut HashMap<(i32, i32), (Object, f64)>,
    forms: &mut HashSet<(i32, i32)>,
    depth: usize,
) {
    let Some(xobjects) = document.get(resources, "XObject") else { return };
    for key in document.keys(xobjects) {
        let Some(object) = document.get(xobjects, &key) else { continue };
        if !document.is_stream(object) {
            continue;
        }
        let id = document.id(object);
        match document.get(object, "Subtype").and_then(|s| document.name(s)).as_deref() {
            Some("Image") => {
                let entry = found.entry(id).or_insert((object, cap));
                entry.1 = entry.1.max(cap);
            }
            // A form can hold pictures of its own; the same form used twice
            // is looked in once.
            Some("Form") if depth < 8 && forms.insert(id) => {
                if let Some(inner) = document.get(object, "Resources") {
                    visit(document, inner, cap, found, forms, depth + 1);
                }
            }
            _ => {}
        }
    }
}

/// Colour channels of a picture Glance can safely re-encode: grey or RGB.
fn channels(document: &Qpdf, image: Object) -> Option<u8> {
    let space = document.get(image, "ColorSpace")?;
    if let Some(name) = document.name(space) {
        return match name.as_str() {
            "DeviceGray" => Some(1),
            "DeviceRGB" => Some(3),
            _ => None,
        };
    }
    // [/ICCBased stream], with the stream saying how many channels.
    let items = document.items(space)?;
    if items.len() == 2 && document.name(items[0]).as_deref() == Some("ICCBased") {
        return match document.get(items[1], "N").and_then(|n| document.integer(n)) {
            Some(1) => Some(1),
            Some(3) => Some(3),
            _ => None,
        };
    }
    None
}

/// The picture's filters, in order.
fn filters(document: &Qpdf, image: Object) -> Option<Vec<String>> {
    let Some(filter) = document.get(image, "Filter") else { return Some(Vec::new()) };
    if let Some(name) = document.name(filter) {
        return Some(vec![name]);
    }
    document.items(filter)?.into_iter().map(|f| document.name(f)).collect()
}

/// Re-encode one picture if that is safe and makes it smaller.
fn recompress(document: &Qpdf, image: Object, cap: f64, quality: u8) -> bool {
    if document.get(image, "ImageMask").and_then(|m| document.boolean(m)) == Some(true)
        || document.get(image, "Decode").is_some()
        // A colour-keyed mask names exact colours, which JPEG would not keep.
        || document.get(image, "Mask").is_some_and(|m| document.items(m).is_some())
        || document.get(image, "BitsPerComponent").and_then(|b| document.integer(b)) != Some(8)
    {
        return false;
    }
    let Some(channels) = channels(document, image) else { return false };
    let size = |key| document.get(image, key).and_then(|v| document.integer(v)).and_then(|v| u32::try_from(v).ok());
    let (Some(width), Some(height)) = (size("Width"), size("Height")) else { return false };
    if width < 16 || height < 16 {
        return false;
    }
    let Some(filters) = filters(document, image) else { return false };
    let Some(stored) = document.stream_data(image, false) else { return false };

    let pixels = if filters == ["DCTDecode"] {
        match decode_jpeg(&stored, channels) {
            Some(pixels) if (pixels.width(), pixels.height()) == (width, height) => pixels,
            _ => return false,
        }
    } else {
        let lossless = ["FlateDecode", "LZWDecode", "ASCIIHexDecode", "ASCII85Decode", "RunLengthDecode"];
        if !filters.iter().all(|f| lossless.contains(&f.as_str())) {
            return false;
        }
        let Some(raw) = document.stream_data(image, true) else { return false };
        let Some(pixels) = from_raw(raw, width, height, channels) else { return false };
        if looks_drawn(&pixels) {
            return false;
        }
        pixels
    };

    let longest = f64::from(width.max(height));
    let scale = (cap / longest).min(1.0);
    let (new_width, new_height) = if scale < 1.0 {
        ((f64::from(width) * scale).round().max(1.0) as u32, (f64::from(height) * scale).round().max(1.0) as u32)
    } else {
        (width, height)
    };
    let resized = if (new_width, new_height) == (width, height) {
        pixels
    } else {
        pixels.resize_exact(new_width, new_height, image::imageops::FilterType::CatmullRom)
    };
    let Some(jpeg) = encode_jpeg(&resized, channels, quality) else { return false };
    // Not worth a generation of JPEG loss for less than a tenth.
    if jpeg.len() as f64 > stored.len() as f64 * 0.9 {
        return false;
    }
    document.replace_with_jpeg(image, &jpeg, new_width, new_height);
    true
}

fn decode_jpeg(bytes: &[u8], channels: u8) -> Option<image::DynamicImage> {
    let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg).ok()?;
    Some(if channels == 1 {
        image::DynamicImage::ImageLuma8(decoded.to_luma8())
    } else {
        image::DynamicImage::ImageRgb8(decoded.to_rgb8())
    })
}

fn from_raw(raw: Vec<u8>, width: u32, height: u32, channels: u8) -> Option<image::DynamicImage> {
    let expected = width as usize * height as usize * usize::from(channels);
    if raw.len() < expected {
        return None;
    }
    let mut raw = raw;
    raw.truncate(expected);
    if channels == 1 {
        image::GrayImage::from_raw(width, height, raw).map(image::DynamicImage::ImageLuma8)
    } else {
        image::RgbImage::from_raw(width, height, raw).map(image::DynamicImage::ImageRgb8)
    }
}

fn encode_jpeg(pixels: &image::DynamicImage, channels: u8, quality: u8) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    let result = if channels == 1 {
        pixels.to_luma8().write_with_encoder(encoder)
    } else {
        pixels.to_rgb8().write_with_encoder(encoder)
    };
    result.ok().map(|()| out)
}

/// A drawing, chart or screenshot, rather than a photo: few distinct colours
/// across a sample of its pixels.
fn looks_drawn(pixels: &image::DynamicImage) -> bool {
    let rgb = pixels.to_rgb8();
    let total = rgb.pixels().len();
    let step = (total / 20_000).max(1);
    let colours: HashSet<[u8; 3]> = rgb.pixels().step_by(step).map(|p| p.0).collect();
    colours.len() < 256
}

/// What the document's permissions are, whoever opened it.
pub fn permissions(source: &Path, password: Option<&str>) -> Option<qpdf::Permissions> {
    Qpdf::read(source, password).ok().map(|document| document.permissions())
}

/// Re-encrypt or decrypt `source` into `out`. Runs on a worker thread.
pub fn protect(source: &Path, password: Option<&str>, protection: Protection<'_>, out: &Path) -> Result<(), String> {
    let document = Qpdf::read(source, password).map_err(|e| e.to_string())?;
    document.write(out, protection).map_err(|e| e.to_string())
}

/// Whether `password` opens the document at `source` — for checking a file
/// written here before it replaces the original.
pub fn opens_with(source: &Path, password: Option<&str>) -> bool {
    !matches!(Qpdf::read(source, password), Err(qpdf::Error::Password))
}

/// Where to write a new version of `target`: beside it, so it can be moved
/// into place in one step, and hidden while it is being written.
pub fn temporary_beside(target: &Path, purpose: &str) -> std::path::PathBuf {
    let name = target.file_name().map_or_else(|| "document.pdf".into(), |n| n.to_string_lossy().into_owned());
    target.with_file_name(format!(".{name}.glance-{purpose}-{}", std::process::id()))
}

/// Put the finished `temporary` in place of `target`, with `target`'s
/// permissions, so a crash leaves one whole file or the other.
pub fn install(temporary: &Path, target: &Path) -> Result<(), String> {
    let io = |e: std::io::Error| e.to_string();
    let target = std::fs::canonicalize(target).map_err(io)?;
    let permissions = std::fs::metadata(&target).map_err(io)?.permissions();
    if permissions.readonly() {
        return Err("the file is read-only".into());
    }
    std::fs::set_permissions(temporary, permissions).map_err(io)?;
    std::fs::File::open(temporary).and_then(|file| file.sync_all()).map_err(io)?;
    std::fs::rename(temporary, &target).map_err(io)
}

/// Keep `temporary` as a copy at `destination`, which may be on another disk.
pub fn keep_as(temporary: &Path, destination: &Path) -> Result<(), String> {
    if std::fs::rename(temporary, destination).is_ok() {
        return Ok(());
    }
    std::fs::copy(temporary, destination).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(temporary);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One A4 page showing two pictures, as PDF objects written by hand: a
    /// large JPEG photo, and an uncompressed drawing of two flat colours.
    fn pictured_pdf(path: &Path) {
        let photo = image::RgbImage::from_fn(2400, 1800, |x, y| {
            let n = (x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)) % 37;
            image::Rgb([((x / 10) % 256) as u8 ^ n as u8, ((y / 8) % 256) as u8, ((x + y) / 17 % 256) as u8])
        });
        let jpeg = encode_jpeg(&image::DynamicImage::ImageRgb8(photo), 3, 95).unwrap();
        let drawing: Vec<u8> = (0..400 * 300).flat_map(|i| if i % 400 < 200 { [255, 255, 255] } else { [0, 0, 200] }).collect();
        let stream = |dict: String, data: &[u8]| [dict.into_bytes(), b"\nstream\n".to_vec(), data.to_vec(), b"\nendstream".to_vec()].concat();
        let content = b"q 500 0 0 375 47 400 cm /Im1 Do Q q 200 0 0 150 47 100 cm /Im2 Do Q";
        let objects: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R \
              /Resources << /XObject << /Im1 5 0 R /Im2 6 0 R >> >> >>"
                .to_vec(),
            stream(format!("<< /Length {} >>", content.len()), content),
            stream(
                format!("<< /Type /XObject /Subtype /Image /Width 2400 /Height 1800 /ColorSpace /DeviceRGB \
                         /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>", jpeg.len()),
                &jpeg,
            ),
            stream(
                format!("<< /Type /XObject /Subtype /Image /Width 400 /Height 300 /ColorSpace /DeviceRGB \
                         /BitsPerComponent 8 /Length {} >>", drawing.len()),
                &drawing,
            ),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n", i + 1).bytes());
            out.extend(object);
            out.extend(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for offset in offsets {
            out.extend(format!("{offset:010} 00000 n \n").bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
        std::fs::write(path, out).unwrap();
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("glance-rewrite-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn poppler(path: &Path, password: Option<&str>) -> Result<poppler::Document, gtk::glib::Error> {
        poppler::Document::from_file(&super::super::document::uri(path), password)
    }

    #[test]
    fn photos_are_scaled_down_and_drawings_left_alone() {
        let dir = scratch("shrink");
        let (original, lossless, smaller) = (dir.join("in.pdf"), dir.join("lossless.pdf"), dir.join("small.pdf"));
        pictured_pdf(&original);

        let kept = shrink(&original, None, Level::Lossless, &lossless).unwrap();
        assert_eq!(kept.photos, 0);
        assert!(kept.after < kept.before, "the loose drawing gets compressed: {} → {}", kept.before, kept.after);

        let report = shrink(&original, None, Level::Photos { dpi: 96.0, quality: 55 }, &smaller).unwrap();
        assert_eq!(report.photos, 1, "the photo, and not the drawing");
        assert!(report.after * 3 < report.before, "{} → {}", report.before, report.after);

        // The photo now fits A4's long side at 96 dpi: 842 / 72 * 96 = 1123.
        let document = Qpdf::read(&smaller, None).unwrap();
        let mut sizes: Vec<(i32, i32, Option<Vec<String>>)> = pictures(&document, 96.0)
            .into_iter()
            .map(|(image, _)| {
                let get = |key| document.get(image, key).and_then(|v| document.integer(v)).unwrap();
                (get("Width"), get("Height"), filters(&document, image))
            })
            .collect();
        sizes.sort();
        assert_eq!(sizes[0], (400, 300, Some(vec!["FlateDecode".to_string()])), "the drawing kept its pixels");
        assert_eq!(sizes[1], (1123, 842, Some(vec!["DCTDecode".to_string()])));
        // And Poppler still reads it.
        assert_eq!(poppler(&smaller, None).unwrap().n_pages(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_protected_copy_opens_only_with_its_password() {
        let dir = scratch("protect");
        let (original, locked, freed) = (dir.join("in.pdf"), dir.join("locked.pdf"), dir.join("freed.pdf"));
        pictured_pdf(&original);
        let allow = qpdf::Permissions { print: false, copy: false, annotate: true, change: true };
        protect(&original, None, Protection::Set { open: "open sesame", owner: "owner", allow }, &locked).unwrap();

        assert!(poppler(&locked, None).is_err(), "no password, no document");
        assert!(poppler(&locked, Some("wrong")).is_err());
        assert!(!opens_with(&locked, Some("wrong")));
        assert!(opens_with(&locked, Some("open sesame")));
        // Opened with the password to open, the restrictions hold...
        let reader = super::super::document::Allowed::of(&poppler(&locked, Some("open sesame")).unwrap());
        assert!(!reader.copy && !reader.print && reader.annotate && !reader.everything);
        // ...and with the permissions password, they do not.
        assert!(super::super::document::Allowed::of(&poppler(&locked, Some("owner")).unwrap()).everything);
        assert_eq!(permissions(&locked, Some("owner")), Some(allow), "read back as written, whoever asks");

        // The permissions password can take it all off again.
        protect(&locked, Some("owner"), Protection::Remove, &freed).unwrap();
        let free = poppler(&freed, None).expect("opens without a password");
        assert!(super::super::document::Allowed::of(&free).everything);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn photos_are_told_from_drawings() {
        let flat = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(200, 200, |x, _| {
            if x < 100 { image::Rgb([255, 255, 255]) } else { image::Rgb([20, 40, 200]) }
        }));
        assert!(looks_drawn(&flat));
        let photo = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(200, 200, |x, y| {
            image::Rgb([(x * 7 % 256) as u8, (y * 5 % 256) as u8, ((x + y) * 3 % 256) as u8])
        }));
        assert!(!looks_drawn(&photo));
    }

    #[test]
    fn raw_pixels_must_fill_the_picture() {
        assert!(from_raw(vec![0; 10 * 10 * 3], 10, 10, 3).is_some());
        assert!(from_raw(vec![0; 10 * 10 * 3 - 1], 10, 10, 3).is_none());
        // Padding some writers leave at the end is ignored.
        assert!(from_raw(vec![0; 10 * 10 + 7], 10, 10, 1).is_some());
    }
}
