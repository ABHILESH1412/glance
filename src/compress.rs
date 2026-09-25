// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Hitting a file size.
//!
//! The job people go to advertising-funded websites for: "make this 500 KB".
//! Two levers do it. JPEG has a quality dial, so a search over quality finds
//! the best-looking file that fits; when even the lowest quality is too big,
//! the picture is scaled down and the search runs again. Formats without a
//! dial — PNG here, and WebP, which this build can only write losslessly —
//! have only the second lever.
//!
//! The other direction is real too: upload forms that demand a *minimum* size.
//! Raising quality is not enough on its own, so the file is padded with a
//! metadata block the format already has a place for — a JPEG comment, a PNG
//! text chunk. The picture is untouched and the file stays valid; the caller
//! is told how much filler went in so it can say so.

use image::DynamicImage;

use crate::export::Target;

pub const KB: u64 = 1024;
pub const MB: u64 = 1024 * KB;

/// Quality is not worth searching below this: the picture is ruined well
/// before it, and shrinking the image looks better than the mush.
const MIN_QUALITY: u8 = 15;
const MAX_QUALITY: u8 = 100;
/// Each round shrinks the picture and searches quality again.
const SHRINK_ROUNDS: usize = 5;
/// No file needs more filler than this, and an amount past it is a sign the
/// shortfall was worked out wrongly rather than a request to honour.
const MOST_PADDING: u64 = 512 * MB;
/// Never shrink below this on the longest edge — a target small enough to need
/// it is better refused than met with a thumbnail.
const MIN_EDGE: u32 = 32;

/// What a size-targeting run settled on.
pub struct Fit {
    pub bytes: Vec<u8>,
    /// The quality the search chose, where the format has a dial.
    pub quality: Option<u8>,
    pub width: u32,
    pub height: u32,
    /// Filler added to reach a minimum size, in bytes.
    pub padding: u64,
}

impl Fit {
    pub fn size(&self) -> u64 {
        self.bytes.len() as u64
    }
}

/// Human sizes, for saying what happened.
pub fn describe(bytes: u64) -> String {
    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} bytes")
    }
}

fn encode(image: &DynamicImage, target: &Target, quality: u8) -> Result<Vec<u8>, String> {
    let mut out = std::io::Cursor::new(Vec::new());
    if target.lossy() {
        // JPEG cannot carry transparency, so flatten first for the same
        // reason the ordinary writer does.
        let rgb = crate::export::flatten(image);
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
            .encode_image(&rgb)
            .map_err(|e| format!("Could not encode: {e}"))?;
    } else {
        let format = image::ImageFormat::from_extension(target.extension)
            .ok_or_else(|| format!("{} cannot be written.", target.label))?;
        image
            .write_to(&mut out, format)
            .map_err(|e| format!("Could not encode: {e}"))?;
    }
    Ok(out.into_inner())
}

/// The best-looking encoding that fits, or `None` when even the worst is over.
fn search_quality(
    image: &DynamicImage,
    target: &Target,
    wanted: u64,
) -> Result<Option<Fit>, String> {
    if !target.lossy() {
        let bytes = encode(image, target, MAX_QUALITY)?;
        return Ok((bytes.len() as u64 <= wanted).then(|| Fit {
            width: image.width(),
            height: image.height(),
            quality: None,
            padding: 0,
            bytes,
        }));
    }

    let mut low = MIN_QUALITY;
    let mut high = MAX_QUALITY;
    let mut best: Option<(u8, Vec<u8>)> = None;
    // Eight or so probes over 15..100, each a full encode.
    while low <= high {
        let middle = low + (high - low) / 2;
        let bytes = encode(image, target, middle)?;
        if bytes.len() as u64 <= wanted {
            best = Some((middle, bytes));
            low = middle + 1;
        } else if middle == MIN_QUALITY {
            break;
        } else {
            high = middle - 1;
        }
    }
    Ok(best.map(|(quality, bytes)| Fit {
        width: image.width(),
        height: image.height(),
        quality: Some(quality),
        padding: 0,
        bytes,
    }))
}

