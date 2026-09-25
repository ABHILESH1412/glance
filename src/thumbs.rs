// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Small versions of images, for the filmstrip.
//!
//! The whole point is to be cheap. Camera raw is the case that makes a naive
//! implementation unusable: developing a 24MP frame takes seconds, and the
//! strip may want five of them at once. Every raw file already carries a small
//! JPEG the camera wrote, so use that and never touch the sensor data.
//!
//! Everything else decodes normally and is shrunk afterwards. That costs a few
//! hundred milliseconds on a big photo, which is fine on a worker thread and is
//! paid once because the results are cached.

use std::path::Path;

use image::metadata::Orientation;
use image::{DynamicImage, GenericImage, Rgba, RgbaImage};
use rawler::decoders::RawDecodeParams;
use rawler::rawsource::RawSource;

use crate::format::{self, Format};
use crate::loader::{self, LoadedImage};

/// Every thumbnail comes back at exactly `width` x `height`, letterboxed on a
/// transparent background.
///
/// Uniform size is the point: `GtkPicture` takes its natural width from the
/// image, so thumbnails of different shapes would claim different amounts of
/// room and the strip would come out ragged.
pub fn generate(path: &Path, width: u32, height: u32) -> Result<LoadedImage, String> {
    loader::quietly(|| generate_inner(path, width, height))
}

fn generate_inner(path: &Path, width: u32, height: u32) -> Result<LoadedImage, String> {
    let shrunk = if matches!(format::detect(path), Format::Raw) {
        match raw_embedded(path) {
            Some(embedded) => shrink_dynamic(embedded, width.max(height)),
            // No embedded image at all: pay for a develop. Rare, and cached.
            None => shrink(loader::decode(path)?, width.max(height)),
        }
    } else {
        shrink(loader::decode(path)?, width.max(height))
    };
    Ok(letterbox(shrunk, width, height))
}

/// Centre the shrunk image on a fixed transparent canvas.
fn letterbox(image: LoadedImage, width: u32, height: u32) -> LoadedImage {
    let scale = (f64::from(width) / f64::from(image.width.max(1)))
        .min(f64::from(height) / f64::from(image.height.max(1)))
        .min(1.0);
    let target_w = ((f64::from(image.width) * scale).round() as u32).clamp(1, width);
    let target_h = ((f64::from(image.height) * scale).round() as u32).clamp(1, height);

    let Some(source) = RgbaImage::from_raw(image.width, image.height, image.rgba) else {
        return blank(width, height, image.premultiplied);
    };
    let scaled = DynamicImage::ImageRgba8(source)
        .thumbnail(target_w, target_h)
        .into_rgba8();

    let mut canvas = RgbaImage::from_pixel(width, height, Rgba([0, 0, 0, 0]));
    let x = (width - scaled.width()) / 2;
    let y = (height - scaled.height()) / 2;
    if canvas.copy_from(&scaled, x, y).is_err() {
        return blank(width, height, image.premultiplied);
    }

    LoadedImage {
        width,
        height,
        rgba: canvas.into_raw(),
        premultiplied: image.premultiplied,
        label: String::new(),
        animation: Vec::new(),
        vector: None,
    }
}

fn blank(width: u32, height: u32, premultiplied: bool) -> LoadedImage {
    LoadedImage {
        width,
        height,
        rgba: vec![0; (width as usize) * (height as usize) * 4],
        premultiplied,
        label: String::new(),
        animation: Vec::new(),
        vector: None,
    }
}

/// The smallest image the camera embedded, preferring the thumbnail over the
/// larger preview: this is destined for a 72px slot.
fn raw_embedded(path: &Path) -> Option<DynamicImage> {
    let source = RawSource::new(path).ok()?;
    let decoder = rawler::get_decoder(&source).ok()?;
    let params = RawDecodeParams::default();

    let mut image = decoder
        .thumbnail_image(&source, &params)
        .ok()
        .flatten()
        .or_else(|| decoder.preview_image(&source, &params).ok().flatten())?;

    // The embedded image sits in sensor orientation; the rotation lives in the
    // raw file's metadata rather than in the JPEG.
    if let Some(orientation) = decoder
        .raw_metadata(&source, &params)
        .ok()
        .and_then(|meta| meta.exif.orientation)
        .and_then(|value| u8::try_from(value).ok())
        .and_then(Orientation::from_exif)
    {
        image.apply_orientation(orientation);
    }
    Some(image)
}

fn fitted(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    let longest = width.max(height).max(1);
    if longest <= max_edge {
        return (width.max(1), height.max(1));
    }
    let scale = f64::from(max_edge) / f64::from(longest);
    (
        ((f64::from(width) * scale).round() as u32).max(1),
        ((f64::from(height) * scale).round() as u32).max(1),
    )
}

fn shrink_dynamic(image: DynamicImage, max_edge: u32) -> LoadedImage {
    let (width, height) = fitted(image.width(), image.height(), max_edge);
    let rgba = image.thumbnail(width, height).into_rgba8();
    let (width, height) = rgba.dimensions();
    LoadedImage {
        width,
        height,
        rgba: rgba.into_raw(),
        premultiplied: false,
        label: String::new(),
        animation: Vec::new(),
        vector: None,
    }
}

fn shrink(full: LoadedImage, max_edge: u32) -> LoadedImage {
    let (width, height) = fitted(full.width, full.height, max_edge);
    if (width, height) == (full.width, full.height) {
        return full;
    }
    let Some(buffer) = RgbaImage::from_raw(full.width, full.height, full.rgba) else {
        // Cannot happen unless the decoder lied about its dimensions.
        return LoadedImage {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 0],
            premultiplied: full.premultiplied,
            label: String::new(),
            animation: Vec::new(),
            vector: None,
        };
    };
    let small = DynamicImage::ImageRgba8(buffer)
        .thumbnail(width, height)
        .into_rgba8();
    let (width, height) = small.dimensions();
    LoadedImage {
        width,
        height,
        rgba: small.into_raw(),
        // Shrinking preserves whichever alpha convention came in.
        premultiplied: full.premultiplied,
        label: String::new(),
        animation: Vec::new(),
        vector: None,
    }
}
