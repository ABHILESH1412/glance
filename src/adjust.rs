//! Brightness, contrast and saturation.
//!
//! The same three numbers drive two paths: a colour matrix GTK applies on the
//! GPU, which is what makes dragging a slider instant on a 50-megapixel photo,
//! and a pixel loop that bakes the result when the image is saved. They have to
//! agree, so both evaluate the same matrix rather than each having their own
//! idea of what "contrast" means.

use gtk::graphene;
use image::{DynamicImage, RgbaImage};

/// Rec. 709 luma weights, which is what sRGB is defined against. Using a flat
/// average instead would turn saturated reds muddy and greens too bright.
const LUMA: [f64; 3] = [0.2126, 0.7152, 0.0722];

/// Each slider runs from -100 to 100 with 0 in the middle.
pub const RANGE: f64 = 100.0;

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Adjustments {
    pub brightness: f64,
    pub contrast: f64,
    pub saturation: f64,
}

impl Adjustments {
    pub fn is_identity(&self) -> bool {
        self.brightness.abs() < 0.01 && self.contrast.abs() < 0.01 && self.saturation.abs() < 0.01
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

    /// The transform as a 3x3 matrix and a shared offset, for straight (not
    /// premultiplied) sRGB values in 0..1.
    ///
    /// Saturation mixes each channel towards the luma, contrast scales what
    /// comes out about mid grey, and brightness shifts it.
    fn coefficients(&self) -> ([[f64; 3]; 3], f64) {
        let k = self.saturation_factor();
        let f = self.contrast_factor();
        let mut m = [[0.0; 3]; 3];
        for (row, line) in m.iter_mut().enumerate() {
            for (col, cell) in line.iter_mut().enumerate() {
                let identity = if row == col { 1.0 } else { 0.0 };
                *cell = f * (LUMA[col] + k * (identity - LUMA[col]));
            }
        }
        // out = f * (sat(in) - 0.5) + 0.5 + brightness
        let offset = 0.5 - 0.5 * f + self.brightness / RANGE;
        (m, offset)
    }

    /// The same transform in the form `gtk::Snapshot::push_color_matrix` wants.
    ///
    /// Alpha is left alone: these are tone controls, not an opacity slider.
    pub fn colour_matrix(&self) -> (graphene::Matrix, graphene::Vec4) {
        let (m, offset) = self.coefficients();
        let c = |row: usize, col: usize| m[row][col] as f32;
        // graphene multiplies a row vector by the matrix, so the coefficients
        // for one output channel run down a column rather than across a row.
        let matrix = graphene::Matrix::from_float([
            c(0, 0), c(1, 0), c(2, 0), 0.0, //
            c(0, 1), c(1, 1), c(2, 1), 0.0, //
            c(0, 2), c(1, 2), c(2, 2), 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ]);
        let o = offset as f32;
        (matrix, graphene::Vec4::new(o, o, o, 0.0))
    }

    /// Bake the adjustment into pixels. Straight alpha in, straight alpha out.
    pub fn bake(&self, image: DynamicImage) -> DynamicImage {
        if self.is_identity() {
            return image;
        }
        let (m, offset) = self.coefficients();
        let mut rgba: RgbaImage = image.to_rgba8();
        for pixel in rgba.pixels_mut() {
            let [r, g, b, a] = pixel.0;
            let source = [
                f64::from(r) / 255.0,
                f64::from(g) / 255.0,
                f64::from(b) / 255.0,
            ];
            for channel in 0..3 {
                let value = m[channel][0] * source[0]
                    + m[channel][1] * source[1]
                    + m[channel][2] * source[2]
                    + offset;
                pixel.0[channel] = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            pixel.0[3] = a;
        }
        DynamicImage::ImageRgba8(rgba)
    }
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

    #[test]
    fn doing_nothing_changes_nothing() {
        let adjust = Adjustments::default();
        assert!(adjust.is_identity());
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
            let [r, ..] = pixel_of(&adjust.bake(one_pixel(128, 128, 128)));
            assert!((i16::from(r) - 128).abs() <= 1, "contrast {contrast} moved mid grey to {r}");
        }
    }

    #[test]
    fn brightness_moves_every_channel_by_the_same_amount() {
        let adjust = Adjustments { brightness: 20.0, ..Default::default() };
        let [r, g, b, _] = pixel_of(&adjust.bake(one_pixel(50, 100, 150)));
        assert_eq!([r, g, b], [101, 151, 201]);
    }

    /// Values must not wrap when a slider pushes them past the ends.
    #[test]
    fn extremes_clamp_rather_than_wrap() {
        let up = Adjustments { brightness: RANGE, ..Default::default() };
        assert_eq!(pixel_of(&up.bake(one_pixel(200, 200, 200)))[0], 255);
        let down = Adjustments { brightness: -RANGE, ..Default::default() };
        assert_eq!(pixel_of(&down.bake(one_pixel(40, 40, 40)))[0], 0);
    }
}