/// Encode `image` as `target`, as close under `wanted` bytes as it can get.
///
/// Going the other way — when the picture will not fill the target even at
/// full quality — the file is padded up to it.
pub fn fit_to_size(image: &DynamicImage, target: &Target, wanted: u64) -> Result<Fit, String> {
    if wanted == 0 {
        return Err("Give a size to aim for.".to_string());
    }

    // The largest this picture goes. If that is already under the target then
    // the job is to grow, not to shrink.
    let largest = encode(image, target, MAX_QUALITY)?;
    if largest.len() as u64 <= wanted {
        let padding = wanted - largest.len() as u64;
        let (width, height) = (image.width(), image.height());
        let bytes = pad(largest, target, padding)?;
        let padding = bytes.len() as u64 - (wanted - padding);
        return Ok(Fit {
            width,
            height,
            quality: target.lossy().then_some(MAX_QUALITY),
            padding,
            bytes,
        });
    }

    let mut working = image.clone();
    for _ in 0..SHRINK_ROUNDS {
        if let Some(fit) = search_quality(&working, target, wanted)? {
            return Ok(fit);
        }
        // Too big even at the worst quality: the picture itself has to give.
        // File size goes roughly with area, so the side scales with the root
        // of the ratio; the margin stops it converging from above for ever.
        let floor = encode(&working, target, MIN_QUALITY)?;
        let ratio = ((wanted as f64 / floor.len() as f64).sqrt() * 0.95).clamp(0.1, 0.9);
        let width = (f64::from(working.width()) * ratio).round().max(1.0) as u32;
        let height = (f64::from(working.height()) * ratio).round().max(1.0) as u32;
        if width.max(height) < MIN_EDGE {
            return Err(format!(
                "{} is too small a target for this picture: it would have to shrink past {MIN_EDGE} pixels.",
                describe(wanted)
            ));
        }
        working = working.resize_exact(width, height, image::imageops::FilterType::Lanczos3);
    }
    Err(format!(
        "Could not get this down to {}. Try a larger target, or resize it first.",
        describe(wanted)
    ))
}

/// Grow a file to meet a minimum, using whatever the format keeps notes in.
///
/// Returns the file unchanged when it has nowhere to put them.
fn pad(bytes: Vec<u8>, target: &Target, wanted: u64) -> Result<Vec<u8>, String> {
    if wanted == 0 {
        return Ok(bytes);
    }
    // A caller that worked out the shortfall with a subtraction that went
    // negative would arrive here asking for an exabyte of filler, and the
    // allocation would take the process with it. Refuse rather than try.
    if wanted > MOST_PADDING {
        return Err(format!(
            "{} is too much padding to add.",
            describe(wanted)
        ));
    }
    match target.extension {
        "jpg" | "jpeg" => Ok(pad_jpeg(bytes, wanted)),
        "png" => Ok(pad_png(bytes, wanted)),
        _ => Ok(bytes),
    }
}

/// A JPEG comment segment holds up to 65533 bytes, so long padding becomes
/// several. They go straight after the start marker, where a comment belongs.
fn pad_jpeg(bytes: Vec<u8>, wanted: u64) -> Vec<u8> {
    const MOST: usize = 65_533;
    const OVERHEAD: usize = 4; // marker plus length
    if bytes.len() < 2 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return bytes;
    }
    let mut filler = Vec::new();
    let mut left = wanted as usize;
    while left > OVERHEAD {
        let payload = (left - OVERHEAD).min(MOST);
        filler.extend_from_slice(&[0xFF, 0xFE]);
        let length = (payload + 2) as u16;
        filler.extend_from_slice(&length.to_be_bytes());
        filler.extend(std::iter::repeat_n(b' ', payload));
        left -= payload + OVERHEAD;
    }
    let mut out = Vec::with_capacity(bytes.len() + filler.len());
    out.extend_from_slice(&bytes[..2]);
    out.extend_from_slice(&filler);
    out.extend_from_slice(&bytes[2..]);
    out
}

/// A PNG text chunk, inserted before the end chunk.
fn pad_png(bytes: Vec<u8>, wanted: u64) -> Vec<u8> {
    const KEYWORD: &[u8] = b"Comment\0";
    const OVERHEAD: usize = 12; // length, type and CRC
    let Some(end) = find_iend(&bytes) else {
        return bytes;
    };
    let payload = (wanted as usize).saturating_sub(OVERHEAD + KEYWORD.len());
    if payload == 0 {
        return bytes;
    }
    let mut data = Vec::with_capacity(KEYWORD.len() + payload);
    data.extend_from_slice(KEYWORD);
    data.extend(std::iter::repeat_n(b' ', payload));

    let mut chunk = Vec::with_capacity(data.len() + OVERHEAD);
    chunk.extend_from_slice(&(data.len() as u32).to_be_bytes());
    chunk.extend_from_slice(b"tEXt");
    chunk.extend_from_slice(&data);
    let mut crc_over = Vec::with_capacity(4 + data.len());
    crc_over.extend_from_slice(b"tEXt");
    crc_over.extend_from_slice(&data);
    chunk.extend_from_slice(&crc32(&crc_over).to_be_bytes());

    let mut out = Vec::with_capacity(bytes.len() + chunk.len());
    out.extend_from_slice(&bytes[..end]);
    out.extend_from_slice(&chunk);
    out.extend_from_slice(&bytes[end..]);
    out
}

