// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Updates, as the reader sees them.
//!
//! GitHub is asked for the newest version when Glance starts, if it has not
//! been asked in the last few hours, and every few hours after that. What it
//! says is remembered, so an update once found is shown at every start until
//! it is in, network or not.
//!
//! With **Update automatically** on, as it starts, a new version is
//! downloaded and put in by itself, and a button in the header offers to
//! restart into it. The Arch package is the exception: pacman needs the
//! administrator's password, so the update is downloaded by itself and the
//! button asks for the password. With it off, the button says an update is
//! available, and nothing happens until it is pressed.
//!
//! One state for the whole program, however many windows it has open.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::glib;

use crate::app::prefs;
use crate::app::window::Window;
use crate::update::{self, Install, Version};

/// How long a check is good for.
const STALE: i64 = 6 * 60 * 60;

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Nothing newer known.
    Idle,
    Available(Version),
    Downloading(Version, f64),
    /// Downloaded, waiting for the password to install it.
    Downloaded(Version),
    Installing(Version),
    /// In, and used from the next start.
    Ready(Version),
}

type Watcher = Box<dyn Fn(&Status) -> bool>;

pub struct Updates {
    install: Install,
    status: RefCell<Status>,
    /// Called with every change of status; one answering false is dropped.
    watchers: RefCell<Vec<Watcher>>,
    started: Cell<bool>,
    checking: Cell<bool>,
    /// What the last check, or the last try at updating, came to.
    note: RefCell<Option<String>>,
    /// Flathub keeps this copy up to date, not Glance.
    managed: Cell<bool>,
    /// Updating by itself failed this run, so it is not tried again unasked.
    gave_up: Cell<bool>,
    file: RefCell<Option<PathBuf>>,
    restart_asked: Cell<Option<Instant>>,
}

thread_local! {
    static SHARED: Rc<Updates> = Rc::new(Updates {
        install: Install::detect(),
        status: RefCell::new(Status::Idle),
        watchers: RefCell::default(),
        started: Cell::new(false),
        checking: Cell::new(false),
        note: RefCell::default(),
        managed: Cell::new(false),
        gave_up: Cell::new(false),
        file: RefCell::default(),
        restart_asked: Cell::new(None),
    });
}

pub fn shared() -> Rc<Updates> {
    SHARED.with(Rc::clone)
}

fn now() -> i64 {
    glib::real_time() / 1_000_000
}

/// Change what is remembered, starting from what is on disk, so a window's
/// own copy of the preferences is not written back over it.
fn remember(change: impl FnOnce(&mut prefs::Reader)) {
    let mut reader = prefs::Reader::load();
    change(&mut reader);
    reader.save();
}

