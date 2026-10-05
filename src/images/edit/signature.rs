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

use std::path::PathBuf;

use gtk::{gdk, glib};

const HEADER: &str = "glance-signature 1";
/// Points closer than this to the one before add nothing to a stroke.
const STEP: f64 = 0.75;

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
        Some(Signature { id: String::new(), strokes, width, height, pen, colour })
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
            if !name.ends_with(".signature") {
                return None;
            }
            Signature::parse(&name, &std::fs::read_to_string(entry.path()).ok()?)
        })
        .collect();
    found.sort_by(|a, b| a.id.cmp(&b.id));
    found
}

/// Keep a signature, and give it back with the name it is kept under.
pub fn save(signature: &Signature) -> Result<Signature, String> {
    let folder = folder();
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    // Named by when it was made, so they list in that order.
    let now = glib::real_time();
    let mut id = format!("{now:020}.signature");
    let mut n = 1;
    while folder.join(&id).exists() {
        id = format!("{now:020}-{n}.signature");
        n += 1;
    }
    std::fs::write(folder.join(&id), signature.text()).map_err(|e| e.to_string())?;
    Ok(Signature { id, ..signature.clone() })
}

/// Keep a removed signature again, under the name it had, so it goes back
/// where it was in the list.
pub fn restore(signature: &Signature) -> Result<(), String> {
    if !is_ours(&signature.id) {
        return Err("not a signature".into());
    }
    let folder = folder();
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    std::fs::write(folder.join(&signature.id), signature.text()).map_err(|e| e.to_string())
}

/// A name of one of our files, in our folder and nowhere else.
fn is_ours(id: &str) -> bool {
    !id.contains(['/', '\\']) && id.ends_with(".signature")
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
    }
}
