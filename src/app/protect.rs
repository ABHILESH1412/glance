// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Password and Permissions: who may open a PDF, and what they may do with it.
//!
//! Two passwords, as the PDF standard has them. The one to open the document,
//! if it has one. And the permissions password, which is what lets someone
//! change the permissions later. Holding back printing, copying or marking up
//! needs one, and it has to differ from the password to open: anyone opening
//! the document with the permissions password may do everything.
//!
//! A document whose permissions were set by someone else can only be changed
//! with its permissions password, as in every other PDF program.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::pdf::{self, Permissions, Protection};

/// What the dialog asks for, once it is filled in properly.
#[derive(Clone, Debug, PartialEq)]
enum Choice {
    /// No password and no restrictions.
    Open,
    Protected { open: String, owner: String, allow: Permissions },
}

/// Check what was typed. `Err` says what is missing, for the reader.
fn validate(
    require: bool,
    open: &str,
    open_again: &str,
    allow: Permissions,
    owner: &str,
    owner_again: &str,
) -> Result<Choice, &'static str> {
    let restricted = !(allow.print && allow.copy && allow.annotate && allow.change);
    if require {
        if open.is_empty() {
            return Err("Type the password to open it.");
        }
        if open != open_again {
            return Err("The two passwords to open it are not the same.");
        }
    }
    if !restricted {
        return Ok(if require {
            // With nothing held back, the one password does both jobs.
            Choice::Protected { open: open.to_string(), owner: open.to_string(), allow }
        } else {
            Choice::Open
        });
    }
    if owner.is_empty() {
        return Err("Holding something back needs a permissions password.");
    }
    if owner != owner_again {
        return Err("The two permissions passwords are not the same.");
    }
    if require && owner == open {
        return Err("The permissions password must differ from the password to open it.");
    }
    let open = if require { open.to_string() } else { String::new() };
    Ok(Choice::Protected { open, owner: owner.to_string(), allow })
}

/// What happened, for the window: the file was replaced, and opens with this
/// password now, or a copy was saved.
pub enum Outcome {
    Replaced { password: Option<String> },
    Copied(PathBuf),
}

struct Form {
    require: adw::SwitchRow,
    open: adw::PasswordEntryRow,
    open_again: adw::PasswordEntryRow,
    print: adw::SwitchRow,
    copy: adw::SwitchRow,
    annotate: adw::SwitchRow,
    change: adw::SwitchRow,
    owner: adw::PasswordEntryRow,
    owner_again: adw::PasswordEntryRow,
    problem: gtk::Label,
    apply: gtk::Button,
    copy_button: gtk::Button,
}

impl Form {
    fn allow(&self) -> Permissions {
        Permissions {
            print: self.print.is_active(),
            copy: self.copy.is_active(),
            annotate: self.annotate.is_active(),
            change: self.change.is_active(),
        }
    }

    fn choice(&self) -> Result<Choice, &'static str> {
        validate(
            self.require.is_active(),
            &self.open.text(),
            &self.open_again.text(),
            self.allow(),
            &self.owner.text(),
            &self.owner_again.text(),
        )
    }

    /// Show only what applies, and say what is still missing.
    fn refresh(&self) {
        let require = self.require.is_active();
        self.open.set_visible(require);
        self.open_again.set_visible(require);
        let allow = self.allow();
        let restricted = !(allow.print && allow.copy && allow.annotate && allow.change);
        self.owner.set_visible(restricted);
        self.owner_again.set_visible(restricted);
        let result = self.choice();
        self.problem.set_text(result.as_ref().err().copied().unwrap_or(""));
        self.problem.set_visible(result.is_err());
        self.apply.set_sensitive(result.is_ok());
        self.copy_button.set_sensitive(result.is_ok());
    }
}

