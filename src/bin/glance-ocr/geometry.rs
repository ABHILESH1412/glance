// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The geometry text detection needs, which the Python reference takes from
//! OpenCV: the separate blobs of a bitmap, the smallest rectangle at any angle
//! around each, and cutting a slanted quadrilateral out of a picture as a
//! straight strip.

pub type Point = [f32; 2];

/// The blobs of `bitmap` (`width * height`, non-zero is set), each as the
/// pixel positions that matter for its outline: the first and last set pixel
/// of every row it covers. Pixels touching at a corner are one blob, as in
/// OpenCV's contours. At most `limit` blobs, in the order they are met.
pub fn blobs(bitmap: &[u8], width: usize, height: usize, limit: usize) -> Vec<Vec<Point>> {
    let mut seen = vec![false; bitmap.len()];
    let mut found = Vec::new();
    let mut stack = Vec::new();
    for start in 0..bitmap.len() {
        if bitmap[start] == 0 || seen[start] {
            continue;
        }
        if found.len() == limit {
            break;
        }
        // Each row's leftmost and rightmost pixel is all the hull needs.
        let mut rows: std::collections::BTreeMap<usize, (usize, usize)> = Default::default();
        seen[start] = true;
        stack.push(start);
        while let Some(at) = stack.pop() {
            let (x, y) = (at % width, at / width);
            let span = rows.entry(y).or_insert((x, x));
            span.0 = span.0.min(x);
            span.1 = span.1.max(x);
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < 0 || ny < 0 || nx >= width as i64 || ny >= height as i64 {
                        continue;
                    }
                    let next = ny as usize * width + nx as usize;
                    if bitmap[next] != 0 && !seen[next] {
                        seen[next] = true;
                        stack.push(next);
                    }
                }
            }
        }
        let mut points = Vec::with_capacity(rows.len() * 2);
        for (y, (left, right)) in rows {
            points.push([left as f32, y as f32]);
            if right != left {
                points.push([right as f32, y as f32]);
            }
        }
        found.push(points);
    }
    found
}

