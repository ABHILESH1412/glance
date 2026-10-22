// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pictures put on a page: a stamp annotation whose appearance is the
//! picture, which is how a PDF carries an image someone added, so every
//! reader shows it, phones included.
//!
//! Poppler draws the stamp from the pixels it is given, but only while the
//! document stays open: moved after being read back from the file, a stamp
//! loses its picture and becomes Poppler's "Draft". So a picture is always
//! moved by taking it off and putting it down again with its pixels, and the
//! pixels of one already in the file are read back from its appearance.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use gtk::prelude::*;
use gtk::{cairo, gdk, glib};

use super::qpdf::{Object, Qpdf};

/// The longest side a picture is kept at. A photograph straight off a phone
/// would otherwise add tens of megabytes to the document; this is still
/// sharp printed at seven inches across.
pub const LONGEST: u32 = 2048;

/// A picture's pixels, in RGBA with straight alpha.
#[derive(Clone, PartialEq, Eq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Pixels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pixels({}×{})", self.width, self.height)
    }
}

impl Pixels {
    /// Width over height.
    pub fn aspect(&self) -> f64 {
        f64::from(self.width.max(1)) / f64::from(self.height.max(1))
    }

    /// The pixels laid out for Poppler, in a Cairo surface: each pixel's
    /// four bytes in the machine's own order, its colour and alpha where
    /// `poppler` says Poppler reads them. The surface is only ever read by
    /// Poppler, never drawn, so it need not be what Cairo would make.
    fn surface(&self, poppler: Reading) -> Option<cairo::ImageSurface> {
        let width = i32::try_from(self.width).ok()?;
        let height = i32::try_from(self.height).ok()?;
        // Four bytes a pixel is already a whole number of words, so Cairo
        // pads no row, and the pixels run on from one row into the next.
        let stride = cairo::Format::ARgb32.stride_for_width(self.width).ok()?;
        if stride as usize != self.width as usize * 4 {
            return None;
        }
        // A pixel to spare at the end, for a Poppler that reads each alpha
        // from the pixel after, and so one past the last.
        let mut data = vec![0u8; self.rgba.len() + 4];
        let (b, g, r, a) = if cfg!(target_endian = "little") { (0, 1, 2, 3) } else { (3, 2, 1, 0) };
        for (i, pixel) in self.rgba.as_chunks::<4>().0.iter().enumerate() {
            let alpha = u32::from(pixel[3]);
            let colour = |c: u8| if poppler.straight { c } else { ((u32::from(c) * alpha + 127) / 255) as u8 };
            let at = i * 4;
            (data[at + r], data[at + g], data[at + b]) = (colour(pixel[0]), colour(pixel[1]), colour(pixel[2]));
            let alpha_at = if poppler.next_alpha { at + 4 } else { at };
            data[alpha_at + a] = pixel[3];
        }
        cairo::ImageSurface::create_for_data(data, cairo::Format::ARgb32, width, height, stride).ok()
    }

    /// The pixels as a texture, for showing the picture while it is dragged.
    pub fn texture(&self) -> gdk::Texture {
        gdk::MemoryTexture::new(
            self.width as i32,
            self.height as i32,
            gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from(&self.rgba),
            self.width as usize * 4,
        )
        .upcast()
    }
}

