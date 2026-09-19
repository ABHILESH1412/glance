//! Decoding happens here, off the main thread.
//!
//! The result is a plain RGBA8 buffer rather than a GDK type on purpose: GDK
//! objects are not `Send`, so decoding straight into one would pin this work to
//! the main loop and freeze the window on large files.

use std::io::ErrorKind;
use std::path::Path;

use image::{DynamicImage, ImageDecoder, ImageReader};

pub struct LoadedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub fn decode(path: &Path) -> Result<LoadedImage, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "this file".to_string());

    let reader = ImageReader::open(path)
        .map_err(|e| open_error(&name, &e))?
        .with_guessed_format()
        .map_err(|e| open_error(&name, &e))?;

    let mut decoder = reader
        .into_decoder()
        .map_err(|e| decode_error(path, &name, &e))?;

    // Read this before consuming the decoder; cameras write portrait shots as
    // landscape plus a rotation tag, and ignoring it shows them sideways.
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);

    let mut image =
        DynamicImage::from_decoder(decoder).map_err(|e| decode_error(path, &name, &e))?;
    image.apply_orientation(orientation);

    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();

    Ok(LoadedImage {
        width,
        height,
        rgba: rgba.into_raw(),
    })
}

fn open_error(name: &str, error: &std::io::Error) -> String {
    match error.kind() {
        ErrorKind::NotFound => format!("“{name}” no longer exists."),
        ErrorKind::PermissionDenied => format!("You do not have permission to read “{name}”."),
        _ => format!("Could not open “{name}”."),
    }
}

/// The toast stays short and human. The technical detail goes to stderr, where
/// it helps when debugging without being shoved in the user's face.
fn decode_error(path: &Path, name: &str, error: &image::ImageError) -> String {
    eprintln!("simple-viewer: {}: {error}", path.display());
    format!("“{name}” is not a supported image, or the file is damaged.")
}
