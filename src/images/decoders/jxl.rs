// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! JPEG XL, read with jxl-oxide: pure Rust, so nothing more to install, and
//! already built in for the raw decoder, which meets it inside newer DNGs.
//!
//! Both kinds of file are read: a bare codestream and the ISO box container
//! around one. Whatever colour space the picture was made in, it is asked for
//! in sRGB, with moxcms doing the work when an ICC profile is involved, and
//! an animation comes back frame by frame like a GIF's.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::time::Duration;

use jxl_oxide::{EnumColourEncoding, JxlImage, Moxcms, Render, RenderingIntent};

use crate::images::decoders::raster::{MAX_ANIMATION_BYTES, MAX_FRAMES, MIN_DELAY, SLOW_DELAY};
use crate::images::loader::{open_error, unsupported, Frame, LoadedImage};

/// A bare codestream's signature, and the container's first box.
const CODESTREAM: &[u8] = &[0xff, 0x0a];
const CONTAINER: &[u8] = &[0, 0, 0, 0x0c, b'J', b'X', b'L', b' ', 0x0d, 0x0a, 0x87, 0x0a];

/// Whether a file's first bytes are JPEG XL's.
pub fn sniff(head: &[u8]) -> bool {
    head.starts_with(CODESTREAM) || head.starts_with(CONTAINER)
}

pub fn decode(path: &Path) -> Result<LoadedImage, String> {
    let file = File::open(path).map_err(|e| open_error(path, &e))?;
    let mut image = JxlImage::builder()
        .read(BufReader::new(file))
        .map_err(|e| unsupported(path, &e))?;

    let grey = image.pixel_format().is_grayscale();
    let hdr = image.hdr_type().is_some();
    image.set_cms(Moxcms);
    image.request_color_encoding(if grey {
        EnumColourEncoding::gray_srgb(RenderingIntent::Relative)
    } else {
        EnumColourEncoding::srgb(RenderingIntent::Relative)
    });

    let mut label = String::from("JPEG XL");
    if hdr {
        label.push_str(" · HDR, fitted to the screen");
    }

    // Ticks are the file's own unit of time, so many to the second.
    let tick = image.image_header().metadata.animation.as_ref().and_then(|animation| {
        (animation.tps_numerator > 0).then(|| {
            Duration::from_secs_f64(f64::from(animation.tps_denominator) / f64::from(animation.tps_numerator))
        })
    });
    let keyframes = image.num_loaded_keyframes();
    if keyframes == 0 {
        return Err(unsupported(path, "no frames"));
    }

    let first = image.render_frame(0).map_err(|e| unsupported(path, &e))?;
    let (width, height, rgba) = to_rgba(&first);
    let Some(tick) = tick.filter(|_| keyframes > 1) else {
        return Ok(LoadedImage {
            width,
            height,
            rgba,
            premultiplied: false,
            label,
            animation: Vec::new(),
            vector: None,
        });
    };

    let delay = |render: &Render| {
        let delay = tick * render.duration();
        if delay < MIN_DELAY { SLOW_DELAY } else { delay }
    };
    let mut frames = vec![Frame { rgba: rgba.clone(), delay: delay(&first) }];
    let mut bytes = rgba.len();
    for index in 1..keyframes {
        if frames.len() >= MAX_FRAMES || bytes >= MAX_ANIMATION_BYTES {
            break;
        }
        // A frame that will not decode ends the animation where it is,
        // rather than losing the frames already shown.
        let Ok(render) = image.render_frame(index) else { break };
        let (w, h, pixels) = to_rgba(&render);
        if (w, h) != (width, height) {
            break;
        }
        bytes += pixels.len();
        frames.push(Frame { rgba: pixels, delay: delay(&render) });
    }
    if frames.len() > 1 {
        label = format!("{label} · {} frames", frames.len());
    }
    Ok(LoadedImage {
        width,
        height,
        rgba,
        premultiplied: false,
        label,
        animation: if frames.len() > 1 { frames } else { Vec::new() },
        vector: None,
    })
}

