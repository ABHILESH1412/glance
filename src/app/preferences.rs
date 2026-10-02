// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Preferences. For now one page: Live Text, saying what has been downloaded
//! for it and how much room that takes, with a way to take it away again.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::app::window::Window;
use crate::live_text::install;

/// The Live Text page, refilled whenever what is downloaded changes.
struct LiveTextPage {
    dialog: adw::PreferencesDialog,
    parts: adw::PreferencesGroup,
    place: adw::PreferencesGroup,
    /// What the last fill added, to take out before the next.
    added: RefCell<Vec<(adw::PreferencesGroup, gtk::Widget)>>,
}

impl LiveTextPage {
    fn add(&self, group: &adw::PreferencesGroup, widget: &impl IsA<gtk::Widget>) {
        group.add(widget);
        self.added.borrow_mut().push((group.clone(), widget.clone().upcast()));
    }
}

impl Window {
    pub(crate) fn show_preferences(&self) {
        let dialog = adw::PreferencesDialog::new();
        dialog.set_title("Preferences");
        let parts = adw::PreferencesGroup::builder()
            .title("Live Text")
            .description(
                "Selecting and copying the text in pictures. What it needs is downloaded the first time it is \
                 used, and the reading happens on this computer: pictures are never sent anywhere.",
            )
            .build();
        let place = adw::PreferencesGroup::new();
        let page = adw::PreferencesPage::builder().title("Live Text").icon_name("glance-live-text-symbolic").build();
        page.add(&parts);
        page.add(&place);
        dialog.add(&page);
        let page = Rc::new(LiveTextPage { dialog: dialog.clone(), parts, place, added: RefCell::default() });
        self.fill_live_text_page(&page);
        // The buttons hold the page weakly; the dialog holds it for as long
        // as it is open.
        dialog.connect_closed(move |_| {
            page.added.borrow_mut().clear();
        });
        dialog.present(Some(self));
    }

    /// What is downloaded, part by part, and the button that fits: Remove
    /// when anything is there, Download when nothing is.
    fn fill_live_text_page(&self, page: &Rc<LiveTextPage>) {
        for (group, widget) in page.added.borrow_mut().drain(..) {
            group.remove(&widget);
        }

        let mut total = 0u64;
        let mut any = false;
        for part in install::parts() {
            let kept = install::kept_size(part);
            any |= kept.is_some();
            total += kept.unwrap_or(0);
            let row = adw::ActionRow::builder()
                .title(part.name)
                .subtitle(match kept {
                    Some(size) => glib::format_size(size).to_string(),
                    None => format!("Not downloaded ({} to download)", glib::format_size(part.size)),
                })
                .build();
            let icon = gtk::Image::from_icon_name(if kept.is_some() { "object-select-symbolic" } else { "folder-download-symbolic" });
            icon.set_tooltip_text(Some(if kept.is_some() { "Downloaded" } else { "Not downloaded" }));
            row.add_suffix(&icon);
            page.add(&page.parts, &row);
        }
        if install::runtime().is_none() {
            let row = adw::ActionRow::builder()
                .title("Not available on this kind of computer")
                .subtitle("The text engine is only made for 64-bit Intel, AMD and ARM processors.")
                .build();
            page.add(&page.parts, &row);
        }

        let folder = install::folder();
        let summary = adw::ActionRow::builder()
            .title(if any { format!("{} on disk", glib::format_size(total)) } else { "Nothing downloaded".to_string() })
            .subtitle(folder.display().to_string())
            .subtitle_selectable(true)
            .build();
        if any {
            let open = gtk::Button::from_icon_name("folder-open-symbolic");
            open.set_tooltip_text(Some("Open the Folder"));
            open.add_css_class("flat");
            open.set_valign(gtk::Align::Center);
            open.connect_clicked(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_| {
                    gtk::FileLauncher::new(Some(&gio::File::for_path(install::folder()))).launch(
                        Some(&window),
                        gio::Cancellable::NONE,
                        |_| {},
                    );
                }
            ));
            summary.add_suffix(&open);
        }
        page.add(&page.place, &summary);

        let button = if any {
            let remove = gtk::Button::with_mnemonic("_Remove Live Text…");
            remove.add_css_class("destructive-action");
            let page = Rc::downgrade(page);
            remove.connect_clicked(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_| {
                    if let Some(page) = page.upgrade() {
                        window.ask_to_remove_live_text(&page);
                    }
                }
            ));
            remove
        } else {
            let download = gtk::Button::with_mnemonic("_Download Live Text…");
            download.add_css_class("suggested-action");
            download.set_sensitive(install::runtime().is_some());
            let page = Rc::downgrade(page);
            download.connect_clicked(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_| {
                    let page = page.clone();
                    window.with_live_text(move |window, _| {
                        if let Some(page) = page.upgrade() {
                            window.fill_live_text_page(&page);
                        }
                    });
                }
            ));
            download
        };
        button.add_css_class("pill");
        button.set_halign(gtk::Align::Center);
        button.set_margin_top(12);
        page.add(&page.place, &button);
    }

    fn ask_to_remove_live_text(&self, page: &Rc<LiveTextPage>) {
        let size: u64 = install::parts().into_iter().filter_map(install::kept_size).sum();
        let ask = adw::AlertDialog::new(
            Some("Remove Live Text?"),
            Some(&format!(
                "The {} downloaded for it will be deleted. It can be downloaded again the next time Live Text is \
                 used.",
                glib::format_size(size)
            )),
        );
        ask.add_response("cancel", "_Cancel");
        ask.add_response("remove", "_Remove");
        ask.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        ask.set_default_response(Some("cancel"));
        ask.set_close_response("cancel");
        let weak = Rc::downgrade(page);
        ask.connect_response(
            Some("remove"),
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, _| {
                    // Off, and its reader let go of, before its files go.
                    window.live_stop();
                    window.live_forget_helper();
                    match install::remove() {
                        Ok(()) => window.toast("Live Text removed."),
                        Err(e) => window.toast(&format!("Could not remove everything: {e}")),
                    }
                    if let Some(page) = weak.upgrade() {
                        window.fill_live_text_page(&page);
                    }
                }
            ),
        );
        ask.present(Some(&page.dialog));
    }
}
