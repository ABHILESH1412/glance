//! Turning the pending edits into a file on disk.
//!
//! Nothing here touches the image being viewed: the original is decoded afresh
//! at full resolution and the edits are replayed onto it, so what is saved is
//! never limited by whatever was on screen.
//!
//! Order matters and mirrors what the view does — flip, then rotate, then cut.
//! That is what puts the crop rectangle, which is recorded in display space,
//! into the same coordinates as the pixels it is cutting.

use std::path::Path;

use image::{DynamicImage, Rgba, RgbaImage};

use crate::canvas::{CropSelection, LiveEdits};
use crate::loader;

/// Apply the live edits, and optionally a crop, to an image already in memory.
///
/// Order matters and mirrors what the view does — resize, flip, rotate, lay on
/// the text and drawings, then cut. Resizing comes first because the crop rectangle is
/// recorded against the size on screen, which is the resized one; cutting first
/// would take the wrong part. Text goes on once the picture is in display
/// space, which is the frame their positions were recorded in, and before the
/// crop, so a cut through a caption or an arrow cuts it exactly as it looked. Tone comes
/// last because it is per-pixel and commutes with all of it, so it may as well
/// run on the fewest pixels.
pub fn apply(
    mut image: DynamicImage,
    live: LiveEdits,
    crop: Option<&CropSelection>,
    display: (f64, f64),
) -> Result<DynamicImage, String> {
    if let Some((width, height)) = live.resize {
        if (width, height) != (image.width(), image.height()) {
            // Lanczos costs more than the alternatives and is worth it: this
            // runs once per export, and a cheap filter is visible as softness
            // on any real downscale.
            image = image.resize_exact(width, height, image::imageops::FilterType::Lanczos3);
        }
    }
    if live.flip_h {
        image = image.fliph();
    }
    if live.flip_v {
        image = image.flipv();
    }
    image = rotate(image, live.rotation);

    if !live.overlays.is_empty() {
        let mut rgba = image.to_rgba8();
        crate::text::composite(&mut rgba, &live.overlays);
        image = DynamicImage::ImageRgba8(rgba);
    }

    if let Some(crop) = crop {
        image = cut(image, crop, display)?;
    }
    Ok(live.adjust.bake(image))
}

/// Where the quality dial sits when nobody has moved it. High enough that a
/// photograph keeps its detail, low enough that the file is not absurd.
pub const DEFAULT_QUALITY: u8 = 85;

/// A format an edited image can be written to.
///
/// Every entry here is a *raster* format, and that is the whole rule about what
/// can be exported to what: an image that has been decoded is pixels, and
/// pixels can be written as any of these. The direction that does not work is
/// the other one — a photograph cannot become an SVG, because nothing can
/// recover the shapes it never had. A drawing going the other way is fine and
/// needs no special case: by the time it reaches here it is pixels like
/// anything else.
pub struct Target {
    pub label: &'static str,
    pub extension: &'static str,
    /// Encoders with a size limit of their own. ICO is the only common one.
    pub max_dimension: Option<u32>,
    /// Shown beside the name when the format costs the picture something.
    pub caveat: Option<&'static str>,
}

impl Target {
    /// Whether this format has a quality to choose. Only JPEG does here: the
    /// others in the list are lossless, and this build writes WebP losslessly
    /// too.
    pub fn lossy(&self) -> bool {
        matches!(self.extension, "jpg" | "jpeg")
    }
}

/// Ordered by how likely someone is to want them, not alphabetically.
pub const TARGETS: &[Target] = &[
    Target { label: "PNG", extension: "png", max_dimension: None, caveat: None },
    Target { label: "JPEG", extension: "jpg", max_dimension: None,
             caveat: Some("no transparency") },
    Target { label: "WebP", extension: "webp", max_dimension: None, caveat: None },
    Target { label: "TIFF", extension: "tiff", max_dimension: None, caveat: None },
    Target { label: "BMP", extension: "bmp", max_dimension: None, caveat: None },
    Target { label: "GIF", extension: "gif", max_dimension: None,
             caveat: Some("256 colours, one frame") },
    Target { label: "ICO", extension: "ico", max_dimension: Some(256), caveat: None },
];

impl Target {
    /// Why this format cannot take an image of this size, if it cannot.
    ///
    /// Checked before the file dialog opens rather than after, so the refusal
    /// arrives while there is still something to do about it.
    pub fn refusal(&self, width: u32, height: u32) -> Option<String> {
        let limit = self.max_dimension?;
        (width > limit || height > limit).then(|| {
            format!(
                "{} cannot hold an image larger than {limit} × {limit}; this one is {width} × {height}. \
                 Resize it first, or pick another format.",
                self.label
            )
        })
    }
}

