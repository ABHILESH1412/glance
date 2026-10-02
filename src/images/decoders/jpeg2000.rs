// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! JPEG 2000, read and written with OpenJPEG, the reference library: both the
//! `.jp2` file and a bare `.j2k` codestream.
//!
//! A JPEG 2000 picture is a set of components, each with a precision of its
//! own and possibly a lower resolution than the first (colour kept at half
//! size, as in a JPEG). They are brought to one size and eight bits here, and
//! a YCbCr picture turned to RGB.

use std::ffi::{c_char, c_void, CStr, CString};
use std::io::{Cursor, Seek, SeekFrom, Write};
use std::path::Path;

use image::DynamicImage;
use openjpeg_sys as opj;

use crate::images::loader::{unsupported, LoadedImage};

/// The JP2 file's signature box, and a bare codestream's first two markers.
const JP2_SIGNATURE: &[u8] = &[0, 0, 0, 0x0c, b'j', b'P', b' ', b' ', 0x0d, 0x0a, 0x87, 0x0a];
const CODESTREAM: &[u8] = &[0xff, 0x4f, 0xff, 0x51];

/// Whether a file's first bytes are JPEG 2000's.
pub fn sniff(head: &[u8]) -> bool {
    head.starts_with(JP2_SIGNATURE) || head.starts_with(CODESTREAM)
}

/// What OpenJPEG said went wrong, kept for the log.
unsafe extern "C" fn keep_message(message: *const c_char, data: *mut c_void) {
    // SAFETY: `data` is the String the caller handed over, alive for as long
    // as the codec is; `message` is a C string OpenJPEG owns.
    unsafe {
        if message.is_null() || data.is_null() {
            return;
        }
        let kept = &mut *data.cast::<String>();
        kept.push_str(CStr::from_ptr(message).to_string_lossy().trim());
    }
}

fn threads() -> i32 {
    std::thread::available_parallelism().map_or(1, |n| i32::try_from(n.get()).unwrap_or(1))
}

pub fn decode(path: &Path) -> Result<LoadedImage, String> {
    let head = std::fs::read(path).map_err(|e| crate::images::loader::open_error(path, &e))?;
    let format = if head.starts_with(CODESTREAM) { opj::CODEC_FORMAT::OPJ_CODEC_J2K } else { opj::CODEC_FORMAT::OPJ_CODEC_JP2 };
    let name = CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| unsupported(path, "a NUL in the file name"))?;
    let mut message = String::new();
    // SAFETY: OpenJPEG's own calls, each object destroyed once on every way
    // out; the image is only read while it is held.
    unsafe {
        let codec = opj::opj_create_decompress(format);
        if codec.is_null() {
            return Err(unsupported(path, "OpenJPEG would not start"));
        }
        opj::opj_set_error_handler(codec, Some(keep_message), std::ptr::from_mut(&mut message).cast());
        let mut parameters: opj::opj_dparameters_t = std::mem::zeroed();
        opj::opj_set_default_decoder_parameters(&mut parameters);
        let stream = opj::opj_stream_create_default_file_stream(name.as_ptr(), 1);
        let mut image: *mut opj::opj_image_t = std::ptr::null_mut();
        let ok = !stream.is_null()
            && opj::opj_setup_decoder(codec, &mut parameters) != 0
            && opj::opj_codec_set_threads(codec, threads()) >= 0
            && opj::opj_read_header(stream, codec, &mut image) != 0
            && opj::opj_decode(codec, stream, image) != 0
            && opj::opj_end_decompress(codec, stream) != 0;
        let result = if ok && !image.is_null() { pixels(&*image) } else { None };
        if !image.is_null() {
            opj::opj_image_destroy(image);
        }
        if !stream.is_null() {
            opj::opj_stream_destroy(stream);
        }
        opj::opj_destroy_codec(codec);
        let Some((width, height, mut rgba, profile)) = result else {
            let why = if message.is_empty() { "OpenJPEG could not decode it".to_string() } else { message };
            return Err(unsupported(path, why));
        };
        if let Some(profile) = profile {
            crate::images::colour::to_srgb(&mut rgba, &profile);
        }
        Ok(LoadedImage {
            width,
            height,
            rgba,
            premultiplied: false,
            label: "JPEG 2000".to_string(),
            animation: Vec::new(),
            vector: None,
        })
    }
}

/// A decoded picture: its size, eight-bit RGBA, and its colour profile, if
/// it has one.
type Decoded = (u32, u32, Vec<u8>, Option<Vec<u8>>);