fn cross(o: Point, a: Point, b: Point) -> f32 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// The convex hull, counter-clockwise, by the monotone chain.
pub fn hull(points: &[Point]) -> Vec<Point> {
    let mut sorted = points.to_vec();
    sorted.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    sorted.dedup();
    if sorted.len() < 3 {
        return sorted;
    }
    let mut lower: Vec<Point> = Vec::new();
    for &p in &sorted {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<Point> = Vec::new();
    for &p in sorted.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// A rectangle at an angle: its centre, its two half-axes as vectors, and
/// their lengths.
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub centre: Point,
    pub axis_u: Point,
    pub axis_v: Point,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn corners(&self) -> [Point; 4] {
        let (c, u, v) = (self.centre, self.axis_u, self.axis_v);
        let (hu, hv) = (self.width / 2.0, self.height / 2.0);
        let at = |a: f32, b: f32| [c[0] + u[0] * a + v[0] * b, c[1] + u[1] * a + v[1] * b];
        [at(-hu, -hv), at(hu, -hv), at(hu, hv), at(-hu, hv)]
    }

    /// The same rectangle, `distance` bigger on every side.
    pub fn grown(&self, distance: f32) -> Rect {
        Rect { width: self.width + 2.0 * distance, height: self.height + 2.0 * distance, ..*self }
    }

    pub fn short_side(&self) -> f32 {
        self.width.min(self.height)
    }
}

/// The smallest rectangle, at any angle, around `points`: one side lies along
/// an edge of the hull, so each edge is tried.
pub fn min_area_rect(points: &[Point]) -> Rect {
    let hull = hull(points);
    if hull.is_empty() {
        return Rect { centre: [0.0, 0.0], axis_u: [1.0, 0.0], axis_v: [0.0, 1.0], width: 0.0, height: 0.0 };
    }
    if hull.len() < 3 {
        // A dot or a line: no area, but still an extent along it.
        let (a, b) = (hull[0], *hull.last().unwrap());
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let length = (dx * dx + dy * dy).sqrt();
        let u = if length > 0.0 { [dx / length, dy / length] } else { [1.0, 0.0] };
        return Rect {
            centre: [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0],
            axis_u: u,
            axis_v: [-u[1], u[0]],
            width: length,
            height: 0.0,
        };
    }
    let mut best: Option<(f32, Rect)> = None;
    for i in 0..hull.len() {
        let (a, b) = (hull[i], hull[(i + 1) % hull.len()]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let length = (dx * dx + dy * dy).sqrt();
        if length == 0.0 {
            continue;
        }
        let u = [dx / length, dy / length];
        let v = [-u[1], u[0]];
        let (mut min_u, mut max_u, mut min_v, mut max_v) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for p in &hull {
            let pu = p[0] * u[0] + p[1] * u[1];
            let pv = p[0] * v[0] + p[1] * v[1];
            min_u = min_u.min(pu);
            max_u = max_u.max(pu);
            min_v = min_v.min(pv);
            max_v = max_v.max(pv);
        }
        let area = (max_u - min_u) * (max_v - min_v);
        if best.as_ref().is_none_or(|(a, _)| area < *a) {
            let (cu, cv) = ((min_u + max_u) / 2.0, (min_v + max_v) / 2.0);
            best = Some((
                area,
                Rect {
                    centre: [u[0] * cu + v[0] * cv, u[1] * cu + v[1] * cv],
                    axis_u: u,
                    axis_v: v,
                    width: max_u - min_u,
                    height: max_v - min_v,
                },
            ));
        }
    }
    best.map(|(_, rect)| rect).unwrap_or(Rect {
        centre: hull[0],
        axis_u: [1.0, 0.0],
        axis_v: [0.0, 1.0],
        width: 0.0,
        height: 0.0,
    })
}

/// A rectangle's corners in the order the reference gives them: sorted by x,
/// then the upper of the left pair, the upper of the right pair, the lower of
/// the right pair, the lower of the left pair.
pub fn mini_box(rect: &Rect) -> [Point; 4] {
    let mut points = rect.corners();
    points.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let (first, fourth) = if points[1][1] > points[0][1] { (0, 1) } else { (1, 0) };
    let (second, third) = if points[3][1] > points[2][1] { (2, 3) } else { (3, 2) };
    [points[first], points[second], points[third], points[fourth]]
}

/// Area and perimeter of a polygon.
pub fn area_and_length(points: &[Point]) -> (f32, f32) {
    let mut area = 0.0;
    let mut length = 0.0;
    for i in 0..points.len() {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        area += a[0] * b[1] - b[0] * a[1];
        length += ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
    }
    (area.abs() / 2.0, length)
}

/// Whether the centre of pixel (x, y) is inside a convex quadrilateral,
/// edges included.
pub fn inside(quad: &[Point; 4], x: f32, y: f32) -> bool {
    let mut sign = 0.0f32;
    for i in 0..4 {
        let c = cross(quad[i], quad[(i + 1) % 4], [x, y]);
        if c.abs() < 1e-3 {
            continue;
        }
        if sign == 0.0 {
            sign = c.signum();
        } else if c.signum() != sign {
            return false;
        }
    }
    true
}

/// Top left, top right, bottom right, bottom left: the two leftmost points
/// by height, then the two rightmost.
pub fn order_clockwise(points: [Point; 4]) -> [Point; 4] {
    let mut by_x = points;
    by_x.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let (mut left, mut right) = ([by_x[0], by_x[1]], [by_x[2], by_x[3]]);
    left.sort_by(|a, b| a[1].total_cmp(&b[1]));
    right.sort_by(|a, b| a[1].total_cmp(&b[1]));
    [left[0], right[0], right[1], left[1]]
}

fn distance(a: Point, b: Point) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// The projective map taking the rectangle `(0, 0)..(width, height)` onto
/// `quad`, as the eight unknowns of a homography.
fn homography(quad: &[Point; 4], width: f32, height: f32) -> Option<[f64; 8]> {
    let from = [[0.0, 0.0], [width, 0.0], [width, height], [0.0, height]];
    let mut m = [[0.0f64; 9]; 8];
    for i in 0..4 {
        let (x, y) = (f64::from(from[i][0]), f64::from(from[i][1]));
        let (u, v) = (f64::from(quad[i][0]), f64::from(quad[i][1]));
        m[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
        m[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
    }
    // Gaussian elimination with partial pivoting.
    for col in 0..8 {
        let pivot = (col..8).max_by(|&a, &b| m[a][col].abs().total_cmp(&m[b][col].abs()))?;
        if m[pivot][col].abs() < 1e-12 {
            return None;
        }
        m.swap(col, pivot);
        let pivot_row = m[col];
        for (row, line) in m.iter_mut().enumerate() {
            if row != col {
                let factor = line[col] / pivot_row[col];
                for (cell, &p) in line.iter_mut().zip(&pivot_row).skip(col) {
                    *cell -= factor * p;
                }
            }
        }
    }
    let mut h = [0.0; 8];
    for i in 0..8 {
        h[i] = m[i][8] / m[i][i];
    }
    Some(h)
}

/// An RGB picture, three bytes a pixel, row by row.
#[derive(Clone)]
pub struct Rgb {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl Rgb {
    /// Bilinear sample at (x, y), with the edge pixels repeated outwards.
    fn sample(&self, x: f32, y: f32) -> [f32; 3] {
        let x = x.clamp(0.0, (self.width - 1) as f32);
        let y = y.clamp(0.0, (self.height - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let px = |x: usize, y: usize, c: usize| f32::from(self.data[(y * self.width + x) * 3 + c]);
        [0, 1, 2].map(|c| {
            let top = px(x0, y0, c) * (1.0 - fx) + px(x1, y0, c) * fx;
            let bottom = px(x0, y1, c) * (1.0 - fx) + px(x1, y1, c) * fx;
            top * (1.0 - fy) + bottom * fy
        })
    }

    /// Resize, sampling pixel centres as OpenCV's linear resize does.
    pub fn resized(&self, width: usize, height: usize) -> Rgb {
        let (sx, sy) = (self.width as f32 / width as f32, self.height as f32 / height as f32);
        let mut data = Vec::with_capacity(width * height * 3);
        for y in 0..height {
            let fy = (y as f32 + 0.5) * sy - 0.5;
            for x in 0..width {
                let fx = (x as f32 + 0.5) * sx - 0.5;
                data.extend(self.sample(fx, fy).map(|v| v.round().clamp(0.0, 255.0) as u8));
            }
        }
        Rgb { width, height, data }
    }

    /// Cut `quad` out as a straight strip as wide as its longer top or bottom
    /// edge and as tall as its longer side, turned upright if it stands taller
    /// than wide by half again (a vertical line of text).
    pub fn crop(&self, quad: &[Point; 4]) -> Option<(Rgb, bool)> {
        let width = distance(quad[0], quad[1]).max(distance(quad[2], quad[3])) as usize;
        let height = distance(quad[0], quad[3]).max(distance(quad[1], quad[2])) as usize;
        if width == 0 || height == 0 {
            return None;
        }
        let h = homography(quad, width as f32, height as f32)?;
        let mut data = Vec::with_capacity(width * height * 3);
        for y in 0..height {
            for x in 0..width {
                let (fx, fy) = (x as f64, y as f64);
                let w = h[6] * fx + h[7] * fy + 1.0;
                let u = (h[0] * fx + h[1] * fy + h[2]) / w;
                let v = (h[3] * fx + h[4] * fy + h[5]) / w;
                // Linear, not the reference's cubic: on the hardest pictures
                // the two differ by a letter now and then, and linear came
                // out a little ahead when measured.
                data.extend(self.sample(u as f32, v as f32).map(|c| c.round().clamp(0.0, 255.0) as u8));
            }
        }
        let strip = Rgb { width, height, data };
        if height as f32 / width as f32 >= 1.5 {
            Some((strip.turned_left(), true))
        } else {
            Some((strip, false))
        }
    }

    /// A quarter turn anticlockwise, as NumPy's `rot90`.
    fn turned_left(&self) -> Rgb {
        let (w, h) = (self.width, self.height);
        let mut data = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let (nx, ny) = (y, w - 1 - x);
                data[(ny * h + nx) * 3..(ny * h + nx) * 3 + 3].copy_from_slice(&self.data[(y * w + x) * 3..(y * w + x) * 3 + 3]);
            }
        }
        Rgb { width: h, height: w, data }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_blobs_are_two_and_corners_join() {
        // An L, a separate dot, and two pixels touching only at a corner.
        let bitmap = [
            1, 0, 0, 0, 1, //
            1, 0, 0, 0, 0, //
            1, 1, 0, 1, 0, //
            0, 0, 0, 0, 1, //
        ];
        let found = blobs(&bitmap, 5, 4, 100);
        assert_eq!(found.len(), 3);
        assert_eq!(blobs(&bitmap, 5, 4, 2).len(), 2, "the limit holds");
    }

    #[test]
    fn the_smallest_rectangle_around_a_slanted_bar_is_slanted() {
        // A bar 40 long and 6 thick, at 30 degrees.
        let (angle, mut points) = (30f32.to_radians(), Vec::new());
        for i in 0..=40 {
            for j in 0..=6 {
                let (a, b) = (i as f32, j as f32);
                points.push([a * angle.cos() - b * angle.sin(), a * angle.sin() + b * angle.cos()]);
            }
        }
        let rect = min_area_rect(&points);
        assert!((rect.width.max(rect.height) - 40.0).abs() < 0.5, "{rect:?}");
        assert!((rect.short_side() - 6.0).abs() < 0.5, "{rect:?}");
        let grown = rect.grown(2.0);
        assert!((grown.short_side() - 10.0).abs() < 0.5);
    }

    #[test]
    fn corners_come_in_reading_order() {
        let quad = order_clockwise([[10.0, 50.0], [100.0, 10.0], [10.0, 10.0], [100.0, 50.0]]);
        assert_eq!(quad, [[10.0, 10.0], [100.0, 10.0], [100.0, 50.0], [10.0, 50.0]]);
        assert!(inside(&quad, 50.0, 30.0) && inside(&quad, 10.0, 10.0) && !inside(&quad, 5.0, 30.0));
        assert_eq!(area_and_length(&quad), (3600.0, 260.0));
    }

    #[test]
    fn a_straight_crop_is_the_pixels_themselves() {
        let picture = Rgb {
            width: 20,
            height: 10,
            data: (0..200).flat_map(|i| [(i % 20) as u8 * 10, (i / 20) as u8 * 20, 7]).collect(),
        };
        let (strip, turned) = picture.crop(&[[2.0, 3.0], [12.0, 3.0], [12.0, 8.0], [2.0, 8.0]]).unwrap();
        assert!(!turned);
        assert_eq!((strip.width, strip.height), (10, 5));
        assert_eq!(&strip.data[0..3], &[20, 60, 7], "the strip's first pixel is the picture's (2, 3)");
        // Taller than wide: turned to read across.
        let (tall, turned) = picture.crop(&[[2.0, 0.0], [5.0, 0.0], [5.0, 9.0], [2.0, 9.0]]).unwrap();
        assert!(turned);
        assert_eq!((tall.width, tall.height), (9, 3));
    }
}
