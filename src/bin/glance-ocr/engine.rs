// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading text with PaddleOCR's PP-OCRv6 models: one finds where the lines
//! of text are, the other reads each line.
//!
//! Every step follows RapidOCR, the reference the models were measured with,
//! so the helper reads as well as that measurement promised: the same sizes,
//! the same thresholds, the same order of colour channels (blue first).

use std::path::Path;

use ort::session::Session;
use ort::value::Tensor;

use crate::geometry::{self, Point, Rgb};
use crate::protocol::Line;

/// The longest side a picture is read at, and the shortest.
const MAX_SIDE: usize = 2000;
const MIN_SIDE: usize = 30;
/// The detector works on pictures at least this big on their shorter side.
const DETECT_MIN_SIDE: usize = 736;
/// How sure the detector must be that a pixel is text, and a blob of them.
const PIXEL_THRESHOLD: f32 = 0.3;
const BOX_THRESHOLD: f32 = 0.5;
const MAX_BOXES: usize = 1000;
/// How far a found line is grown back out, for the margin the detector
/// trims off.
const UNCLIP_RATIO: f32 = 1.6;
const MIN_BOX_SIDE: f32 = 3.0;
/// Lines whose tops are closer than this are one row, read left to right.
const SAME_ROW: f32 = 10.0;
/// Lines are read 48 pixels tall, at least 320 wide, six at a time.
const LINE_HEIGHT: usize = 48;
const LINE_MIN_WIDTH: usize = 320;
const BATCH: usize = 6;
/// Lines read with less confidence than this are dropped.
const TEXT_THRESHOLD: f32 = 0.5;

pub struct Engine {
    detect: Session,
    recognise: Session,
    /// The recogniser's characters: a blank first, then the model's own list,
    /// then a space.
    characters: Vec<String>,
}

fn error(context: &str, e: impl std::fmt::Display) -> String {
    format!("{context}: {e}")
}