/// Open the dialog for the PDF at `path`, opened with `password`.
pub fn present(
    parent: &impl IsA<gtk::Widget>,
    path: PathBuf,
    password: Option<String>,
    allowed: pdf::Allowed,
    done: impl Fn(Outcome) + 'static,
) {
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar.add_top_bar(&header);
    let dialog = adw::Dialog::builder()
        .title("Password and Permissions")
        .content_width(460)
        .content_height(680)
        .child(&toolbar)
        .build();
    let done: Rc<dyn Fn(Outcome)> = Rc::new(done);
    let stack = gtk::Stack::new();
    toolbar.set_content(Some(&stack));

    let form_page = build_form(&dialog, &header, &path, done.clone());
    stack.add_named(&form_page.0, Some("form"));

    if allowed.everything {
        form_page.1(password, None);
        stack.set_visible_child_name("form");
    } else {
        // Someone else's restrictions: their permissions password first.
        let ask = ask_owner_password(&path, {
            let stack = stack.clone();
            let show_form = form_page.1.clone();
            move |owner| {
                show_form(password.clone(), Some(owner));
                stack.set_visible_child_name("form");
            }
        });
        stack.add_named(&ask, Some("ask"));
        stack.set_visible_child_name("ask");
    }
    dialog.present(Some(parent));
}

/// Asking for the permissions password of a document someone else restricted.
fn ask_owner_password(path: &Path, unlocked: impl Fn(String) + 'static) -> gtk::Widget {
    let entry = adw::PasswordEntryRow::builder().title("Permissions Password").build();
    let group = adw::PreferencesGroup::new();
    group.add(&entry);
    let unlock = gtk::Button::builder()
        .label("_Unlock")
        .use_underline(true)
        .halign(gtk::Align::Center)
        .css_classes(["pill", "suggested-action"])
        .sensitive(false)
        .build();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 18);
    column.append(&group);
    column.append(&unlock);
    let page = adw::StatusPage::builder()
        .icon_name("system-lock-screen-symbolic")
        .title("Set by Its Author")
        .description("The permissions on this document were set by whoever made it. Enter its permissions password to change them.")
        .child(&column)
        .build();
    page.add_css_class("compact");

    entry.connect_changed(glib::clone!(
        #[weak]
        unlock,
        move |entry| {
            unlock.set_sensitive(!entry.text().is_empty());
            entry.remove_css_class("error");
        }
    ));
    let uri = gio::File::for_path(path).uri().to_string();
    let check = Rc::new(move |entry: &adw::PasswordEntryRow| {
        let typed = entry.text().to_string();
        if typed.is_empty() {
            return;
        }
        // The permissions password opens the document with everything
        // allowed; anything else is not it.
        let owner = poppler::Document::from_file(&uri, Some(&typed))
            .ok()
            .filter(|document| pdf::Allowed::of(document).everything);
        if owner.is_some() {
            unlocked(typed);
        } else {
            entry.add_css_class("error");
        }
    });
    entry.connect_entry_activated(glib::clone!(
        #[strong]
        check,
        move |entry| check(entry)
    ));
    unlock.connect_clicked(glib::clone!(
        #[weak]
        entry,
        move |_| check(&entry)
    ));
    page.upcast()
}

