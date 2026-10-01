// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reduce File Size: three choices, from changing nothing that shows to the
//! smallest file, each tried in the background as soon as it is picked, so
//! the new size is known before anything is saved.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::pdf::{self, Level};

const CHOICES: &[(&str, &str, Level)] = &[
    ("Keep Everything as It Is", "Packed more tightly. Nothing looks different", Level::Lossless),
    (
        "Good for Reading on Screen",
        "Photos scaled to 150 dots per inch",
        Level::Photos { dpi: 150.0, quality: 75 },
    ),
    ("Smallest", "Photos scaled to 96 dots per inch, at lower quality", Level::Photos { dpi: 96.0, quality: 55 }),
];

/// What happened, for the window.
pub enum Outcome {
    Replaced { before: u64, after: u64 },
    Copied { path: PathBuf, before: u64, after: u64 },
}

/// A size as people say it: "3.4 MB".
pub fn readable(bytes: u64) -> String {
    glib::format_size(bytes).to_string()
}

struct State {
    path: PathBuf,
    password: Option<String>,
    /// Bumped per try, so a slower, older one is ignored.
    attempt: Cell<u64>,
    /// The finished try: where it was written and how it went.
    ready: RefCell<Option<(PathBuf, pdf::Report)>>,
}

impl State {
    fn discard(&self) {
        if let Some((written, _)) = self.ready.take() {
            let _ = std::fs::remove_file(written);
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.discard();
    }
}

pub fn present(
    parent: &impl IsA<gtk::Widget>,
    path: PathBuf,
    password: Option<String>,
    done: impl Fn(Outcome) + 'static,
) {
    let done: Rc<dyn Fn(Outcome)> = Rc::new(done);
    let state = Rc::new(State { path: path.clone(), password, attempt: Cell::new(0), ready: RefCell::default() });

    let group = adw::PreferencesGroup::new();
    let mut first: Option<gtk::CheckButton> = None;
    let mut buttons = Vec::new();
    for (title, subtitle, _) in CHOICES {
        let check = gtk::CheckButton::new();
        if let Some(first) = &first {
            check.set_group(Some(first));
        } else {
            first = Some(check.clone());
        }
        let row = adw::ActionRow::builder().title(*title).subtitle(*subtitle).activatable_widget(&check).build();
        row.add_prefix(&check);
        group.add(&row);
        buttons.push(check);
    }

    let now = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let result = adw::ActionRow::builder().title("Now").subtitle(readable(now)).build();
    let spinner = gtk::Spinner::new();
    result.add_suffix(&spinner);
    let outcome = adw::PreferencesGroup::new();
    outcome.add(&result);

    let replace =
        gtk::Button::builder().label("_Replace").use_underline(true).css_classes(["suggested-action"]).build();
    replace.set_tooltip_text(Some("Save over this file"));
    replace.set_sensitive(false);
    let copy = gtk::Button::builder()
        .label("Save as _Copy…")
        .use_underline(true)
        .halign(gtk::Align::Center)
        .css_classes(["pill"])
        .sensitive(false)
        .build();
    let cancel = gtk::Button::with_mnemonic("_Cancel");
    let header = adw::HeaderBar::builder().show_start_title_buttons(false).show_end_title_buttons(false).build();
    header.pack_start(&cancel);
    header.pack_end(&replace);

    let column = gtk::Box::new(gtk::Orientation::Vertical, 24);
    column.set_margin_top(12);
    column.set_margin_bottom(24);
    column.set_margin_start(12);
    column.set_margin_end(12);
    column.append(&group);
    column.append(&outcome);
    column.append(&copy);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&column));
    let dialog = adw::Dialog::builder()
        .title("Reduce File Size")
        .content_width(440)
        .follows_content_size(true)
        .child(&toolbar)
        .build();

    // Try a choice in the background, and say what it comes to.
    let attempt = {
        let state = state.clone();
        let (result, spinner, replace, copy) = (result.clone(), spinner.clone(), replace.clone(), copy.clone());
        Rc::new(move |level: Level| {
            state.discard();
            let number = state.attempt.get() + 1;
            state.attempt.set(number);
            replace.set_sensitive(false);
            copy.set_sensitive(false);
            spinner.set_spinning(true);
            result.set_title("Working Out the New Size…");
            result.set_subtitle(" ");
            let written = pdf::temporary_beside(&state.path, &format!("smaller{number}"));
            let (source, out, password) = (state.path.clone(), written.clone(), state.password.clone());
            let (sender, receiver) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let report = pdf::shrink(&source, password.as_deref(), level, &out);
                if report.is_err() {
                    let _ = std::fs::remove_file(&out);
                }
                let _ = sender.send_blocking(report);
            });
            let state = Rc::downgrade(&state);
            let (result, spinner, replace, copy) = (result.clone(), spinner.clone(), replace.clone(), copy.clone());
            glib::spawn_future_local(async move {
                let report = receiver.recv().await.unwrap_or_else(|_| Err("stopped unexpectedly".into()));
                let Some(state) = state.upgrade().filter(|s| s.attempt.get() == number) else {
                    // The dialog was closed or another choice made meanwhile.
                    let _ = std::fs::remove_file(&written);
                    return;
                };
                spinner.set_spinning(false);
                match report {
                    // Under a hundredth smaller is not worth replacing a file for.
                    Ok(report) if report.after < report.before - report.before / 100 => {
                        let saved = 100.0 * (1.0 - report.after as f64 / report.before.max(1) as f64);
                        result.set_title(&format!("{} → {}", readable(report.before), readable(report.after)));
                        result.set_subtitle(&match report.photos {
                            0 => format!("{saved:.0}% smaller"),
                            1 => format!("{saved:.0}% smaller, one photo scaled down"),
                            n => format!("{saved:.0}% smaller, {n} photos scaled down"),
                        });
                        state.ready.replace(Some((written, report)));
                        replace.set_sensitive(true);
                        copy.set_sensitive(true);
                    }
                    Ok(report) => {
                        let _ = std::fs::remove_file(&written);
                        result.set_title(&format!("Already {}", readable(report.before)));
                        result.set_subtitle("This would not make it any smaller.");
                    }
                    Err(error) => {
                        result.set_title("Could Not Make It Smaller");
                        result.set_subtitle(&error);
                    }
                }
            });
        })
    };

    for (button, (_, _, level)) in buttons.iter().zip(CHOICES) {
        let attempt = attempt.clone();
        let level = *level;
        button.connect_toggled(move |button| {
            if button.is_active() {
                attempt(level);
            }
        });
    }
    // Reading on screen is what most people shrinking a PDF want.
    buttons[1].set_active(true);

    cancel.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            dialog.close();
        }
    ));
    {
        let (state, done) = (state.clone(), done.clone());
        let dialog = dialog.downgrade();
        let result = result.clone();
        replace.connect_clicked(move |_| {
            let Some((written, report)) = state.ready.take() else { return };
            match pdf::install(&written, &state.path) {
                Ok(()) => {
                    if let Some(dialog) = dialog.upgrade() {
                        dialog.close();
                    }
                    done(Outcome::Replaced { before: report.before, after: report.after });
                }
                Err(error) => {
                    let _ = std::fs::remove_file(&written);
                    result.set_title("Could Not Save");
                    result.set_subtitle(&error);
                }
            }
        });
    }
    {
        let dialog_weak = dialog.downgrade();
        copy.connect_clicked(move |_| {
            let Some(dialog) = dialog_weak.upgrade() else { return };
            let (state, done, dialog_weak, result) = (state.clone(), done.clone(), dialog_weak.clone(), result.clone());
            let original = state.path.clone();
            crate::app::protect::save_copy_as(&dialog, &original, "smaller", move |destination| {
                let Some((written, report)) = state.ready.take() else { return };
                match pdf::keep_as(&written, &destination) {
                    Ok(()) => {
                        if let Some(dialog) = dialog_weak.upgrade() {
                            dialog.close();
                        }
                        done(Outcome::Copied { path: destination, before: report.before, after: report.after });
                    }
                    Err(error) => {
                        let _ = std::fs::remove_file(&written);
                        result.set_title("Could Not Save the Copy");
                        result.set_subtitle(&error);
                    }
                }
            });
        });
    }
    dialog.present(Some(parent));
}