impl Engine {
    pub fn load(detect: &Path, recognise: &Path, threads: usize) -> Result<Self, String> {
        let session = |path: &Path| -> Result<Session, String> {
            Session::builder()
                .map_err(|e| error("starting ONNX Runtime", e))?
                .with_intra_threads(threads)
                .map_err(|e| error("setting threads", e))?
                // No memory pool and no planned buffers: both grow to the
                // biggest picture yet and keep it, which for pictures of
                // every size meant a gigabyte held after two reads.
                .with_memory_pattern(false)
                .map_err(|e| error("setting memory", e))?
                .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])
                .map_err(|e| error("setting memory", e))?
                .commit_from_file(path)
                .map_err(|e| error(&format!("loading {}", path.display()), e))
        };
        let detect = session(detect)?;
        let recognise = session(recognise)?;
        let list = recognise
            .metadata()
            .map_err(|e| error("reading the recogniser", e))?
            .custom("character")
            .ok_or("the recogniser has no character list")?;
        let mut characters = vec![String::new()];
        characters.extend(list.split('\n').map(str::to_string));
        characters.push(" ".to_string());
        Ok(Engine { detect, recognise, characters })
    }

    /// Read the text in an RGBA picture.
    pub fn read(&mut self, rgba: &[u8], width: usize, height: usize) -> Result<Vec<Line>, String> {
        let picture = to_bgr(rgba, width, height);
        // Within bounds first, as the reference does: big photographs are
        // read at 2000 pixels, which is plenty for any text a person reads.
        let (picture, scale_x, scale_y) = within_bounds(picture);
        let quads = self.find_lines(&picture)?;
        let mut strips = Vec::new();
        let mut kept = Vec::new();
        for quad in quads {
            if let Some((strip, turned)) = picture.crop(&quad) {
                strips.push(strip);
                kept.push((quad, turned));
            }
        }
        let read = self.read_lines(&strips)?;
        let mut lines = Vec::new();
        for ((quad, turned), (text, score, cuts)) in kept.into_iter().zip(read) {
            if text.trim().is_empty() || score < TEXT_THRESHOLD {
                continue;
            }
            // A turned strip reads down its quadrilateral, not across.
            let quad = if turned { [quad[3], quad[0], quad[1], quad[2]] } else { quad };
            let quad = quad.map(|[x, y]| [x * scale_x, y * scale_y]);
            lines.push(Line { text, score, quad, cuts });
        }
        Ok(lines)
    }

    /// The lines of text, as quadrilaterals in reading order.
    fn find_lines(&mut self, picture: &Rgb) -> Result<Vec<[Point; 4]>, String> {
        let (w, h) = (picture.width, picture.height);
        let ratio = if w.min(h) < DETECT_MIN_SIDE { DETECT_MIN_SIDE as f32 / w.min(h) as f32 } else { 1.0 };
        let to_32 = |v: f32| ((v / 32.0).round_ties_even() as usize * 32).max(32);
        let (dw, dh) = (to_32((w as f32 * ratio).trunc()), to_32((h as f32 * ratio).trunc()));
        let input = picture.resized(dw, dh);
        let tensor = Tensor::from_array(([1usize, 3, dh, dw], normalise(&input)))
            .map_err(|e| error("preparing the picture", e))?;
        let outputs = self.detect.run(ort::inputs![tensor]).map_err(|e| error("finding text", e))?;
        let (shape, map) = outputs[0].try_extract_tensor::<f32>().map_err(|e| error("reading the text map", e))?;
        let (mh, mw) = (shape[2] as usize, shape[3] as usize);

        // Set where the detector is fairly sure, then grown by a pixel down
        // and right, as the reference's 2x2 dilation does.
        let set: Vec<u8> = map.iter().map(|&p| u8::from(p > PIXEL_THRESHOLD)).collect();
        let mut bitmap = set.clone();
        for y in 0..mh {
            for x in 0..mw {
                if set[y * mw + x] != 0 {
                    for (dx, dy) in [(1, 0), (0, 1), (1, 1)] {
                        if x + dx < mw && y + dy < mh {
                            bitmap[(y + dy) * mw + x + dx] = 1;
                        }
                    }
                }
            }
        }

        let mut quads = Vec::new();
        for blob in geometry::blobs(&bitmap, mw, mh, MAX_BOXES) {
            let rect = geometry::min_area_rect(&blob);
            if rect.short_side() < MIN_BOX_SIDE {
                continue;
            }
            let corners = geometry::mini_box(&rect);
            if box_score(map, mw, mh, &corners) < BOX_THRESHOLD {
                continue;
            }
            let (area, length) = geometry::area_and_length(&corners);
            let grown = geometry::min_area_rect(&geometry::mini_box(&rect.grown(area * UNCLIP_RATIO / length)));
            if grown.short_side() < MIN_BOX_SIDE + 2.0 {
                continue;
            }
            // Back to the picture's own size, clipped to it.
            let scaled = geometry::mini_box(&grown).map(|[x, y]| {
                [
                    (x / mw as f32 * w as f32).round_ties_even().clamp(0.0, w as f32),
                    (y / mh as f32 * h as f32).round_ties_even().clamp(0.0, h as f32),
                ]
            });
            let quad = geometry::order_clockwise(scaled)
                .map(|[x, y]| [x.clamp(0.0, (w - 1) as f32).trunc(), y.clamp(0.0, (h - 1) as f32).trunc()]);
            let side = |a: Point, b: Point| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt() as usize;
            if side(quad[0], quad[1]) <= 3 || side(quad[0], quad[3]) <= 3 {
                continue;
            }
            quads.push(quad);
        }
        Ok(sorted(quads))
    }

    /// Read each strip: its text, how sure, and where each character falls.
    fn read_lines(&mut self, strips: &[Rgb]) -> Result<Vec<(String, f32, Vec<f32>)>, String> {
        let mut results = vec![(String::new(), 0.0, Vec::new()); strips.len()];
        // Similar widths together, so a batch pads as little as it can.
        let mut order: Vec<usize> = (0..strips.len()).collect();
        let ratio = |s: &Rgb| s.width as f32 / s.height as f32;
        order.sort_by(|&a, &b| ratio(&strips[a]).total_cmp(&ratio(&strips[b])));

        for batch in order.chunks(BATCH) {
            let widest = batch
                .iter()
                .map(|&i| ratio(&strips[i]))
                .fold(LINE_MIN_WIDTH as f32 / LINE_HEIGHT as f32, f32::max);
            let width = (LINE_HEIGHT as f32 * widest) as usize;
            let mut data = vec![0f32; batch.len() * 3 * LINE_HEIGHT * width];
            for (slot, &i) in batch.iter().enumerate() {
                let strip = &strips[i];
                let resized_width = ((LINE_HEIGHT as f32 * ratio(strip)).ceil() as usize).min(width).max(1);
                let line = strip.resized(resized_width, LINE_HEIGHT);
                let plane = LINE_HEIGHT * width;
                for y in 0..LINE_HEIGHT {
                    for x in 0..resized_width {
                        for c in 0..3 {
                            let value = f32::from(line.data[(y * resized_width + x) * 3 + c]) / 255.0;
                            data[slot * 3 * plane + c * plane + y * width + x] = (value - 0.5) / 0.5;
                        }
                    }
                }
            }
            let tensor = Tensor::from_array(([batch.len(), 3, LINE_HEIGHT, width], data))
                .map_err(|e| error("preparing the lines", e))?;
            let outputs = self.recognise.run(ort::inputs![tensor]).map_err(|e| error("reading text", e))?;
            let (shape, probabilities) =
                outputs[0].try_extract_tensor::<f32>().map_err(|e| error("reading the characters", e))?;
            let (steps, classes) = (shape[1] as usize, shape[2] as usize);
            for (slot, &i) in batch.iter().enumerate() {
                let own = &probabilities[slot * steps * classes..(slot + 1) * steps * classes];
                // How many steps the strip itself covers, before the padding.
                let covered = steps as f32 * ratio(&strips[i]) / widest;
                results[i] = decode(own, steps, classes, &self.characters, covered);
            }
        }
        Ok(results)
    }
}

