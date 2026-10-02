// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Photoshop documents. Reading shows the picture as Photoshop last saved
//! it, all its layers together; writing makes a document of one layer, which
//! every program that opens PSDs can read, transparency and all.

use std::path::Path;

use image::DynamicImage;

use crate::images::loader::{open_error, unsupported, LoadedImage};

/// "8BPS", version 1: a Photoshop document (version 2 is the large kind).
pub fn sniff(head: &[u8]) -> bool {
    head.starts_with(b"8BPS")
}

pub fn decode(path: &Path) -> Result<LoadedImage, String> {
    let bytes = std::fs::read(path).map_err(|e| open_error(path, &e))?;
    let large = bytes.get(4..6) == Some(&[0, 2]);
    let (width, height, rgba) = merged(&bytes).map_err(|why| unsupported(path, why))?;
    Ok(LoadedImage {
        width,
        height,
        rgba,
        premultiplied: false,
        label: if large { "PSB (large Photoshop document)".to_string() } else { "PSD".to_string() },
        animation: Vec::new(),
        vector: None,
    })
}

/// Reads big-endian numbers off the front of a document.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|&end| end <= self.bytes.len()).ok_or("the file ends too soon")?;
        let taken = &self.bytes[self.at..end];
        self.at = end;
        Ok(taken)
    }

    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap_or_default()))
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap_or_default()))
    }

    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap_or_default()))
    }

    /// A length: four bytes, or eight in a large document.
    fn length(&mut self, large: bool) -> Result<usize, String> {
        let n = if large { self.u64()? } else { u64::from(self.u32()?) };
        usize::try_from(n).map_err(|_| "a section too large to read".to_string())
    }
}