/// The decoded components as eight-bit RGBA, and the colour profile, if
/// there is one.
///
/// # Safety
/// `image` must be an image OpenJPEG has decoded.
unsafe fn pixels(image: &opj::opj_image_t) -> Option<Decoded> {
    let count = image.numcomps as usize;
    if count == 0 || image.comps.is_null() {
        return None;
    }
    // SAFETY: OpenJPEG gives `numcomps` components, each with `w * h` samples.
    let comps = unsafe { std::slice::from_raw_parts(image.comps, count) };
    let first = comps[0];
    let (width, height) = (first.w, first.h);
    if width == 0 || height == 0 || comps.iter().any(|c| c.data.is_null() || c.w == 0 || c.h == 0) {
        return None;
    }
    // Each component's samples, a closure that reads one at a full-size
    // pixel as eight bits.
    let sample = |c: &opj::opj_image_comp_t, x: u32, y: u32| -> f32 {
        let cx = (u64::from(x) * u64::from(first.dx) / u64::from(c.dx.max(1))).min(u64::from(c.w - 1)) as usize;
        let cy = (u64::from(y) * u64::from(first.dy) / u64::from(c.dy.max(1))).min(u64::from(c.h - 1)) as usize;
        // SAFETY: inside the component's `w * h` samples.
        let raw = unsafe { *c.data.add(cy * c.w as usize + cx) };
        let precision = c.prec.clamp(1, 31);
        let value = if c.sgnd != 0 { i64::from(raw) + (1 << (precision - 1)) } else { i64::from(raw) };
        let most = ((1u64 << precision) - 1) as f32;
        (value as f32 / most * 255.0).clamp(0.0, 255.0)
    };
    let alpha = comps.iter().position(|c| c.alpha != 0);
    let colours: Vec<&opj::opj_image_comp_t> =
        comps.iter().enumerate().filter(|(i, _)| Some(*i) != alpha).map(|(_, c)| c).collect();
    let ycc = matches!(image.color_space, opj::COLOR_SPACE::OPJ_CLRSPC_SYCC | opj::COLOR_SPACE::OPJ_CLRSPC_EYCC);
    let cmyk = image.color_space == opj::COLOR_SPACE::OPJ_CLRSPC_CMYK && colours.len() >= 4;
    // With no alpha marked, a second component beside grey, or a fourth
    // beside colour, is taken for one.
    let alpha = alpha.or(match colours.len() {
        2 => Some(1),
        4 if !cmyk => Some(3),
        _ => None,
    });
    let colours: Vec<&opj::opj_image_comp_t> =
        comps.iter().enumerate().filter(|(i, _)| Some(*i) != alpha).map(|(_, c)| c).collect();

    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let [r, g, b] = if cmyk {
                let k = 1.0 - sample(colours[3], x, y) / 255.0;
                [0, 1, 2].map(|i| (255.0 - sample(colours[i], x, y)) * k)
            } else if colours.len() >= 3 {
                let (a, b, c) = (sample(colours[0], x, y), sample(colours[1], x, y), sample(colours[2], x, y));
                if ycc {
                    let (cb, cr) = (b - 128.0, c - 128.0);
                    [a + 1.402 * cr, a - 0.344_136 * cb - 0.714_136 * cr, a + 1.772 * cb]
                } else {
                    [a, b, c]
                }
            } else {
                let grey = sample(colours[0], x, y);
                [grey, grey, grey]
            };
            let a = alpha.map_or(255.0, |i| sample(&comps[i], x, y));
            rgba.extend([r, g, b, a].map(|v| v.round().clamp(0.0, 255.0) as u8));
        }
    }
    let profile = (!image.icc_profile_buf.is_null() && image.icc_profile_len > 0).then(|| {
        // SAFETY: OpenJPEG's own buffer, `icc_profile_len` long.
        unsafe { std::slice::from_raw_parts(image.icc_profile_buf, image.icc_profile_len as usize) }.to_vec()
    });
    Some((width, height, rgba, profile))
}

/// Where an encoder writes: the bytes so far, and the place in them.
type Sink = Cursor<Vec<u8>>;

unsafe extern "C" fn sink_write(buffer: *mut c_void, length: usize, data: *mut c_void) -> usize {
    // SAFETY: `data` is the Sink handed to the stream; `buffer` holds
    // `length` bytes.
    unsafe {
        let sink = &mut *data.cast::<Sink>();
        let bytes = std::slice::from_raw_parts(buffer.cast::<u8>(), length);
        sink.write_all(bytes).map_or(usize::MAX, |()| length)
    }
}

unsafe extern "C" fn sink_skip(by: i64, data: *mut c_void) -> i64 {
    // SAFETY: as above.
    let sink = unsafe { &mut *data.cast::<Sink>() };
    sink.seek(SeekFrom::Current(by)).map_or(-1, |_| by)
}

unsafe extern "C" fn sink_seek(to: i64, data: *mut c_void) -> i32 {
    // SAFETY: as above.
    let sink = unsafe { &mut *data.cast::<Sink>() };
    i32::from(u64::try_from(to).is_ok_and(|to| sink.seek(SeekFrom::Start(to)).is_ok()))
}