/// A picture file, any kind Glance opens, made small enough to keep in a
/// document.
pub fn open(path: &Path) -> Result<Pixels, String> {
    let decoded = crate::images::loader::decode(path)?;
    let mut rgba = decoded.rgba;
    if decoded.premultiplied {
        for pixel in rgba.as_chunks_mut::<4>().0 {
            let a = u32::from(pixel[3]);
            if a > 0 && a < 255 {
                for c in &mut pixel[..3] {
                    *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
    }
    let pixels = Pixels { width: decoded.width, height: decoded.height, rgba };
    Ok(shrunk(pixels, LONGEST))
}

/// No more than `longest` pixels on either side, in proportion.
fn shrunk(pixels: Pixels, longest: u32) -> Pixels {
    let side = pixels.width.max(pixels.height);
    if side <= longest {
        return pixels;
    }
    let scale = f64::from(longest) / f64::from(side);
    let width = ((f64::from(pixels.width) * scale).round() as u32).max(1);
    let height = ((f64::from(pixels.height) * scale).round() as u32).max(1);
    let Some(image) = image::RgbaImage::from_raw(pixels.width, pixels.height, pixels.rgba.clone()) else { return pixels };
    let smaller = image::imageops::resize(&image, width, height, image::imageops::FilterType::CatmullRom);
    Pixels { width, height, rgba: smaller.into_raw() }
}

/// How Poppler reads the pixels it is given for a stamp.
///
/// Every Poppler from 22.07, when stamps could have a picture, to 26.09 at
/// least steps on to the next pixel before reading the alpha, so each pixel
/// takes the next one's, and the last reads past the end; and it takes the
/// colour as it is, where Cairo's is premultiplied. Handed pixels that allow
/// for both, it writes the picture as it should be. Should a later Poppler
/// read them as Cairo means them, that is found out once, by trying.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Reading {
    /// Each pixel's alpha read from the pixel after it.
    next_alpha: bool,
    /// The colour read as it is, rather than premultiplied by the alpha.
    straight: bool,
}

/// How every Poppler so far reads them.
const KNOWN: Reading = Reading { next_alpha: true, straight: true };

fn reading() -> Reading {
    static READING: OnceLock<Reading> = OnceLock::new();
    *READING.get_or_init(|| probe().unwrap_or(KNOWN))
}

/// Give a stamp in a page of its own three pixels, each colour and alpha
/// different, save it, and see what Poppler wrote.
fn probe() -> Option<Reading> {
    use glib::translate::{from_glib_full, ToGlibPtr};
    let calls = super::newer::get().stamp.as_ref()?;
    // A folder for each try, as two may run at once.
    static TRIES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let attempt = TRIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("glance-stamp-check-{}-{attempt}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let found = (|| {
        let (blank, out) = (dir.join("blank.pdf"), dir.join("stamp.pdf"));
        std::fs::write(&blank, one_page()).ok()?;
        let document = poppler::Document::from_file(&super::document::uri(&blank), None).ok()?;
        let page = document.page(0)?;
        // Red 200 throughout; the alphas 50, 150 and 250, with 99 spare.
        let (r, a) = if cfg!(target_endian = "little") { (2, 3) } else { (1, 0) };
        let mut data = vec![0u8; 16];
        for (i, alpha) in [50u8, 150, 250, 99].into_iter().enumerate() {
            data[i * 4 + a] = alpha;
            if i < 3 {
                data[i * 4 + r] = 200;
            }
        }
        let surface = cairo::ImageSurface::create_for_data(data, cairo::Format::ARgb32, 3, 1, 12).ok()?;
        let mut rect = poppler::ffi::PopplerRectangle { x1: 10.0, y1: 10.0, x2: 40.0, y2: 20.0 };
        // SAFETY: Poppler's own calls, found by name with its headers'
        // signatures; an error it sets is ours to free.
        unsafe {
            let raw = (calls.new)(document.to_glib_none().0, &mut rect);
            let annot: poppler::Annot = from_glib_full(raw);
            super::annots::attach(&page, &annot, None, true);
            let mut error = std::ptr::null_mut();
            let ok = (calls.set_image)(raw, surface.to_raw_none(), &mut error);
            if !error.is_null() {
                glib::ffi::g_error_free(error);
            }
            if ok == 0 {
                return None;
            }
        }
        document.save(&super::document::uri(&out)).ok()?;
        let pdf = Qpdf::read(&out, None).ok()?;
        let page = *pdf.pages().first()?;
        let stamp = *pdf.get(page, "Annots").and_then(|a| pdf.items(a))?.first()?;
        let appearance = pdf.get(stamp, "AP").and_then(|ap| pdf.get(ap, "N"))?;
        let image = single_image(&pdf, appearance)?;
        let (_, _, colour) = samples(&pdf, image)?;
        let (_, _, alpha) = pdf.get(image, "SMask").and_then(|mask| samples(&pdf, mask))?;
        let next_alpha = match alpha[..] {
            [150, 250, 99] => true,
            [50, 150, 250] => false,
            _ => return None,
        };
        Some(Reading { next_alpha, straight: colour.first() == Some(&200) })
    })();
    let _ = std::fs::remove_dir_all(&dir);
    found
}

/// The smallest PDF there is: one empty page.
fn one_page() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>",
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    pdf
}

