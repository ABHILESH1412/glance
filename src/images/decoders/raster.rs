//! PNG, JPEG, GIF, WebP, TIFF, BMP and the rest of the `image` crate's formats.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::time::Duration;

use image::codecs::gif::GifDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat, ImageReader};

use crate::images::loader::{open_error, unsupported, Frame, LoadedImage};

/// A very long animation could otherwise hold a lot of decoded frames at once,
/// so stop collecting past this and show what was gathered.
const MAX_ANIMATION_BYTES: usize = 512 * 1024 * 1024;
const MAX_FRAMES: usize = 2000;
/// Browsers treat absurdly short frame delays as a mistake and slow them down;
/// matching that keeps old GIFs looking the way they are meant to.
const MIN_DELAY: Duration = Duration::from_millis(20);
const SLOW_DELAY: Duration = Duration::from_millis(100);

pub fn decode(path: &Path) -> Result<LoadedImage, String> {
    let reader = ImageReader::open(path)
        .map_err(|e| open_error(path, &e))?
        .with_guessed_format()
        .map_err(|e| open_error(path, &e))?;

    let format = reader.format();
    let label = format.map(label_for).unwrap_or("Image").to_string();

    // GIF and WebP may hold more than one frame; everything else is a still.
    if matches!(format, Some(ImageFormat::Gif) | Some(ImageFormat::WebP)) {
        if let Some(animated) = decode_animation(path, format, &label) {
            return Ok(animated);
        }
    }

    let mut decoder = reader
        .into_decoder()
        .map_err(|e| unsupported(path, &e))?;

    // Both of these have to be read before the decoder is consumed. Cameras
    // write portrait shots as landscape plus a rotation tag, and ignoring that
    // shows them sideways; they also say which colour space the numbers are
    // in, and ignoring that shows an Adobe RGB photograph flat.
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let profile = decoder.icc_profile().ok().flatten();

    let mut image = DynamicImage::from_decoder(decoder).map_err(|e| unsupported(path, &e))?;
    image.apply_orientation(orientation);

    // OpenEXR and Radiance HDR hold light, not screen colours: linear, and
    // as bright as the scene was. Shown as they are they would be dark, or
    // white where the sky is; brought into range, they look as they should.
    let mut label = label;
    let rgba = if matches!(image.color(), image::ColorType::Rgb32F | image::ColorType::Rgba32F) {
        let (rgba, exposed) = expose(&image.into_rgba32f());
        if exposed {
            label.push_str(" · HDR, fitted to the screen");
        }
        rgba
    } else {
        image.into_rgba8()
    };
    let (width, height) = rgba.dimensions();
    let mut rgba = rgba.into_raw();
    if let Some(profile) = profile {
        crate::images::colour::to_srgb(&mut rgba, &profile);
    }

    Ok(LoadedImage {
        width,
        height,
        rgba,
        premultiplied: false,
        label,
        animation: Vec::new(),
        vector: None,
    })
}

/// Returns `None` for a single-frame file, which then takes the still path.
fn decode_animation(path: &Path, format: Option<ImageFormat>, label: &str) -> Option<LoadedImage> {
    let file = BufReader::new(File::open(path).ok()?);
    let frames = match format? {
        ImageFormat::Gif => GifDecoder::new(file).ok()?.into_frames(),
        ImageFormat::WebP => WebPDecoder::new(file).ok()?.into_frames(),
        _ => return None,
    };

    let mut collected: Vec<Frame> = Vec::new();
    let mut bytes = 0usize;
    let mut size = None;

    for frame in frames {
        let frame = frame.ok()?;
        let mut delay = Duration::from(frame.delay());
        if delay < MIN_DELAY {
            delay = SLOW_DELAY;
        }
        let buffer = frame.into_buffer();
        size.get_or_insert((buffer.width(), buffer.height()));
        let rgba = buffer.into_raw();
        bytes += rgba.len();
        collected.push(Frame { rgba, delay });
        if collected.len() >= MAX_FRAMES || bytes >= MAX_ANIMATION_BYTES {
            break;
        }
    }

    let (width, height) = size?;
    if collected.len() < 2 {
        return None; // A single frame is just a picture.
    }

    Some(LoadedImage {
        width,
        height,
        rgba: collected[0].rgba.clone(),
        premultiplied: false,
        label: format!("{label} · {} frames", collected.len()),
        animation: collected,
        vector: None,
    })
}

fn label_for(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Gif => "GIF",
        ImageFormat::WebP => "WebP",
        ImageFormat::Tiff => "TIFF",
        ImageFormat::Bmp => "BMP",
        ImageFormat::Ico => "ICO",
        ImageFormat::Qoi => "QOI",
        ImageFormat::Tga => "TGA",
        ImageFormat::Pnm => "PNM",
        ImageFormat::OpenExr => "OpenEXR",
        ImageFormat::Hdr => "Radiance HDR",
        _ => "Image",
    }
}

/// Linear light as screen colours: scaled so all but the brightest few
/// highlights fit under white, then given sRGB's curve. Says whether it had
/// to be scaled, which is when there was more light than a screen shows.
pub(crate) fn expose(linear: &image::Rgba32FImage) -> (image::RgbaImage, bool) {
    // The brightness below which 99.5% of the picture lies, from a sample of
    // it: a few specular highlights are allowed to clip, as in any photo.
    let step = (linear.len() / 4 / 200_000).max(1);
    let mut levels: Vec<f32> = linear
        .pixels()
        .step_by(step)
        .map(|p| (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]).max(0.0))
        .filter(|v| v.is_finite())
        .collect();
    levels.sort_by(f32::total_cmp);
    let high = levels.get(levels.len().saturating_sub(1) * 995 / 1000).copied().unwrap_or(1.0);
    let scale = if high > 1.0 { 1.0 / high } else { 1.0 };
    let encode = |v: f32| {
        let v = (v * scale).clamp(0.0, 1.0);
        let v = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (v * 255.0).round() as u8
    };
    let mut out = image::RgbaImage::new(linear.width(), linear.height());
    for (to, from) in out.pixels_mut().zip(linear.pixels()) {
        *to = image::Rgba([encode(from[0]), encode(from[1]), encode(from[2]), (from[3].clamp(0.0, 1.0) * 255.0).round() as u8]);
    }
    (out, scale < 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_is_shown_with_the_screen_s_curve_and_fitted_under_white() {
        // Mid grey in linear light is 18%, which the screen's curve shows as
        // about 118 of 255.
        let grey = image::Rgba32FImage::from_pixel(4, 4, image::Rgba([0.18, 0.18, 0.18, 1.0]));
        let (shown, exposed) = expose(&grey);
        assert!(!exposed);
        assert!((i32::from(shown.get_pixel(0, 0)[0]) - 118).abs() <= 1, "{:?}", shown.get_pixel(0, 0));
        // A scene four times brighter than white is brought down to fit.
        let bright = image::Rgba32FImage::from_pixel(4, 4, image::Rgba([4.0, 4.0, 4.0, 1.0]));
        let (shown, exposed) = expose(&bright);
        assert!(exposed);
        assert_eq!(shown.get_pixel(0, 0)[0], 255);
    }
}
