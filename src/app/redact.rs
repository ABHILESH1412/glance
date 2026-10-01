// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Applying redactions, for pictures and PDFs alike.
//!
//! Marking an area changes nothing. Only this does, and only after saying
//! plainly what will happen and asking where: a redacted copy beside the
//! original, which is offered first, or the original itself.

use std::path::{Path, PathBuf};

use adw::prelude::*;
use gtk::glib;

/// Where the redacted version goes.
pub enum Choice {
    Original,
    Copy(PathBuf),
}

/// Ask before redacting `count` areas of `original`, then call `done` with
/// where to put the result. Nothing happens if the reader backs out.
pub fn confirm(
    parent: &impl IsA<gtk::Widget>,
    original: &Path,
    count: usize,
    pdf: bool,
    done: impl Fn(Choice) + 'static,
) {
    let areas = if count == 1 { "The marked area".to_string() } else { format!("The {count} marked areas") };
    let them = if count == 1 { "it" } else { "them" };
    let mut body = if pdf {
        format!(
            "{areas} will be blacked out, and everything under {them} — text, pictures and drawings — \
             destroyed. No program can bring it back."
        )
    } else {
        format!("{areas} will be painted solid black, and the pixels under {them} destroyed. No program can bring them back.")
    };
    if pdf {
        body.push_str(
            "\n\nEvery page with a redaction becomes a picture of itself; the rest of its words stay \
             searchable and can still be copied.",
        );
    }
    body.push_str("\n\nKeep the original as it is and save a redacted copy, or redact the original?");

    let dialog = adw::AlertDialog::new(Some("Redact Permanently?"), Some(&body));
    dialog.add_response("cancel", "_Keep Editing");
    dialog.add_response("copy", "Save Redacted _Copy…");
    dialog.add_response("original", "_Redact Original");
    dialog.set_response_appearance("copy", adw::ResponseAppearance::Suggested);
    dialog.set_response_appearance("original", adw::ResponseAppearance::Destructive);
    // The safe choice is the one Enter takes.
    dialog.set_default_response(Some("copy"));
    dialog.set_close_response("cancel");
    let done = std::rc::Rc::new(done);
    let original = original.to_path_buf();
    let anchor = parent.as_ref().clone();
    dialog.connect_response(None, move |_, response| match response {
        "original" => done(Choice::Original),
        "copy" => {
            let done = done.clone();
            crate::app::protect::save_copy_as(&anchor, &original, "redacted", move |path| {
                done(Choice::Copy(path));
            });
        }
        _ => {}
    });
    dialog.present(Some(parent));
}

/// Throw away the thumbnails desktop programs keep of a file, which would
/// otherwise go on showing what was redacted. They are made again from the
/// redacted file when next wanted.
pub fn forget_thumbnails(path: &Path) {
    let uri = gtk::gio::File::for_path(path).uri();
    let Some(name) = glib::compute_checksum_for_string(glib::ChecksumType::Md5, &uri) else { return };
    let cache = glib::user_cache_dir().join("thumbnails");
    for size in ["normal", "large", "x-large", "xx-large"] {
        let _ = std::fs::remove_file(cache.join(size).join(format!("{name}.png")));
    }
}