/// A rendered frame as eight-bit RGBA, whatever channels it came with:
/// grey, grey and alpha, colour, or colour and alpha.
fn to_rgba(render: &Render) -> (u32, u32, Vec<u8>) {
    let mut stream = render.stream();
    let (width, height, channels) = (stream.width(), stream.height(), stream.channels() as usize);
    let pixels = width as usize * height as usize;
    let mut samples = vec![0u8; pixels * channels];
    stream.write_to_buffer(&mut samples);
    if channels == 4 {
        return (width, height, samples);
    }
    let mut rgba = Vec::with_capacity(pixels * 4);
    for pixel in samples.chunks_exact(channels) {
        match *pixel {
            [v] => rgba.extend_from_slice(&[v, v, v, 255]),
            [v, a] => rgba.extend_from_slice(&[v, v, v, a]),
            [r, g, b] => rgba.extend_from_slice(&[r, g, b, 255]),
            // More than four: colour and alpha come first, the rest (depth,
            // spot colours) are not for showing.
            [r, g, b, a, ..] => rgba.extend_from_slice(&[r, g, b, a]),
            [] => {}
        }
    }
    (width, height, rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three 100×100 frames, red, green and blue, half a second each: what
    /// `cjxl` makes of a three-frame GIF.
    const ANIMATION: &[u8] = &[
        0xff, 0x0a, 0x18, 0x13, 0x41, 0x00, 0x02, 0x8a, 0x4b, 0x02, 0x08, 0x00, 0xca, 0x00, 0x00, 0x40,
        0x00, 0x4b, 0x18, 0x8b, 0x15, 0x80, 0x5d, 0xfe, 0x7f, 0x00, 0xc9, 0x16, 0x0e, 0xae, 0x2e, 0xfd,
        0x01, 0x08, 0x00, 0xca, 0x00, 0x00, 0x40, 0x00, 0x4b, 0x18, 0x8b, 0x15, 0x80, 0x5d, 0xfe, 0x7f,
        0xc6, 0x7e, 0x16, 0x0e, 0x28, 0x64, 0x01, 0x00, 0x08, 0x00, 0xca, 0x04, 0x00, 0x40, 0x00, 0x4b,
        0x18, 0x8b, 0x15, 0x80, 0x5d, 0xfe, 0x7f, 0x42, 0x70, 0x17, 0x0e, 0xc4, 0xe5, 0xfc, 0x01,
    ];

    fn read(name: &str, bytes: &[u8]) -> LoadedImage {
        let dir = std::env::temp_dir().join(format!("glance-jxl-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(crate::images::format::detect(&path), crate::images::format::Format::Jxl);
        let image = decode(&path);
        std::fs::remove_dir_all(&dir).ok();
        image.unwrap()
    }

    #[test]
    fn both_kinds_of_file_are_known_by_their_first_bytes() {
        assert!(sniff(ANIMATION));
        assert!(sniff(CONTAINER));
        assert!(!sniff(b"\x89PNG\r\n\x1a\n"));
        // JPEG 2000's signature box differs only in its name.
        assert!(!sniff(&[0, 0, 0, 0x0c, b'j', b'P', b' ', b' ', 0x0d, 0x0a, 0x87, 0x0a]));
    }

    #[test]
    fn an_animation_comes_back_frame_by_frame_in_its_own_time() {
        let image = read("frames.jxl", ANIMATION);
        assert_eq!((image.width, image.height), (100, 100));
        assert_eq!(image.label, "JPEG XL · 3 frames");
        let firsts: Vec<&[u8]> = image.animation.iter().map(|frame| &frame.rgba[..4]).collect();
        assert_eq!(firsts, [[255, 0, 0, 255], [0, 128, 0, 255], [0, 0, 255, 255]]);
        assert!(image.animation.iter().all(|frame| frame.delay == Duration::from_millis(500)));
        assert_eq!(image.rgba, image.animation[0].rgba);
    }

    #[test]
    fn a_damaged_file_is_refused_rather_than_shown_half_drawn() {
        let dir = std::env::temp_dir().join(format!("glance-jxl-{}-cut", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cut.jxl");
        std::fs::write(&path, &ANIMATION[..12]).unwrap();
        let result = decode(&path);
        std::fs::remove_dir_all(&dir).ok();
        assert!(result.is_err());
    }
}
