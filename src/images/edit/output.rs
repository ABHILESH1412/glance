// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Writing the edited picture out: saving over the original, exporting a copy,
//! and copying it to the clipboard.

use std::path::PathBuf;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, gio, glib};
use crate::images::edit::compress;
use crate::images::canvas;
use crate::images::edit::export;
use crate::app::window::Window;

/// A texture's pixels, as straight-alpha RGBA the editor can work on.
fn pixels_of(texture: &gdk::Texture) -> image::DynamicImage {
    let mut downloader = gdk::TextureDownloader::new(texture);
    downloader.set_format(gdk::MemoryFormat::R8g8b8a8);
    let (bytes, stride) = downloader.download_bytes();
    let (width, height) = (texture.width() as usize, texture.height() as usize);
    let mut rgba = Vec::with_capacity(width * height * 4);
    for row in bytes.chunks(stride).take(height) {
        rgba.extend_from_slice(&row[..width * 4]);
    }
    image::DynamicImage::ImageRgba8(
        image::RgbaImage::from_raw(width as u32, height as u32, rgba).unwrap_or_default(),
    )
}

impl Window {
    fn downloads_dir() -> PathBuf {
        glib::user_special_dir(glib::UserDirectory::Downloads).unwrap_or_else(glib::home_dir)
    }

