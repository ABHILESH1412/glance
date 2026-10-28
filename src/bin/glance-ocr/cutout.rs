// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Telling the subject of a picture from its background, with BiRefNet-lite
//! (MIT): how sure it is, pixel by pixel, that each is part of the subject.
//!
//! The model looks at the picture squeezed to 640 by 640, so its answer is
//! that coarse. It is brought back to the picture's own size and then given
//! the picture's edges with a guided filter: an edge in the answer that lies
//! near an edge in the picture moves onto it, so hair and outlines are traced
//! from the picture itself rather than from a blurred, scaled-up guess.

use std::path::Path;

use ort::session::Session;
use ort::value::Tensor;

/// The size a model that takes any size is run at, both ways. Memory grows
/// with the square of it. Measured with BiRefNet-lite on a 1920 by 2366
/// photograph: 512 needs 0.8 GB, 640 about 1.0 GB, 768 1.15 GB, and 1024 more
/// than 3 GB, too much for a laptop with other things open. At 640 it still
/// finds a raised hand that 512 and 768 both lose.
const ANY_SIDE: usize = 640;
/// How the model was trained to see colours: ImageNet's mean and spread.
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const SPREAD: [f32; 3] = [0.229, 0.224, 0.225];
/// The guided filter's reach, as a share of the picture's shorter side, and
/// how much it trusts an edge in the picture over the model's answer.
const REACH: f32 = 0.006;
const SMOOTHING: f32 = 1e-3;

pub struct Cutout {
    session: Session,
    /// The size it looks at, both ways: the model's own, or `ANY_SIDE`.
    side: usize,
    /// Whether the model's answer still needs its sigmoid: some exports end
    /// on it, some leave it to whoever runs them.
    logits: bool,
}

fn error(context: &str, e: impl std::fmt::Display) -> String {
    format!("{context}: {e}")
}

impl Cutout {
    pub fn load(model: &Path, threads: usize) -> Result<Self, String> {
        let session = Session::builder()
            .map_err(|e| error("starting ONNX Runtime", e))?
            .with_intra_threads(threads)
            .map_err(|e| error("setting threads", e))?
            // As for Live Text: no pool and no planned buffers, which would
            // keep the most ever used.
            .with_memory_pattern(false)
            .map_err(|e| error("setting memory", e))?
            .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])
            .map_err(|e| error("setting memory", e))?
            .commit_from_file(model)
            .map_err(|e| error(&format!("loading {}", model.display()), e))?;
        let ends_on_sigmoid = session.outputs().first().is_some_and(|output| output.name().to_ascii_lowercase().contains("sigmoid"));
        let side = session
            .inputs()
            .first()
            .and_then(|input| input.dtype().tensor_shape())
            .and_then(|shape| shape.last().copied())
            .filter(|&n| n > 0)
            .map_or(ANY_SIDE, |n| n as usize);
        Ok(Cutout { session, side, logits: !ends_on_sigmoid })
    }

    /// What the model expects and gives, for trying it out by hand.
    pub fn describe(&self) -> String {
        let outlets = |list: &[ort::value::Outlet]| {
            list.iter().map(|o| format!("{} {:?}", o.name(), o.dtype())).collect::<Vec<_>>().join(", ")
        };
        format!("at {}: in: {}; out: {}", self.side, outlets(self.session.inputs()), outlets(self.session.outputs()))
    }

    /// For each pixel of an RGBA picture, 0 for background to 255 for the
    /// subject.
    pub fn mask(&mut self, rgba: &[u8], width: usize, height: usize) -> Result<Vec<u8>, String> {
        if width == 0 || height == 0 || rgba.len() != width * height * 4 {
            return Err("the picture is not the size it says".to_string());
        }
        let side = self.side;
        let input = Tensor::from_array(([1usize, 3, side, side], planes(rgba, width, height, side)))
            .map_err(|e| error("preparing the picture", e))?;
        let outputs = self.session.run(ort::inputs![input]).map_err(|e| error("finding the subject", e))?;
        // The last output is the finest; the others are its rougher drafts.
        let last = outputs.len().checked_sub(1).ok_or("the model gave nothing back")?;
        let (shape, answer) = outputs[last].try_extract_tensor::<f32>().map_err(|e| error("reading the answer", e))?;
        let (ah, aw) = match shape[..] {
            [.., h, w] => (h as usize, w as usize),
            _ => return Err("the answer has no size".to_string()),
        };
        if answer.len() < ah * aw {
            return Err("the answer is shorter than its size".to_string());
        }
        let sure: Vec<f32> = answer[..ah * aw]
            .iter()
            .map(|&v| if self.logits { 1.0 / (1.0 + (-v).exp()) } else { v.clamp(0.0, 1.0) })
            .collect();
        let rough = resize_plane(&sure, aw, ah, width, height);
        let refined = guided(&grey(rgba), &rough, width, height);
        Ok(refined.iter().map(|&v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect())
    }
}

