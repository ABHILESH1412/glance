// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The working copy being edited: baking changes into it, the undo history,
//! and leaving the editor.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};
use crate::images::edit::adjust::Adjustments;
use crate::images::canvas;
use crate::images::edit::export;
use crate::app::window::Window;

impl Window {
    /// Accept the selection and apply it to the working pixels, so the crop is
    /// what you see and further edits build on it. Nothing reaches the file
    /// until it is saved.
    pub(crate) fn commit_crop(&self) {
        let imp = self.imp();
        let Some(crop) = imp.view.canvas().crop() else {
            return;
        };
        // Put the crop tool away immediately; the pixels follow.
        imp.syncing_panel.set(true);
        imp.crop_toggle.set_active(false);
        imp.freehand_toggle.set_active(false);
        imp.syncing_panel.set(false);
        self.set_cropping(false);
        self.bake(Some(crop));
    }

    /// Fold everything pending into the working pixels, so what is on screen
    /// becomes what the next edit builds on. This is the step that puts an
    /// edit into the undo history.
    pub(crate) fn bake(&self, crop: Option<canvas::CropSelection>) {
        let imp = self.imp();
        let canvas = imp.view.canvas();
        let Some(display) = canvas.display_size() else {
            return;
        };
        let Some(working) = imp.working.borrow().clone() else {
            self.toast("Still preparing this image for editing.");
            return;
        };
        let live = canvas.live_edits();
        if live.is_identity() && crop.is_none() {
            return;
        }
        // Redactions baked into the working pixels are still not in the file;
        // saving them goes through the redaction prompt all the same.
        let redacting = canvas.redaction_count() > 0;

        let (sender, receiver) = async_channel::bounded(1);
        std::thread::spawn(move || {
            // The copy kept for undo is made here rather than on the main loop.
            let previous = working.clone();
            let result = export::apply(working, live, crop.as_ref(), display)
                .map(|baked| (previous, baked));
            let _ = sender.send_blocking(result);
        });

        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                match receiver.recv().await {
                    Ok(Ok((previous, baked))) => {
                        if redacting {
                            window.imp().redactions_baked.set(true);
                        }
                        window.push_history(previous);
                        window.imp().working.replace(Some(baked));
                        window.show_working();
                        window.update_edit_state();
                    }
                    Ok(Err(message)) => window.toast(&message),
                    Err(_) => window.toast("The editor stopped unexpectedly."),
                }
            }
        ));
    }

    /// How many states of history to keep. Each is a full copy of the image,
    /// so this trades memory for depth the way every editor has to.
    const HISTORY_LIMIT: usize = 8;

    /// Decode the file into the editing buffer, once, when editing starts.
    pub(crate) fn load_working(&self) {
        let imp = self.imp();
        if imp.working.borrow().is_some() {
            return;
        }
        let Some(source) = imp.current.borrow().clone() else {
            return;
        };
        imp.crop_toggle.set_sensitive(false);
        // Only the start of the file is read, so this is quick.
        let dpi = crate::images::resolution::read(&source);
        imp.dpi_file.set(dpi);
        imp.dpi.set(dpi);

        let (sender, receiver) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = sender.send_blocking(export::open(&source));
        });
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                match receiver.recv().await {
                    Ok(Ok(image)) => {
                        window.imp().working.replace(Some(image));
                        window.imp().crop_toggle.set_sensitive(true);
                        // Anything asked of the tone thread before the pixels
                        // were ready can be answered now.
                        window.refresh_tone();
                    }
                    _ => window.toast("This image cannot be edited."),
                }
            }
        ));
    }

    /// Throw away the edit session, which belongs to one image.
    pub(crate) fn reset_editing(&self) {
        let imp = self.imp();
        imp.working.replace(None);
        imp.history.borrow_mut().clear();
        imp.redo.borrow_mut().clear();
        imp.dirty.set(false);
        imp.redactions_baked.set(false);
        self.stop_tone_worker();
        imp.dpi_file.set(None);
        imp.dpi.set(None);
        // Zero the canvas and the sliders together rather than relying on
        // whatever replaces the texture next: a slider still reading -50 over
        // an untouched picture is a lie the next session would inherit.
        imp.view.canvas().set_adjustments(Adjustments::default());
        self.sync_tone_panel();
        imp.view.canvas().reset_size();
        imp.resize_toggle.set_active(false);
        imp.view.canvas().clear_text();
        imp.text_toggle.set_active(false);
        imp.view.canvas().clear_marks();
        imp.draw_toggle.set_active(false);
        self.update_edit_state();
    }

    pub(crate) fn update_edit_state(&self) {
        let imp = self.imp();
        let canvas = imp.view.canvas();
        let undo = !imp.history.borrow().is_empty() || canvas.has_marks() || canvas.has_text();
        let redo = !imp.redo.borrow().is_empty();
        imp.undo_button.set_sensitive(undo);
        imp.redo_button.set_sensitive(redo);
        for (name, enabled) in [("undo", undo), ("redo", redo)] {
            if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(enabled);
            }
        }
        let steps = imp.history.borrow().len();
        imp.pending_crop.set_visible(imp.dirty.get());
        imp.pending_crop.set_text(&match steps {
            0 => "Edited. Save to write it back.".to_string(),
            1 => "1 edit. Save to write it back.".to_string(),
            n => format!("{n} edits. Save to write it back."),
        });
        self.update_redaction_banner();
    }

    /// Put an image on screen as the thing being edited.
    pub(crate) fn show_working(&self) {
        let imp = self.imp();
        let Some(image) = imp.working.borrow().clone() else {
            return;
        };
        let rgba = image.to_rgba8();
        let (width, height) = rgba.dimensions();
        let texture = canvas::texture_from(width, height, false, rgba.into_raw());
        // The tone thread was working from the pixels being replaced.
        self.stop_tone_worker();
        // Resets zoom, rotation and flips, which is right: they are now baked
        // into these pixels.
        // Resets zoom, rotation, flips and tone, which is right: they are now
        // baked into these pixels. The sliders have to follow.
        imp.view.canvas().set_texture(Some(texture));
        self.sync_tone_panel();
        // Baking gives the picture a new real size, and turns the tool off.
        imp.resize_toggle.set_active(false);
        self.sync_resize_panel();
        // Baking burns the words into the pixels, so the tool starts empty.
        imp.text_toggle.set_active(false);
        self.sync_text_panel();
        // The strokes are pixels now, but the pen should still be in hand:
        // closing the tool after every save would make drawing a chore.
        // `set_texture` dropped the canvas's copy, so hand it back.
        self.sync_draw_tool();
        imp.title
            .set_subtitle(&format!("Edited · {width} × {height}"));
        // Levels, if open, needs the new picture's histogram.
        self.refresh_tone();
    }

    fn push_history(&self, previous: image::DynamicImage) {
        let imp = self.imp();
        let mut history = imp.history.borrow_mut();
        history.push(previous);
        if history.len() > Self::HISTORY_LIMIT {
            history.remove(0);
        }
        drop(history);
        imp.redo.borrow_mut().clear();
        imp.dirty.set(true);
    }

    pub(crate) fn undo(&self) {
        let imp = self.imp();
        let Some(previous) = imp.history.borrow_mut().pop() else {
            return;
        };
        if let Some(current) = imp.working.replace(Some(previous)) {
            imp.redo.borrow_mut().push(current);
        }
        self.show_working();
        self.update_edit_state();
    }

    pub(crate) fn redo(&self) {
        let imp = self.imp();
        let Some(next) = imp.redo.borrow_mut().pop() else {
            return;
        };
        if let Some(current) = imp.working.replace(Some(next)) {
            imp.history.borrow_mut().push(current);
        }
        self.show_working();
        self.update_edit_state();
    }

    /// Everything the view is showing, as pixels: the working image with any
    /// rotation or flip that has not been baked in yet.
    pub(crate) fn rendered(&self) -> Option<image::DynamicImage> {
        let imp = self.imp();
        let canvas = imp.view.canvas();
        let working = imp.working.borrow().clone()?;
        let display = canvas.display_size()?;
        export::apply(working, canvas.live_edits(), None, display).ok()
    }

    /// Cheaply: asking `live_edits` would draw every text item into pixels
    /// just to find out whether there are any.
    pub(crate) fn has_live_transform(&self) -> bool {
        let canvas = self.imp().view.canvas();
        canvas.rotation().abs() > 0.01
            || canvas.flip_horizontal()
            || canvas.flip_vertical()
            || canvas.has_resize()
            || canvas.has_text()
            || canvas.has_marks()
            || !canvas.adjustments().is_identity()
            || self.resolution_changed()
    }

    /// Leave the editor, throwing the session away. Asked for out loud first:
    /// the pixels on screen may be several edits from the file on disk.
    pub(crate) fn cancel_editing(&self) {
        let imp = self.imp();
        if !imp.edit_button.is_active() {
            return;
        }
        let unsaved = imp.dirty.get() || self.has_live_transform();
        let dialog = adw::AlertDialog::new(
            Some("Cancel Editing?"),
            Some(if unsaved {
                "Your unsaved changes to this image will be lost."
            } else {
                "This will close the editor."
            }),
        );
        dialog.add_response("no", "No");
        dialog.add_response("yes", "Yes");
        if unsaved {
            dialog.set_response_appearance("yes", adw::ResponseAppearance::Destructive);
        }
        // Escape and clicking away both mean "carry on editing".
        dialog.set_default_response(Some("no"));
        dialog.set_close_response("no");
        dialog.connect_response(
            None,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, response| {
                    if response == "yes" {
                        window.discard_editing();
                    }
                }
            ),
        );
        dialog.present(Some(self));
    }

    /// Drop the edit session and put the file back on screen untouched.
    fn discard_editing(&self) {
        let imp = self.imp();
        self.reset_editing();
        // Closing the panel also puts the crop tool away and restores the
        // filmstrip, through the toggle's own handler.
        imp.edit_button.set_active(false);
        // The canvas is showing edited pixels with a live rotation possibly on
        // top, so re-read the file rather than trying to unwind either.
        let path = imp.current.borrow().clone();
        if let Some(path) = path {
            self.load(path, false);
        }
    }
}
