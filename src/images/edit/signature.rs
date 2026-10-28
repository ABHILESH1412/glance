// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Signatures: written once on the signature pad, kept, and put on any
//! picture or PDF page after that.
//!
//! A signature is kept as the pen's strokes, not as pixels, so it is sharp at
//! any size and goes into a PDF as ink like any other drawing. Each is a small
//! text file in Glance's data folder, one stroke to a line:
//!
//! ```text
//! glance-signature 1
//! ink 0.000 0.000 0.000 1.000
//! pen 3.00
//! 0.00,12.50 1.25,11.75 …
//! ```
//!
//! Points are from the top-left of the signature's own box, in the pad's
//! pixels; only their proportions matter once it is placed.
//!
//! A signature can also be a picture of one: a scan or a photograph of it on
//! paper, or a PNG made elsewhere. Its paper is made see-through when it is
//! brought in, so only the ink is left, as the pen left it, and it is kept as
//! a PNG beside the others. On a PDF it goes down as a picture; on a picture,
//! as a layer that is part of it once saved.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use gtk::{gdk, glib};
use image::RgbaImage;

const HEADER: &str = "glance-signature 1";
/// Points closer than this to the one before add nothing to a stroke.
const STEP: f64 = 0.75;
/// The longest side a picture of a signature is kept at: sharp printed two
/// inches across at several hundred to the inch, and small in a PDF.
const PICTURE_SIDE: u32 = 1600;

/// A signature that is a picture, its paper made see-through.
pub struct Picture {
    pub pixels: RgbaImage,
    /// The pixels as a texture, made once, for showing it.
    texture: OnceLock<gdk::Texture>,
}

impl Picture {
    pub fn new(pixels: RgbaImage) -> Self {
        Picture { pixels, texture: OnceLock::new() }
    }

    pub fn texture(&self) -> gdk::Texture {
        self.texture
            .get_or_init(|| {
                let (width, height) = self.pixels.dimensions();
                crate::images::canvas::texture_from(width, height, false, self.pixels.as_raw().clone())
            })
            .clone()
    }
}

impl PartialEq for Picture {
    fn eq(&self, other: &Self) -> bool {
        self.pixels == other.pixels
    }
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({}×{})", self.pixels.width(), self.pixels.height())
    }
}

/// What colour a picture of a signature is given when brought in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ink {
    /// The pen's own, as the picture shows it.
    AsItIs,
    /// One colour throughout, red, green and blue from 0 to 255.
    Colour([u8; 3]),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Signature {
    /// The file's name, without its folder; empty until saved.
    pub id: String,
    pub strokes: Vec<Vec<(f64, f64)>>,
    /// The box the strokes span, from the origin.
    pub width: f64,
    pub height: f64,
    /// How thick the pen was, in the strokes' units.
    pub pen: f64,
    pub colour: gdk::RGBA,
    /// For a signature that is a picture, the picture; its strokes are then
    /// none, and its width and height are the picture's.
    pub picture: Option<Arc<Picture>>,
}

impl Signature {
    /// A signature from strokes as written, moved to the origin. None if there
    /// is too little of it to be a signature: a dot or a slip of the pen.
    pub fn from_strokes(strokes: &[Vec<(f64, f64)>], pen: f64, colour: gdk::RGBA) -> Option<Signature> {
        let strokes: Vec<Vec<(f64, f64)>> = strokes
            .iter()
            .map(|stroke| {
                let mut kept: Vec<(f64, f64)> = Vec::with_capacity(stroke.len());
                for &p in stroke {
                    if kept.last().is_none_or(|q| (p.0 - q.0).hypot(p.1 - q.1) >= STEP) {
                        kept.push(p);
                    }
                }
                kept
            })
            .filter(|stroke| !stroke.is_empty())
            .collect();
        let points = strokes.iter().flatten();
        let (mut x1, mut y1, mut x2, mut y2) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for &(x, y) in points {
            (x1, y1, x2, y2) = (x1.min(x), y1.min(y), x2.max(x), y2.max(y));
        }
        let (width, height) = (x2 - x1, y2 - y1);
        if strokes.is_empty() || width.max(height) < pen * 4.0 {
            return None;
        }
        let strokes = strokes.into_iter().map(|s| s.into_iter().map(|(x, y)| (x - x1, y - y1)).collect()).collect();
        Some(Signature { id: String::new(), strokes, width, height, pen, colour, picture: None })
    }