/// Decode a file into the buffer that editing works on.
pub fn open(source: &Path) -> Result<DynamicImage, String> {
    let decoded = loader::decode(source)?;
    let buffer = RgbaImage::from_raw(decoded.width, decoded.height, decoded.rgba)
        .ok_or_else(|| "The decoder reported a size that does not match its data.".to_string())?;
    Ok(DynamicImage::ImageRgba8(buffer))
}

/// Write the image out. `quality` is honoured by the formats that have a dial;
/// the rest are lossless and ignore it.
pub fn write(
    image: &DynamicImage,
    destination: &Path,
    quality: Option<u8>,
) -> Result<(), String> {
    let extension = destination
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();

    if matches!(extension.as_str(), "jpg" | "jpeg") {
        // JPEG cannot carry transparency, so anything a freehand cut removed
        // would otherwise come out black.
        let rgb = flatten(image);
        let file = std::fs::File::create(destination)
            .map_err(|error| format!("Could not save: {error}"))?;
        let mut writer = std::io::BufWriter::new(file);
        return image::codecs::jpeg::JpegEncoder::new_with_quality(
            &mut writer,
            quality.unwrap_or(DEFAULT_QUALITY),
        )
        .encode_image(&rgb)
        .map_err(|error| format!("Could not save: {error}"));
    }
    image
        .save(destination)
        .map_err(|error| format!("Could not save: {error}"))
}

pub(crate) fn flatten(image: &DynamicImage) -> image::RgbImage {
    let rgba = image.to_rgba8();
    let mut out = image::RgbImage::new(rgba.width(), rgba.height());
    for (x, y, pixel) in rgba.enumerate_pixels() {
        let alpha = f32::from(pixel[3]) / 255.0;
        let over = |c: u8| (f32::from(c) * alpha + 255.0 * (1.0 - alpha)).round() as u8;
        out.put_pixel(x, y, image::Rgb([over(pixel[0]), over(pixel[1]), over(pixel[2])]));
    }
    out
}

fn rotate(image: DynamicImage, degrees: f64) -> DynamicImage {
    let normalised = degrees.rem_euclid(360.0);
    let turns = (normalised / 90.0).round() as i64 % 4;
    if (normalised - turns as f64 * 90.0).abs() < 0.01 {
        // Exact quarter turns are lossless, so take that path whenever it fits.
        return match turns {
            1 => image.rotate90(),
            2 => image.rotate180(),
            3 => image.rotate270(),
            _ => image,
        };
    }
    DynamicImage::ImageRgba8(rotate_freely(&image.to_rgba8(), normalised))
}

/// Rotate by an arbitrary angle onto a canvas big enough to hold the result,
/// sampling bilinearly so edges do not come out jagged.
fn rotate_freely(source: &RgbaImage, degrees: f64) -> RgbaImage {
    let (width, height) = (f64::from(source.width()), f64::from(source.height()));
    let (sin, cos) = degrees.to_radians().sin_cos();
    let out_w = (width * cos.abs() + height * sin.abs()).round().max(1.0);
    let out_h = (width * sin.abs() + height * cos.abs()).round().max(1.0);

    let mut out = RgbaImage::from_pixel(out_w as u32, out_h as u32, Rgba([0, 0, 0, 0]));
    let (cx, cy) = (width / 2.0, height / 2.0);
    let (ox, oy) = (out_w / 2.0, out_h / 2.0);

    for y in 0..out.height() {
        for x in 0..out.width() {
            // Walk backwards from the destination so every output pixel is
            // filled; going forwards would leave gaps.
            let dx = f64::from(x) + 0.5 - ox;
            let dy = f64::from(y) + 0.5 - oy;
            let sx = dx * cos + dy * sin + cx;
            let sy = -dx * sin + dy * cos + cy;
            out.put_pixel(x, y, sample(source, sx, sy));
        }
    }
    out
}

fn sample(source: &RgbaImage, x: f64, y: f64) -> Rgba<u8> {
    let transparent = Rgba([0, 0, 0, 0]);
    if x < 0.0 || y < 0.0 || x >= f64::from(source.width()) || y >= f64::from(source.height()) {
        return transparent;
    }
    // Pixel coordinates name a pixel's *centre*, so shift by half before
    // splitting into neighbour and fraction. Without this a sample landing
    // squarely on a pixel blends it half-and-half with the one beside it.
    let (bx, by) = (x - 0.5, y - 0.5);
    let (x0, y0) = (bx.floor(), by.floor());
    let (fx, fy) = (bx - x0, by - y0);
    let at = |ix: f64, iy: f64| -> Rgba<u8> {
        if ix < 0.0 || iy < 0.0 || ix >= f64::from(source.width()) || iy >= f64::from(source.height())
        {
            transparent
        } else {
            *source.get_pixel(ix as u32, iy as u32)
        }
    };
    let (p00, p10) = (at(x0, y0), at(x0 + 1.0, y0));
    let (p01, p11) = (at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));

    let mut channels = [0u8; 4];
    for (index, channel) in channels.iter_mut().enumerate() {
        let top = f64::from(p00[index]) * (1.0 - fx) + f64::from(p10[index]) * fx;
        let bottom = f64::from(p01[index]) * (1.0 - fx) + f64::from(p11[index]) * fx;
        *channel = (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
    }
    Rgba(channels)
}

