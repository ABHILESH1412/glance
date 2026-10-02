// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! A picture's resolution: how many of its pixels make an inch on paper.
//!
//! It changes nothing on screen. It is a number in the file, which a word
//! processor or a print dialog reads to decide how big to make the picture.
//! It is read from JPEG (its JFIF header, or the camera's EXIF), PNG, BMP and
//! TIFF, and written into JPEG, PNG and BMP, the formats whose encoders here
//! leave room for it. The other formats have nowhere to keep it, or keep it
//! only in metadata Glance does not write.

use std::io::Read;
use std::path::Path;

/// The most recording a resolution can add to a file, in bytes: a PNG's
/// chunk is 21, a JPEG header 18 when it has none.
pub const ROOM: u64 = 32;

/// What most programs assume when a file does not say.
pub const ASSUMED: f64 = 72.0;

const CM_PER_INCH: f64 = 2.54;
const METRES_PER_INCH: f64 = 0.0254;

/// How much of a file to look through. Every format's resolution comes long
/// before the pixels, and EXIF is at most 64 KB.
const HEAD: u64 = 256 * 1024;

/// Whether a file with this extension can record a resolution when Glance
/// writes it.
pub fn can_store(extension: &str) -> bool {
    matches!(extension.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png" | "bmp")
}

/// The resolution `path` records, in pixels per inch, if it records one.
pub fn read(path: &Path) -> Option<f64> {
    let mut head = Vec::new();
    std::fs::File::open(path).ok()?.take(HEAD).read_to_end(&mut head).ok()?;
    read_bytes(&head)
}

fn read_bytes(bytes: &[u8]) -> Option<f64> {
    let sensible = |dpi: f64| (dpi.is_finite() && (1.0..=100_000.0).contains(&dpi)).then_some(dpi);
    if bytes.starts_with(&[0xFF, 0xD8]) {
        jpeg(bytes).and_then(sensible)
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        png(bytes).and_then(sensible)
    } else if bytes.starts_with(b"BM") && bytes.len() >= 46 {
        let ppm = i32::from_le_bytes(bytes[38..42].try_into().ok()?);
        sensible(f64::from(ppm) * METRES_PER_INCH).map(|dpi| dpi.round())
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        tiff(bytes).and_then(sensible)
    } else {
        None
    }
}

/// The JFIF header if it gives units, otherwise the EXIF block.
fn jpeg(bytes: &[u8]) -> Option<f64> {
    let mut exif = None;
    let mut at = 2;
    while at + 4 <= bytes.len() && bytes[at] == 0xFF {
        let marker = bytes[at + 1];
        // Start of the picture data: nothing further is header.
        if marker == 0xDA {
            break;
        }
        let length = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
        let body = bytes.get(at + 4..at + 2 + length)?;
        if marker == 0xE0 && body.starts_with(b"JFIF\0") && body.len() >= 12 {
            let x = f64::from(u16::from_be_bytes([body[8], body[9]]));
            match body[7] {
                1 => return Some(x),
                2 => return Some(x * CM_PER_INCH),
                _ => {}
            }
        }
        if marker == 0xE1 && body.starts_with(b"Exif\0\0") {
            exif = tiff(&body[6..]);
        }
        at += 2 + length;
    }
    exif
}

fn png(bytes: &[u8]) -> Option<f64> {
    let mut at = 8;
    while at + 8 <= bytes.len() {
        let length = u32::from_be_bytes(bytes[at..at + 4].try_into().ok()?) as usize;
        let kind = &bytes[at + 4..at + 8];
        if kind == b"IDAT" {
            break;
        }
        if kind == b"pHYs" && length == 9 {
            let data = bytes.get(at + 8..at + 17)?;
            // Unit 1 is the metre; 0 is only an aspect ratio.
            if data[8] == 1 {
                let ppm = u32::from_be_bytes(data[0..4].try_into().ok()?);
                return Some((f64::from(ppm) * METRES_PER_INCH).round());
            }
            return None;
        }
        at += 12 + length;
    }
    None
}

/// The first directory of a TIFF structure: XResolution, in ResolutionUnit.
fn tiff(bytes: &[u8]) -> Option<f64> {
    let little = match bytes.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let b: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
        Some(if little { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
        Some(if little { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    };
    let directory = u32_at(4)? as usize;
    let count = usize::from(u16_at(directory)?);
    let mut resolution = None;
    // Inches unless it says otherwise.
    let mut unit = 2;
    for i in 0..count {
        let entry = directory + 2 + i * 12;
        match u16_at(entry)? {
            0x011A => {
                let at = u32_at(entry + 8)? as usize;
                let (top, bottom) = (u32_at(at)?, u32_at(at + 4)?);
                if bottom != 0 {
                    resolution = Some(f64::from(top) / f64::from(bottom));
                }
            }
            0x0128 => unit = u16_at(entry + 8)?,
            _ => {}
        }
    }
    match unit {
        2 => resolution,
        3 => resolution.map(|r| r * CM_PER_INCH),
        // No unit: an aspect ratio, not a size.
        _ => None,
    }
}

/// Record `dpi` in an encoded file, if it is a JPEG, PNG or BMP; anything
/// else comes back as it was.
pub fn stamp(mut bytes: Vec<u8>, dpi: f64) -> Vec<u8> {
    let dpi = dpi.clamp(1.0, 65_535.0);
    if bytes.starts_with(&[0xFF, 0xD8]) {
        let density = (dpi.round() as u16).to_be_bytes();
        let has_jfif = bytes.len() > 20 && bytes[2..4] == [0xFF, 0xE0] && &bytes[6..11] == b"JFIF\0";
        if has_jfif {
            bytes[13] = 1;
            bytes[14..16].copy_from_slice(&density);
            bytes[16..18].copy_from_slice(&density);
        } else {
            let mut app0 = vec![0xFF, 0xE0, 0x00, 0x10];
            app0.extend_from_slice(b"JFIF\0");
            app0.extend_from_slice(&[1, 1, 1]);
            app0.extend_from_slice(&density);
            app0.extend_from_slice(&density);
            app0.extend_from_slice(&[0, 0]);
            bytes.splice(2..2, app0);
        }
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let ppm = ((dpi / METRES_PER_INCH).round() as u32).to_be_bytes();
        let mut data = Vec::with_capacity(9);
        data.extend_from_slice(&ppm);
        data.extend_from_slice(&ppm);
        data.push(1);
        let mut chunk = 9u32.to_be_bytes().to_vec();
        chunk.extend_from_slice(b"pHYs");
        chunk.extend_from_slice(&data);
        chunk.extend_from_slice(&crc32(&chunk[4..]).to_be_bytes());
        // Replace one already there, or go straight after the header.
        let mut at = 8;
        let mut placed = false;
        while at + 8 <= bytes.len() {
            let length = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap_or_default()) as usize;
            let kind = bytes[at + 4..at + 8].to_vec();
            if kind == b"pHYs" {
                bytes.splice(at..at + 12 + length, chunk.clone());
                placed = true;
                break;
            }
            if kind == b"IDAT" {
                break;
            }
            at += 12 + length;
        }
        if !placed {
            // IHDR is always first, and always 13 bytes long.
            bytes.splice(33..33, chunk);
        }
    } else if bytes.starts_with(b"BM") && bytes.len() >= 46 {
        let ppm = ((dpi / METRES_PER_INCH).round() as i32).to_le_bytes();
        bytes[38..42].copy_from_slice(&ppm);
        bytes[42..46].copy_from_slice(&ppm);
    }
    bytes
}

/// The CRC that closes every PNG chunk.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, RgbImage};

    fn encoded(format: ImageFormat) -> Vec<u8> {
        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 6, image::Rgb([200, 100, 50])));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, format).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn a_stamped_resolution_reads_back_and_the_file_still_decodes() {
        for format in [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::Bmp] {
            for dpi in [72.0, 300.0, 96.0] {
                let bytes = stamp(encoded(format), dpi);
                assert_eq!(read_bytes(&bytes), Some(dpi), "{format:?} at {dpi}");
                assert!(bytes.len() as u64 <= encoded(format).len() as u64 + ROOM);
                let decoded = image::load_from_memory(&bytes).expect("still a picture");
                assert_eq!((decoded.width(), decoded.height()), (8, 6));
            }
            // Stamping twice replaces rather than piles up.
            let twice = stamp(stamp(encoded(format), 150.0), 600.0);
            assert_eq!(read_bytes(&twice), Some(600.0));
            assert_eq!(twice.len(), stamp(encoded(format), 600.0).len());
        }
    }

    #[test]
    fn png_chunks_are_well_formed() {
        // The decoder checks CRCs, so a bad one would fail to load; check the
        // chunk is where a strict reader wants it too: before the pixels.
        let bytes = stamp(encoded(ImageFormat::Png), 300.0);
        let phys = bytes.windows(4).position(|w| w == b"pHYs").unwrap();
        let idat = bytes.windows(4).position(|w| w == b"IDAT").unwrap();
        assert!(phys < idat);
        assert_eq!(crc32(b"IEND"), 0xAE42_6082, "the CRC every PNG ends with");
    }

    #[test]
    fn a_file_that_does_not_say_has_no_resolution() {
        // The JPEG encoder writes JFIF with an aspect ratio only.
        assert_eq!(read_bytes(&encoded(ImageFormat::Jpeg)), None);
        assert_eq!(read_bytes(&encoded(ImageFormat::Png)), None);
        assert_eq!(read_bytes(&encoded(ImageFormat::Gif)), None);
        assert_eq!(stamp(encoded(ImageFormat::Gif), 300.0), encoded(ImageFormat::Gif));
    }

    #[test]
    fn exif_resolution_is_read_in_either_byte_order_and_unit() {
        // A minimal TIFF directory: XResolution 300/1 and ResolutionUnit.
        let directory = |little: bool, unit: u16| {
            let w16 = |v: u16| if little { v.to_le_bytes() } else { v.to_be_bytes() };
            let w32 = |v: u32| if little { v.to_le_bytes() } else { v.to_be_bytes() };
            let mut t = Vec::new();
            t.extend_from_slice(if little { b"II" } else { b"MM" });
            t.extend_from_slice(&w16(42));
            t.extend_from_slice(&w32(8));
            t.extend_from_slice(&w16(2));
            // XResolution, RATIONAL, 1, at offset 38.
            t.extend_from_slice(&w16(0x011A));
            t.extend_from_slice(&w16(5));
            t.extend_from_slice(&w32(1));
            t.extend_from_slice(&w32(38));
            t.extend_from_slice(&w16(0x0128));
            t.extend_from_slice(&w16(3));
            t.extend_from_slice(&w32(1));
            t.extend_from_slice(&w16(unit));
            t.extend_from_slice(&[0, 0]);
            t.extend_from_slice(&w32(0));
            t.extend_from_slice(&w32(300));
            t.extend_from_slice(&w32(1));
            t
        };
        assert_eq!(tiff(&directory(true, 2)), Some(300.0));
        assert_eq!(tiff(&directory(false, 2)), Some(300.0));
        assert_eq!(tiff(&directory(true, 3)), Some(300.0 * 2.54));
        assert_eq!(tiff(&directory(true, 1)), None);

        // Inside a JPEG with no JFIF units.
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
        let body: Vec<u8> = b"Exif\0\0".iter().copied().chain(directory(false, 2)).collect();
        jpeg.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
        jpeg.extend_from_slice(&body);
        jpeg.extend_from_slice(&[0xFF, 0xDA, 0, 2]);
        assert_eq!(read_bytes(&jpeg), Some(300.0));
    }
}