/// Put the picture on a stamp, which must already be on its page: Poppler
/// keeps the pixels only for a stamp that belongs to a document.
pub(super) fn paint(stamp: *mut poppler::ffi::PopplerAnnot, pixels: &Pixels) -> bool {
    let Some(calls) = super::newer::get().stamp.as_ref() else { return false };
    let Some(surface) = pixels.surface(reading()) else { return false };
    let mut error = std::ptr::null_mut();
    // SAFETY: Poppler's own call, found by name with its header's signature;
    // it copies the pixels, and an error it sets is ours to free.
    unsafe {
        let ok = (calls.set_image)(stamp, surface.to_raw_none(), &mut error);
        if !error.is_null() {
            glib::ffi::g_error_free(error);
        }
        ok != 0
    }
}

/// The pictures on a page of the file at `path`, read back from the stamps'
/// appearances: each stamp's box, in the PDF's own coordinates, and its
/// pixels. Stamps that are not a single picture, such as another program's
/// "Approved", are left out, so they are left alone.
pub fn read_back(path: &Path, password: Option<&str>, page: usize) -> Vec<([f64; 4], Arc<Pixels>)> {
    let Ok(pdf) = Qpdf::read(path, password) else { return Vec::new() };
    let Some(&page) = pdf.pages().get(page) else { return Vec::new() };
    let Some(annots) = pdf.get(page, "Annots").and_then(|a| pdf.items(a)) else { return Vec::new() };
    annots
        .into_iter()
        .filter(|&annot| pdf.get(annot, "Subtype").and_then(|s| pdf.name(s)).as_deref() == Some("Stamp"))
        .filter_map(|annot| {
            let rect = pdf.get(annot, "Rect").and_then(|r| pdf.items(r))?;
            let rect: Vec<f64> = rect.into_iter().filter_map(|n| pdf.number(n)).collect();
            let [x1, y1, x2, y2] = rect[..] else { return None };
            let appearance = pdf.get(annot, "AP").and_then(|ap| pdf.get(ap, "N"))?;
            let pixels = pixels_of(&pdf, appearance)?;
            Some(([x1.min(x2), y1.min(y2), x1.max(x2), y1.max(y2)], Arc::new(pixels)))
        })
        .collect()
}

/// The one picture an appearance draws, if that is all it draws.
fn single_image(pdf: &Qpdf, appearance: Object) -> Option<Object> {
    let objects = pdf.get(appearance, "Resources").and_then(|r| pdf.get(r, "XObject"))?;
    let images: Vec<Object> = pdf
        .keys(objects)
        .iter()
        .filter_map(|key| pdf.get(objects, key))
        .filter(|&object| pdf.get(object, "Subtype").and_then(|s| pdf.name(s)).as_deref() == Some("Image"))
        .collect();
    let [image] = images[..] else { return None };
    Some(image)
}

/// The pixels of the one picture an appearance draws.
fn pixels_of(pdf: &Qpdf, appearance: Object) -> Option<Pixels> {
    let image = single_image(pdf, appearance)?;
    let (width, height, colour) = samples(pdf, image)?;
    let alpha = pdf
        .get(image, "SMask")
        .and_then(|mask| samples(pdf, mask))
        .filter(|&(w, h, ref grey)| (w, h) == (width, height) && grey.len() == (width * height) as usize);
    let rgba: Vec<u8> = match (colour.len() / (width * height) as usize, alpha) {
        (3, alpha) => colour
            .as_chunks::<3>()
            .0
            .iter()
            .enumerate()
            .flat_map(|(i, c)| [c[0], c[1], c[2], alpha.as_ref().map_or(255, |(_, _, a)| a[i])])
            .collect(),
        (1, alpha) => colour
            .iter()
            .enumerate()
            .flat_map(|(i, &v)| [v, v, v, alpha.as_ref().map_or(255, |(_, _, a)| a[i])])
            .collect(),
        _ => return None,
    };
    Some(Pixels { width, height, rgba })
}

