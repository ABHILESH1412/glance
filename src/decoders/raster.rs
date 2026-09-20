//! PNG, JPEG, GIF, WebP, TIFF, BMP and the rest of the `image` crate's formats.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::time::Duration;

use image::codecs::gif::GifDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat, ImageReader};

use crate::loader::{open_error, unsupported, Frame, LoadedImage};

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
        _ => "Image",
    }
}
