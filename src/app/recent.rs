// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The files opened lately, offered again by the empty window.
//!
//! They are kept in the desktop's own list of recently used files, marked as
//! opened by Glance: the list is the same in every window, and what other
//! programs opened stays off it.

use std::path::{Path, PathBuf};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};

use crate::app::window::Window;

/// What the files are marked with, and looked for by.
const APP_NAME: &str = "Glance";

impl Window {
    /// The list in the empty window, kept in step with the desktop's.
    pub(crate) fn install_recent(&self) {
        self.imp().view.connect_recent(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |path| match path {
                Some(path) => window.open_file(&gio::File::for_path(path)),
                None => forget(),
            }
        ));
        gtk::RecentManager::default().connect_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.refresh_recent()
        ));
        self.refresh_recent();
    }

    fn refresh_recent(&self) {
        self.imp().view.set_recent(opened_lately());
    }
}

/// Put a file that has just opened at the top of the list.
pub(crate) fn remember(path: &Path) {
    let file = gio::File::for_path(path);
    let (mime, _) = gio::content_type_guess(Some(path), None::<&[u8]>);
    let mime = gio::content_type_get_mime_type(&mime).map_or_else(|| "application/octet-stream".to_string(), |m| m.to_string());
    let data = gtk::RecentData::new(None, None, &mime, APP_NAME, "glance %u", &[], false);
    gtk::RecentManager::default().add_full(&file.uri(), &data);
}

/// The local files Glance opened, newest first, that are still there.
fn opened_lately() -> Vec<PathBuf> {
    let mut items: Vec<gtk::RecentInfo> = gtk::RecentManager::default()
        .items()
        .into_iter()
        .filter(|item| item.has_application(APP_NAME) && item.is_local() && item.exists())
        .collect();
    items.sort_by_key(|item| std::cmp::Reverse(item.modified().to_unix()));
    items.iter().filter_map(|item| gio::File::for_uri(&item.uri()).path()).collect()
}

/// Take Glance's files off the list. A file is one entry however many
/// programs opened it, so it goes for them all.
fn forget() {
    let manager = gtk::RecentManager::default();
    for item in manager.items() {
        if item.has_application(APP_NAME) {
            let _ = manager.remove_item(&item.uri());
        }
    }
}
