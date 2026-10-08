// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Preferences, everything Glance remembers in one window:
//!
//! - General: light or dark, and how long a slideshow lingers.
//! - PDFs: how a document is laid out and coloured, the highlighter's colour,
//!   and the paper pictures go on when combined into a PDF.
//! - Signatures: the ones kept, to remove or add to.
//! - Live Text: what has been downloaded for it and how much room that takes,
//!   with a way to take it away again.
//!
//! Each setting is the same action the menus use, so the two never disagree.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::subclass::prelude::ObjectSubclassIsExt;
use gtk::{gio, glib};

use crate::app::prefs;
use crate::app::window::Window;
use crate::images::edit::signature::{self, Signature};
use crate::live_text::install;
use crate::pdf;

/// The page of signatures, refilled whenever one is added or removed.
struct SignaturesPage {
    dialog: adw::PreferencesDialog,
    group: adw::PreferencesGroup,
    added: RefCell<Vec<gtk::Widget>>,
}

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

/// The pages that can be asked for by name.
fn page_name(page: Option<&str>) -> Option<&str> {
    page.filter(|name| ["general", "pdfs", "signatures", "live-text"].contains(name))
}

/// When a signature was written, from the name it is kept under.
fn made(id: &str) -> String {
    let micros: Option<i64> = id.split(['.', '-']).next().and_then(|n| n.parse().ok());
    micros
        .and_then(|m| glib::DateTime::from_unix_local(m / 1_000_000).ok())
        .and_then(|when| when.format("Written %-d %B %Y").ok())
        .map(String::from)
        .unwrap_or_default()
}

impl Window {
    pub(crate) fn show_preferences(&self) {
        self.show_preferences_at(None);
    }

    /// Preferences, open at the page of that name if one is given.
    pub(crate) fn show_preferences_at(&self, wanted: Option<&str>) {
        let dialog = adw::PreferencesDialog::new();
        dialog.set_title("Preferences");
        dialog.set_search_enabled(true);
        dialog.add(&self.general_page());
        dialog.add(&self.pdf_page(&dialog));
        let signatures = adw::PreferencesPage::builder()
            .name("signatures")
            .title("Signatures")
            .icon_name("document-edit-symbolic")
            .build();
        let group = adw::PreferencesGroup::builder()
            .title("Signatures")
            .description(
                "Written once, then put on pictures and PDFs with the Signature button in the Draw panel. \
                 They are kept on this computer only.",
            )
            .build();
        signatures.add(&group);
        dialog.add(&signatures);
        let signatures = Rc::new(SignaturesPage { dialog: dialog.clone(), group, added: RefCell::default() });
        self.fill_signatures_page(&signatures);
        let parts = adw::PreferencesGroup::builder()
            .title("Live Text")
            .description(
                "Selecting and copying the text in pictures. What it needs is downloaded the first time it is \
                 used, and the reading happens on this computer: pictures are never sent anywhere.",
            )
            .build();
        let place = adw::PreferencesGroup::new();
        let page =
            adw::PreferencesPage::builder().name("live-text").title("Live Text").icon_name("glance-live-text-symbolic").build();
        page.add(&parts);
        page.add(&place);
        dialog.add(&page);
        let page = Rc::new(LiveTextPage { dialog: dialog.clone(), parts, place, added: RefCell::default() });
        self.fill_live_text_page(&page);
        // The buttons hold the page weakly; the dialog holds it for as long
        // as it is open.
        dialog.connect_closed(move |_| {
            page.added.borrow_mut().clear();
            signatures.added.borrow_mut().clear();
        });
        if let Some(name) = page_name(wanted) {
            dialog.set_visible_page_name(name);
        }
        dialog.present(Some(self));
    }

    /// Appearance, and the slideshow.
    fn general_page(&self) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder().name("general").title("General").icon_name("preferences-system-symbolic").build();