impl Updates {
    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }

    pub fn note(&self) -> Option<String> {
        self.note.borrow().clone()
    }

    pub fn is_checking(&self) -> bool {
        self.checking.get()
    }

    /// Whether this copy can put an update in at all.
    pub fn can_install(&self) -> bool {
        self.install.asset().is_some() && !self.managed.get()
    }

    pub fn watch(&self, watcher: impl Fn(&Status) -> bool + 'static) {
        self.watchers.borrow_mut().push(Box::new(watcher));
    }

    fn set(&self, status: Status) {
        self.status.replace(status.clone());
        self.notify();
    }

    fn notify(&self) {
        let status = self.status();
        let watchers = std::mem::take(&mut *self.watchers.borrow_mut());
        let kept: Vec<Watcher> = watchers.into_iter().filter(|watch| watch(&status)).collect();
        // Any added while these ran go after them.
        let mut list = self.watchers.borrow_mut();
        let added = std::mem::take(&mut *list);
        *list = kept;
        list.extend(added);
    }

    /// Once per run: show what is already known, and ask again if that is
    /// old, then every few hours.
    pub fn start(self: &Rc<Self>) {
        if self.started.replace(true) {
            return;
        }
        let known = prefs::Reader::load();
        if let Some(newest) = known.newest.filter(|&v| v > Version::current()) {
            self.set(Status::Available(newest));
        }
        if now() - known.update_checked >= STALE {
            self.check();
        } else {
            self.maybe_update_by_itself();
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(STALE as u32, move || match weak.upgrade() {
            Some(updates) => {
                updates.check();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
    }

    /// Ask GitHub for the newest version, on a thread.
    pub fn check(self: &Rc<Self>) {
        if self.checking.replace(true) {
            return;
        }
        self.notify();
        let flatpak = matches!(self.install, Install::Flatpak { .. });
        let (sender, receiver) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let managed = flatpak && update::flatpak_from_flathub();
            let _ = sender.send_blocking((update::latest(), managed));
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok((result, managed)) = receiver.recv().await else { return };
            let Some(updates) = weak.upgrade() else { return };
            updates.checking.set(false);
            updates.managed.set(managed);
            match result {
                Ok(newest) => {
                    remember(|p| {
                        p.update_checked = now();
                        p.newest = Some(newest);
                    });
                    updates.note.replace(None);
                    if newest > Version::current() {
                        // Already on its way, or in: left as it is.
                        if matches!(updates.status(), Status::Idle | Status::Available(_)) {
                            updates.set(Status::Available(newest));
                        }
                    } else if matches!(updates.status(), Status::Available(_)) {
                        updates.set(Status::Idle);
                    }
                }
                Err(error) => {
                    updates.note.replace(Some(error));
                }
            }
            updates.notify();
            updates.maybe_update_by_itself();
        });
    }

    /// With updating by itself on, start on an update that is available.
    fn maybe_update_by_itself(self: &Rc<Self>) {
        let Status::Available(version) = self.status() else { return };
        if prefs::Reader::load().auto_update && self.can_install() && !self.gave_up.get() {
            self.begin(version, false);
        }
    }

    /// Download `version`, and put it in if that needs nothing asked, or the
    /// reader asked for it.
    pub fn begin(self: &Rc<Self>, version: Version, asked: bool) {
        if !matches!(self.status(), Status::Idle | Status::Available(_)) || !self.can_install() {
            return;
        }
        self.set(Status::Downloading(version, 0.0));
        let install = self.install.clone();
        let quietly = install.installs_quietly();
        let (sender, receiver) = async_channel::unbounded();
        std::thread::spawn(move || {
            let progress = |done: f64| {
                let _ = sender.send_blocking(Step::Progress(done));
            };
            let result = update::download(version, &install, &progress);
            let result = match result {
                Ok(file) if quietly => {
                    let _ = sender.send_blocking(Step::Installing);
                    update::install(&install, &file).map(|()| None)
                }
                Ok(file) => Ok(Some(file)),
                Err(e) => Err(e),
            };
            let _ = sender.send_blocking(Step::Done(result));
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(step) = receiver.recv().await {
                let Some(updates) = weak.upgrade() else { return };
                match step {
                    Step::Progress(done) => updates.set(Status::Downloading(version, done)),
                    Step::Installing => updates.set(Status::Installing(version)),
                    Step::Done(Ok(None)) => updates.set(Status::Ready(version)),
                    Step::Done(Ok(Some(file))) => {
                        updates.file.replace(Some(file));
                        updates.set(Status::Downloaded(version));
                        // Asked for, so straight on to the password.
                        if asked {
                            updates.install_downloaded();
                        }
                    }
                    Step::Done(Err(error)) => updates.failed(version, error),
                }
            }
        });
    }

    /// Put in a downloaded update that needs the password.
    pub fn install_downloaded(self: &Rc<Self>) {
        let Status::Downloaded(version) = self.status() else { return };
        let Some(file) = self.file.borrow().clone() else { return };
        self.set(Status::Installing(version));
        let install = self.install.clone();
        let (sender, receiver) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = sender.send_blocking(update::install(&install, &file));
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(result) = receiver.recv().await else { return };
            let Some(updates) = weak.upgrade() else { return };
            match result {
                Ok(()) => {
                    updates.file.replace(None);
                    updates.set(Status::Ready(version));
                }
                Err(error) => {
                    // Still downloaded: the button offers it again.
                    updates.note.replace(Some(error));
                    updates.set(Status::Downloaded(version));
                }
            }
        });
    }

    fn failed(&self, version: Version, error: String) {
        self.gave_up.set(true);
        self.note.replace(Some(error));
        self.set(Status::Available(version));
    }

    /// End this run, each window asking about unsaved work as usual, and
    /// start the new version once it has ended.
    pub fn restart(&self, window: &Window) {
        self.restart_asked.set(Some(Instant::now()));
        if let Some(app) = window.application() {
            app.activate_action("quit", None);
        }
    }

    /// Called as the program ends: start the new version, if a restart was
    /// asked for just now — not if a window with unsaved work stopped it and
    /// the program ended long after.
    pub fn relaunch_if_asked(&self) {
        let asked = self.restart_asked.get().is_some_and(|at| at.elapsed() < Duration::from_secs(120));
        if asked {
            if let Some(mut command) = update::relaunch_command(&self.install) {
                let _ = command.spawn();
            }
        }
    }

    /// For Preferences: what Update automatically means for this copy.
    pub fn explain(&self) -> &'static str {
        if self.managed.get() {
            return "Flathub keeps this copy up to date: updates come through your software centre";
        }
        match self.install {
            Install::AppImage(_) | Install::Flatpak { .. } if self.install.asset().is_some() => {
                "New versions are downloaded and installed by themselves, and used from the next start"
            }
            Install::Pacman if self.install.asset().is_some() => {
                "New versions are downloaded by themselves; installing one asks for your password"
            }
            _ => "This copy was built from source or installed by hand, so new versions are only announced",
        }
    }
}