/// The picture squeezed to the model's size, as three planes of normalised
/// red, green and blue. Transparent parts are seen against white.
fn planes(rgba: &[u8], width: usize, height: usize, side: usize) -> Vec<f32> {
    let mut out = vec![0f32; 3 * side * side];
    let plane = side * side;
    let sx = width as f32 / side as f32;
    let sy = height as f32 / side as f32;
    for y in 0..side {
        // Sampled at each pixel's middle, between its four neighbours.
        let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (height - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        let y1 = (y0 + 1).min(height - 1);
        for x in 0..side {
            let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (width - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let x1 = (x0 + 1).min(width - 1);
            for c in 0..3 {
                let at = |px: usize, py: usize| {
                    let p = &rgba[(py * width + px) * 4..(py * width + px) * 4 + 4];
                    let a = f32::from(p[3]) / 255.0;
                    f32::from(p[c]) / 255.0 * a + (1.0 - a)
                };
                let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
                let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
                let value = top * (1.0 - ty) + bottom * ty;
                out[c * plane + y * side + x] = (value - MEAN[c]) / SPREAD[c];
            }
        }
    }
    out
}

/// One plane from its size to another, smoothly.
fn resize_plane(plane: &[f32], from_w: usize, from_h: usize, to_w: usize, to_h: usize) -> Vec<f32> {
    let mut out = vec![0f32; to_w * to_h];
    let sx = from_w as f32 / to_w as f32;
    let sy = from_h as f32 / to_h as f32;
    for y in 0..to_h {
        let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (from_h - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        let y1 = (y0 + 1).min(from_h - 1);
        for x in 0..to_w {
            let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (from_w - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let x1 = (x0 + 1).min(from_w - 1);
            let top = plane[y0 * from_w + x0] * (1.0 - tx) + plane[y0 * from_w + x1] * tx;
            let bottom = plane[y1 * from_w + x0] * (1.0 - tx) + plane[y1 * from_w + x1] * tx;
            out[y * to_w + x] = top * (1.0 - ty) + bottom * ty;
        }
    }
    out
}

/// How light each pixel is, 0 to 1, seen against white where transparent.
fn grey(rgba: &[u8]) -> Vec<f32> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            let a = f32::from(p[3]) / 255.0;
            let luma = (0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2])) / 255.0;
            luma * a + (1.0 - a)
        })
        .collect()
}

/// He, Sun and Tang's guided filter: `mask` made to follow the edges of
/// `guide`, locally a scaled and shifted copy of it.
fn guided(guide: &[f32], mask: &[f32], width: usize, height: usize) -> Vec<f32> {
    let radius = ((width.min(height) as f32 * REACH).round() as usize).max(1);
    let mean = |plane: &[f32]| box_mean(plane, width, height, radius);
    let mean_i = mean(guide);
    let mean_p = mean(mask);
    let product: Vec<f32> = guide.iter().zip(mask).map(|(i, p)| i * p).collect();
    let square: Vec<f32> = guide.iter().map(|i| i * i).collect();
    let mean_ip = mean(&product);
    let mean_ii = mean(&square);
    let mut a = vec![0f32; guide.len()];
    let mut b = vec![0f32; guide.len()];
    for k in 0..guide.len() {
        let covariance = mean_ip[k] - mean_i[k] * mean_p[k];
        let variance = mean_ii[k] - mean_i[k] * mean_i[k];
        a[k] = covariance / (variance + SMOOTHING);
        b[k] = mean_p[k] - a[k] * mean_i[k];
    }
    let (mean_a, mean_b) = (mean(&a), mean(&b));
    guide.iter().enumerate().map(|(k, i)| mean_a[k] * i + mean_b[k]).collect()
}