    /// A signature from a picture of one: its paper made see-through, the
    /// ink kept as it was or given one colour, trimmed to the ink and made
    /// no bigger than it needs to be. None if no ink is found.
    pub fn from_picture(rgba: &RgbaImage, ink: Ink) -> Option<Signature> {
        let mut ink_only = without_paper(rgba);
        if let Ink::Colour(colour) = ink {
            for pixel in ink_only.pixels_mut() {
                pixel.0[..3].copy_from_slice(&colour);
            }
        }
        let trimmed = trimmed(&ink_only)?;
        let side = trimmed.width().max(trimmed.height());
        let pixels = if side > PICTURE_SIDE {
            let scale = f64::from(PICTURE_SIDE) / f64::from(side);
            let (w, h) = (trimmed.width(), trimmed.height());
            let to = |n: u32| ((f64::from(n) * scale).round() as u32).max(1);
            image::imageops::resize(&trimmed, to(w), to(h), image::imageops::FilterType::CatmullRom)
        } else {
            trimmed
        };
        Some(Signature::of_picture(String::new(), pixels))
    }

    fn of_picture(id: String, pixels: RgbaImage) -> Signature {
        let (width, height) = (f64::from(pixels.width()), f64::from(pixels.height()));
        Signature {
            id,
            strokes: Vec::new(),
            width,
            height,
            pen: 0.0,
            colour: gdk::RGBA::BLACK,
            picture: Some(Arc::new(Picture::new(pixels))),
        }
    }

    /// Width over height, never zero or endless however thin it is.
    pub fn aspect(&self) -> f64 {
        (self.width.max(1.0) / self.height.max(1.0)).clamp(0.05, 50.0)
    }

    /// The strokes stretched to fill the box x, y, w, h.
    pub fn fitted(&self, [x, y, w, h]: [f64; 4]) -> Vec<Vec<(f64, f64)>> {
        // A perfectly straight signature has no height to scale: it sits
        // along the middle instead.
        let along = |p: f64, size: f64, start: f64, span: f64| {
            if size > 0.0 { start + p / size * span } else { start + span / 2.0 }
        };
        self.strokes
            .iter()
            .map(|stroke| stroke.iter().map(|&(px, py)| (along(px, self.width, x, w), along(py, self.height, y, h))).collect())
            .collect()
    }

    /// The pen's thickness once the signature is `w` wide.
    pub fn pen_at(&self, w: f64) -> f64 {
        self.pen * w / self.width.max(1.0)
    }

    fn text(&self) -> String {
        let mut text = format!(
            "{HEADER}\nink {:.3} {:.3} {:.3} {:.3}\npen {:.2}\n",
            self.colour.red(),
            self.colour.green(),
            self.colour.blue(),
            self.colour.alpha(),
            self.pen
        );
        for stroke in &self.strokes {
            let points: Vec<String> = stroke.iter().map(|(x, y)| format!("{x:.2},{y:.2}")).collect();
            text.push_str(&points.join(" "));
            text.push('\n');
        }
        text
    }

    fn parse(id: &str, text: &str) -> Option<Signature> {
        let mut lines = text.lines();
        if lines.next()?.trim() != HEADER {
            return None;
        }
        let (mut colour, mut pen) = (gdk::RGBA::BLACK, 3.0);
        let mut strokes = Vec::new();
        for line in lines {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("ink ") {
                let parts: Vec<f32> = rest.split_whitespace().filter_map(|n| n.parse().ok()).collect();
                if let [r, g, b, a] = parts[..] {
                    colour = gdk::RGBA::new(r, g, b, a);
                }
            } else if let Some(rest) = line.strip_prefix("pen ") {
                pen = rest.trim().parse().ok().filter(|p: &f64| *p > 0.0).unwrap_or(pen);
            } else if !line.is_empty() {
                let stroke: Vec<(f64, f64)> = line
                    .split_whitespace()
                    .filter_map(|p| p.split_once(','))
                    .filter_map(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?)))
                    .collect();
                if !stroke.is_empty() {
                    strokes.push(stroke);
                }
            }
        }
        let mut signature = Signature::from_strokes(&strokes, pen, colour)?;
        signature.id = id.to_string();
        Some(signature)
    }
}