/// The form itself, and a way to fill it in for a password.
#[allow(clippy::type_complexity)]
fn build_form(
    dialog: &adw::Dialog,
    header: &adw::HeaderBar,
    path: &Path,
    done: Rc<dyn Fn(Outcome)>,
) -> (gtk::Widget, Rc<dyn Fn(Option<String>, Option<String>)>) {
    let switch = |title: &str, subtitle: &str| {
        adw::SwitchRow::builder().title(title).subtitle(subtitle).active(true).build()
    };
    let require = adw::SwitchRow::builder()
        .title("Require a Password")
        .subtitle("Nobody can open the document without it")
        .build();
    let open = adw::PasswordEntryRow::builder().title("Password").build();
    let open_again = adw::PasswordEntryRow::builder().title("Password Again").build();
    let opening = adw::PreferencesGroup::builder().title("Opening").build();
    opening.add(&require);
    opening.add(&open);
    opening.add(&open_again);

    let print = switch("Printing", "");
    let copy = switch("Copying Text and Pictures", "");
    let annotate = switch("Adding Notes and Marks", "Highlights, notes, drawings and text");
    let change = switch("Changing Pages and Forms", "Adding, removing or turning pages, and filling in forms");
    let owner = adw::PasswordEntryRow::builder().title("Permissions Password").build();
    let owner_again = adw::PasswordEntryRow::builder().title("Permissions Password Again").build();
    let permissions = adw::PreferencesGroup::builder()
        .title("Permissions")
        .description("What anyone may do without the permissions password.")
        .build();
    for row in [&print, &copy, &annotate, &change] {
        permissions.add(row);
    }
    permissions.add(&owner);
    permissions.add(&owner_again);

    let problem = gtk::Label::builder().wrap(true).xalign(0.0).visible(false).build();
    problem.add_css_class("warning");
    let note = gtk::Label::builder()
        .label("Keep the passwords somewhere safe. A forgotten password cannot be recovered, by Glance or anyone else.")
        .wrap(true)
        .xalign(0.0)
        .build();
    note.add_css_class("dim-label");
    note.add_css_class("caption");

    let apply = gtk::Button::builder().label("_Apply").use_underline(true).css_classes(["suggested-action"]).build();
    apply.set_tooltip_text(Some("Change this file"));
    // Shown with the form, not while a permissions password is asked for.
    apply.set_visible(false);
    let copy_button = gtk::Button::builder()
        .label("Save as _Copy…")
        .use_underline(true)
        .halign(gtk::Align::Center)
        .css_classes(["pill"])
        .build();
    let cancel = gtk::Button::with_mnemonic("_Cancel");
    header.set_show_start_title_buttons(false);
    header.set_show_end_title_buttons(false);
    header.pack_start(&cancel);
    header.pack_end(&apply);
    let spinner = gtk::Spinner::new();
    header.pack_end(&spinner);

    let column = gtk::Box::new(gtk::Orientation::Vertical, 24);
    column.append(&opening);
    column.append(&permissions);
    column.append(&problem);
    column.append(&note);
    column.append(&copy_button);
    column.set_margin_top(12);
    column.set_margin_bottom(24);
    column.set_margin_start(12);
    column.set_margin_end(12);
    let clamp = adw::Clamp::builder().maximum_size(520).child(&column).build();
    let scroller = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&clamp).build();

    let form = Rc::new(Form {
        require,
        open,
        open_again,
        print,
        copy,
        annotate,
        change,
        owner,
        owner_again,
        problem,
        apply: apply.clone(),
        copy_button: copy_button.clone(),
    });
    // The password the document opened with, to read it with.
    let read_with: Rc<RefCell<Option<String>>> = Rc::default();

    for row in [&form.require, &form.print, &form.copy, &form.annotate, &form.change] {
        let form = Rc::downgrade(&form);
        row.connect_active_notify(move |_| {
            if let Some(form) = form.upgrade() {
                form.refresh();
            }
        });
    }
    for row in [&form.open, &form.open_again, &form.owner, &form.owner_again] {
        let form = Rc::downgrade(&form);
        row.connect_changed(move |_| {
            if let Some(form) = form.upgrade() {
                form.refresh();
            }
        });
    }
    cancel.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            dialog.close();
        }
    ));

    // Fill in what the document has now.
    // `opened` is the password the document was opened with, `owner` its
    // permissions password when that was asked for here.
    let fill: Rc<dyn Fn(Option<String>, Option<String>)> = {
        let form = form.clone();
        let read_with = read_with.clone();
        let path = path.to_path_buf();
        Rc::new(move |opened: Option<String>, owner: Option<String>| {
            form.apply.set_visible(true);
            let uri = gio::File::for_path(&path).uri();
            // What a password does: open the document at all, and with
            // everything allowed or not.
            let grants = |password: &str| {
                poppler::Document::from_file(&uri, Some(password)).ok().map(|d| pdf::Allowed::of(&d).everything)
            };
            let needs_password = poppler::Document::from_file(&uri, None).is_err();
            let now = pdf::current_permissions(&path, owner.as_deref().or(opened.as_deref()));
            let restricted = now.is_some_and(|n| !(n.print && n.copy && n.annotate && n.change));
            // The document may have been opened with its permissions password,
            // which is not the one to open it.
            let (open, owner) = match (opened, owner) {
                (Some(p), owner) if restricted && owner.is_none() && grants(&p) == Some(true) => (None, Some(p)),
                (opened, owner) => (opened, owner),
            };
            read_with.replace(owner.clone().or_else(|| open.clone()));
            form.require.set_active(needs_password);
            if let (true, Some(open)) = (needs_password, &open) {
                form.open.set_text(open);
                form.open_again.set_text(open);
            }
            if let Some(now) = now {
                form.print.set_active(now.print);
                form.copy.set_active(now.copy);
                form.annotate.set_active(now.annotate);
                form.change.set_active(now.change);
            }
            // The permissions password stays as it is unless changed.
            if let (true, Some(owner)) = (restricted, &owner) {
                form.owner.set_text(owner);
                form.owner_again.set_text(owner);
            }
            form.refresh();
        })
    };

    // Write the new version beside the file, in the background, then hand it
    // to `finish`.
    let run = {
        let form = form.clone();
        let read_with = read_with.clone();
        let path = path.to_path_buf();
        let spinner = spinner.clone();
        Rc::new(move |finish: Box<dyn Fn(PathBuf, Option<String>)>| {
            let Ok(choice) = form.choice() else { return };
            form.apply.set_sensitive(false);
            form.copy_button.set_sensitive(false);
            spinner.set_spinning(true);
            let temporary = pdf::temporary_beside(&path, "protect");
            let (source, written, password) = (path.clone(), temporary.clone(), read_with.borrow().clone());
            let new_password = match &choice {
                Choice::Protected { open, .. } if !open.is_empty() => Some(open.clone()),
                _ => None,
            };
            let (sender, receiver) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let protection = match &choice {
                    Choice::Open => Protection::Remove,
                    Choice::Protected { open, owner, allow } => Protection::Set { open, owner, allow: *allow },
                };
                let mut result = pdf::protect(&source, password.as_deref(), protection, &written);
                // Never put a file in place that its new password does not open.
                if result.is_ok() && !pdf::opens_with(&written, Some(open_of(&choice))) {
                    result = Err("the new file did not open with its password".into());
                }
                if result.is_err() {
                    let _ = std::fs::remove_file(&written);
                }
                let _ = sender.send_blocking(result);
            });
            let form = form.clone();
            let spinner = spinner.clone();
            glib::spawn_future_local(async move {
                let result = receiver.recv().await.unwrap_or_else(|_| Err("stopped unexpectedly".into()));
                spinner.set_spinning(false);
                form.refresh();
                match result {
                    Ok(()) => finish(temporary, new_password),
                    Err(error) => {
                        form.problem.set_text(&format!("Could not protect the document: {error}"));
                        form.problem.set_visible(true);
                    }
                }
            });
        })
    };

    {
        let run = run.clone();
        let done = done.clone();
        let path = path.to_path_buf();
        let dialog = dialog.downgrade();
        let problem = form.problem.clone();
        apply.connect_clicked(move |_| {
            let done = done.clone();
            let path = path.clone();
            let dialog = dialog.clone();
            let problem = problem.clone();
            run(Box::new(move |temporary, password| match pdf::install(&temporary, &path) {
                Ok(()) => {
                    if let Some(dialog) = dialog.upgrade() {
                        dialog.close();
                    }
                    done(Outcome::Replaced { password: password.clone() });
                }
                Err(error) => {
                    let _ = std::fs::remove_file(&temporary);
                    problem.set_text(&format!("Could not save the document: {error}"));
                    problem.set_visible(true);
                }
            }));
        });
    }
    {
        let path = path.to_path_buf();
        let dialog_weak = dialog.downgrade();
        let problem = form.problem.clone();
        copy_button.connect_clicked(move |_| {
            let Some(dialog) = dialog_weak.upgrade() else { return };
            let run = run.clone();
            let done = done.clone();
            let dialog_weak = dialog_weak.clone();
            let problem = problem.clone();
            save_copy_as(&dialog, &path, "protected", move |destination| {
                let done = done.clone();
                let dialog_weak = dialog_weak.clone();
                let problem = problem.clone();
                run(Box::new(move |temporary, _| match pdf::keep_as(&temporary, &destination) {
                    Ok(()) => {
                        if let Some(dialog) = dialog_weak.upgrade() {
                            dialog.close();
                        }
                        done(Outcome::Copied(destination.clone()));
                    }
                    Err(error) => {
                        let _ = std::fs::remove_file(&temporary);
                        problem.set_text(&format!("Could not save the copy: {error}"));
                        problem.set_visible(true);
                    }
                }));
            });
        });
    }
    (scroller.upcast(), fill)
}