/// The picture as Photoshop last saved it, all its layers together: every
/// document carries this merged copy at its end, for programs that do not
/// draw layers themselves.
fn merged(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let mut r = Reader { bytes, at: 0 };
    if r.take(4)? != b"8BPS" {
        return Err("not a Photoshop document".into());
    }
    let large = match r.u16()? {
        1 => false,
        2 => true,
        _ => return Err("a Photoshop version not known".into()),
    };
    r.take(6)?;
    let channels = usize::from(r.u16()?);
    let (height, width) = (r.u32()?, r.u32()?);
    let depth = r.u16()?;
    let mode = r.u16()?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 1 << 30 {
        return Err(format!("a picture of {width} × {height}"));
    }
    let palette = {
        let length = r.u32()? as usize;
        r.take(length)?.to_vec()
    };
    let resources = r.u32()? as usize;
    r.take(resources)?;
    // The layers: only whether the merged copy's first extra channel is its
    // transparency, which a negative layer count says.
    let layers = r.length(large)?;
    let transparent = layers > 0 && {
        let mut inner = Reader { bytes: r.take(layers)?, at: 0 };
        let info = inner.length(large).unwrap_or(0);
        info > 0 && inner.u16().is_ok_and(|count| (count as i16) < 0)
    };

    let (w, h) = (width as usize, height as usize);
    let bytes_per = match depth {
        1 => 0,
        8 => 1,
        16 => 2,
        32 => 4,
        _ => return Err(format!("{depth} bits a channel")),
    };
    let row_bytes = if depth == 1 { w.div_ceil(8) } else { w * bytes_per };
    let compression = r.u16()?;
    let mut planes: Vec<Vec<u8>> = Vec::with_capacity(channels);
    match compression {
        0 => {
            for _ in 0..channels {
                planes.push(r.take(row_bytes * h)?.to_vec());
            }
        }
        1 => {
            let mut counts = Vec::with_capacity(channels * h);
            for _ in 0..channels * h {
                counts.push(if large { r.u32()? as usize } else { usize::from(r.u16()?) });
            }
            for c in 0..channels {
                let mut plane = Vec::with_capacity(row_bytes * h);
                for row in 0..h {
                    let packed = r.take(counts[c * h + row])?;
                    unpack(packed, row_bytes, &mut plane);
                }
                planes.push(plane);
            }
        }
        other => return Err(format!("compression {other}, which is not read yet")),
    }

    // Each sample as eight bits.
    let value = |plane: &[u8], i: usize| -> u8 {
        match bytes_per {
            0 => {
                let byte = plane[(i / w) * row_bytes + (i % w) / 8];
                // A bitmap document's 1 is black.
                if byte & (0x80 >> (i % w % 8)) != 0 { 0 } else { 255 }
            }
            1 => plane[i],
            2 => plane[i * 2],
            _ => {
                let v = f32::from_be_bytes([plane[i * 4], plane[i * 4 + 1], plane[i * 4 + 2], plane[i * 4 + 3]]);
                (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round() as u8
            }
        }
    };
    let base = match mode {
        0..=2 | 8 => 1,
        3 => 3,
        4 => 4,
        9 => return Err("a Lab colour document, which is not read yet".into()),
        _ => return Err(format!("colour mode {mode}, which is not read yet")),
    };
    if planes.len() < base {
        return Err("fewer channels than its colour mode needs".into());
    }
    // A fourth (or second) channel is transparency when the layers say so,
    // or when there are no layers to say otherwise.
    let alpha = (planes.len() > base && (transparent || layers == 0)).then_some(base);
    // Photoshop lays a transparent document's merged colours over white, and
    // they are taken back off it below. Not every program does: ImageMagick
    // writes them as they are. Over white, no colour can be darker than the
    // white mixed into it, so one that is shows it never was.
    let transparent = transparent
        && alpha.is_some_and(|a| {
            let floor = |i: usize| 255.0 * (1.0 - f32::from(value(&planes[a], i)) / 255.0) - 2.0;
            (0..w * h).step_by((w * h / 100_000).max(1)).all(|i| {
                let colours = if base >= 3 { 3 } else { 1 };
                (0..colours).all(|c| f32::from(value(&planes[c], i)) >= floor(i))
            })
        });
    let mut rgba = Vec::with_capacity(w * h * 4);
    for i in 0..w * h {
        let [red, green, blue] = match mode {
            3 => [value(&planes[0], i), value(&planes[1], i), value(&planes[2], i)],
            4 => {
                // Photoshop stores CMYK inverted: 255 is no ink.
                let k = f32::from(value(&planes[3], i)) / 255.0;
                [0, 1, 2].map(|c| (f32::from(value(&planes[c], i)) * k).round() as u8)
            }
            2 if palette.len() >= 768 => {
                let index = usize::from(value(&planes[0], i));
                [palette[index], palette[256 + index], palette[512 + index]]
            }
            _ => {
                let grey = value(&planes[0], i);
                [grey, grey, grey]
            }
        };
        let a = alpha.map_or(255, |a| value(&planes[a], i));
        // Laid over white, as above: taken off again.
        let [red, green, blue] = if transparent { [red, green, blue].map(|v| off_white(v, a)) } else { [red, green, blue] };
        rgba.extend_from_slice(&[red, green, blue, a]);
    }
    Ok((width, height, rgba))
}

/// One row of PackBits: a count byte, then either that many bytes as they
/// are or one byte repeated. Always exactly `row` bytes come out, so a
/// damaged row cannot shift the rows after it.
fn unpack(packed: &[u8], row: usize, out: &mut Vec<u8>) {
    let start = out.len();
    let mut i = 0;
    while i < packed.len() && out.len() - start < row {
        let n = packed[i] as i8;
        i += 1;
        if n >= 0 {
            let count = n as usize + 1;
            let end = (i + count).min(packed.len());
            out.extend_from_slice(&packed[i..end]);
            i = end;
        } else if n != -128 {
            let count = 1 - n as isize;
            if let Some(&byte) = packed.get(i) {
                out.extend(std::iter::repeat_n(byte, count as usize));
            }
            i += 1;
        }
    }
    out.resize(start + row, 0);
}

/// A Photoshop document of one layer holding the picture, with the merged
/// picture after it for programs that read only that.
pub fn encode(picture: &DynamicImage) -> Result<Vec<u8>, String> {
    let rgba = picture.to_rgba8();
    let (width, height) = rgba.dimensions();
    if width > 30_000 || height > 30_000 {
        return Err("A Photoshop document can be at most 30,000 pixels on a side.".to_string());
    }
    let area = width as usize * height as usize;
    // The four channels, each a plane of its own: red, green, blue, alpha.
    let planes: Vec<Vec<u8>> = (0..4).map(|c| rgba.pixels().map(|p| p[c]).collect()).collect();

    let mut out = Vec::with_capacity(area * 8 + 1024);
    let u16be = |out: &mut Vec<u8>, v: u16| out.extend_from_slice(&v.to_be_bytes());
    let u32be = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_be_bytes());
    // Header.
    out.extend_from_slice(b"8BPS");
    u16be(&mut out, 1);
    out.extend_from_slice(&[0; 6]);
    u16be(&mut out, 4);
    u32be(&mut out, height);
    u32be(&mut out, width);
    u16be(&mut out, 8);
    u16be(&mut out, 3); // RGB
    // No colour table, no resources.
    u32be(&mut out, 0);
    u32be(&mut out, 0);

    // The one layer.
    let mut layer = Vec::new();
    // Negative: the merged picture's first extra channel is its transparency.
    layer.extend_from_slice(&(-1i16).to_be_bytes());
    for edge in [0, 0, height, width] {
        u32be(&mut layer, edge);
    }
    u16be(&mut layer, 4);
    let channel_length = u32::try_from(2 + area).map_err(|_| "This picture is too large for a Photoshop document.".to_string())?;
    for id in [-1i16, 0, 1, 2] {
        layer.extend_from_slice(&id.to_be_bytes());
        u32be(&mut layer, channel_length);
    }
    layer.extend_from_slice(b"8BIMnorm");
    layer.extend_from_slice(&[255, 0, 0, 0]); // opacity, clipping, flags, filler
    let name = b"Layer 1";
    let mut extra = Vec::new();
    u32be(&mut extra, 0); // no mask
    u32be(&mut extra, 0); // no blending ranges
    extra.push(name.len() as u8);
    extra.extend_from_slice(name);
    while extra.len() % 4 != 0 {
        extra.push(0);
    }
    u32be(&mut layer, extra.len() as u32);
    layer.extend_from_slice(&extra);
    // Its channels, alpha first, as listed, each uncompressed.
    for plane in [&planes[3], &planes[0], &planes[1], &planes[2]] {
        u16be(&mut layer, 0);
        layer.extend_from_slice(plane);
    }
    if layer.len() % 2 != 0 {
        layer.push(0);
    }
    let layer_length = u32::try_from(layer.len()).map_err(|_| "This picture is too large for a Photoshop document.".to_string())?;
    // The layer and mask section: the layers, then no global mask.
    u32be(&mut out, layer_length + 8);
    u32be(&mut out, layer_length);
    out.extend_from_slice(&layer);
    u32be(&mut out, 0);

    // The merged picture, uncompressed, red, green, blue, then alpha. As
    // Photoshop writes it, its colour is laid over white, and readers take
    // the white back out again.
    u16be(&mut out, 0);
    for (c, plane) in planes.iter().enumerate() {
        if c == 3 {
            out.extend_from_slice(plane);
        } else {
            out.extend(plane.iter().zip(&planes[3]).map(|(&v, &a)| over_white(v, a)));
        }
    }
    Ok(out)
}

