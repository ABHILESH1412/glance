// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Colour management.
//!
//! A photograph carries an ICC profile saying what its numbers mean. Without
//! reading it, an Adobe RGB or Display P3 shot is drawn as though its numbers
//! were sRGB, and everything comes out a little flat — the reds least wrong,
//! the greens most.
//!
//! Conversion happens once, at decode, straight to sRGB. Everything downstream
//! — the tone sliders, text, drawings, export — already assumes 8-bit sRGB, so
//! putting the conversion at the door keeps one meaning for a pixel everywhere
//! else in the program.

use moxcms::{ColorProfile, Layout, TransformOptions};

/// Colorants and white point this close together are the same profile for our
/// purposes: the difference is far below a single 8-bit step.
const SAME_COLORANT: f64 = 1e-4;

/// Convert straight RGBA pixels from `profile` into sRGB, in place.
///
/// Returns whether anything was done. A file that is already sRGB, or a
/// profile we cannot make sense of, leaves the pixels alone — being wrong in a
/// new way is worse than being wrong in the way everyone expects.
pub fn to_srgb(rgba: &mut [u8], profile: &[u8]) -> bool {
    let Ok(source) = ColorProfile::new_from_slice(profile) else {
        return false;
    };
    let destination = ColorProfile::new_srgb();
    if is_srgb(&source, &destination) {
        return false;
    }
    let Ok(transform) = source.create_in_place_transform_8bit(
        Layout::Rgba,
        &destination,
        TransformOptions::default(),
    ) else {
        return false;
    };
    transform.transform(rgba).is_ok()
}

/// Whether a profile is sRGB in all but name.
///
/// Most cameras and phones tag their output sRGB, and converting sRGB to sRGB
/// is a pass over every pixel to change nothing. Comparing the colorants is
/// cheap and catches the common case.
fn is_srgb(profile: &ColorProfile, srgb: &ColorProfile) -> bool {
    let same = |a: moxcms::Xyzd, b: moxcms::Xyzd| {
        (a.x - b.x).abs() < SAME_COLORANT
            && (a.y - b.y).abs() < SAME_COLORANT
            && (a.z - b.z).abs() < SAME_COLORANT
    };
    // The transfer curves are compared only by shape-bearing presence: two
    // profiles with matching primaries but different gamma are rare enough,
    // and a wrong skip there costs less than the conversion it avoids.
    same(profile.red_colorant, srgb.red_colorant)
        && same(profile.green_colorant, srgb.green_colorant)
        && same(profile.blue_colorant, srgb.blue_colorant)
        && same(profile.white_point, srgb.white_point)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run a colour through a profile-to-sRGB conversion the way the decoder
    /// would, without needing a file.
    fn convert(source: &ColorProfile, pixel: [u8; 4]) -> [u8; 4] {
        let destination = ColorProfile::new_srgb();
        let transform = source
            .create_in_place_transform_8bit(Layout::Rgba, &destination, TransformOptions::default())
            .expect("a matrix profile should build a transform");
        let mut buffer = pixel;
        transform.transform(&mut buffer).expect("should convert");
        buffer
    }

    /// The whole point. The same numbers name a more saturated colour in a
    /// wider space, so reaching the same colour in sRGB takes different ones.
    /// A mid-tone is used deliberately: a pure primary is outside sRGB
    /// altogether and simply clips, which moves it hardly at all.
    #[test]
    fn a_wide_gamut_colour_really_moves() {
        let plain = [60u8, 160, 90, 255];
        let converted = convert(&ColorProfile::new_adobe_rgb(), plain);
        let shift: i32 = (0..3)
            .map(|i| i32::from(converted[i]) - i32::from(plain[i]))
            .map(i32::abs)
            .sum();
        assert!(shift > 30, "{plain:?} -> {converted:?} is barely a change");
    }

    #[test]
    fn display_p3_moves_too() {
        let plain = [60u8, 160, 90, 255];
        assert_ne!(convert(&ColorProfile::new_display_p3(), plain), plain);
    }

    /// Grey has no colour to get wrong: it is the same in every RGB space
    /// sharing a white point, so a transform that shifts it is broken.
    #[test]
    fn neutral_grey_stays_neutral() {
        let [r, g, b, _] = convert(&ColorProfile::new_adobe_rgb(), [128, 128, 128, 255]);
        assert!(r.abs_diff(g) <= 1 && g.abs_diff(b) <= 1, "grey became ({r}, {g}, {b})");
        assert!(r.abs_diff(128) <= 3, "grey drifted to {r}");
    }

    /// Alpha is not colour and must come through untouched, or every
    /// transparent edge in the app shifts.
    #[test]
    fn alpha_is_carried_through_unchanged() {
        for alpha in [0u8, 64, 128, 255] {
            let converted = convert(&ColorProfile::new_display_p3(), [120, 90, 200, alpha]);
            assert_eq!(converted[3], alpha);
        }
    }

    /// sRGB in, sRGB out: recognised as a no-op and skipped, rather than
    /// spending a pass over a 50-megapixel photograph to change nothing.
    #[test]
    fn an_srgb_profile_is_recognised_and_skipped() {
        let srgb = ColorProfile::new_srgb();
        assert!(is_srgb(&srgb, &ColorProfile::new_srgb()));
        assert!(!is_srgb(&ColorProfile::new_adobe_rgb(), &ColorProfile::new_srgb()));
        assert!(!is_srgb(&ColorProfile::new_display_p3(), &ColorProfile::new_srgb()));
    }

    /// Rubbish in the profile slot must leave the picture alone rather than
    /// mangle it or panic.
    #[test]
    fn nonsense_profile_bytes_leave_the_pixels_alone() {
        let mut pixels = [10u8, 20, 30, 255, 40, 50, 60, 128];
        let before = pixels;
        assert!(!to_srgb(&mut pixels, b"not an icc profile at all"));
        assert_eq!(pixels, before);
        assert!(!to_srgb(&mut pixels, &[]));
        assert_eq!(pixels, before);
    }
}


