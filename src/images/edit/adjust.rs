// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tone, colour and detail: exposure, brightness, contrast, highlights and
//! shadows, saturation, white balance, sepia, sharpness and levels.
//!
//! Most of these are a matrix: each output channel a weighted sum of the
//! input channels plus an offset. Those GTK applies on the GPU, which is what
//! makes dragging a slider instant on a 50-megapixel photo. Highlights and
//! shadows, the midtones of levels, and sharpness are not, so while any of
//! them is in use the picture on screen is drawn by `render`, on a thread,
//! from a smaller copy first and the whole picture after (see `tone`).
//!
//! Either way the saved pixels come from `render` too, and `render` uses the
//! very matrix the GPU is given whenever there is nothing else to do, so the
//! preview and the file cannot disagree about what "contrast" means.
//!
//! The order is the one photo editors use: white balance and exposure first,
//! as if the camera had got them right; then highlights and shadows; then
//! brightness, contrast and saturation; sepia; levels last, so its histogram
//! shows the picture as everything else left it; and sharpening on the result.

use gtk::graphene;
use image::{DynamicImage, RgbaImage};

/// Rec. 709 luma weights, which is what sRGB is defined against. Using a flat
/// average instead would turn saturated reds muddy and greens too bright.
const LUMA: [f64; 3] = [0.2126, 0.7152, 0.0722];

/// Each slider runs from -100 to 100 with 0 in the middle; sepia from 0.
pub const RANGE: f64 = 100.0;

/// How far exposure goes either way, in stops: each doubles or halves the light.
const STOPS: f64 = 2.0;

/// sRGB is close enough to a 2.2 power curve that multiplying the light by k
/// multiplies the stored values by k^(1/2.2). That is what lets exposure and
/// white balance, which act on light, still be a matrix on stored values.
const GAMMA: f64 = 2.2;

/// Classic sepia: every colour mapped onto the brown of an old print.
const SEPIA: [[f64; 3]; 3] = [[0.393, 0.769, 0.189], [0.349, 0.686, 0.168], [0.272, 0.534, 0.131]];

/// How strongly highlights and shadows push at their ends of the scale, in
/// stops. Above about 3.2 the curve could turn back on itself.
const TONE_PUSH: f64 = 2.0;

/// Entries in the tables that stand in for powers and curves, per pixel.
const TABLE: usize = 4096;

/// One output channel per row: three weights and an offset.
type Affine = [[f64; 4]; 3];

const IDENTITY: Affine = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];

/// `a` after `b`.
fn compose(a: &Affine, b: &Affine) -> Affine {
    let mut out = [[0.0; 4]; 3];
    for (row, line) in out.iter_mut().enumerate() {
        for (col, cell) in line.iter_mut().enumerate() {
            *cell = (0..3).map(|j| a[row][j] * b[j][col]).sum::<f64>() + if col == 3 { a[row][3] } else { 0.0 };
        }
    }
    out
}

fn diagonal(gains: [f64; 3], offsets: [f64; 3]) -> Affine {
    let mut m = [[0.0; 4]; 3];
    for c in 0..3 {
        m[c][c] = gains[c];
        m[c][3] = offsets[c];
    }
    m
}

/// Levels for one channel, or all of them: the input values that become black
/// and white, and the midtone's gamma. Black and white run 0..1.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Levels {
    pub black: f64,
    pub gamma: f64,
    pub white: f64,
}

impl Default for Levels {
    fn default() -> Self {
        Levels { black: 0.0, gamma: 1.0, white: 1.0 }
    }
}

impl Levels {
    /// The midtone handle's range, as most editors have it.
    pub const GAMMA_MIN: f64 = 0.1;
    pub const GAMMA_MAX: f64 = 9.99;

    pub fn is_identity(&self) -> bool {
        self.black.abs() < 1e-4 && (self.white - 1.0).abs() < 1e-4 && self.is_linear()
    }

    fn is_linear(&self) -> bool {
        (self.gamma - 1.0).abs() < 1e-3
    }

    /// Never zero, so black and white meeting cannot divide by it.
    fn span(&self) -> f64 {
        (self.white - self.black).max(1.0 / 255.0)
    }