/// The best character at each step; repeats and blanks dropped, as CTC asks.
/// Gives the text, the mean confidence of its characters, and where each
/// character begins and ends as a fraction of the strip's text.
fn decode(probabilities: &[f32], steps: usize, classes: usize, characters: &[String], covered: f32) -> (String, f32, Vec<f32>) {
    let mut text = String::new();
    let mut confidences = Vec::new();
    let mut columns = Vec::new();
    let mut previous = usize::MAX;
    for step in 0..steps {
        let row = &probabilities[step * classes..(step + 1) * classes];
        let (best, &p) = row.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap_or((0, &0.0));
        if best != 0 && best != previous {
            if let Some(character) = characters.get(best) {
                text.push_str(character);
                confidences.push(p);
                columns.push(step as f32);
            }
        }
        previous = best;
    }
    let score = if confidences.is_empty() { 0.0 } else { confidences.iter().sum::<f32>() / confidences.len() as f32 };
    (text, score, cuts(&columns, covered))
}

/// Character boundaries from the steps each character was read at: halfway
/// between neighbours, and half a gap out at either end.
fn cuts(columns: &[f32], covered: f32) -> Vec<f32> {
    if columns.is_empty() || covered <= 0.0 {
        return Vec::new();
    }
    let gap = if columns.len() > 1 { (columns[columns.len() - 1] - columns[0]) / (columns.len() - 1) as f32 } else { 2.0 };
    let mut out = Vec::with_capacity(columns.len() + 1);
    out.push(columns[0] + 0.5 - gap / 2.0);
    for pair in columns.windows(2) {
        out.push((pair[0] + pair[1]) / 2.0 + 0.5);
    }
    out.push(columns[columns.len() - 1] + 0.5 + gap / 2.0);
    out.iter().map(|c| (c / covered).clamp(0.0, 1.0)).collect()
}

