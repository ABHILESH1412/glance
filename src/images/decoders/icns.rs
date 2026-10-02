// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! macOS icons. An `.icns` file holds the same icon at several sizes; the
//! largest is shown. Writing makes every standard size up to the picture's
//! own, from 16 to 1024 pixels square.

use std::path::Path;

use image::DynamicImage;

use crate::images::loader::{open_error, unsupported, LoadedImage};

/// The sizes an icon is written at, as many as fit under the picture.
const SIZES: &[u32] = &[16, 32, 64, 128, 256, 512, 1024];

pub fn sniff(head: &[u8]) -> bool {
    head.starts_with(b"icns")
}

pub fn decode(path: &Path) -> Result<LoadedImage, String> {
    let file = std::fs::File::open(path).map_err(|e| open_error(path, &e))?;
    let family = icns::IconFamily::read(std::io::BufReader::new(file)).map_err(|e| unsupported(path, e))?;
    let largest = family
        .available_icons()
        .into_iter()
        .max_by_key(|kind| kind.pixel_width())
        .ok_or_else(|| unsupported(path, "no icon in it that can be shown"))?;
    let icon = family.get_icon_with_type(largest).map_err(|e| unsupported(path, e))?;
    let icon = icon.convert_to(icns::PixelFormat::RGBA);
    let count = family.available_icons().len();
    Ok(LoadedImage {
        width: icon.width(),
        height: icon.height(),
        rgba: icon.into_data().into_vec(),
        premultiplied: false,
        label: if count > 1 { format!("ICNS · {count} sizes") } else { "ICNS".to_string() },
        animation: Vec::new(),
        vector: None,
    })
}

/// Why a picture cannot be an icon as it is, if it cannot.
pub fn refusal(width: u32, height: u32) -> Option<String> {
    if width != height {
        return Some(format!(
            "A macOS icon is square; this picture is {width} × {height}. Crop it to a square first, or pick another format."
        ));
    }
    (width < SIZES[0]).then(|| format!("A macOS icon is at least {0} × {0} pixels; this one is {width} × {height}.", SIZES[0]))
}

pub fn encode(picture: &DynamicImage) -> Result<Vec<u8>, String> {
    let (width, height) = (picture.width(), picture.height());
    if let Some(refusal) = refusal(width, height) {
        return Err(refusal);
    }
    let mut family = icns::IconFamily::new();
    // Every standard size the picture reaches, scaled down to it.
    for &size in SIZES.iter().filter(|&&size| size <= width) {
        let scaled = if size == width {
            picture.to_rgba8()
        } else {
            picture.resize_exact(size, size, image::imageops::FilterType::Lanczos3).to_rgba8()
        };
        let icon = icns::Image::from_data(icns::PixelFormat::RGBA, size, size, scaled.into_raw())
            .map_err(|e| format!("Could not save as an icon: {e}"))?;
        family.add_icon(&icon).map_err(|e| format!("Could not save as an icon: {e}"))?;
    }
    let mut out = Vec::new();
    family.write(&mut out).map_err(|e| format!("Could not save as an icon: {e}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_icon_holds_every_size_up_to_the_picture_and_shows_the_largest() {
        let picture = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(256, 256, image::Rgba([20, 120, 220, 200])));
        let bytes = encode(&picture).unwrap();
        assert!(sniff(&bytes));
        let family = icns::IconFamily::read(std::io::Cursor::new(&bytes)).unwrap();
        let mut sizes: Vec<u32> = family.available_icons().iter().map(|kind| kind.pixel_width()).collect();
        sizes.sort_unstable();
        sizes.dedup();
        assert_eq!(sizes, vec![16, 32, 64, 128, 256]);

        let path = std::env::temp_dir().join(format!("glance-icns-{}.icns", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();
        let back = decode(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!((back.width, back.height), (256, 256));
        assert_eq!(&back.rgba[..4], &[20, 120, 220, 200]);
    }

    #[test]
    fn only_a_square_picture_makes_an_icon() {
        assert!(refusal(512, 300).is_some_and(|why| why.contains("square")));
        assert!(refusal(8, 8).is_some());
        assert!(refusal(48, 48).is_none());
    }
}