    /// One value through: stretched so black and white land on 0 and 1,
    /// clipped, then bent by the gamma.
    pub fn apply(&self, value: f64) -> f64 {
        let t = ((value - self.black) / self.span()).clamp(0.0, 1.0);
        if self.is_linear() { t } else { t.powf(1.0 / self.gamma) }
    }

    /// Where the midtone handle sits: the input that comes out mid grey.
    pub fn midpoint(&self) -> f64 {
        self.black + self.span() * 0.5f64.powf(self.gamma)
    }

    /// The gamma that puts the midtone handle at `input`.
    pub fn gamma_for_midpoint(&self, input: f64) -> f64 {
        let t = ((input - self.black) / self.span()).clamp(0.001, 0.999);
        (t.ln() / 0.5f64.ln()).clamp(Self::GAMMA_MIN, Self::GAMMA_MAX)
    }

    /// The stretch alone, as a matrix row for channel `c`.
    fn affine(&self) -> (f64, f64) {
        (1.0 / self.span(), -self.black / self.span())
    }
}

/// How many pixels have each value, per channel: red, green, blue, and luma.
#[derive(Clone, Debug, PartialEq)]
pub struct Histogram {
    pub counts: [[u32; 256]; 4],
}

impl Histogram {
    pub const LUMA: usize = 3;