/// How sure the detector is, on average, inside a box.
fn box_score(map: &[f32], width: usize, height: usize, corners: &[Point; 4]) -> f32 {
    let clamp_x = |v: f32| (v.max(0.0) as usize).min(width - 1);
    let clamp_y = |v: f32| (v.max(0.0) as usize).min(height - 1);
    let xs = corners.map(|p| p[0]);
    let ys = corners.map(|p| p[1]);
    let (x0, x1) = (clamp_x(xs.iter().copied().fold(f32::MAX, f32::min).floor()), clamp_x(xs.iter().copied().fold(f32::MIN, f32::max).ceil()));
    let (y0, y1) = (clamp_y(ys.iter().copied().fold(f32::MAX, f32::min).floor()), clamp_y(ys.iter().copied().fold(f32::MIN, f32::max).ceil()));
    // The reference fills the polygon at whole pixels.
    let quad = corners.map(|[x, y]| [x.trunc(), y.trunc()]);
    let (mut sum, mut count) = (0.0, 0usize);
    for y in y0..=y1 {
        for x in x0..=x1 {
            if geometry::inside(&quad, x as f32, y as f32) {
                sum += map[y * width + x];
                count += 1;
            }
        }
    }
    if count == 0 { 0.0 } else { sum / count as f32 }
}

/// Top to bottom; lines whose tops are within a few pixels are one row,
/// read left to right.
fn sorted(mut quads: Vec<[Point; 4]>) -> Vec<[Point; 4]> {
    quads.sort_by(|a, b| a[0][1].total_cmp(&b[0][1]));
    let mut row = 0;
    let mut rows = Vec::with_capacity(quads.len());
    for (i, quad) in quads.iter().enumerate() {
        if i > 0 && quad[0][1] - quads[i - 1][0][1] >= SAME_ROW {
            row += 1;
        }
        rows.push(row);
    }
    let mut keyed: Vec<(usize, [Point; 4])> = rows.into_iter().zip(quads).collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1[0][0].total_cmp(&b.1[0][0])));
    keyed.into_iter().map(|(_, quad)| quad).collect()
}

/// Channel planes, blue first, scaled to -1..1.
fn normalise(picture: &Rgb) -> Vec<f32> {
    let plane = picture.width * picture.height;
    let mut out = vec![0f32; plane * 3];
    for (i, pixel) in picture.data.chunks_exact(3).enumerate() {
        for c in 0..3 {
            out[c * plane + i] = (f32::from(pixel[c]) / 255.0 - 0.5) / 0.5;
        }
    }
    out
}

/// RGBA to the blue-first order the models were trained on. Transparency is
/// laid on white for dark content and on black for light, so text keeps its
/// contrast either way.
fn to_bgr(rgba: &[u8], width: usize, height: usize) -> Rgb {
    let (mut total, mut opaque) = (0.0f64, 0usize);
    for p in rgba.chunks_exact(4) {
        if p[3] > 0 {
            total += 0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2]);
            opaque += 1;
        }
    }
    let ground = if opaque == 0 || total / (opaque as f64) < 128.0 { 255.0 } else { 0.0 };
    let mut data = Vec::with_capacity(width * height * 3);
    for p in rgba.chunks_exact(4) {
        let a = f32::from(p[3]) / 255.0;
        for c in [2, 1, 0] {
            data.push((f32::from(p[c]) * a + ground * (1.0 - a)) as u8);
        }
    }
    Rgb { width, height, data }
}

