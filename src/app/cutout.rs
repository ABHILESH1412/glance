// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Taking the background off a picture, on this computer.
//!
//! BiRefNet-lite, run by the `glance-ocr` helper, says how sure it is that
//! each pixel is part of the subject; that becomes the picture's alpha. In the
//! editor it is one more edit, undone like any other. From the picture's menu,
//! Copy Subject copies just the subject, trimmed to it.
//!
//! The helper is started for each picture and gone once it has answered, so
//! its gigabyte or so of working memory is never held while nothing is being
//! cut out.

use std::path::{Path, PathBuf};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use image::{DynamicImage, GrayImage, RgbaImage};

use crate::app::window::Window;
use crate::images::canvas;
use crate::images::edit::export;
use crate::live_text::client;
use crate::live_text::install::CutoutPaths;

/// The longest side the helper is given. The model looks at 1024 pixels
/// either way; this is the size its edges are then traced at, before being
/// brought up to the picture's own.
const SEND_SIDE: u32 = 2048;
/// Below this much alpha a pixel does not count as part of the subject, when
/// trimming to it.
const TRIM_ALPHA: u8 = 8;

/// What the subject is wanted for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Use {
    /// The picture being edited loses its background.
    Edit,
    /// The subject goes to the clipboard.
    Copy,
}

/// The picture as it was when the cut-out started, to tell whether it is
/// still the one on screen when the answer comes.
#[derive(Clone, Debug, PartialEq)]
struct Started {
    path: PathBuf,
    history: usize,
    size: (u32, u32),
}

impl Window {
    /// The pill that says the subject is being found.
    pub(crate) fn build_cutout_status(&self) {
        let imp = self.imp();
        #[allow(deprecated)]
        let spinner = gtk::Spinner::builder().spinning(true).build();
        let label = gtk::Label::new(Some("Finding the subject…"));
        let pill = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        pill.add_css_class("osd");
        pill.add_css_class("live-status");
        pill.append(&spinner);
        pill.append(&label);
        let status = &imp.cutout_status;
        status.set_child(Some(&pill));
        status.set_transition_type(gtk::RevealerTransitionType::Crossfade);
        status.set_halign(gtk::Align::Center);
        status.set_valign(gtk::Align::Start);
        status.set_margin_top(12);
        status.set_can_target(false);
        status.set_reveal_child(false);
    }

    /// Take the background off the picture being edited.
    pub(crate) fn remove_background(&self) {
        self.find_subject(Use::Edit);
    }

    /// Copy the subject of the picture on screen, without its background.
    pub(crate) fn copy_subject(&self) {
        self.find_subject(Use::Copy);
    }

    fn find_subject(&self, then: Use) {
        let imp = self.imp();
        if imp.showing_pdf.get() || !imp.view.canvas().has_image() {
            return;
        }
        if imp.cutting.borrow().is_some() {
            self.toast("Already finding the subject of a picture.");
            return;
        }
        if then == Use::Edit && imp.working.borrow().is_none() {
            self.toast("Still preparing this image for editing.");
            return;
        }
        self.with_cutout(move |window, paths| window.cut_out(paths, then));
    }

    fn cut_out(&self, paths: CutoutPaths, then: Use) {
        let imp = self.imp();
        let Some(program) = client::helper_path() else {
            self.toast("The background remover is missing from this installation of Glance.");
            return;
        };
        let Some(source) = imp.current.borrow().clone() else { return };
        let canvas = imp.view.canvas();
        let Some(display) = canvas.display_size() else { return };
        let live = canvas.live_edits();
        // In the editor, what is being edited; otherwise the file afresh,
        // or the frame on screen of an animation.
        let existing = imp.working.borrow().clone();
        let frame = (existing.is_none() && canvas.frame_count() > 1)
            .then(|| canvas.frames().get(canvas.frame()).map(|(texture, _)| crate::images::edit::output::pixels_of(texture)))
            .flatten();
        let started = Started {
            path: source.clone(),
            history: imp.history.borrow().len(),
            size: existing.as_ref().map_or((0, 0), |image| (image.width(), image.height())),
        };
        imp.cutting.replace(Some(source.clone()));
        imp.cutout_button.set_sensitive(false);
        imp.cutout_status.set_reveal_child(true);

        let (sender, receiver) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let result = (|| {
                let previous = existing.clone();
                let image = match (existing, frame) {
                    (Some(image), _) => image,
                    (None, Some(image)) => image,
                    (None, None) => export::open(&source)?,
                };
                // Whatever is waiting to be baked in is the picture as seen.
                let image = export::apply(image, live, None, display)?;
                let cut = without_background(image.to_rgba8(), &program, &paths)?;
                Ok::<_, String>((previous, cut))
            })();
            let _ = sender.send_blocking(result);
        });

        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                let outcome = receiver.recv().await.unwrap_or_else(|_| Err("The background remover stopped unexpectedly.".into()));
                let imp = window.imp();
                imp.cutting.replace(None);
                imp.cutout_button.set_sensitive(true);
                imp.cutout_status.set_reveal_child(false);
                match (outcome, then) {
                    (Err(message), _) => window.toast(&message),
                    (Ok((_, cut)), Use::Copy) => {
                        let Some(subject) = trimmed(&cut) else {
                            window.toast("No subject was found in this picture.");
                            return;
                        };
                        let (width, height) = subject.dimensions();
                        window.clipboard().set_texture(&canvas::texture_from(width, height, false, subject.into_raw()));
                        window.toast("Subject copied.");
                    }
                    (Ok((previous, cut)), Use::Edit) => window.cut_out_done(started, previous, cut),
                }
            }
        ));
    }

    /// The picture being edited, without its background, as an edit of its
    /// own: unless it has changed meanwhile, when the answer is for a picture
    /// no longer there.
    fn cut_out_done(&self, started: Started, previous: Option<DynamicImage>, cut: RgbaImage) {
        let imp = self.imp();
        let editing = imp.edit_button.is_active();
        let now = Started {
            path: imp.current.borrow().clone().unwrap_or_default(),
            history: imp.history.borrow().len(),
            size: imp.working.borrow().as_ref().map_or((0, 0), |image| (image.width(), image.height())),
        };
        let (Some(previous), true, true) = (previous, editing, now == started) else {
            self.toast("The picture changed meanwhile, so its background was left as it was.");
            return;
        };
        if trimmed(&cut).is_none() {
            self.toast("No subject was found in this picture, so nothing was changed.");
            return;
        }
        self.push_history(previous);
        imp.working.replace(Some(DynamicImage::ImageRgba8(cut)));
        self.show_working();
        self.update_edit_state();
        self.toast(if keeps_no_transparency(&started.path) {
            "Background removed. This kind of file has no transparency: Export as PNG or WebP to keep it."
        } else {
            "Background removed."
        });
    }
}

