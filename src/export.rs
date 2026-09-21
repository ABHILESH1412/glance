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

use crate::canvas::CropSelection;
use crate::loader;

/// Apply the live transforms, and optionally a crop, to an image already in
/// memory.
///
/// Order matters and mirrors what the view does — flip, then rotate, then cut.
/// That is what puts the crop rectangle, which is recorded in display space,
/// into the same coordinates as the pixels it is cutting.
pub fn apply(
    mut image: DynamicImage,
    rotation: f64,
    flip_h: bool,
    flip_v: bool,
    crop: Option<&CropSelection>,
    display: (f64, f64),
) -> Result<DynamicImage, String> {
    if flip_h {
        image = image.fliph();
    }
    if flip_v {
        image = image.flipv();
    }
    image = rotate(image, rotation);

    match crop {
        Some(crop) => cut(image, crop, display),
        None => Ok(image),
    }
}

/// Decode a file into the buffer that editing works on.
pub fn open(source: &Path) -> Result<DynamicImage, String> {
    let decoded = loader::decode(source)?;
    let buffer = RgbaImage::from_raw(decoded.width, decoded.height, decoded.rgba)
        .ok_or_else(|| "The decoder reported a size that does not match its data.".to_string())?;
    Ok(DynamicImage::ImageRgba8(buffer))
}

pub fn write(image: &DynamicImage, destination: &Path) -> Result<(), String> {
    let extension = destination
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();

    let result = if matches!(extension.as_str(), "jpg" | "jpeg") {
        // JPEG cannot carry transparency, so anything a freehand cut removed
        // would otherwise come out black.
        DynamicImage::ImageRgb8(flatten(image)).save(destination)
    } else {
        image.save(destination)
    };
    result.map_err(|error| format!("Could not save: {error}"))
}

fn flatten(image: &DynamicImage) -> image::RgbImage {
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