/// Where signatures are kept.
pub fn folder() -> PathBuf {
    glib::user_data_dir().join("glance").join("signatures")
}

/// Every signature kept, oldest first.
pub fn all() -> Vec<Signature> {
    let Ok(entries) = std::fs::read_dir(folder()) else { return Vec::new() };
    let mut found: Vec<Signature> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(PICTURE) {
                let pixels = image::open(entry.path()).ok()?.to_rgba8();
                return Some(Signature::of_picture(name, pixels));
            }
            if !name.ends_with(STROKES) {
                return None;
            }
            Signature::parse(&name, &std::fs::read_to_string(entry.path()).ok()?)
        })
        .collect();
    found.sort_by(|a, b| a.id.cmp(&b.id));
    found
}

/// What a kept signature's file name ends with: its strokes, or its picture.
const STROKES: &str = ".signature";
const PICTURE: &str = ".png";

/// Keep a signature, and give it back with the name it is kept under.
pub fn save(signature: &Signature) -> Result<Signature, String> {
    let folder = folder();
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    // Named by when it was made, so they list in that order.
    let end = if signature.picture.is_some() { PICTURE } else { STROKES };
    let now = glib::real_time();
    let mut id = format!("{now:020}{end}");
    let mut n = 1;
    while folder.join(&id).exists() {
        id = format!("{now:020}-{n}{end}");
        n += 1;
    }
    let kept = Signature { id, ..signature.clone() };
    write(&kept)?;
    Ok(kept)
}

/// Keep a removed signature again, under the name it had, so it goes back
/// where it was in the list.
pub fn restore(signature: &Signature) -> Result<(), String> {
    if !is_ours(&signature.id) {
        return Err("not a signature".into());
    }
    std::fs::create_dir_all(folder()).map_err(|e| e.to_string())?;
    write(signature)
}

fn write(signature: &Signature) -> Result<(), String> {
    let path = folder().join(&signature.id);
    match &signature.picture {
        Some(picture) => picture.pixels.save_with_format(&path, image::ImageFormat::Png).map_err(|e| e.to_string()),
        None => std::fs::write(&path, signature.text()).map_err(|e| e.to_string()),
    }
}

/// A name of one of our files, in our folder and nowhere else.
fn is_ours(id: &str) -> bool {
    !id.contains(['/', '\\']) && (id.ends_with(STROKES) || id.ends_with(PICTURE))
}

/// How light a pixel is, 0 to 1.
fn lightness(p: &image::Rgba<u8>) -> f32 {
    (0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2])) / 255.0
}

/// A picture of a signature with its paper see-through: a picture that is
/// see-through already is left so. Otherwise the paper is the picture's
/// lighter tones and the ink its darkest, and each pixel is as opaque as it
/// is dark between the two, with a little either side let go so the grain
/// of the paper and the middle of a stroke both come out clean. The paper's
/// tint is taken back out of the ink where they blend, at a stroke's edge.
fn without_paper(rgba: &RgbaImage) -> RgbaImage {
    let pixels = rgba.pixels().len().max(1);
    let clear = rgba.pixels().filter(|p| p[3] < 128).count();
    if clear * 20 > pixels {
        return rgba.clone();
    }
    // From a sample: a few hundred thousand pixels tell the paper from the
    // ink as well as all of them.
    let step = (pixels / 250_000).max(1);
    let mut tones: Vec<f32> = rgba.pixels().step_by(step).map(lightness).collect();
    tones.sort_by(f32::total_cmp);
    let at = |share: f32| tones[((tones.len() - 1) as f32 * share) as usize];
    let (ink, paper) = (at(0.01), at(0.7));
    if paper - ink < 0.15 {
        // No ink darker than its paper: nothing to keep.
        return RgbaImage::new(rgba.width(), rgba.height());
    }
    let mut paper_colour = [0f32; 3];
    let mut counted = 0f32;
    for p in rgba.pixels().step_by(step).filter(|p| lightness(p) >= paper) {
        for c in 0..3 {
            paper_colour[c] += f32::from(p[c]);
        }
        counted += 1.0;
    }
    for c in &mut paper_colour {
        *c /= counted.max(1.0);
    }
    let mut out = rgba.clone();
    for pixel in out.pixels_mut() {
        let dark = (paper - lightness(pixel)) / (paper - ink);
        let alpha = ((dark - 0.12) / 0.6).clamp(0.0, 1.0);
        if alpha <= 0.0 {
            *pixel = image::Rgba([0, 0, 0, 0]);
            continue;
        }
        for c in 0..3 {
            let unblended = (f32::from(pixel[c]) - paper_colour[c] * (1.0 - alpha)) / alpha;
            pixel[c] = unblended.clamp(0.0, 255.0).round() as u8;
        }
        pixel[3] = (alpha * f32::from(pixel[3])).round() as u8;
    }
    out
}