/// A colour of some opacity, laid over white.
fn over_white(value: u8, alpha: u8) -> u8 {
    let a = f32::from(alpha) / 255.0;
    (f32::from(value) * a + 255.0 * (1.0 - a)).round() as u8
}

/// The colour a merged picture laid over white, taken back off it.
fn off_white(value: u8, alpha: u8) -> u8 {
    if alpha == 0 || alpha == 255 {
        return value;
    }
    let a = f32::from(alpha) / 255.0;
    ((f32::from(value) - 255.0 * (1.0 - a)) / a).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_document_reads_back_as_it_was() {
        let mut rgba = image::RgbaImage::new(7, 5);
        for (x, y, pixel) in rgba.enumerate_pixels_mut() {
            *pixel = image::Rgba([(x * 30) as u8, (y * 50) as u8, 9, if x == 3 { 0 } else { 255 }]);
            if x == 5 {
                // Half see-through: laid over white and taken off again.
                *pixel = image::Rgba([200, 100, 40, 128]);
            }
        }
        let bytes = encode(&DynamicImage::ImageRgba8(rgba.clone())).unwrap();
        assert!(sniff(&bytes));
        let (width, height, back) = merged(&bytes).unwrap();
        assert_eq!((width, height), (7, 5));
        for (got, want) in back.chunks(4).zip(rgba.pixels()) {
            let close = got.iter().zip(want.0).all(|(a, b)| a.abs_diff(b) <= 1) || (want[3] == 0 && got[3] == 0);
            assert!(close, "{got:?} is not {want:?}");
        }
    }

    #[test]
    fn colours_not_laid_over_white_are_left_as_they_are() {
        // As ImageMagick writes it: dark blue at 40%, not laid over white.
        let mut rgba = image::RgbaImage::from_pixel(4, 4, image::Rgba([30, 40, 200, 102]));
        rgba.put_pixel(0, 0, image::Rgba([30, 40, 200, 255]));
        let mut bytes = encode(&DynamicImage::ImageRgba8(rgba.clone())).unwrap();
        // Overwrite the merged colours with the unblended ones.
        let start = bytes.len() - 16 * 4;
        for (c, plane) in (0..3).map(|c| (c, rgba.pixels().map(move |p| p[c]).collect::<Vec<u8>>())) {
            bytes[start + c * 16..start + (c + 1) * 16].copy_from_slice(&plane);
        }
        let (_, _, back) = merged(&bytes).unwrap();
        assert_eq!(&back[4..8], &[30, 40, 200, 102]);
    }

    #[test]
    fn packed_rows_come_out_their_own_length() {
        let mut out = Vec::new();
        // Three as they are, then 'z' four times.
        unpack(&[2, b'a', b'b', b'c', 0xfd, b'z'], 7, &mut out);
        assert_eq!(out, b"abczzzz");
        // Damaged: cut short, and padded rather than running into the next.
        let mut out = Vec::new();
        unpack(&[5, b'a'], 4, &mut out);
        assert_eq!(out.len(), 4);
    }
}