/// No longer than `MAX_SIDE` and no shorter than `MIN_SIDE`, both rounded to
/// whole multiples of 32 (halves to even, as Python rounds); with how much each axis shrank, to put what is
/// found back where it was.
fn within_bounds(picture: Rgb) -> (Rgb, f32, f32) {
    let (w, h) = (picture.width, picture.height);
    let fit = |picture: Rgb, ratio: f32| -> (Rgb, f32, f32) {
        let to_32 = |v: f32| ((v / 32.0).round_ties_even() as usize * 32).max(32);
        let (nw, nh) = (to_32((picture.width as f32 * ratio).trunc()), to_32((picture.height as f32 * ratio).trunc()));
        let (sx, sy) = (picture.width as f32 / nw as f32, picture.height as f32 / nh as f32);
        (picture.resized(nw, nh), sx, sy)
    };
    let (picture, mut sx, mut sy) = if w.max(h) > MAX_SIDE { fit(picture, MAX_SIDE as f32 / w.max(h) as f32) } else { (picture, 1.0, 1.0) };
    if picture.width.min(picture.height) < MIN_SIDE {
        let ratio = MIN_SIDE as f32 / picture.width.min(picture.height) as f32;
        let (bigger, bx, by) = fit(picture, ratio);
        sx *= bx;
        sy *= by;
        return (bigger, sx, sy);
    }
    (picture, sx, sy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctc_drops_blanks_and_repeats_but_keeps_doubled_letters() {
        let characters: Vec<String> = ["", "l", "o", " "].iter().map(|s| s.to_string()).collect();
        // l l blank l o o: "llo", the blank separating the two l's.
        let steps = [1, 1, 0, 1, 2, 2];
        let mut probabilities = vec![0.0f32; steps.len() * 4];
        for (t, &c) in steps.iter().enumerate() {
            probabilities[t * 4 + c] = 0.9;
        }
        let (text, score, cuts) = decode(&probabilities, steps.len(), 4, &characters, 6.0);
        assert_eq!(text, "llo");
        assert!((score - 0.9).abs() < 1e-6);
        assert_eq!(cuts.len(), 4, "one more boundary than characters");
        assert!(cuts.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn rows_read_left_to_right_then_down() {
        let quad = |x: f32, y: f32| [[x, y], [x + 10.0, y], [x + 10.0, y + 5.0], [x, y + 5.0]];
        let order = sorted(vec![quad(50.0, 103.0), quad(0.0, 40.0), quad(10.0, 100.0), quad(0.0, 0.0)]);
        assert_eq!(order.iter().map(|q| q[0]).collect::<Vec<_>>(), vec![[0.0, 0.0], [0.0, 40.0], [10.0, 100.0], [50.0, 103.0]]);
    }

    #[test]
    fn big_pictures_are_read_at_2000_and_small_ones_grown() {
        let picture = |w, h| Rgb { width: w, height: h, data: vec![128; w * h * 3] };
        let (big, sx, sy) = within_bounds(picture(4000, 3000));
        // Halves round to even, as Python's round does: 62.5 is 62.
        assert_eq!((big.width, big.height), (1984, 1504));
        assert!((sx - 4000.0 / 1984.0).abs() < 1e-4 && (sy - 3000.0 / 1504.0).abs() < 1e-4);
        let (small, _, _) = within_bounds(picture(300, 12));
        assert_eq!(small.height, 32);
        let (same, sx, sy) = within_bounds(picture(800, 600));
        assert_eq!((same.width, sx, sy), (800, 1.0, 1.0));
    }

    #[test]
    fn transparency_lands_on_a_contrasting_ground() {
        // Dark, half-transparent text-ish pixels go on white.
        let rgba = [0, 0, 0, 128, 0, 0, 0, 0];
        let bgr = to_bgr(&rgba, 2, 1);
        assert_eq!(bgr.data, vec![126, 126, 126, 255, 255, 255]);
    }
}