/// The picture trimmed to its ink, with a pixel or two to spare: None when
/// there is none.
fn trimmed(rgba: &RgbaImage) -> Option<RgbaImage> {
    let (mut x1, mut y1, mut x2, mut y2) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, pixel) in rgba.enumerate_pixels() {
        if pixel[3] >= 16 {
            (x1, y1, x2, y2) = (x1.min(x), y1.min(y), x2.max(x), y2.max(y));
        }
    }
    if x1 > x2 || y1 > y2 {
        return None;
    }
    let spare = 2;
    let (x1, y1) = (x1.saturating_sub(spare), y1.saturating_sub(spare));
    let (x2, y2) = ((x2 + spare).min(rgba.width() - 1), (y2 + spare).min(rgba.height() - 1));
    Some(image::imageops::crop_imm(rgba, x1, y1, x2 - x1 + 1, y2 - y1 + 1).to_image())
}

pub fn remove(id: &str) -> Result<(), String> {
    if !is_ours(id) {
        return Err("not a signature".into());
    }
    std::fs::remove_file(folder().join(id)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written() -> Signature {
        let strokes = vec![
            vec![(110.0, 60.0), (110.2, 60.1), (130.0, 40.0), (150.0, 70.0)],
            vec![(160.0, 50.0), (200.0, 55.0)],
        ];
        Signature::from_strokes(&strokes, 3.0, gdk::RGBA::new(0.1, 0.2, 0.6, 1.0)).unwrap()
    }

    #[test]
    fn a_signature_is_moved_to_the_origin_and_thinned_of_repeats() {
        let signature = written();
        assert_eq!((signature.width, signature.height), (90.0, 30.0));
        assert_eq!(signature.strokes[0], vec![(0.0, 20.0), (20.0, 0.0), (40.0, 30.0)], "the point barely moved went");
        assert!((signature.aspect() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn a_dot_or_a_slip_is_not_a_signature() {
        assert!(Signature::from_strokes(&[], 3.0, gdk::RGBA::BLACK).is_none());
        assert!(Signature::from_strokes(&[vec![(5.0, 5.0), (8.0, 6.0)]], 3.0, gdk::RGBA::BLACK).is_none());
    }

    #[test]
    fn a_signature_fills_the_box_it_is_put_in() {
        let fitted = written().fitted([100.0, 200.0, 180.0, 60.0]);
        assert_eq!(fitted[0][0], (100.0, 240.0));
        assert_eq!(fitted[1][1], (280.0, 230.0));
        assert!((written().pen_at(180.0) - 6.0).abs() < 1e-9, "twice as wide, twice as thick");
    }

    #[test]
    fn a_signature_comes_back_as_it_was_kept() {
        let signature = Signature { id: "x.signature".into(), ..written() };
        let back = Signature::parse("x.signature", &signature.text()).unwrap();
        assert_eq!(back.strokes, signature.strokes);
        assert_eq!((back.pen, back.width, back.height), (3.0, 90.0, 30.0));
        assert!((back.colour.blue() - 0.6).abs() < 1e-3);
        assert!(Signature::parse("y", "something else\n1,2 3,4").is_none());
    }

    #[test]
    fn only_a_kept_signature_can_be_removed() {
        assert!(remove("../reader").is_err());
        assert!(remove("theme").is_err());
        assert!(is_ours("1.png") && is_ours("1.signature") && !is_ours("../1.png"));
    }

    /// Blue ink on off-white paper, as a scan has it: a solid stroke from
    /// x = 20 to 79 on rows 10 and 11, its edges half ink and half paper.
    fn scanned() -> RgbaImage {
        let paper = image::Rgba([240, 236, 226, 255]);
        let ink = image::Rgba([20, 40, 140, 255]);
        let mut scan = RgbaImage::from_pixel(100, 30, paper);
        for x in 20..80 {
            for y in 10..12 {
                scan.put_pixel(x, y, ink);
            }
            let mixed = image::Rgba([130, 138, 183, 255]);
            scan.put_pixel(x, 9, mixed);
            scan.put_pixel(x, 12, mixed);
        }
        scan
    }

    #[test]
    fn a_scanned_signature_keeps_its_ink_and_loses_its_paper() {
        let signature = Signature::from_picture(&scanned(), Ink::AsItIs).unwrap();
        let pixels = &signature.picture.as_ref().unwrap().pixels;
        // Trimmed to the ink, rows 9 to 12 and columns 20 to 79, two spare.
        assert_eq!(pixels.dimensions(), (64, 8));
        assert_eq!((signature.width, signature.height), (64.0, 8.0));
        assert!(signature.strokes.is_empty());
        let at = |x, y| *pixels.get_pixel(x, y);
        assert_eq!(at(0, 0)[3], 0, "paper is see-through");
        let solid = at(30, 3);
        assert_eq!(solid[3], 255, "ink is solid");
        assert_eq!(&solid.0[..3], &[20, 40, 140], "and its own colour");
        let edge = at(30, 2);
        assert!(edge[3] > 60 && edge[3] < 220, "an edge is part see-through: {edge:?}");
        // Laid back over the same paper, it is the scan again; and its own
        // colour is the ink's more than the scan's mixture was.
        let a = f32::from(edge[3]) / 255.0;
        let over = |c: usize, paper: f32| (f32::from(edge[c]) * a + paper * (1.0 - a)).round();
        for (c, (paper, scanned)) in [(240.0, 130.0), (236.0, 138.0), (226.0, 183.0)].into_iter().enumerate() {
            assert!((over(c, paper) - scanned).abs() <= 2.0, "channel {c}: {} against {scanned}", over(c, paper));
        }
        assert!(edge[0] < 130 && edge[2] < 183 && edge[2] > edge[0], "with the paper's tint taken out: {edge:?}");
    }

    #[test]
    fn a_signature_can_be_given_one_ink() {
        let signature = Signature::from_picture(&scanned(), Ink::Colour([0, 0, 0])).unwrap();
        let pixels = &signature.picture.as_ref().unwrap().pixels;
        assert!(pixels.pixels().filter(|p| p[3] > 0).all(|p| p.0[..3] == [0, 0, 0]));
    }

    #[test]
    fn a_see_through_png_is_kept_as_it_is_and_blank_paper_is_no_signature() {
        let mut png = RgbaImage::new(50, 20);
        for x in 10..40 {
            png.put_pixel(x, 10, image::Rgba([200, 0, 0, 255]));
        }
        let signature = Signature::from_picture(&png, Ink::AsItIs).unwrap();
        let pixels = &signature.picture.as_ref().unwrap().pixels;
        assert_eq!(pixels.dimensions(), (34, 5));
        assert_eq!(*pixels.get_pixel(5, 2), image::Rgba([200, 0, 0, 255]));
        let blank = RgbaImage::from_pixel(80, 40, image::Rgba([250, 250, 248, 255]));
        assert!(Signature::from_picture(&blank, Ink::AsItIs).is_none());
    }

    #[test]
    fn a_big_scan_is_kept_at_a_sensible_size() {
        let mut scan = RgbaImage::from_pixel(4000, 1000, image::Rgba([255, 255, 255, 255]));
        for x in 0..4000 {
            for y in 400..600 {
                scan.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        let signature = Signature::from_picture(&scan, Ink::AsItIs).unwrap();
        let (w, h) = signature.picture.as_ref().unwrap().pixels.dimensions();
        assert_eq!(w, PICTURE_SIDE);
        assert!((h as i32 - 82).abs() <= 1, "in proportion: {h}");
    }
}