/// Offset of the final chunk's length field.
fn find_iend(bytes: &[u8]) -> Option<usize> {
    // 8-byte signature, then chunks of length, type, data, CRC.
    let mut at = 8;
    while at + 8 <= bytes.len() {
        let length = u32::from_be_bytes(bytes[at..at + 4].try_into().ok()?) as usize;
        if &bytes[at + 4..at + 8] == b"IEND" {
            return Some(at);
        }
        at += 12 + length;
    }
    None
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            // The PNG polynomial, reflected.
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg() -> &'static Target {
        crate::export::TARGETS.iter().find(|t| t.label == "JPEG").unwrap()
    }
    fn png() -> &'static Target {
        crate::export::TARGETS.iter().find(|t| t.label == "PNG").unwrap()
    }

    /// Detailed enough that JPEG cannot trivially crush it, so the search has
    /// real work to do.
    fn busy(width: u32, height: u32) -> DynamicImage {
        let mut buffer = image::RgbaImage::new(width, height);
        for (x, y, pixel) in buffer.enumerate_pixels_mut() {
            let n = (x * 7 + y * 13) % 255;
            *pixel = image::Rgba([
                (x % 255) as u8,
                ((y * 3) % 255) as u8,
                n as u8,
                255,
            ]);
        }
        DynamicImage::ImageRgba8(buffer)
    }

    #[test]
    fn crc32_matches_the_known_check_value() {
        // The standard CRC-32 check: "123456789" is 0xCBF43926.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn compressing_lands_under_the_target_and_close_to_it() {
        let image = busy(600, 400);
        let wanted = 40 * KB;
        let fit = fit_to_size(&image, jpeg(), wanted).expect("should fit");
        assert!(fit.size() <= wanted, "{} is over {wanted}", fit.size());
        // Not a token effort: it should be using most of the budget.
        assert!(fit.size() > wanted / 2, "only reached {}", fit.size());
        assert!(image::load_from_memory(&fit.bytes).is_ok(), "should still decode");
    }

    /// A target too small for any quality forces the picture down as well.
    #[test]
    fn an_impossible_quality_target_shrinks_the_picture() {
        let image = busy(1200, 900);
        let fit = fit_to_size(&image, jpeg(), 6 * KB).expect("should fit by shrinking");
        assert!(fit.size() <= 6 * KB, "{}", fit.size());
        assert!(fit.width < 1200, "should have shrunk, still {}", fit.width);
        let decoded = image::load_from_memory(&fit.bytes).expect("should still decode");
        assert_eq!(decoded.width(), fit.width);
    }

    /// The other direction: a minimum size, met by padding, with the picture
    /// left exactly as it was.
    #[test]
    fn inflating_reaches_the_target_without_touching_the_picture() {
        let image = busy(80, 60);
        let plain = encode(&image, jpeg(), MAX_QUALITY).unwrap();
        let wanted = plain.len() as u64 + 200 * KB;
        let fit = fit_to_size(&image, jpeg(), wanted).expect("should inflate");
        assert!(fit.size() >= wanted - 8, "only reached {}", fit.size());
        assert!(fit.size() <= wanted + 8, "overshot to {}", fit.size());
        assert!(fit.padding > 100 * KB, "padding was {}", fit.padding);
        let decoded = image::load_from_memory(&fit.bytes).expect("padded file must still decode");
        assert_eq!((decoded.width(), decoded.height()), (80, 60));
    }

    #[test]
    fn a_padded_png_is_still_a_png() {
        let image = busy(40, 30);
        let plain = encode(&image, png(), MAX_QUALITY).unwrap();
        let wanted = plain.len() as u64 + 64 * KB;
        let fit = fit_to_size(&image, png(), wanted).expect("should inflate");
        assert!(fit.size() >= wanted - 16, "only reached {}", fit.size());
        let decoded = image::load_from_memory(&fit.bytes).expect("padded file must still decode");
        assert_eq!((decoded.width(), decoded.height()), (40, 30));
        // Lossless in, lossless out: padding must not have touched a pixel.
        assert_eq!(decoded.to_rgba8(), image.to_rgba8());
    }

    #[test]
    fn a_target_of_nothing_is_refused_rather_than_attempted() {
        assert!(fit_to_size(&busy(50, 50), jpeg(), 0).is_err());
    }
}


#[cfg(test)]
mod guards {
    use super::*;

    fn jpeg() -> &'static Target {
        crate::export::TARGETS.iter().find(|t| t.label == "JPEG").unwrap()
    }

    /// An absurd amount of filler means somebody's arithmetic went negative
    /// and wrapped. Allocating it would take the whole process down, so it is
    /// refused at the door.
    #[test]
    fn absurd_padding_is_refused_rather_than_allocated() {
        let result = pad(vec![0xFF, 0xD8, 0xFF, 0xD9], jpeg(), u64::MAX);
        assert!(result.is_err(), "should refuse, got {:?}", result.map(|b| b.len()));
    }
}