fn open_of(choice: &Choice) -> &str {
    match choice {
        Choice::Open => "",
        Choice::Protected { open, .. } => open,
    }
}

/// Ask where to keep a copy, suggesting "name (what).pdf" beside the original.
pub fn save_copy_as(parent: &impl IsA<gtk::Widget>, original: &Path, what: &str, chosen: impl Fn(PathBuf) + 'static) {
    let stem = original.file_stem().map_or_else(|| "document".into(), |s| s.to_string_lossy().into_owned());
    // The copy keeps the original's kind: a picture stays the picture it was.
    let extension = original.extension().map_or_else(|| "pdf".into(), |e| e.to_string_lossy().into_owned());
    let chooser = gtk::FileDialog::builder()
        .title("Save a Copy")
        .initial_name(format!("{stem} ({what}).{extension}"))
        .modal(true)
        .build();
    if extension.eq_ignore_ascii_case("pdf") {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("PDF documents"));
        filter.add_mime_type("application/pdf");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        chooser.set_filters(Some(&filters));
    }
    if let Some(folder) = original.parent() {
        chooser.set_initial_folder(Some(&gio::File::for_path(folder)));
    }
    let window = parent.root().and_downcast::<gtk::Window>();
    chooser.save(window.as_ref(), gio::Cancellable::NONE, move |result| {
        if let Some(path) = result.ok().and_then(|file| file.path()) {
            chosen(path);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: Permissions = Permissions { print: true, copy: true, annotate: true, change: true };
    const NO_COPY: Permissions = Permissions { print: true, copy: false, annotate: true, change: true };

    #[test]
    fn a_password_alone_does_both_jobs() {
        assert_eq!(
            validate(true, "open", "open", ALL, "", ""),
            Ok(Choice::Protected { open: "open".into(), owner: "open".into(), allow: ALL })
        );
        assert_eq!(validate(false, "", "", ALL, "", ""), Ok(Choice::Open));
    }

    #[test]
    fn passwords_must_be_typed_the_same_twice() {
        assert!(validate(true, "open", "opne", ALL, "", "").is_err());
        assert!(validate(true, "", "", ALL, "", "").is_err());
        assert!(validate(false, "", "", NO_COPY, "own", "onw").is_err());
    }

    #[test]
    fn holding_back_needs_its_own_password() {
        assert!(validate(false, "", "", NO_COPY, "", "").is_err());
        assert!(validate(true, "same", "same", NO_COPY, "same", "same").is_err(), "it must differ");
        assert_eq!(
            validate(false, "ignored", "", NO_COPY, "own", "own"),
            Ok(Choice::Protected { open: String::new(), owner: "own".into(), allow: NO_COPY })
        );
    }
}
