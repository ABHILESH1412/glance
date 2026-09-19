//! Camera raw, via rawler.
//!
//! Two ways to get pixels out of a raw file, and the choice matters:
//!
//! * The **embedded preview** is a JPEG the camera wrote alongside the sensor
//!   data. It is instant and carries the maker's own colour and tone rendering.
//!   Nine of rawler's twenty-eight decoders expose one (ARW, CR2, CR3, DNG,
//!   NEF, RW2, RAF, TFR, PEF). Its size is entirely up to the camera: a Canon
//!   EOS R embeds the full 6720x4480, a Sony a7III embeds 1616x1080, and a DJI
//!   Mavic 3 Pro embeds a mere 960x720 for a 20MP sensor.
//! * **Developing** demosaics the sensor data, then white-balances, calibrates
//!   and sRGB-encodes it. Always full resolution, works for all twenty-eight
//!   formats, but costs seconds of CPU and allocates several times the image
//!   size in f32 intermediates.
//!
//! So the preview is used only when it is actually big enough to look at, and
//! developing covers everything else. The natural next step is to show the
//! preview immediately and quietly replace it with the developed image once it
//! is ready, which would pair well with zoom.

use std::path::Path;

use image::metadata::Orientation;
use image::{DynamicImage, RgbImage};
use rawler::decoders::{Decoder, RawDecodeParams};
use rawler::imgop::develop::{Intermediate, RawDevelop};
use rawler::rawsource::RawSource;

use crate::loader::{open_error, unsupported, LoadedImage};

/// Below this, an embedded preview is too small to be worth showing in place
/// of the real image — a window on any current display is wider than this.
const MIN_PREVIEW_LONG_EDGE: u32 = 1600;

pub fn decode(path: &Path) -> Result<LoadedImage, String> {
    let source = RawSource::new(path).map_err(|e| open_error(path, &e))?;
    let decoder = rawler::get_decoder(&source).map_err(|e| unsupported(path, &e))?;
    let params = RawDecodeParams::default();

    // Only the MRW decoder implements this, but it is a full-resolution image
    // for free when present.
    if let Some(image) = decoder.full_image(&source, &params).ok().flatten() {
        let orientation = metadata_orientation(decoder.as_ref(), &source, &params);
        return Ok(finish(image, orientation, "RAW (embedded image)"));
    }

    let preview = decoder.preview_image(&source, &params).ok().flatten();
    if let Some(image) = preview.as_ref().filter(|i| large_enough(i)) {
        let orientation = metadata_orientation(decoder.as_ref(), &source, &params);
        return Ok(finish(image.clone(), orientation, "RAW (preview)"));
    }

    match develop(path, decoder.as_ref(), &source, &params) {
        // `develop` applies its own orientation.
        Ok(image) => Ok(finish(image, None, "RAW (developed)")),
        Err(error) => match preview {
            // A small preview still beats refusing to show the file.
            Some(image) => {
                let orientation = metadata_orientation(decoder.as_ref(), &source, &params);
                Ok(finish(image, orientation, "RAW (small preview)"))
            }
            None => Err(error),
        },
    }
}

fn large_enough(image: &DynamicImage) -> bool {
    image.width().max(image.height()) >= MIN_PREVIEW_LONG_EDGE
}

/// The embedded paths hand back an image in sensor orientation, with the
/// rotation recorded in the raw file's metadata rather than in the JPEG.
fn finish(mut image: DynamicImage, orientation: Option<Orientation>, label: &str) -> LoadedImage {
    if let Some(orientation) = orientation {
        image.apply_orientation(orientation);
    }
    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    LoadedImage {
        width,
        height,
        rgba: rgba.into_raw(),
        premultiplied: false,
        label: label.to_string(),
    }
}

fn metadata_orientation(
    decoder: &dyn Decoder,
    source: &RawSource,
    params: &RawDecodeParams,
) -> Option<Orientation> {
    decoder
        .raw_metadata(source, params)
        .ok()
        .and_then(|meta| meta.exif.orientation)
        .and_then(|value| u8::try_from(value).ok())
        .and_then(Orientation::from_exif)
}

/// Demosaic the sensor data into a viewable image.
///
/// rawler can serialise a developed frame to TIFF, but the tags it writes are
/// rejected by the `image` crate's TIFF parser ("tag has invalid type"), so the
/// round-trip through a container is skipped entirely: `develop_intermediate`
/// hands back the pixels and they are converted here. That also avoids
/// compressing and re-parsing a few hundred megabytes for nothing.
fn develop(
    path: &Path,
    decoder: &dyn Decoder,
    source: &RawSource,
    params: &RawDecodeParams,
) -> Result<DynamicImage, String> {
    let raw = decoder
        .raw_image(source, params, false)
        .map_err(|e| unsupported(path, &e))?;

    // The develop pipeline does not rotate pixels; it records the orientation
    // and expects the consumer to apply it.
    let orientation = u8::try_from(raw.orientation.to_u16())
        .ok()
        .and_then(Orientation::from_exif)
        .unwrap_or(Orientation::NoTransforms);

    let developed = RawDevelop::default()
        .develop_intermediate(&raw)
        .map_err(|e| unsupported(path, &e))?;

    // The pipeline's last step is sRGB encoding, so these floats are display
    // referred and simply need scaling to 8 bits.
    let (width, height, rgb) = match developed {
        Intermediate::Monochrome(plane) => {
            let mut rgb = Vec::with_capacity(plane.data.len() * 3);
            for &value in &plane.data {
                let grey = to_u8(value);
                rgb.extend_from_slice(&[grey, grey, grey]);
            }
            (plane.width, plane.height, rgb)
        }
        Intermediate::ThreeColor(plane) => {
            let mut rgb = Vec::with_capacity(plane.data.len() * 3);
            for pixel in &plane.data {
                rgb.extend_from_slice(&[to_u8(pixel[0]), to_u8(pixel[1]), to_u8(pixel[2])]);
            }
            (plane.width, plane.height, rgb)
        }
        // Four-colour sensors (CMYG, RGBE). Taking the leading three channels
        // is an approximation, but it beats refusing to show the file.
        Intermediate::FourColor(plane) => {
            let mut rgb = Vec::with_capacity(plane.data.len() * 3);
            for pixel in &plane.data {
                rgb.extend_from_slice(&[to_u8(pixel[0]), to_u8(pixel[1]), to_u8(pixel[2])]);
            }
            (plane.width, plane.height, rgb)
        }
    };

    let buffer = RgbImage::from_raw(width as u32, height as u32, rgb)
        .ok_or_else(|| unsupported(path, "developed image had unexpected dimensions"))?;

    let mut image = DynamicImage::ImageRgb8(buffer);
    image.apply_orientation(orientation);
    Ok(image)
}

fn to_u8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}