/// The picture with its background made see-through: the helper is given a
/// copy at most `SEND_SIDE` across, and its answer is brought back up to the
/// picture's own size.
fn without_background(mut rgba: RgbaImage, program: &Path, paths: &CutoutPaths) -> Result<RgbaImage, String> {
    let (width, height) = rgba.dimensions();
    let scale = (f64::from(SEND_SIDE) / f64::from(width.max(height))).min(1.0);
    let (sw, sh) = (((f64::from(width) * scale).round() as u32).max(1), ((f64::from(height) * scale).round() as u32).max(1));
    let small = if scale < 1.0 { image::imageops::resize(&rgba, sw, sh, image::imageops::FilterType::Triangle) } else { rgba.clone() };
    let mask = client::cut_out(program, paths, sw, sh, small.as_raw())?;
    drop(small);
    let mask = GrayImage::from_raw(sw, sh, mask).ok_or("The background remover answered for another size.")?;
    let mask = if (sw, sh) == (width, height) {
        mask
    } else {
        image::imageops::resize(&mask, width, height, image::imageops::FilterType::Triangle)
    };
    apply_mask(&mut rgba, &mask);
    Ok(rgba)
}

/// Each pixel's alpha scaled by how sure the mask is that it is the subject.
fn apply_mask(rgba: &mut RgbaImage, mask: &GrayImage) {
    for (pixel, sure) in rgba.pixels_mut().zip(mask.pixels()) {
        pixel[3] = ((u16::from(pixel[3]) * u16::from(sure[0]) + 127) / 255) as u8;
    }
}

/// The picture trimmed to what is left of it: None when nothing is.
fn trimmed(rgba: &RgbaImage) -> Option<RgbaImage> {
    let (mut x1, mut y1, mut x2, mut y2) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, pixel) in rgba.enumerate_pixels() {
        if pixel[3] >= TRIM_ALPHA {
            (x1, y1, x2, y2) = (x1.min(x), y1.min(y), x2.max(x), y2.max(y));
        }
    }
    (x1 <= x2 && y1 <= y2).then(|| image::imageops::crop_imm(rgba, x1, y1, x2 - x1 + 1, y2 - y1 + 1).to_image())
}

/// Whether saving over this file loses the transparency.
fn keeps_no_transparency(path: &Path) -> bool {
    let extension = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    matches!(extension.as_str(), "jpg" | "jpeg" | "jpe" | "jfif" | "bmp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mask_becomes_the_alpha_and_keeps_what_was_already_see_through() {
        let mut rgba = RgbaImage::from_raw(3, 1, vec![10, 20, 30, 255, 10, 20, 30, 255, 10, 20, 30, 128]).unwrap();
        let mask = GrayImage::from_raw(3, 1, vec![0, 255, 255]).unwrap();
        apply_mask(&mut rgba, &mask);
        assert_eq!(rgba.as_raw()[3], 0, "background");
        assert_eq!(rgba.as_raw()[7], 255, "subject");
        assert_eq!(rgba.as_raw()[11], 128, "already half see-through, and kept so");
        assert_eq!(&rgba.as_raw()[..3], &[10, 20, 30], "colour untouched");
    }

    #[test]
    fn a_subject_is_trimmed_to_itself_and_nothing_is_nothing() {
        let mut rgba = RgbaImage::new(10, 8);
        for (x, y) in [(3, 2), (6, 5)] {
            rgba.put_pixel(x, y, image::Rgba([1, 2, 3, 255]));
        }
        rgba.put_pixel(9, 7, image::Rgba([1, 2, 3, TRIM_ALPHA - 1]));
        let subject = trimmed(&rgba).unwrap();
        assert_eq!(subject.dimensions(), (4, 4), "from (3, 2) to (6, 5); the faint pixel left out");
        assert!(trimmed(&RgbaImage::new(5, 5)).is_none());
    }

    #[test]
    fn only_files_without_transparency_are_warned_about() {
        assert!(keeps_no_transparency(Path::new("/a/photo.JPG")));
        assert!(!keeps_no_transparency(Path::new("/a/photo.png")));
        assert!(!keeps_no_transparency(Path::new("/a/photo.webp")));
    }
}
