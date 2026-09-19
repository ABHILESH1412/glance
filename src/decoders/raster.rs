//! PNG, JPEG, GIF, WebP, TIFF, BMP and the rest of the `image` crate's formats.

use std::path::Path;

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};

use crate::loader::{open_error, unsupported, LoadedImage};

pub fn decode(path: &Path) -> Result<LoadedImage, String> {
    let reader = ImageReader::open(path)
        .map_err(|e| open_error(path, &e))?
        .with_guessed_format()
        .map_err(|e| open_error(path, &e))?;

    let label = reader.format().map(label_for).unwrap_or("Image").to_string();

    let mut decoder = reader
        .into_decoder()
        .map_err(|e| unsupported(path, &e))?;

    // Read this before consuming the decoder; cameras write portrait shots as
    // landscape plus a rotation tag, and ignoring it shows them sideways.
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);

    let mut image = DynamicImage::from_decoder(decoder).map_err(|e| unsupported(path, &e))?;
    image.apply_orientation(orientation);

    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();

    Ok(LoadedImage {
        width,
        height,
        rgba: rgba.into_raw(),
        premultiplied: false,
        label,
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
        _ => "Image",
    }
}