/// The average over the square of `radius` round each pixel, cut short at
/// the edges, in time that does not depend on the radius.
fn box_mean(plane: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    // Across each row, then down each column, with running sums.
    let mut across = vec![0f32; plane.len()];
    for y in 0..height {
        let row = &plane[y * width..(y + 1) * width];
        let mut sum: f64 = row[..radius.min(width - 1) + 1].iter().map(|&v| f64::from(v)).sum();
        for x in 0..width {
            let (left, right) = (x.saturating_sub(radius), (x + radius).min(width - 1));
            across[y * width + x] = (sum / (right - left + 1) as f64) as f32;
            if x + radius + 1 < width {
                sum += f64::from(row[x + radius + 1]);
            }
            if x >= radius {
                sum -= f64::from(row[x - radius]);
            }
        }
    }
    let mut out = vec![0f32; plane.len()];
    for x in 0..width {
        let mut sum: f64 = (0..=radius.min(height - 1)).map(|y| f64::from(across[y * width + x])).sum();
        for y in 0..height {
            let (top, bottom) = (y.saturating_sub(radius), (y + radius).min(height - 1));
            out[y * width + x] = (sum / (bottom - top + 1) as f64) as f32;
            if y + radius + 1 < height {
                sum += f64::from(across[(y + radius + 1) * width + x]);
            }
            if y >= radius {
                sum -= f64::from(across[(y - radius) * width + x]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_mean_is_the_plain_average_and_keeps_a_flat_plane_flat() {
        let flat = vec![0.25f32; 7 * 5];
        assert!(box_mean(&flat, 7, 5, 2).iter().all(|v| (v - 0.25).abs() < 1e-6));
        // One bright pixel in the middle of a 3x3: a radius of 1 shares it
        // out over each pixel's neighbours, fewer at the corners.
        let mut dot = vec![0f32; 9];
        dot[4] = 9.0;
        let mean = box_mean(&dot, 3, 3, 1);
        assert!((mean[4] - 1.0).abs() < 1e-6, "middle: nine pixels, one bright");
        assert!((mean[0] - 9.0 / 4.0).abs() < 1e-6, "corner: four pixels, one bright");
        assert!((mean[1] - 9.0 / 6.0).abs() < 1e-6, "side: six pixels, one bright");
    }

    #[test]
    fn a_blurred_edge_moves_onto_the_picture_s_own() {
        // A picture dark on the left, light from x = 40; an answer that
        // fades from 0 to 1 across x = 30..50, as a scaled-up one would.
        let (w, h) = (80, 20);
        let guide: Vec<f32> = (0..w * h).map(|k| if k % w >= 40 { 1.0 } else { 0.0 }).collect();
        let rough: Vec<f32> = (0..w * h).map(|k| ((k % w) as f32 - 30.0).clamp(0.0, 20.0) / 20.0).collect();
        let sharp = guided(&guide, &rough, w, h);
        let at = |x: usize| sharp[10 * w + x];
        assert!(at(36) < 0.35 && at(44) > 0.65, "steeper at the picture's edge: {} {}", at(36), at(44));
        assert!(at(5) < 0.05 && at(75) > 0.95, "and left alone away from it");
    }

    #[test]
    fn a_picture_reaches_the_model_squeezed_and_normalised() {
        // A white picture of any size is white everywhere at the model's.
        let white = vec![255u8; 30 * 10 * 4];
        let side = 64;
        let seen = planes(&white, 30, 10, side);
        assert_eq!(seen.len(), 3 * side * side);
        for c in 0..3 {
            let want = (1.0 - MEAN[c]) / SPREAD[c];
            assert!((seen[c * side * side + 517] - want).abs() < 1e-4);
        }
        // Transparent counts as white too.
        let clear = vec![0u8; 4 * 4 * 4];
        assert!((planes(&clear, 4, 4, side)[0] - (1.0 - MEAN[0]) / SPREAD[0]).abs() < 1e-4);
    }
}
