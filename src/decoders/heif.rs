//! HEIC/HEIF and AVIF, via the system libheif.
//!
//! libheif applies the container's own rotation/crop/mirror during decode, so
//! unlike the raster path there is no EXIF orientation step to do by hand.

use std::path::Path;

use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};

use crate::loader::{unsupported, LoadedImage};

pub fn decode(path: &Path, avif: bool) -> Result<LoadedImage, String> {
    let path_str = path
        .to_str()
        .ok_or_else(|| "That file name is not valid text.".to_string())?;

    let lib = LibHeif::new();
    let context = HeifContext::read_from_file(path_str).map_err(|e| unsupported(path, &e))?;
    let handle = context
        .primary_image_handle()
        .map_err(|e| unsupported(path, &e))?;

    let container = if avif { "AVIF" } else { "HEIF" };
    // >8 bits per channel means the source carries more range than the sRGB
    // texture we hand to GDK, which is worth saying out loud.
    let label = if handle.luma_bits_per_pixel() > 8 {
        format!("{container} · 10-bit")
    } else {
        container.to_string()
    };

    let image = lib
        .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgba), None)
        .map_err(|e| unsupported(path, &e))?;

    let planes = image.planes();
    let plane = planes
        .interleaved
        .ok_or_else(|| unsupported(path, "libheif returned no interleaved RGBA plane"))?;

    let width = plane.width;
    let height = plane.height;
    let row_bytes = width as usize * 4;

    // The plane is padded to `stride`, which is usually wider than the image.
    // Copying row by row drops that padding.
    let mut rgba = Vec::with_capacity(row_bytes * height as usize);
    for y in 0..height as usize {
        let start = y * plane.stride;
        let end = start + row_bytes;
        let Some(row) = plane.data.get(start..end) else {
            return Err(unsupported(path, "libheif plane was shorter than expected"));
        };
        rgba.extend_from_slice(row);
    }

    Ok(LoadedImage {
        width,
        height,
        rgba,
        premultiplied: handle.is_premultiplied_alpha(),
        label,
        animation: Vec::new(),
        vector: None,
    })
}