/// Write a picture as a `.jp2` file. Quality 100 is lossless; below that,
/// each step down squeezes harder.
pub fn encode(picture: &DynamicImage, quality: u8) -> Result<Vec<u8>, String> {
    let rgba = picture.to_rgba8();
    let (width, height) = rgba.dimensions();
    let opaque = rgba.pixels().all(|p| p[3] == 255);
    let count: usize = if opaque { 3 } else { 4 };
    let mut sink: Sink = Cursor::new(Vec::new());
    let mut message = String::new();
    // SAFETY: OpenJPEG's own calls, each object destroyed once; the sink
    // outlives the stream that writes to it.
    unsafe {
        let mut layout: Vec<opj::opj_image_cmptparm_t> = (0..count)
            .map(|_| opj::opj_image_cmptparm_t { dx: 1, dy: 1, w: width, h: height, x0: 0, y0: 0, prec: 8, bpp: 8, sgnd: 0 })
            .collect();
        let image = opj::opj_image_create(count as u32, layout.as_mut_ptr(), opj::COLOR_SPACE::OPJ_CLRSPC_SRGB);
        if image.is_null() {
            return Err("Could not save: OpenJPEG would not start.".into());
        }
        (*image).x1 = width;
        (*image).y1 = height;
        let comps = std::slice::from_raw_parts_mut((*image).comps, count);
        if !opaque {
            comps[3].alpha = 1;
        }
        for (i, pixel) in rgba.pixels().enumerate() {
            for (c, comp) in comps.iter_mut().enumerate() {
                *comp.data.add(i) = i32::from(pixel[c]);
            }
        }

        let mut parameters: opj::opj_cparameters_t = std::mem::zeroed();
        opj::opj_set_default_encoder_parameters(&mut parameters);
        parameters.tcp_numlayers = 1;
        parameters.cp_disto_alloc = 1;
        if quality >= 100 {
            parameters.tcp_rates[0] = 0.0;
            parameters.irreversible = 0;
        } else {
            // From about 3:1 at the top of the dial to 50:1 at the bottom.
            parameters.tcp_rates[0] = 3.0 + f32::from(100 - quality.max(1)) * 0.48;
            parameters.irreversible = 1;
        }
        parameters.tcp_mct = 1;
        // Each resolution halves the picture; a small one has fewer to give.
        let smallest = width.min(height).max(1);
        parameters.numresolution = (smallest.ilog2() as i32 + 1).clamp(1, 6);

        let codec = opj::opj_create_compress(opj::CODEC_FORMAT::OPJ_CODEC_JP2);
        opj::opj_set_error_handler(codec, Some(keep_message), std::ptr::from_mut(&mut message).cast());
        let stream = opj::opj_stream_default_create(0);
        opj::opj_stream_set_user_data(stream, std::ptr::from_mut(&mut sink).cast(), None);
        opj::opj_stream_set_write_function(stream, Some(sink_write));
        opj::opj_stream_set_skip_function(stream, Some(sink_skip));
        opj::opj_stream_set_seek_function(stream, Some(sink_seek));
        let ok = !codec.is_null()
            && !stream.is_null()
            && opj::opj_setup_encoder(codec, &mut parameters, image) != 0
            && opj::opj_codec_set_threads(codec, threads()) >= 0
            && opj::opj_start_compress(codec, image, stream) != 0
            && opj::opj_encode(codec, stream) != 0
            && opj::opj_end_compress(codec, stream) != 0;
        if !stream.is_null() {
            opj::opj_stream_destroy(stream);
        }
        if !codec.is_null() {
            opj::opj_destroy_codec(codec);
        }
        opj::opj_image_destroy(image);
        if !ok {
            return Err(format!("Could not save as JPEG 2000: {}", if message.is_empty() { "OpenJPEG failed" } else { &message }));
        }
    }
    Ok(sink.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture() -> DynamicImage {
        let mut rgba = image::RgbaImage::new(40, 24);
        for (x, y, pixel) in rgba.enumerate_pixels_mut() {
            *pixel = image::Rgba([(x * 6) as u8, (y * 10) as u8, 200, if x < 20 { 255 } else { 90 }]);
        }
        DynamicImage::ImageRgba8(rgba)
    }

    #[test]
    fn a_lossless_picture_comes_back_exactly_with_its_transparency() {
        let bytes = encode(&picture(), 100).unwrap();
        assert!(sniff(&bytes), "a JP2 file starts with its signature");
        let path = std::env::temp_dir().join(format!("glance-j2k-{}.jp2", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();
        let back = decode(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!((back.width, back.height), (40, 24));
        assert_eq!(back.rgba, picture().to_rgba8().into_raw());
    }

    #[test]
    fn a_lower_quality_is_a_smaller_file() {
        let (best, worst) = (encode(&picture(), 100).unwrap(), encode(&picture(), 10).unwrap());
        assert!(worst.len() < best.len(), "{} is not under {}", worst.len(), best.len());
    }
}