    /// Put the picture on the clipboard exactly as it is on screen, edits and
    /// all, so pasting elsewhere gives what the viewer is showing.
    pub(crate) fn copy_to_clipboard(&self) {
        let imp = self.imp();
        let Some(source) = imp.current.borrow().clone() else {
            return;
        };
        let canvas = imp.view.canvas();
        let Some(display) = canvas.display_size() else {
            return;
        };
        let live = canvas.live_edits();
        // Reuse the editing buffer when there is one; otherwise decode afresh
        // rather than retaining a full-resolution copy just to copy once.
        let existing = imp.working.borrow().clone();
        // An animation: the frame on screen, not the first one the file
        // would decode to.
        let frame = (existing.is_none() && canvas.frame_count() > 1)
            .then(|| canvas.frames().get(canvas.frame()).map(|(texture, _)| (canvas.frame(), pixels_of(texture))))
            .flatten();
        let copied = match &frame {
            Some((index, _)) => format!("Frame {} copied.", index + 1),
            None => "Image copied.".to_string(),
        };

        let (sender, receiver) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let result = match (existing, frame) {
                (Some(image), _) => Ok(image),
                (None, Some((_, image))) => Ok(image),
                (None, None) => export::open(&source),
            }
            .and_then(|image| export::apply(image, live, None, display));
            let _ = sender.send_blocking(result);
        });

        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                match receiver.recv().await {
                    Ok(Ok(image)) => {
                        let rgba = image.to_rgba8();
                        let (width, height) = rgba.dimensions();
                        let texture =
                            canvas::texture_from(width, height, false, rgba.into_raw());
                        window.clipboard().set_texture(&texture);
                        window.toast(&copied);
                    }
                    Ok(Err(message)) => window.toast(&message),
                    Err(_) => window.toast("Copying stopped unexpectedly."),
                }
            }
        ));
    }

    /// Save writes over the image being viewed, which is what Save means.
    /// Save writes over the file being viewed, so it asks first.
    ///
    /// The original is gone once this runs — there is no copy kept and nothing
    /// to undo it with — which is worth one click to be sure of, and the
    /// wording points at Export for anyone who wanted a copy instead.
    pub(crate) fn save_default(&self) {
        let Some(source) = self.imp().current.borrow().clone() else {
            return;
        };
        // Redactions are saved through their own prompt, which offers a copy.
        if self.redactions_pending() {
            self.apply_redactions();
            return;
        }
        if !self.imp().dirty.get() && !self.has_live_transform() {
            self.toast("No changes to save.");
            return;
        }
        let name = source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "this image".to_string());

        let dialog = adw::AlertDialog::new(
            Some("Replace the original?"),
            Some(&format!(
                "“{name}” will be overwritten with these edits, and the original \
                 cannot be brought back. Export writes a copy instead."
            )),
        );
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("replace", "Replace");
        dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
        // Escape and clicking away both leave the file alone.
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.connect_response(
            None,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, response| {
                    if response == "replace" {
                        window.write_edited(source.clone(), true);
                    }
                }
            ),
        );
        dialog.present(Some(self));
    }

    /// Write a copy in the chosen format, at whatever size the resize tool is
    /// showing.
    pub(crate) fn export(&self) {
        let Some(source) = self.imp().current.borrow().clone() else {
            return;
        };
        let Some(target) = export::TARGETS.get(self.imp().format_drop.selected() as usize) else {
            return;
        };
        let canvas = self.imp().view.canvas();
        // Ask the format before the file dialog: a refusal after choosing a
        // name and a folder is a refusal arriving too late to be useful.
        if let Some((width, height)) = canvas.target_size() {
            if let Some(refusal) = target.refusal(width, height) {
                self.toast(&refusal);
                return;
            }
        }
        let stem = source
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "image".to_string());
        let dialog = gtk::FileDialog::builder()
            .title(format!("Export as {}", target.label))
            .modal(true)
            .initial_name(format!("{stem}.{}", target.extension))
            .initial_folder(&gio::File::for_path(Self::downloads_dir()))
            .build();
        dialog.save(
            Some(self),
            gio::Cancellable::NONE,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |result| match result {
                    Ok(file) => {
                        if let Some(path) = file.path() {
                            match window.size_wanted() {
                                Some(wanted) => window.write_fitted(path, wanted),
                                None => window.write_edited(path, false),
                            }
                        }
                    }
                    Err(error) => {
                        if !error.matches(gtk::DialogError::Dismissed) {
                            window.toast(&format!("Could not export: {error}"));
                        }
                    }
                }
            ),
        );
    }

    /// Export with a file size to hit, rather than whatever the encoder's
    /// defaults produce.
    fn write_fitted(&self, destination: PathBuf, wanted: u64) {
        self.load_working();
        let Some(image) = self.rendered() else {
            self.toast("Nothing to export yet.");
            return;
        };
        let Some(target) = export::TARGETS.get(self.imp().format_drop.selected() as usize) else {
            return;
        };
        let original = (image.width(), image.height());
        self.imp()
            .size_note
            .set_text(&format!("Working towards {}…", compress::describe(wanted)));

        let (sender, receiver) = async_channel::bounded(1);
        let path = destination.clone();
        let dpi = self.imp().dpi.get();
        std::thread::spawn(move || {
            // A dozen or so encodes of a full-size picture: nowhere near the
            // main loop.
            // Room for the resolution, so recording it cannot push the file
            // over the size asked for.
            let room = if dpi.is_some() { crate::images::resolution::ROOM } else { 0 };
            let outcome = compress::fit_to_size(&image, target, wanted.saturating_sub(room)).and_then(|mut fit| {
                // A few bytes more, for the resolution.
                if let Some(dpi) = dpi {
                    fit.bytes = crate::images::resolution::stamp(std::mem::take(&mut fit.bytes), dpi);
                }
                std::fs::write(&path, &fit.bytes)
                    .map_err(|error| format!("Could not write the file: {error}"))
                    .map(|()| fit)
            });
            let _ = sender.send_blocking(outcome);
        });

        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                match receiver.recv().await {
                    Ok(Ok(fit)) => {
                        let name = destination
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        // Say exactly what it took, including anything that is
                        // not picture: padding is honest or it is nothing.
                        let mut how = Vec::new();
                        if let Some(quality) = fit.quality {
                            how.push(format!("quality {quality}"));
                            // The search knows what the dial should have said.
                            window.imp().quality_scale.set_value(f64::from(quality));
                        }
                        if (fit.width, fit.height) != original {
                            how.push(format!("scaled to {} × {}", fit.width, fit.height));
                        }
                        if fit.padding > 0 {
                            how.push(format!("{} of padding", compress::describe(fit.padding)));
                        }
                        let detail = if how.is_empty() {
                            String::new()
                        } else {
                            format!(" — {}", how.join(", "))
                        };
                        window.imp().size_note.set_text(&format!(
                            "Wrote {}{detail}.",
                            compress::describe(fit.size())
                        ));
                        window.toast(&format!("Exported {name}"));
                    }
                    Ok(Err(message)) => {
                        window.imp().size_note.set_text(&message);
                        window.toast(&message);
                    }
                    Err(_) => window.toast("The exporter stopped unexpectedly."),
                }
            }
        ));
    }

    /// `in_place` means this became the file on screen, so the session carries
    /// on from the saved pixels with nothing left pending.
    pub(crate) fn write_edited(&self, destination: PathBuf, in_place: bool) {
        self.load_working();
        let Some(image) = self.rendered() else {
            self.toast("Nothing to save yet.");
            return;
        };

        let (sender, receiver) = async_channel::bounded(1);
        let target = destination.clone();
        let encoded = image.clone();
        let quality = self.quality();
        let dpi = self.imp().dpi.get();
        std::thread::spawn(move || {
            // Encoding a large image is slow enough to matter.
            let _ = sender.send_blocking(export::write(&encoded, &target, quality, dpi));
        });

        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                match receiver.recv().await {
                    Ok(Ok(())) => {
                        let shown = destination
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        if in_place {
                            // Thumbnails of the old picture would outlive it.
                            crate::app::redact::forget_thumbnails(&destination);
                            // The transforms are on disk now, so fold them into
                            // the working pixels and start clean.
                            let imp = window.imp();
                            imp.redactions_baked.set(false);
                            // The file records the resolution now.
                            imp.dpi_file.set(imp.dpi.get());
                            imp.working.replace(Some(image));
                            imp.history.borrow_mut().clear();
                            imp.redo.borrow_mut().clear();
                            imp.dirty.set(false);
                            window.show_working();
                            window.imp().title.set_subtitle("");
                            window.update_edit_state();
                        }
                        window.toast(&format!("Saved {shown}"));
                    }
                    Ok(Err(message)) => window.toast(&message),
                    Err(_) => window.toast("The exporter stopped unexpectedly."),
                }
            }
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame copied comes out pixel for pixel, padded rows or not.
    #[test]
    fn a_texture_s_pixels_come_back_as_they_went_in() {
        let (width, height, stride) = (3usize, 2usize, 16usize);
        let mut bytes = vec![0u8; stride * height];
        for y in 0..height {
            for x in 0..width {
                bytes[y * stride + x * 4..y * stride + x * 4 + 4].copy_from_slice(&[x as u8 * 80, y as u8 * 90, 7, 200]);
            }
        }
        let texture = gdk::MemoryTexture::new(
            width as i32,
            height as i32,
            gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_owned(bytes),
            stride,
        );
        let image = pixels_of(texture.upcast_ref()).to_rgba8();
        assert_eq!(image.dimensions(), (3, 2));
        assert_eq!(image.get_pixel(2, 1).0, [160, 90, 7, 200]);
        assert_eq!(image.get_pixel(0, 0).0, [0, 0, 7, 200]);
    }
}