    /// Auto Levels: each channel stretched so its darkest and brightest
    /// thousandth of the pixels become black and white. Stretching the
    /// channels separately is also what takes out a colour cast.
    pub fn auto_levels(&self) -> [Levels; 4] {
        let mut levels = [Levels::default(); 4];
        for (channel, counts) in self.counts.iter().take(3).enumerate() {
            let total: u64 = counts.iter().map(|&n| u64::from(n)).sum();
            if total == 0 {
                continue;
            }
            let clip = total / 1000;
            let find = |values: &mut dyn Iterator<Item = usize>| {
                let mut seen = 0u64;
                for value in values {
                    seen += u64::from(counts[value]);
                    if seen > clip {
                        return value;
                    }
                }
                0
            };
            let low = find(&mut (0..256));
            let high = find(&mut (0..256).rev());
            // A channel with almost no range (a flat colour) is left alone
            // rather than blown up into noise.
            if high > low + 8 {
                levels[channel + 1] = Levels { black: low as f64 / 255.0, gamma: 1.0, white: high as f64 / 255.0 };
            }
        }
        levels
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Adjustments {
    pub exposure: f64,
    pub brightness: f64,
    pub contrast: f64,
    pub highlights: f64,
    pub shadows: f64,
    pub saturation: f64,
    pub temperature: f64,
    pub tint: f64,
    /// 0 to 100: how far towards an old brown print.
    pub sepia: f64,
    /// Below zero softens, above sharpens.
    pub sharpness: f64,
    /// All channels together first in the list, then red, green and blue.
    /// Each channel's own levels apply first, then the shared ones.
    pub levels: [Levels; 4],
}

impl Adjustments {
    pub fn is_identity(&self) -> bool {
        self.sliders_are_identity() && self.levels_are_identity()
    }

    /// The sliders alone, leaving levels out.
    pub fn sliders_are_identity(&self) -> bool {
        [
            self.exposure,
            self.brightness,
            self.contrast,
            self.highlights,
            self.shadows,
            self.saturation,
            self.temperature,
            self.tint,
            self.sepia,
            self.sharpness,
        ]
        .iter()
        .all(|value| value.abs() < 0.01)
    }

    pub fn levels_are_identity(&self) -> bool {
        self.levels.iter().all(Levels::is_identity)
    }

    fn has_tone_curve(&self) -> bool {
        self.highlights.abs() >= 0.01 || self.shadows.abs() >= 0.01
    }

    fn colour_is_affine(&self) -> bool {
        !self.has_tone_curve() && self.levels.iter().all(Levels::is_linear)
    }

    /// Whether the GPU's colour matrix shows all of this. When it does not,
    /// the picture on screen has to come from `render`.
    pub fn shows_on_gpu(&self) -> bool {
        self.colour_is_affine() && self.sharpness.abs() < 0.01
    }

    /// White balance and exposure, as gains on stored values. Warmer is more
    /// red and less blue; more tint is more magenta, less is greener. The
    /// gains are balanced so a grey keeps its brightness.
    fn light(&self) -> Affine {
        let t = self.temperature / RANGE;
        let m = self.tint / RANGE;
        let linear = [2f64.powf(0.5 * t), 2f64.powf(-0.4 * m), 2f64.powf(-0.5 * t)];
        let stored = linear.map(|gain| gain.powf(1.0 / GAMMA));
        let grey: f64 = (0..3).map(|c| LUMA[c] * stored[c]).sum();
        let exposure = 2f64.powf(self.exposure / RANGE * STOPS / GAMMA);
        diagonal(stored.map(|gain| gain / grey * exposure), [0.0; 3])
    }

    /// Contrast doubles at +100 and halves at -100, so the slider covers the
    /// same ratio either side of the middle rather than being lopsided.
    fn contrast_factor(&self) -> f64 {
        2f64.powf(self.contrast / RANGE)
    }

    /// 0 is grey, 1 is untouched, 2 is twice as colourful.
    fn saturation_factor(&self) -> f64 {
        1.0 + self.saturation / RANGE
    }

    /// Brightness, contrast and saturation, then sepia.
    ///
    /// Saturation mixes each channel towards the luma, contrast scales what
    /// comes out about mid grey, and brightness shifts it.
    fn tone(&self) -> Affine {
        let k = self.saturation_factor();
        let f = self.contrast_factor();
        // out = f * (sat(in) - 0.5) + 0.5 + brightness
        let offset = 0.5 - 0.5 * f + self.brightness / RANGE;
        let mut m = [[0.0; 4]; 3];
        for (row, line) in m.iter_mut().enumerate() {
            for col in 0..3 {
                let identity = if row == col { 1.0 } else { 0.0 };
                line[col] = f * (LUMA[col] + k * (identity - LUMA[col]));
            }
            line[3] = offset;
        }
        let s = (self.sepia / RANGE).clamp(0.0, 1.0);
        let mut sepia = IDENTITY;
        for (row, line) in sepia.iter_mut().enumerate() {
            for col in 0..3 {
                line[col] = (1.0 - s) * line[col] + s * SEPIA[row][col];
            }
        }
        compose(&sepia, &m)
    }

    /// Levels' stretches, each channel's own and then the shared one, with
    /// the gammas left out.
    fn levels_affine(&self) -> Affine {
        let (shared_gain, shared_offset) = self.levels[0].affine();
        let mut gains = [0.0; 3];
        let mut offsets = [0.0; 3];
        for c in 0..3 {
            let (gain, offset) = self.levels[c + 1].affine();
            gains[c] = shared_gain * gain;
            offsets[c] = shared_gain * offset + shared_offset;
        }
        diagonal(gains, offsets)
    }

    /// Everything that is a matrix, as one. Exact when `colour_is_affine`;
    /// otherwise the nearest the GPU can show while `render` catches up.
    fn affine(&self) -> Affine {
        compose(&self.levels_affine(), &compose(&self.tone(), &self.light()))
    }

    /// The matrix in the form `gtk::Snapshot::push_color_matrix` wants.
    ///
    /// Alpha is left alone: these are tone controls, not an opacity slider.
    pub fn colour_matrix(&self) -> (graphene::Matrix, graphene::Vec4) {
        let m = self.affine();
        let c = |row: usize, col: usize| m[row][col] as f32;
        // graphene multiplies a row vector by the matrix, so the coefficients
        // for one output channel run down a column rather than across a row.
        let matrix = graphene::Matrix::from_float([
            c(0, 0), c(1, 0), c(2, 0), 0.0, //
            c(0, 1), c(1, 1), c(2, 1), 0.0, //
            c(0, 2), c(1, 2), c(2, 2), 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ]);
        (matrix, graphene::Vec4::new(c(0, 3), c(1, 3), c(2, 3), 0.0))
    }

    /// Bake the adjustments into pixels. Straight alpha in, straight alpha out.
    pub fn bake(&self, image: DynamicImage) -> DynamicImage {
        if self.is_identity() {
            return image;
        }
        let mut rgba = image.into_rgba8();
        let longest = rgba.width().max(rgba.height());
        self.render(&mut rgba, longest);
        DynamicImage::ImageRgba8(rgba)
    }

    /// Apply everything to `rgba`, which may be a smaller copy of a picture
    /// whose longest side is `full_longest`: sharpening is scaled to match, so
    /// a preview shows what saving will do.
    pub fn render(&self, rgba: &mut RgbaImage, full_longest: u32) {
        let prepared = Prepared::new(self);
        let width = rgba.width() as usize;
        parallel_rows(rgba.as_mut(), width * 4, |_, rows| {
            for pixel in rows.chunks_exact_mut(4) {
                let out = prepared.colour(byte_rgb(pixel));
                for c in 0..3 {
                    pixel[c] = to_byte(out[c]);
                }
            }
        });
        if self.sharpness.abs() >= 0.01 {
            let longest = rgba.width().max(rgba.height());
            let sigma = detail_radius(full_longest) * f64::from(longest) / f64::from(full_longest.max(1));
            let amount = self.sharpness / RANGE;
            let amount = if amount > 0.0 { amount * 2.0 } else { amount };
            sharpen(rgba, amount as f32, sigma);
        }
    }

    /// The histogram levels works on: the picture with everything but levels
    /// and sharpening done to it. Fully transparent pixels do not count.
    pub fn histogram(&self, rgba: &RgbaImage) -> Histogram {
        let prepared = Prepared::new(self);
        let mut counts = [[0u32; 256]; 4];
        for pixel in rgba.as_raw().chunks_exact(4) {
            if pixel[3] == 0 {
                continue;
            }
            let rgb = prepared.before_levels(byte_rgb(pixel));
            let mut luma = 0.0;
            for c in 0..3 {
                counts[c][usize::from(to_byte(rgb[c]))] += 1;
                luma += LUMA[c] as f32 * rgb[c];
            }
            counts[Histogram::LUMA][usize::from(to_byte(luma))] += 1;
        }
        Histogram { counts }
    }
}

fn byte_rgb(pixel: &[u8]) -> [f32; 3] {
    [0, 1, 2].map(|c| f32::from(pixel[c]) / 255.0)
}

fn to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Sharpening's radius on the full picture: a pixel and a bit, more on a big
/// photo, so the slider does something visible at the size people look at it.
fn detail_radius(longest: u32) -> f64 {
    (f64::from(longest) / 2500.0).max(1.0)
}

/// A function of 0..1 as a table, looked up with straight-line steps between
/// entries: a power or a curve costs the same as a multiply that way.
struct Table(Vec<f32>);

impl Table {
    fn new(f: impl Fn(f64) -> f64) -> Self {
        Table((0..=TABLE).map(|i| f(i as f64 / TABLE as f64) as f32).collect())
    }

    fn get(&self, t: f32) -> f32 {
        let x = t.clamp(0.0, 1.0) * TABLE as f32;
        let i = (x as usize).min(TABLE - 1);
        let f = x - i as f32;
        self.0[i] + (self.0[i + 1] - self.0[i]) * f
    }
}

/// One level, ready to run per pixel.
struct LevelsRun {
    gain: f32,
    offset: f32,
    gamma: Option<Table>,
}

impl LevelsRun {
    fn new(levels: &Levels) -> Self {
        let (gain, offset) = levels.affine();
        let gamma = (!levels.is_linear()).then(|| {
            let g = levels.gamma;
            Table::new(move |t| t.powf(1.0 / g))
        });
        LevelsRun { gain: gain as f32, offset: offset as f32, gamma }
    }

    fn apply(&self, value: f32) -> f32 {
        let t = (value * self.gain + self.offset).clamp(0.0, 1.0);
        match &self.gamma {
            Some(table) => table.get(t),
            None => t,
        }
    }
}

/// The adjustments worked out once, for a picture's worth of pixels.
struct Prepared {
    /// When the whole colour pipeline is one matrix, just that.
    affine: Option<[[f32; 4]; 3]>,
    light: [[f32; 4]; 3],
    /// Highlights and shadows: luma in, luma out.
    curve: Option<Table>,
    tone: [[f32; 4]; 3],
    /// Each channel's own, then the shared one.
    levels: [LevelsRun; 4],
}

fn single(m: &Affine) -> [[f32; 4]; 3] {
    m.map(|row| row.map(|v| v as f32))
}

fn transform(m: &[[f32; 4]; 3], v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|c| m[c][0] * v[0] + m[c][1] * v[1] + m[c][2] * v[2] + m[c][3])
}

/// Highlights and shadows as one curve over luma. Shadows multiplies the dark
/// end by up to two stops either way and fades out towards the light;
/// highlights does the same to the distance from white. Black and white stay
/// where they are, and the curve never turns back on itself, so no two tones
/// swap places.
fn tone_curve(shadows: f64, highlights: f64) -> impl Fn(f64) -> f64 {
    move |y: f64| {
        let lifted = y * 2f64.powf(TONE_PUSH * shadows * (1.0 - y).powi(3));
        let gap = 1.0 - lifted;
        1.0 - gap * 2f64.powf(-TONE_PUSH * highlights * lifted.powi(3))
    }
}

impl Prepared {
    fn new(adjust: &Adjustments) -> Self {
        let curve = adjust.has_tone_curve().then(|| {
            Table::new(tone_curve(adjust.shadows / RANGE, adjust.highlights / RANGE))
        });
        Prepared {
            affine: adjust.colour_is_affine().then(|| single(&adjust.affine())),
            light: single(&adjust.light()),
            curve,
            tone: single(&adjust.tone()),
            levels: [0, 1, 2, 3].map(|i| LevelsRun::new(&adjust.levels[i])),
        }
    }

