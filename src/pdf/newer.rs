// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Poppler calls newer than the oldest Poppler Glance runs on.
//!
//! Drawing on a page needs ink annotations, from Poppler 25.06; text in a font
//! of your choosing needs 24.12. Calling them directly would stop Glance from
//! starting at all on an older Poppler, so they are looked up by name while it
//! runs, once. Where one is missing, only that tool is unavailable.

use std::ffi::{c_char, c_void, CStr};
use std::sync::OnceLock;

use poppler::ffi::{PopplerAnnot, PopplerColor, PopplerDocument, PopplerFontDescription, PopplerPoint, PopplerRectangle};

pub struct Ink {
    pub new: unsafe extern "C" fn(*mut PopplerDocument, *mut PopplerRectangle) -> *mut PopplerAnnot,
    pub path_new: unsafe extern "C" fn(*mut PopplerPoint, usize) -> *mut c_void,
    pub path_free: unsafe extern "C" fn(*mut c_void),
    pub path_points: unsafe extern "C" fn(*mut c_void, *mut usize) -> *mut PopplerPoint,
    pub set_list: unsafe extern "C" fn(*mut PopplerAnnot, *mut *mut c_void, usize),
}

pub struct Fonts {
    pub desc_new: unsafe extern "C" fn(*const c_char) -> *mut PopplerFontDescription,
    pub desc_free: unsafe extern "C" fn(*mut PopplerFontDescription),
    pub set_desc: unsafe extern "C" fn(*mut PopplerAnnot, *mut PopplerFontDescription),
    pub get_desc: unsafe extern "C" fn(*mut PopplerAnnot) -> *mut PopplerFontDescription,
    pub set_colour: unsafe extern "C" fn(*mut PopplerAnnot, *mut PopplerColor),
    pub get_colour: unsafe extern "C" fn(*mut PopplerAnnot) -> *mut PopplerColor,
}

pub struct Border {
    pub set: unsafe extern "C" fn(*mut PopplerAnnot, f64),
    pub get: unsafe extern "C" fn(*mut PopplerAnnot, *mut f64) -> i32,
}

pub struct Newer {
    pub ink: Option<Ink>,
    pub fonts: Option<Fonts>,
    pub border: Option<Border>,
}

/// A function in a library already loaded, by name.
///
/// # Safety
/// `F` must be a function pointer type matching the C function's signature.
unsafe fn lookup<F: Copy>(name: &CStr) -> Option<F> {
    // SAFETY: dlsym only reads the name; the caller vouches for the type.
    unsafe {
        let found = libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr());
        (!found.is_null()).then(|| std::mem::transmute_copy::<*mut c_void, F>(&found))
    }
}

pub fn get() -> &'static Newer {
    static NEWER: OnceLock<Newer> = OnceLock::new();
    // SAFETY: each type below is the one Poppler's own headers give.
    NEWER.get_or_init(|| unsafe {
        let ink = (|| {
            Some(Ink {
                new: lookup(c"poppler_annot_ink_new")?,
                path_new: lookup(c"poppler_path_new_from_array")?,
                path_free: lookup(c"poppler_path_free")?,
                path_points: lookup(c"poppler_path_get_points")?,
                set_list: lookup(c"poppler_annot_ink_set_ink_list")?,
            })
        })();
        let fonts = (|| {
            Some(Fonts {
                desc_new: lookup(c"poppler_font_description_new")?,
                desc_free: lookup(c"poppler_font_description_free")?,
                set_desc: lookup(c"poppler_annot_free_text_set_font_desc")?,
                get_desc: lookup(c"poppler_annot_free_text_get_font_desc")?,
                set_colour: lookup(c"poppler_annot_free_text_set_font_color")?,
                get_colour: lookup(c"poppler_annot_free_text_get_font_color")?,
            })
        })();
        let border = (|| {
            Some(Border {
                set: lookup(c"poppler_annot_set_border_width")?,
                get: lookup(c"poppler_annot_get_border_width")?,
            })
        })();
        Newer { ink, fonts, border }
    })
}

#[cfg(test)]
mod tests {
    /// Poppler's own version, as (major, minor).
    fn version() -> (u32, u32) {
        // SAFETY: a static string Poppler owns.
        let text = unsafe { std::ffi::CStr::from_ptr(poppler::ffi::poppler_get_version()) }.to_string_lossy().into_owned();
        let mut parts = text.split('.').map(|p| p.parse().unwrap_or(0));
        (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
    }

    #[test]
    fn every_call_is_found_where_poppler_has_it() {
        // Catches a misspelt name: on a Poppler new enough, all are there.
        let newer = super::get();
        let v = version();
        assert_eq!(newer.fonts.is_some(), v >= (24, 12), "Poppler {v:?}");
        assert_eq!(newer.border.is_some(), v >= (24, 12), "Poppler {v:?}");
        assert_eq!(newer.ink.is_some(), v >= (25, 6), "Poppler {v:?}");
    }
}
