// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Closing the window with work not yet saved asks first: edits to a picture,
//! areas marked for redaction, or pages put together into a PDF.
//!
//! The close button, Ctrl+W and Quit all come through here.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use crate::app::window::Window;

/// What closing now would lose.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Unsaved {
    /// Edits to the picture.
    Edits,
    /// Areas marked for redaction, not yet applied.
    Redactions,
    /// Pages put together in Combine into PDF.
    Pages,
}

impl Window {
    pub(crate) fn install_close_guard(&self) {
        self.connect_close_request(|window| {
            let imp = window.imp();
            if imp.closing_confirmed.get() {
                return glib::Propagation::Proceed;
            }
            match window.unsaved_work() {
                None => glib::Propagation::Proceed,
                Some(unsaved) => {
                    window.ask_before_closing(unsaved);
                    glib::Propagation::Stop
                }
            }
        });
    }

    fn unsaved_work(&self) -> Option<Unsaved> {
        let imp = self.imp();
        if imp.combining.get() {
            return imp.combine.has_unsaved().then_some(Unsaved::Pages);
        }
        // Redactions, marked or already in the pixels, are saved through
        // their own prompt, which offers a copy.
        if self.redactions_pending() {
            return Some(Unsaved::Redactions);
        }
        // A turn given to look at a picture is not an edit; only what the
        // editor holds is.
        let editing = imp.edit_button.is_active();
        if imp.dirty.get() || (editing && self.has_live_transform()) {
            return Some(Unsaved::Edits);
        }
        None
    }

    fn ask_before_closing(&self, unsaved: Unsaved) {
        let name = self
            .imp()
            .current
            .borrow()
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "this picture".to_string());
        let (heading, body) = match unsaved {
            Unsaved::Edits => (
                "Save Changes?".to_string(),
                format!("“{name}” has edits that have not been saved. Closing now loses them; saving writes them over the original."),
            ),
            Unsaved::Redactions => (
                "Close Without Redacting?".to_string(),
                "Some areas are marked for redaction, but nothing has been removed from the file yet. Closing now forgets them."
                    .to_string(),
            ),
            Unsaved::Pages => (
                "Discard These Pages?".to_string(),
                "The pages put together here have not been saved as a PDF yet. The files they came from are not changed either way."
                    .to_string(),
            ),
        };
        let dialog = adw::AlertDialog::new(Some(&heading), Some(&body));
        dialog.add_response("cancel", "_Cancel");
        dialog.add_response(
            "discard",
            match unsaved {
                Unsaved::Edits => "Close _Without Saving",
                Unsaved::Redactions => "Close _Without Redacting",
                Unsaved::Pages => "_Discard",
            },
        );
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        match unsaved {
            Unsaved::Edits => {
                dialog.add_response("save", "_Save");
                dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
            }
            Unsaved::Redactions => {
                dialog.add_response("save", "_Apply Redactions…");
                dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
            }
            Unsaved::Pages => {}
        }
        // Escape and clicking away both mean "do not close".
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.connect_response(
            None,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, response| match response {
                    "discard" => window.close_anyway(),
                    "save" => match unsaved {
                        Unsaved::Edits => window.save_and_close(),
                        // The redaction prompt asks where the result goes;
                        // the window stays open to show it.
                        _ => window.apply_redactions(),
                    },
                    _ => {}
                }
            ),
        );
        dialog.present(Some(self));
    }

    fn close_anyway(&self) {
        self.imp().closing_confirmed.set(true);
        self.close();
    }

    /// Write the edits over the original, and close once that worked.
    fn save_and_close(&self) {
        let imp = self.imp();
        let Some(path) = imp.current.borrow().clone() else {
            self.close_anyway();
            return;
        };
        imp.close_after_save.set(true);
        self.write_edited(path, true);
    }

    /// The save `save_and_close` started has finished.
    pub(crate) fn saved_for_closing(&self, worked: bool) {
        let imp = self.imp();
        if imp.close_after_save.replace(false) && worked {
            self.close_anyway();
        }
    }
}