enum Step {
    Progress(f64),
    Installing,
    Done(Result<Option<PathBuf>, String>),
}

/// The header's update button, for one window: hidden until there is news.
pub fn header_button(window: &Window) -> gtk::Button {
    let content = adw::ButtonContent::builder().icon_name("software-update-available-symbolic").build();
    let button = gtk::Button::builder().child(&content).visible(false).css_classes(["suggested-action"]).build();
    let updates = shared();

    let show = {
        let (button, content) = (button.downgrade(), content.downgrade());
        let window = window.downgrade();
        let last = Cell::new(None::<Version>);
        move |status: &Status| -> bool {
            let (Some(button), Some(content), Some(window)) = (button.upgrade(), content.upgrade(), window.upgrade()) else {
                return false;
            };
            let (label, tip, enabled) = match status {
                Status::Idle => {
                    button.set_visible(false);
                    return true;
                }
                Status::Available(v) => ("Update Available".to_string(), format!("Glance {v} is available"), true),
                Status::Downloading(v, done) => {
                    (format!("Updating… {}%", (done * 100.0).round()), format!("Downloading Glance {v}"), false)
                }
                Status::Downloaded(v) => {
                    ("Install Update".to_string(), format!("Glance {v} is downloaded; installing it asks for your password"), true)
                }
                Status::Installing(v) => ("Updating…".to_string(), format!("Installing Glance {v}"), false),
                Status::Ready(v) => {
                    // Said once, as it happens.
                    if last.replace(Some(*v)) != Some(*v) {
                        window.toast(&format!("Glance {v} is installed. Restart Glance to use it."));
                    }
                    ("Restart to Update".to_string(), format!("Glance {v} is installed; restart to use it"), true)
                }
            };
            content.set_label(&label);
            button.set_tooltip_text(Some(&tip));
            button.set_sensitive(enabled);
            button.set_visible(true);
            true
        }
    };
    show(&updates.status());
    updates.watch(show);

    button.connect_clicked(glib::clone!(
        #[weak]
        window,
        move |_| {
            let updates = shared();
            match updates.status() {
                Status::Available(version) => offer(&window, version),
                Status::Downloaded(_) => updates.install_downloaded(),
                Status::Ready(_) => updates.restart(&window),
                _ => {}
            }
        }
    ));
    updates.start();
    button
}