        let looks = adw::PreferencesGroup::builder().title("Appearance").build();
        const THEMES: [(&str, &str); 3] = [("system", "Follow System"), ("light", "Light"), ("dark", "Dark")];
        let style = adw::ComboRow::builder()
            .title("Style")
            .subtitle("Light or dark, or whichever the desktop is set to")
            .model(&gtk::StringList::new(&THEMES.map(|(_, name)| name)))
            .build();
        let app = self.application();
        let now = app.as_ref().and_then(|app| app.action_state("theme")).and_then(|v| v.str().map(str::to_owned));
        style.set_selected(THEMES.iter().position(|(key, _)| Some(*key) == now.as_deref()).unwrap_or(0) as u32);
        style.connect_selected_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |row| {
                let Some((key, _)) = THEMES.get(row.selected() as usize) else { return };
                if let Some(app) = window.application() {
                    app.change_action_state("theme", &key.to_variant());
                }
            }
        ));
        looks.add(&style);
        page.add(&looks);

        let pictures = adw::PreferencesGroup::builder().title("Pictures").build();
        let intervals = crate::app::viewing::INTERVALS;
        let names: Vec<String> =
            intervals.iter().map(|s| format!("{s} seconds")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let slideshow = adw::ComboRow::builder()
            .title("Slideshow")
            .subtitle("How long each picture stays up")
            .model(&gtk::StringList::new(&names))
            .build();
        let chosen = self.imp().reader_prefs.get().slideshow;
        slideshow.set_selected(intervals.iter().position(|&s| s == chosen).unwrap_or(2) as u32);
        slideshow.connect_selected_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |row| {
                // The slideshow's own chooser keeps it, and says so on screen.
                window.imp().slideshow_every.set_selected(row.selected());
            }
        ));
        pictures.add(&slideshow);
        page.add(&pictures);
        page.add(&crate::app::updates::preferences_group());
        page
    }

    /// How documents are shown, marked, and combined.
    fn pdf_page(&self, dialog: &adw::PreferencesDialog) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder().name("pdfs").title("PDFs").icon_name("x-office-document-symbolic").build();
        let prefs = self.imp().reader_prefs.get();

        let reading = adw::PreferencesGroup::builder().title("Reading").build();
        const LAYOUTS: [(pdf::Mode, &str); 3] =
            [(pdf::Mode::Continuous, "Continuous Scroll"), (pdf::Mode::Single, "Single Page"), (pdf::Mode::Double, "Two Pages")];
        let layout = adw::ComboRow::builder()
            .title("Page layout")
            .model(&gtk::StringList::new(&LAYOUTS.map(|(_, name)| name)))
            .build();
        layout.set_selected(LAYOUTS.iter().position(|(mode, _)| *mode == prefs.mode).unwrap_or(0) as u32);
        layout.connect_selected_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |row| {
                let Some((mode, _)) = LAYOUTS.get(row.selected() as usize) else { return };
                window.change_action_state("pdf-layout", &prefs::mode_name(*mode).to_variant());
            }
        ));
        reading.add(&layout);

        let night = adw::SwitchRow::builder()
            .title("Night mode")
            .subtitle("Black paper and white text, with colours keeping their hue")
            .active(prefs.night)
            .build();
        night.connect_active_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |row| window.change_action_state("night-mode", &row.is_active().to_variant())
        ));
        reading.add(&night);
        page.add(&reading);

        let marking = adw::PreferencesGroup::builder().title("Marking Up").build();
        let highlight = adw::ActionRow::builder()
            .title("Highlight colour")
            .subtitle("What selected text is highlighted with")
            .activatable(true)
            .build();
        let swatch = gtk::DrawingArea::builder().content_width(28).content_height(20).valign(gtk::Align::Center).build();
        let shown = Rc::new(std::cell::Cell::new(prefs.highlight));
        {
            let shown = shown.clone();
            swatch.set_draw_func(move |_, cr, w, h| {
                let rgba = shown.get().to_rgba();
                cr.rectangle(0.5, 0.5, f64::from(w) - 1.0, f64::from(h) - 1.0);
                cr.set_source_rgb(f64::from(rgba.red()), f64::from(rgba.green()), f64::from(rgba.blue()));
                let _ = cr.fill_preserve();
                cr.set_source_rgba(0.0, 0.0, 0.0, 0.3);
                cr.set_line_width(1.0);
                let _ = cr.stroke();
            });
        }
        highlight.add_suffix(&swatch);
        highlight.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        highlight.connect_activated(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[weak]
            dialog,
            #[weak]
            swatch,
            move |_| {
                let current = window.imp().reader_prefs.get().highlight.to_rgba();
                let shown = shown.clone();
                crate::app::colour::choose(&dialog, "Highlight Colour", &current, false, move |rgba| {
                    let rgb = pdf::Rgb::from_rgba(&rgba);
                    window.change_action_state("highlight-colour", &rgb.hex().to_variant());
                    shown.set(rgb);
                    swatch.queue_draw();
                });
            }
        ));
        marking.add(&highlight);
        page.add(&marking);

        let combining = adw::PreferencesGroup::builder().title("Combine into PDF").build();
        const PAPERS: [(Option<pdf::Paper>, &str); 4] = [
            (None, "No Default"),
            (Some(pdf::Paper::A4), "A4"),
            (Some(pdf::Paper::Letter), "US Letter"),
            (Some(pdf::Paper::Own), "Each Picture's Own Size"),
        ];
        let paper = adw::ComboRow::builder()
            .title("Paper for pictures")
            .subtitle("Offered first when pictures are saved into a PDF, and the size of a blank page")
            .model(&gtk::StringList::new(&PAPERS.map(|(_, name)| name)))
            .build();
        paper.set_selected(PAPERS.iter().position(|(p, _)| *p == prefs.paper).unwrap_or(0) as u32);
        paper.connect_selected_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |row| {
                let Some((chosen, _)) = PAPERS.get(row.selected() as usize) else { return };
                window.update_prefs(|prefs| prefs.paper = *chosen);
            }
        ));
        combining.add(&paper);
        page.add(&combining);
        page
    }

    /// Each signature kept, with a way to remove it, and a way to add one.
    fn fill_signatures_page(&self, page: &Rc<SignaturesPage>) {
        for widget in page.added.borrow_mut().drain(..) {
            page.group.remove(&widget);
        }
        let mut added = Vec::new();
        let kept = signature::all();
        if kept.is_empty() {
            let none = adw::ActionRow::builder()
                .title("No signatures yet")
                .subtitle("Write one once, and it is here to put down on any picture or PDF.")
                .build();
            added.push(none.upcast::<gtk::Widget>());
        }
        for (n, kept) in kept.into_iter().enumerate() {
            let row = adw::ActionRow::builder().title(format!("Signature {}", n + 1)).subtitle(made(&kept.id)).build();
            let face = crate::app::signatures::thumbnail(&kept, 150, 48);
            face.set_valign(gtk::Align::Center);
            face.set_margin_top(6);
            face.set_margin_bottom(6);
            row.add_prefix(&face);
            let remove = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("Remove This Signature")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            let weak = Rc::downgrade(page);
            remove.connect_clicked(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_| {
                    if let Some(page) = weak.upgrade() {
                        window.remove_signature(&page, kept.clone());
                    }
                }
            ));
            row.add_suffix(&remove);
            added.push(row.upcast());
        }
        let new = gtk::Button::builder()
            .label("_New Signature…")
            .use_underline(true)
            .halign(gtk::Align::Center)
            .margin_top(12)
            .css_classes(["pill", "suggested-action"])
            .build();
        let weak = Rc::downgrade(page);
        new.connect_clicked(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| {
                let Some(page) = weak.upgrade() else { return };
                let weak = Rc::downgrade(&page);
                crate::app::signatures::write_new(&page.dialog, move |_| {
                    if let Some(page) = weak.upgrade() {
                        window.fill_signatures_page(&page);
                    }
                });
            }
        ));
        added.push(new.upcast());
        for widget in &added {
            page.group.add(widget);
        }
        page.added.replace(added);
    }

    /// Remove a signature at once, with a moment to take that back.
    fn remove_signature(&self, page: &Rc<SignaturesPage>, kept: Signature) {
        if let Err(error) = signature::remove(&kept.id) {
            page.dialog.add_toast(adw::Toast::new(&format!("Could not remove the signature: {error}")));
            return;
        }
        self.fill_signatures_page(page);
        let toast = adw::Toast::builder().title("Signature removed").button_label("_Undo").build();
        let weak = Rc::downgrade(page);
        toast.connect_button_clicked(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| {
                let Some(page) = weak.upgrade() else { return };
                if let Err(error) = signature::restore(&kept) {
                    page.dialog.add_toast(adw::Toast::new(&format!("Could not put it back: {error}")));
                }
                window.fill_signatures_page(&page);
            }
        ));
        page.dialog.add_toast(toast);
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