    fn before_levels(&self, rgb: [f32; 3]) -> [f32; 3] {
        let mut v = transform(&self.light, rgb);
        if let Some(curve) = &self.curve {
            // The curve moves luma; the colour keeps its proportions by
            // scaling all three channels together.
            let y = LUMA[0] as f32 * v[0] + LUMA[1] as f32 * v[1] + LUMA[2] as f32 * v[2];
            let clipped = y.clamp(0.0, 1.0);
            let wanted = curve.get(clipped) + (y - clipped);
            if y > 1e-5 {
                let ratio = wanted / y;
                v = v.map(|c| c * ratio);
            } else {
                v = v.map(|c| c + (wanted - y));
            }
        }
        transform(&self.tone, v)
    }

    fn colour(&self, rgb: [f32; 3]) -> [f32; 3] {
        if let Some(m) = &self.affine {
            return transform(m, rgb);
        }
        let v = self.before_levels(rgb);
        [0, 1, 2].map(|c| self.levels[0].apply(self.levels[c + 1].apply(v[c])))
    }
}

/// Run `f` over `data` in bands of whole rows, one band per core or so.
/// `f` is told the first row of its band.
fn parallel_rows<T: Send>(data: &mut [T], row_len: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    if row_len == 0 || data.is_empty() {
        return;
    }
    let rows = data.len() / row_len;
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let band = rows.div_ceil(threads * 2).max(16);
    std::thread::scope(|scope| {
        for (i, chunk) in data.chunks_mut(band * row_len).enumerate() {
            let f = &f;
            scope.spawn(move || f(i * band, chunk));
        }
    });
}

/// Unsharp masking on luma: each pixel pushed away from (or, with a negative
/// amount, towards) the blurred brightness around it. Working on luma alone
/// sharpens edges without fringing them with colour.
fn sharpen(rgba: &mut RgbaImage, amount: f32, sigma: f64) {
    // Under about a third of a pixel the blur is no blur, and nor is this.
    if sigma < 0.35 {
        return;
    }
    let (width, height) = (rgba.width() as usize, rgba.height() as usize);
    let radius = (sigma * 3.0).ceil() as isize;
    let mut kernel: Vec<f32> = (-radius..=radius).map(|i| (-(i * i) as f64 / (2.0 * sigma * sigma)).exp() as f32).collect();
    let total: f32 = kernel.iter().sum();
    kernel.iter_mut().for_each(|k| *k /= total);
    let luma = |p: &[u8]| LUMA[0] as f32 * f32::from(p[0]) + LUMA[1] as f32 * f32::from(p[1]) + LUMA[2] as f32 * f32::from(p[2]);

    // Across first, into a buffer of its own.
    let mut across = vec![0f32; width * height];
    {
        let source = rgba.as_raw();
        parallel_rows(&mut across, width, |first, rows| {
            for (r, out) in rows.chunks_exact_mut(width).enumerate() {
                let line = &source[(first + r) * width * 4..(first + r + 1) * width * 4];
                for (x, cell) in out.iter_mut().enumerate() {
                    let mut sum = 0.0;
                    for (k, weight) in kernel.iter().enumerate() {
                        let sx = (x as isize + k as isize - radius).clamp(0, width as isize - 1) as usize;
                        sum += weight * luma(&line[sx * 4..sx * 4 + 3]);
                    }
                    *cell = sum;
                }
            }
        });
    }
    // Then down, straight into the pixels.
    let across = &across;
    parallel_rows(rgba.as_mut(), width * 4, |first, rows| {
        for (r, line) in rows.chunks_exact_mut(width * 4).enumerate() {
            let y = (first + r) as isize;
            for x in 0..width {
                let mut blurred = 0.0;
                for (k, weight) in kernel.iter().enumerate() {
                    let sy = (y + k as isize - radius).clamp(0, height as isize - 1) as usize;
                    blurred += weight * across[sy * width + x];
                }
                let pixel = &mut line[x * 4..x * 4 + 4];
                let push = amount * (luma(pixel) - blurred);
                for channel in &mut pixel[..3] {
                    *channel = (f32::from(*channel) + push).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn one_pixel(r: u8, g: u8, b: u8) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, Rgba([r, g, b, 200])))
    }

    fn pixel_of(image: &DynamicImage) -> [u8; 4] {
        image.to_rgba8().get_pixel(0, 0).0
    }

    fn baked(adjust: Adjustments, rgb: [u8; 3]) -> [u8; 3] {
        let [r, g, b, _] = pixel_of(&adjust.bake(one_pixel(rgb[0], rgb[1], rgb[2])));
        [r, g, b]
    }

    /// What the GPU would show: the matrix, applied by hand.
    fn on_gpu(adjust: &Adjustments, rgb: [u8; 3]) -> [u8; 3] {
        let m = adjust.affine();
        let v = rgb.map(|c| f64::from(c) / 255.0);
        [0, 1, 2].map(|c| {
            let out = m[c][0] * v[0] + m[c][1] * v[1] + m[c][2] * v[2] + m[c][3];
            (out.clamp(0.0, 1.0) * 255.0).round() as u8
        })
    }

    #[test]
    fn doing_nothing_changes_nothing() {
        let adjust = Adjustments::default();
        assert!(adjust.is_identity());
        assert!(adjust.shows_on_gpu());
        assert_eq!(pixel_of(&adjust.bake(one_pixel(10, 120, 250))), [10, 120, 250, 200]);
    }

    /// Fully desaturated, every channel must land on the luma of the original
    /// — which is the whole reason for weighting them rather than averaging.
    #[test]
    fn full_desaturation_lands_on_the_luma() {
        let adjust = Adjustments { saturation: -RANGE, ..Default::default() };
        let [r, g, b, a] = pixel_of(&adjust.bake(one_pixel(255, 0, 0)));
        let expected = (LUMA[0] * 255.0).round() as u8;
        assert_eq!([r, g, b], [expected; 3]);
        // Alpha is a tone control's business to leave alone.
        assert_eq!(a, 200);
    }

    /// Contrast pivots about mid grey, so mid grey itself must not move.
    #[test]
    fn contrast_leaves_mid_grey_where_it_is() {
        for contrast in [-RANGE, -40.0, 40.0, RANGE] {
            let adjust = Adjustments { contrast, ..Default::default() };
            let [r, ..] = baked(adjust, [128; 3]);
            assert!((i16::from(r) - 128).abs() <= 1, "contrast {contrast} moved mid grey to {r}");
        }
    }

    #[test]
    fn brightness_moves_every_channel_by_the_same_amount() {
        let adjust = Adjustments { brightness: 20.0, ..Default::default() };
        assert_eq!(baked(adjust, [50, 100, 150]), [101, 151, 201]);
    }

    /// Values must not wrap when a slider pushes them past the ends.
    #[test]
    fn extremes_clamp_rather_than_wrap() {
        let up = Adjustments { brightness: RANGE, ..Default::default() };
        assert_eq!(baked(up, [200; 3])[0], 255);
        let down = Adjustments { brightness: -RANGE, ..Default::default() };
        assert_eq!(baked(down, [40; 3])[0], 0);
    }

    /// One stop more is twice the light: a mid grey of 0.5 stored is about
    /// 0.22 of the light, and twice that is stored as about 0.68.
    #[test]
    fn a_stop_of_exposure_doubles_the_light() {
        let adjust = Adjustments { exposure: RANGE / STOPS, ..Default::default() };
        let [r, g, b] = baked(adjust, [128; 3]);
        assert_eq!([r, g], [g, b], "grey stays grey");
        let expected = (0.5f64.powf(GAMMA) * 2.0).powf(1.0 / GAMMA) * 255.0;
        assert!((f64::from(r) - expected).abs() <= 1.5, "{r} against {expected}");
        assert_eq!(baked(adjust, [0; 3]), [0; 3], "black has no light to double");
    }

    #[test]
    fn warmer_is_redder_and_tint_is_magenta() {
        let warm = baked(Adjustments { temperature: 60.0, ..Default::default() }, [128; 3]);
        assert!(warm[0] > 128 && warm[2] < 128, "{warm:?}");
        let cool = baked(Adjustments { temperature: -60.0, ..Default::default() }, [128; 3]);
        assert!(cool[0] < 128 && cool[2] > 128, "{cool:?}");
        let magenta = baked(Adjustments { tint: 60.0, ..Default::default() }, [128; 3]);
        assert!(magenta[1] < magenta[0] && magenta[1] < magenta[2], "{magenta:?}");
        // The grey keeps roughly its brightness.
        let luma = |p: [u8; 3]| (0..3).map(|c| LUMA[c] * f64::from(p[c])).sum::<f64>();
        assert!((luma(warm) - 128.0).abs() < 4.0);
    }

    #[test]
    fn full_sepia_is_the_classic_brown() {
        let adjust = Adjustments { sepia: RANGE, ..Default::default() };
        let [r, g, b] = baked(adjust, [100; 3]);
        assert!(r > g && g > b, "{:?}", [r, g, b]);
        assert_eq!(r, (100.0f64 * (0.393 + 0.769 + 0.189)).round() as u8);
    }

    /// Shadows lift the dark without moving black, white or the light end
    /// much; highlights do the opposite.
    #[test]
    fn highlights_and_shadows_work_on_their_own_ends() {
        let shadows = Adjustments { shadows: RANGE, ..Default::default() };
        assert!(!shadows.shows_on_gpu());
        assert!(baked(shadows, [40; 3])[0] > 60);
        assert!(baked(shadows, [230; 3])[0] <= 231);
        assert_eq!(baked(shadows, [0; 3]), [0; 3]);
        assert_eq!(baked(shadows, [255; 3]), [255; 3]);
        let highlights = Adjustments { highlights: -RANGE, ..Default::default() };
        assert!(baked(highlights, [235; 3])[0] < 215);
        assert!(baked(highlights, [40; 3])[0] >= 39);
        // Colour keeps its proportions.
        let [r, g, _] = baked(shadows, [60, 30, 0]);
        assert!((f64::from(r) / f64::from(g) - 2.0).abs() < 0.1);
    }

    #[test]
    fn the_tone_curve_never_turns_back() {
        for (s, h) in [(1.0, 1.0), (-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0)] {
            let curve = tone_curve(s, h);
            let mut last = curve(0.0);
            for i in 1..=1000 {
                let y = curve(f64::from(i) / 1000.0);
                assert!(y >= last - 1e-9, "shadows {s}, highlights {h} at {i}");
                last = y;
            }
            assert!(curve(0.0).abs() < 1e-9 && (curve(1.0) - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn levels_stretch_and_bend() {
        let mut adjust = Adjustments::default();
        adjust.levels[0] = Levels { black: 50.0 / 255.0, gamma: 1.0, white: 200.0 / 255.0 };
        assert!(adjust.shows_on_gpu(), "a stretch alone is a matrix");
        assert_eq!(baked(adjust, [50, 126, 200]), [0, 129, 255]);
        assert_eq!(baked(adjust, [10, 10, 250]), [0, 0, 255]);
        adjust.levels[0].gamma = 2.0;
        assert!(!adjust.shows_on_gpu());
        let [_, mid, _] = baked(adjust, [50, 126, 200]);
        assert!((i16::from(mid) - ((76.0f64 / 150.0).sqrt() * 255.0).round() as i16).abs() <= 1, "{mid}");
        // The midtone handle and the gamma agree.
        let levels = adjust.levels[0];
        assert!((levels.gamma_for_midpoint(levels.midpoint()) - 2.0).abs() < 1e-6);
        assert!((levels.apply(levels.midpoint()) - 0.5).abs() < 1e-9);
    }

    /// The matrix the GPU shows and the pixels saved must be the same picture.
    #[test]
    fn the_gpu_and_the_saved_file_agree() {
        let mut adjust = Adjustments {
            exposure: 30.0,
            brightness: -10.0,
            contrast: 25.0,
            saturation: 40.0,
            temperature: -35.0,
            tint: 20.0,
            sepia: 30.0,
            ..Default::default()
        };
        adjust.levels[0] = Levels { black: 0.05, gamma: 1.0, white: 0.9 };
        adjust.levels[2] = Levels { black: 0.1, gamma: 1.0, white: 0.95 };
        assert!(adjust.shows_on_gpu());
        // Step by step, as when something else needs the pixels path.
        let mut steps = Prepared::new(&adjust);
        steps.affine = None;
        for rgb in [[0, 0, 0], [255, 255, 255], [200, 30, 90], [12, 140, 250], [128, 128, 128]] {
            let saved = steps.colour(rgb.map(|c| f32::from(c) / 255.0)).map(to_byte);
            assert_eq!(baked(adjust, rgb), on_gpu(&adjust, rgb));
            let shown = on_gpu(&adjust, rgb);
            for c in 0..3 {
                assert!((i16::from(saved[c]) - i16::from(shown[c])).abs() <= 1, "{rgb:?}: {saved:?} against {shown:?}");
            }
        }
    }

    #[test]
    fn auto_levels_stretches_each_channel() {
        let image = RgbaImage::from_fn(100, 100, |x, _| {
            let v = 60 + (x as u8);
            Rgba([v, v / 2 + 40, v, 255])
        });
        let adjust = Adjustments::default();
        let histogram = adjust.histogram(&image);
        let levels = histogram.auto_levels();
        assert!(levels[0].is_identity(), "the shared levels are left alone");
        assert!((levels[1].black * 255.0 - 60.0).abs() < 1.5 && (levels[1].white * 255.0 - 159.0).abs() < 1.5);
        assert!((levels[2].black * 255.0 - 70.0).abs() < 1.5);
        let stretched = Adjustments { levels, ..Default::default() };
        let mut out = image.clone();
        stretched.render(&mut out, 100);
        let reds: Vec<u8> = out.pixels().map(|p| p.0[0]).collect();
        assert_eq!((reds.iter().min(), reds.iter().max()), (Some(&0), Some(&255)));
    }

    #[test]
    fn sharpening_steepens_an_edge_and_softening_eases_it() {
        let edge = RgbaImage::from_fn(40, 10, |x, _| if x < 20 { Rgba([80, 80, 80, 255]) } else { Rgba([170, 170, 170, 255]) });
        let mut sharp = edge.clone();
        Adjustments { sharpness: 80.0, ..Default::default() }.render(&mut sharp, 40);
        assert!(sharp.get_pixel(19, 5).0[0] < 80 && sharp.get_pixel(20, 5).0[0] > 170);
        assert_eq!(sharp.get_pixel(2, 5).0[0], 80, "flat areas stay flat");
        let mut soft = edge.clone();
        Adjustments { sharpness: -80.0, ..Default::default() }.render(&mut soft, 40);
        assert!(soft.get_pixel(19, 5).0[0] > 80 && soft.get_pixel(20, 5).0[0] < 170);
        assert_eq!(soft.get_pixel(19, 5).0[3], 255);
    }

    #[test]
    fn work_split_across_cores_covers_every_row_once() {
        let mut data = vec![0u32; 1000 * 7];
        parallel_rows(&mut data, 7, |first, rows| {
            for (r, row) in rows.chunks_exact_mut(7).enumerate() {
                row.iter_mut().for_each(|v| *v += (first + r) as u32);
            }
        });
        for (r, row) in data.chunks_exact(7).enumerate() {
            assert!(row.iter().all(|&v| v == r as u32));
        }
    }
}