/// An image's size and its samples, eight bits each: one per pixel for grey,
/// three for RGB. Only the kinds a picture put on a page comes as.
fn samples(pdf: &Qpdf, image: Object) -> Option<(u32, u32, Vec<u8>)> {
    let width = u32::try_from(pdf.get(image, "Width").and_then(|w| pdf.integer(w))?).ok()?;
    let height = u32::try_from(pdf.get(image, "Height").and_then(|h| pdf.integer(h))?).ok()?;
    let filter = pdf.get(image, "Filter").and_then(|f| pdf.name(f));
    if filter.as_deref() == Some("DCTDecode") {
        // A JPEG, as another program may have put it there.
        let jpeg = pdf.stream_data(image, false)?;
        let decoded = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg).ok()?.into_rgb8();
        return ((decoded.width(), decoded.height()) == (width, height)).then(|| (width, height, decoded.into_raw()));
    }
    if pdf.get(image, "BitsPerComponent").and_then(|b| pdf.integer(b)) != Some(8) {
        return None;
    }
    let channels = match pdf.get(image, "ColorSpace").and_then(|c| pdf.name(c)).as_deref() {
        Some("DeviceRGB") => 3,
        Some("DeviceGray") | None => 1,
        _ => return None,
    };
    let data = pdf.stream_data(image, true)?;
    (data.len() == (width * height) as usize * channels).then_some((width, height, data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_big_photograph_is_shrunk_in_proportion_and_a_small_one_kept() {
        let big = Pixels { width: 4000, height: 3000, rgba: vec![128; 4000 * 3000 * 4] };
        let kept = shrunk(big, LONGEST);
        assert_eq!((kept.width, kept.height), (2048, 1536));
        assert_eq!(kept.rgba.len(), 2048 * 1536 * 4);
        let small = Pixels { width: 30, height: 20, rgba: vec![9; 30 * 20 * 4] };
        assert_eq!(shrunk(small.clone(), LONGEST), small);
    }

    #[test]
    fn the_pixels_are_laid_out_as_poppler_reads_them() {
        let pixels = Pixels { width: 2, height: 1, rgba: vec![255, 0, 0, 255, 0, 0, 255, 128] };
        let word = |surface: &mut cairo::ImageSurface, i: usize| {
            let data = surface.data().unwrap();
            u32::from_ne_bytes(data[i * 4..i * 4 + 4].try_into().unwrap())
        };
        // As Cairo means them: premultiplied, each alpha with its colour.
        let mut cairo = pixels.surface(Reading { next_alpha: false, straight: false }).unwrap();
        assert_eq!(word(&mut cairo, 0), 0xffff_0000, "opaque red");
        assert_eq!(word(&mut cairo, 1), 0x8000_0080, "half see-through blue, premultiplied");
        // As Poppler reads them: colour as it is, alpha a pixel on, the
        // last one's in the pixel spare at the end.
        let mut poppler = pixels.surface(KNOWN).unwrap();
        assert_eq!(word(&mut poppler, 0), 0x00ff_0000, "red, its alpha in the next pixel");
        assert_eq!(word(&mut poppler, 1), 0xff00_00ff, "blue at full strength, with red's alpha");
    }

    #[test]
    fn this_poppler_is_asked_how_it_reads_pixels() {
        if crate::pdf::newer::get().stamp.is_none() {
            return;
        }
        assert_eq!(probe(), Some(KNOWN), "a Poppler that reads stamp pixels differently: the picture test shows whether it is allowed for");
    }
}