/// The dialog an available update opens: what it is, and what pressing
/// Update will do here.
fn offer(window: &Window, version: Version) {
    let updates = shared();
    let current = Version::current();
    let mut body = format!("You have Glance {current}.");
    if let Some(note) = updates.note() {
        body.push_str(&format!("\n\nThe last try did not work: {note}"));
    }
    let can = updates.can_install();
    body.push_str(&format!(
        "\n\n{}",
        if updates.managed.get() {
            "Flathub provides this copy: update it from your software centre, or with “flatpak update”."
        } else if !can {
            "This copy cannot update itself. The download page has the new version for every kind of system."
        } else if updates.install.installs_quietly() {
            "Update downloads and installs it now; Glance then restarts into it when you are ready."
        } else {
            "Update downloads it now, and installing it asks for your password."
        }
    ));
    let dialog = adw::AlertDialog::new(Some(&format!("Glance {version} Is Available")), Some(&body));
    dialog.add_response("notes", "_What’s New");
    dialog.add_response("later", "_Later");
    if can {
        dialog.add_response("update", "_Update");
    } else if !updates.managed.get() {
        dialog.add_response("page", "_Download Page");
    }
    for answer in ["update", "page"] {
        dialog.set_response_appearance(answer, adw::ResponseAppearance::Suggested);
    }
    dialog.set_default_response(Some(if can { "update" } else { "later" }));
    dialog.set_close_response("later");
    dialog.connect_response(None, move |dialog, answer| {
        let open = |address: String| {
            let root = dialog.root().and_downcast::<gtk::Window>();
            gtk::UriLauncher::new(&address).launch(root.as_ref(), gtk::gio::Cancellable::NONE, |_| {});
        };
        match answer {
            "notes" => open(version.page()),
            "page" => open(format!("{}/releases/latest", update::repository())),
            "update" => {
                let updates = shared();
                updates.gave_up.set(false);
                updates.begin(version, true);
            }
            _ => {}
        }
    });
    dialog.present(Some(window));
}

/// Preferences' Updates group: updating by itself, the version, and a way
/// to check now.
pub fn preferences_group() -> adw::PreferencesGroup {
    let updates = shared();
    let group = adw::PreferencesGroup::builder().title("Updates").build();
    let automatic = adw::SwitchRow::builder()
        .title("Update automatically")
        .subtitle(updates.explain())
        .active(prefs::Reader::load().auto_update)
        .sensitive(updates.can_install())
        .build();
    automatic.connect_active_notify(|row| {
        let on = row.is_active();
        remember(|p| p.auto_update = on);
        if on {
            shared().maybe_update_by_itself();
        }
    });
    group.add(&automatic);

    let version = adw::ActionRow::builder().title(format!("Glance {}", Version::current())).build();
    let check = gtk::Button::builder().label("_Check Now").use_underline(true).valign(gtk::Align::Center).build();
    version.add_suffix(&check);
    let describe = {
        let (version, check, automatic) = (version.downgrade(), check.downgrade(), automatic.downgrade());
        move |status: &Status| -> bool {
            let (Some(version), Some(check), Some(automatic)) = (version.upgrade(), check.upgrade(), automatic.upgrade()) else {
                return false;
            };
            let updates = shared();
            let text = if updates.is_checking() {
                "Checking for updates…".to_string()
            } else {
                match status {
                    Status::Idle => match updates.note() {
                        Some(note) => note,
                        None => "Up to date".to_string(),
                    },
                    Status::Available(v) => format!("Glance {v} is available"),
                    Status::Downloading(v, done) => format!("Downloading Glance {v}… {}%", (done * 100.0).round()),
                    Status::Downloaded(v) => format!("Glance {v} is downloaded, ready to install"),
                    Status::Installing(v) => format!("Installing Glance {v}…"),
                    Status::Ready(v) => format!("Glance {v} is installed; restart to use it"),
                }
            };
            version.set_subtitle(&text);
            check.set_sensitive(!updates.is_checking() && matches!(status, Status::Idle | Status::Available(_)));
            automatic.set_subtitle(updates.explain());
            automatic.set_sensitive(updates.can_install());
            true
        }
    };
    describe(&updates.status());
    updates.watch(describe);
    check.connect_clicked(|_| shared().check());
    group.add(&version);
    group
}