fn cut(
    image: DynamicImage,
    crop: &CropSelection,
    display: (f64, f64),
) -> Result<DynamicImage, String> {
    if display.0 <= 0.0 || display.1 <= 0.0 {
        return Ok(image);
    }
    // A vector is rasterised at a size of its own choosing, so convert from
    // display units to pixels rather than assuming they match.
    let scale = f64::from(image.width()) / display.0;

    let x = (crop.rect.0 * scale).round().max(0.0) as u32;
    let y = (crop.rect.1 * scale).round().max(0.0) as u32;
    let w = ((crop.rect.2 * scale).round() as u32).min(image.width().saturating_sub(x));
    let h = ((crop.rect.3 * scale).round() as u32).min(image.height().saturating_sub(y));
    if w == 0 || h == 0 {
        return Err("The crop area is empty.".to_string());
    }

    let mut cropped = image.crop_imm(x, y, w, h);

    if crop.path.len() > 2 {
        // Everything outside the drawn outline becomes transparent.
        let polygon: Vec<(f64, f64)> = crop
            .path
            .iter()
            .map(|(px, py)| (px * scale - f64::from(x), py * scale - f64::from(y)))
            .collect();
        let mut rgba = cropped.to_rgba8();
        for (px, py, pixel) in rgba.enumerate_pixels_mut() {
            if !contains(&polygon, f64::from(px) + 0.5, f64::from(py) + 0.5) {
                *pixel = Rgba([0, 0, 0, 0]);
            }
        }
        cropped = DynamicImage::ImageRgba8(rgba);
    }
    Ok(cropped)
}

/// Ray casting: count how many edges a ray to the right crosses.
fn contains(polygon: &[(f64, f64)], x: f64, y: f64) -> bool {
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let (xi, yi) = polygon[i];
        let (xj, yj) = polygon[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_rotation_matches_the_exact_quarter_turn() {
        // A recognisable asymmetric image, so a wrong direction shows up.
        let mut source = RgbaImage::from_pixel(8, 4, Rgba([0, 0, 0, 255]));
        source.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        source.put_pixel(7, 0, Rgba([0, 255, 0, 255]));

        let exact = DynamicImage::ImageRgba8(source.clone()).rotate90().to_rgba8();
        let free = rotate_freely(&source, 90.0);
        assert_eq!((free.width(), free.height()), (exact.width(), exact.height()));

        // Corners carry the signal; bilinear sampling can differ by a hair.
        for (x, y) in [(0u32, 0u32), (free.width() - 1, 0), (0, free.height() - 1)] {
            let (a, b) = (free.get_pixel(x, y), exact.get_pixel(x, y));
            for channel in 0..3 {
                assert!(
                    a[channel].abs_diff(b[channel]) <= 2,
                    "at {x},{y} channel {channel}: {a:?} vs {b:?}"
                );
            }
        }
    }

    #[test]
    fn polygon_test_knows_inside_from_outside() {
        let square = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert!(contains(&square, 5.0, 5.0));
        assert!(!contains(&square, 15.0, 5.0));
        assert!(!contains(&square, 5.0, -1.0));
    }
}


#[cfg(test)]
mod format_tests {
    use super::*;

    fn sample(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(width, height, Rgba([200, 80, 80, 128])))
    }

    /// Offering a format the build cannot actually encode would fail only at
    /// the moment someone tried to use it, which is the worst time to find out.
    #[test]
    fn every_offered_format_can_be_written() {
        let dir = std::env::temp_dir().join("simple-viewer-format-test");
        std::fs::create_dir_all(&dir).unwrap();
        for target in TARGETS {
            let limit = target.max_dimension.unwrap_or(64).min(64);
            let path = dir.join(format!("probe.{}", target.extension));
            let result = write(&sample(limit, limit), &path, Some(DEFAULT_QUALITY));
            assert!(result.is_ok(), "{} failed: {:?}", target.label, result);
            let written = image::open(&path).expect("what we wrote should read back");
            assert_eq!((written.width(), written.height()), (limit, limit));
            let _ = std::fs::remove_file(&path);
        }
    }

    /// ICO is the one format with a size of its own to answer to.
    #[test]
    fn a_format_with_a_size_limit_says_so_before_writing() {
        let ico = TARGETS.iter().find(|t| t.label == "ICO").unwrap();
        assert!(ico.refusal(256, 256).is_none());
        let refusal = ico.refusal(512, 300).expect("512 is past the limit");
        assert!(refusal.contains("512"), "the message should name the size: {refusal}");
        let png = TARGETS.iter().find(|t| t.label == "PNG").unwrap();
        assert!(png.refusal(30_000, 30_000).is_none());
    }
}
