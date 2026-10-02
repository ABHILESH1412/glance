//! HEIC/HEIF and AVIF, via the system libheif.
//!
//! libheif applies the container's own rotation/crop/mirror during decode, so
//! unlike the raster path there is no EXIF orientation step to do by hand.

use std::path::Path;

use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};

use crate::images::loader::{unsupported, LoadedImage};

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

    // HEIC and AVIF carry a profile like any other photograph; a phone shot in
    // Display P3 is flat without it.
    if let Some(profile) = handle.color_profile_raw() {
        crate::images::colour::to_srgb(&mut rgba, &profile.data);
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

/// Write a picture as HEIC, with the system's HEVC encoder. Quality 100 is
/// lossless.
pub fn encode(picture: &image::DynamicImage, quality: u8) -> Result<Vec<u8>, String> {
    use libheif_rs::{Channel, CompressionFormat, EncoderQuality, Image};
    let fail = |e: libheif_rs::HeifError| format!("Could not save as HEIC: {}", e.message);
    let rgba = picture.to_rgba8();
    let (width, height) = rgba.dimensions();
    let opaque = rgba.pixels().all(|p| p[3] == 255);
    let (chroma, channels) = if opaque { (RgbChroma::Rgb, 3) } else { (RgbChroma::Rgba, 4) };
    let lib = LibHeif::new();
    let mut image = Image::new(width, height, ColorSpace::Rgb(chroma)).map_err(fail)?;
    image.create_plane(Channel::Interleaved, width, height, 8).map_err(fail)?;
    {
        let planes = image.planes_mut();
        let plane = planes.interleaved.ok_or("Could not save as HEIC: no plane to write into.")?;
        let row = width as usize * channels;
        for (y, pixels) in rgba.rows().enumerate() {
            let start = y * plane.stride;
            let target = &mut plane.data[start..start + row];
            for (to, from) in target.chunks_exact_mut(channels).zip(pixels) {
                to.copy_from_slice(&from.0[..channels]);
            }
        }
    }
    let mut encoder = lib.encoder_for_format(CompressionFormat::Hevc).map_err(|_| {
        "Could not save as HEIC: this computer has no HEVC encoder for libheif (the x265 plugin).".to_string()
    })?;
    encoder
        .set_quality(if quality >= 100 { EncoderQuality::LossLess } else { EncoderQuality::Lossy(quality) })
        .map_err(fail)?;
    let mut context = HeifContext::new().map_err(fail)?;
    context.encode_image(&image, &mut encoder, None).map_err(fail)?;
    context.write_to_bytes().map_err(fail)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_picture_written_as_heic_opens_again() {
        let picture = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(64, 48, |x, y| {
            image::Rgba([(x * 4) as u8, (y * 5) as u8, 128, 255])
        }));
        let bytes = match super::encode(&picture, 90) {
            Ok(bytes) => bytes,
            // A libheif without an HEVC encoder: said plainly, as above.
            Err(why) if why.contains("no HEVC encoder") => return,
            Err(why) => panic!("{why}"),
        };
        let path = std::env::temp_dir().join(format!("glance-heic-{}.heic", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();
        let back = super::decode(&path, false).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!((back.width, back.height), (64, 48));
        // Lossy, so near rather than exact.
        let near = back.rgba.iter().zip(picture.to_rgba8().as_raw()).all(|(a, b)| a.abs_diff(*b) < 24);
        assert!(near, "the colours drifted too far");
    }
}
