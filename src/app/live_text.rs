// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Live Text in the window: asking before the one-time download, showing it
//! happen, and saying plainly what went wrong if it did not work.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};

use crate::app::window::Window;
use crate::images::live_layer::{LiveLayer, TextLine};
use crate::live_text::client::{self, Helper};
use crate::live_text::install::{self, Part, Paths};
use crate::live_text::protocol::{self, Reply};

/// How long the helper is kept after its last read, ready for the next
/// picture, before it is let go and its memory with it.
const IDLE: Duration = Duration::from_secs(30);
/// Pictures are sent no bigger than this on their longest side; the helper
/// reads at 2000 anyway, and a 50-megapixel photo is 200 MB of pixels.
const SEND_SIDE: u32 = 2048;
/// How many pictures' text is remembered.
const REMEMBERED: usize = 32;
/// Said by the client when the helper's answers end: it has gone.
const GONE: u64 = u64::MAX;

/// Which picture some text was read from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    path: PathBuf,
    /// The frame of an animation, or the page of a PDF.
    frame: usize,
    modified: Option<SystemTime>,
    /// For a picture on a PDF's page, where it is, in hundredths of a point.
    area: Option<[i64; 4]>,
}

/// What to do with text once it has been read.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Use {
    /// Show it over the picture on screen.
    Picture,
    /// Show it over a picture on a PDF's page, to select and copy.
    PdfShow(crate::pdf::PageImage),
    /// Copy all of it, from a picture on a PDF's page.
    PdfCopy,
}

struct Pending {
    id: u64,
    key: Key,
    /// From the pixels sent to the picture's own units, and where those
    /// start: a picture on a page is measured in the page's points.
    scale: (f64, f64),
    offset: (f64, f64),
    then: Use,
    lines: Vec<protocol::Line>,
}

/// A picture ready to send, once the helper is free.
struct Waiting {
    pending: Pending,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

/// What Live Text keeps between pictures.
#[derive(Default)]
pub struct LiveState {
    paths: Option<Paths>,
    helper: Option<Helper>,
    /// Which helper is current, so answers from one already let go of are
    /// not taken for the new one's.
    helper_generation: u64,
    idle: Option<glib::SourceId>,
    next_id: u64,
    /// The picture the helper is reading now, and the one to read after
    /// it. Only ever one of each: moving on through pictures quickly
    /// replaces the one waiting, rather than queueing every one passed.
    reading: Option<Pending>,
    waiting: Option<Waiting>,
    read: HashMap<Key, Vec<TextLine>>,
}

/// What to do once Live Text's parts are in place.
type Then = Rc<RefCell<Option<Box<dyn FnOnce(&Window, Paths)>>>>;

impl Window {
    /// Run `then` with Live Text's files, downloading them first, after
    /// asking, if they are not here yet.
    pub(crate) fn with_live_text(&self, then: impl FnOnce(&Window, Paths) + 'static) {
        if install::runtime().is_none() {
            self.toast("Live Text is not available on this kind of computer.");
            return;
        }
        if let Some(paths) = install::installed() {
            then(self, paths);
            return;
        }
        let then: Then = Rc::new(RefCell::new(Some(Box::new(then))));
        self.ask_to_download(then);
    }

    fn ask_to_download(&self, then: Then) {
        let missing = install::missing();
        let size: u64 = missing.iter().map(|part| part.size).sum();
        let dialog = adw::AlertDialog::new(
            Some("Download Live Text?"),
            Some(&format!(
                "Reading the text in pictures needs a one-time download of {}: the text engine, and \
                 what it needs to read English and about 45 other languages written in the Latin \
                 alphabet. It is kept in your home folder for next time.\n\nYour pictures never leave \
                 your computer.",
                glib::format_size(size)
            )),
        );
        dialog.add_response("cancel", "_Cancel");
        dialog.add_response("download", "_Download");
        dialog.set_response_appearance("download", adw::ResponseAppearance::Suggested);
        // Live Text was asked for, so going ahead is the default; Escape and
        // clicking away still leave it.
        dialog.set_default_response(Some("download"));
        dialog.set_close_response("cancel");
        dialog.connect_response(
            Some("download"),
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, _| window.download_live_text(install::missing(), then.clone())
            ),
        );
        dialog.present(Some(self));
    }

    fn download_live_text(&self, parts: Vec<&'static Part>, then: Then) {
        let total: u64 = parts.iter().map(|part| part.size).sum();
        let what = gtk::Label::new(parts.first().map(|part| part.name));
        what.add_css_class("dim-label");
        what.set_wrap(true);
        let bar = gtk::ProgressBar::new();
        bar.set_show_text(true);
        bar.set_text(Some(&format!("0 of {}", glib::format_size(total))));
        let cancel = gtk::Button::with_mnemonic("_Cancel");
        cancel.add_css_class("pill");
        cancel.set_halign(gtk::Align::Center);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 18);
        column.set_margin_top(24);
        column.set_margin_bottom(24);
        column.set_margin_start(24);
        column.set_margin_end(24);
        column.append(&what);
        column.append(&bar);
        column.append(&cancel);
        let toolbar = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        header.set_show_end_title_buttons(false);
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&column));
        let dialog = adw::Dialog::builder().title("Downloading Live Text").content_width(420).child(&toolbar).build();

        // Cancelling, by the button or by closing the dialog, stops the
        // download between any two reads.
        let cancellable = gio::Cancellable::new();
        cancel.connect_clicked(glib::clone!(
            #[weak]
            dialog,
            move |_| {
                dialog.close();
            }
        ));
        dialog.connect_closed(glib::clone!(
            #[strong]
            cancellable,
            move |_| cancellable.cancel()
        ));
        dialog.present(Some(self));

        let (progress_sender, progress) = async_channel::unbounded::<(u64, u64)>();
        let (result_sender, result) = async_channel::bounded::<Result<(), String>>(1);
        let worker_cancel = cancellable.clone();
        std::thread::spawn(move || {
            let outcome = install::download(
                &parts,
                Box::new(move |done, total| {
                    let _ = progress_sender.send_blocking((done, total));
                }),
                &worker_cancel,
            );
            let _ = result_sender.send_blocking(outcome);
        });

        // Which part is coming in, by how far along the whole download is.
        let names: Vec<(u64, &'static str)> = install::missing()
            .iter()
            .scan(0, |sum, part| {
                *sum += part.size;
                Some((*sum, part.name))
            })
            .collect();
        glib::spawn_future_local(glib::clone!(
            #[weak]
            bar,
            #[weak]
            what,
            async move {
                while let Ok((done, total)) = progress.recv().await {
                    bar.set_fraction(done as f64 / total.max(1) as f64);
                    bar.set_text(Some(&format!("{} of {}", glib::format_size(done), glib::format_size(total))));
                    if let Some((_, name)) = names.iter().find(|(end, _)| done < *end) {
                        what.set_label(name);
                    }
                }
            }
        ));
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                let outcome = result.recv().await.unwrap_or_else(|_| Err("The download stopped unexpectedly.".into()));
                let cancelled = cancellable.is_cancelled();
                dialog.force_close();
                match outcome {
                    Ok(()) => match install::installed() {
                        Some(paths) => {
                            if let Some(then) = then.borrow_mut().take() {
                                then(&window, paths);
                            }
                        }
                        None => window.download_failed("The files arrived, but could not be found afterwards.".into(), then),
                    },
                    Err(_) if cancelled => window.toast("Live Text was not downloaded."),
                    Err(message) => window.download_failed(message, then),
                }
            }
        ));
    }

    fn download_failed(&self, message: String, then: Then) {
        let dialog = adw::AlertDialog::new(
            Some("Could Not Download Live Text"),
            Some(&format!("{message}\n\nCheck the internet connection and try again. Nothing was kept from this attempt.")),
        );
        dialog.add_response("close", "_Close");
        dialog.add_response("retry", "_Try Again");
        dialog.set_response_appearance("retry", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("retry"));
        dialog.set_close_response("close");
        dialog.connect_response(
            Some("retry"),
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, _| window.download_live_text(install::missing(), then.clone())
            ),
        );
        dialog.present(Some(self));
    }
}

impl Window {
    /// The overlay that says text is being read.
    pub(crate) fn build_live_status(&self) {
        let imp = self.imp();
        #[allow(deprecated)]
        let spinner = gtk::Spinner::builder().spinning(true).build();
        let label = gtk::Label::new(Some("Reading text…"));
        let pill = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        pill.add_css_class("osd");
        pill.add_css_class("live-status");
        pill.append(&spinner);
        pill.append(&label);
        let status = &imp.live_status;
        status.set_child(Some(&pill));
        status.set_transition_type(gtk::RevealerTransitionType::Crossfade);
        status.set_halign(gtk::Align::Center);
        status.set_valign(gtk::Align::Start);
        status.set_margin_top(12);
        status.set_can_target(false);
        status.set_reveal_child(false);
    }

    /// Live Text was turned on or off, by the button, the menus or the key.
    pub(crate) fn want_live_text(&self, on: bool) {
        if !on {
            self.live_stop();
            return;
        }
        let imp = self.imp();
        if imp.live_on.get() || imp.showing_pdf.get() || !imp.view.canvas().has_image() {
            return;
        }
        self.with_live_text(|window, paths| window.live_start(paths));
    }

    fn sync_live_state(&self) {
        let on = self.imp().live_on.get();
        if let Some(action) = self.lookup_action("live-text").and_downcast::<gio::SimpleAction>() {
            action.set_state(&on.to_variant());
        }
        for name in ["copy-text", "copy-all-text", "live-select-all"] {
            if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(on);
            }
        }
    }

    fn live_start(&self, paths: Paths) {
        let imp = self.imp();
        imp.live.borrow_mut().paths = Some(paths);
        imp.live_on.set(true);
        self.sync_live_state();
        // An animation holds still on its frame while its text is read.
        imp.view.canvas().set_playing(false);
        self.live_read_current();
    }

    /// Turn Live Text off: the text goes from the picture. The helper is
    /// let go of soon after, as after any read.
    pub(crate) fn live_stop(&self) {
        let imp = self.imp();
        if !imp.live_on.replace(false) {
            return;
        }
        imp.live.borrow_mut().waiting = None;
        imp.live_status.set_reveal_child(false);
        imp.view.canvas().set_live_text(None);
        self.sync_live_state();
        self.live_schedule_idle();
    }

    /// Which picture is on screen, as text read from it is remembered.
    fn live_key(&self) -> Option<Key> {
        let imp = self.imp();
        let path = imp.current.borrow().clone()?;
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        Some(Key { path, frame: imp.view.canvas().frame(), modified, area: None })
    }

    /// Read the picture on screen, or show what was read from it before.
    pub(crate) fn live_read_current(&self) {
        let imp = self.imp();
        if !imp.live_on.get() {
            return;
        }
        let canvas = imp.view.canvas();
        canvas.set_live_text(None);
        let (Some(key), Some(texture), Some(logical)) = (self.live_key(), canvas.texture(), canvas.logical_size()) else {
            return;
        };
        if let Some(lines) = imp.live.borrow().read.get(&key).cloned() {
            imp.live_status.set_reveal_child(false);
            self.live_show(lines);
            return;
        }
        let Some(program) = client::helper_path() else {
            self.toast("Live Text cannot start: its reader, glance-ocr, is not installed with Glance.");
            self.live_stop();
            return;
        };
        if !self.live_ensure_helper(&program) {
            return;
        }

        let id = {
            let mut state = imp.live.borrow_mut();
            state.next_id += 1;
            state.next_id
        };
        imp.live_status.set_reveal_child(true);
        // The pixels on screen: an animation's current frame, a vector at
        // the size it is drawn. Made smaller on a thread before sending.
        let pixels = crate::images::edit::output::pixels_of(&texture).into_rgba8();
        let (sender, receiver) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let (w, h) = pixels.dimensions();
            let ratio = (f64::from(SEND_SIDE) / f64::from(w.max(h))).min(1.0);
            let pixels = if ratio < 1.0 {
                let (nw, nh) = (((f64::from(w) * ratio).round() as u32).max(1), ((f64::from(h) * ratio).round() as u32).max(1));
                image::imageops::resize(&pixels, nw, nh, image::imageops::FilterType::Triangle)
            } else {
                pixels
            };
            let _ = sender.send_blocking(pixels);
        });
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                let Ok(pixels) = receiver.recv().await else { return };
                let imp = window.imp();
                let (width, height) = pixels.dimensions();
                let scale = (logical.0 / f64::from(width), logical.1 / f64::from(height));
                let pending = Pending { id, key, scale, offset: (0.0, 0.0), then: Use::Picture, lines: Vec::new() };
                imp.live.borrow_mut().waiting = Some(Waiting { pending, width, height, rgba: pixels.into_raw() });
                window.live_send();
            }
        ));
    }

    /// Hand the waiting picture to the helper, if it is free.
    fn live_send(&self) {
        let imp = self.imp();
        let sent = {
            let mut state = imp.live.borrow_mut();
            if state.reading.is_some() {
                return;
            }
            let Some(waiting) = state.waiting.take() else { return };
            let id = waiting.pending.id;
            let ok = state.helper.as_ref().is_some_and(|helper| helper.read(id, waiting.width, waiting.height, waiting.rgba));
            if ok {
                state.reading = Some(waiting.pending);
            }
            ok
        };
        if !sent {
            self.live_failed("The text reader stopped unexpectedly.");
        }
    }

    /// Start the helper if it is not running, and keep it from being let go
    /// of while it is needed.
    fn live_ensure_helper(&self, program: &std::path::Path) -> bool {
        let imp = self.imp();
        let mut state = imp.live.borrow_mut();
        if let Some(timer) = state.idle.take() {
            timer.remove();
        }
        if state.helper.is_some() {
            return true;
        }
        let Some(paths) = state.paths.clone() else { return false };
        state.helper_generation += 1;
        let generation = state.helper_generation;
        let window = self.downgrade();
        match Helper::start(program, &paths, move |reply| {
            if let Some(window) = window.upgrade() {
                window.live_reply(generation, reply);
            }
        }) {
            Ok(helper) => {
                state.helper = Some(helper);
                true
            }
            Err(e) => {
                drop(state);
                self.toast(&format!("Live Text cannot start its reader: {e}"));
                self.live_stop();
                false
            }
        }
    }

    fn live_reply(&self, generation: u64, reply: Reply) {
        let imp = self.imp();
        if imp.live.borrow().helper_generation != generation {
            return;
        }
        match reply {
            Reply::Line(line) => {
                if let Some(reading) = imp.live.borrow_mut().reading.as_mut() {
                    reading.lines.push(line);
                }
            }
            Reply::Done(id) => {
                let finished = {
                    let mut state = imp.live.borrow_mut();
                    match state.reading.take() {
                        Some(reading) if reading.id == id => Some(reading),
                        other => {
                            state.reading = other;
                            None
                        }
                    }
                };
                let Some(pending) = finished else { return };
                let (sx, sy) = pending.scale;
                let (ox, oy) = pending.offset;
                let lines: Vec<TextLine> = pending
                    .lines
                    .iter()
                    // Shapes taken for text: a line with no letter or digit
                    // in it (a round sun read as "●"), or a letter or two
                    // read with little confidence (a ball read as "O").
                    .filter(|line| {
                        line.text.chars().any(char::is_alphanumeric)
                            && !(line.text.trim().chars().count() <= 2 && line.score < 0.8)
                    })
                    .map(|line| {
                        let quad = line.quad.map(|[x, y]| [ox + f64::from(x) * sx, oy + f64::from(y) * sy]);
                        TextLine::new(&line.text, quad, line.cuts.iter().map(|&c| f64::from(c)).collect())
                    })
                    .collect();
                {
                    let mut state = imp.live.borrow_mut();
                    if state.read.len() >= REMEMBERED {
                        state.read.clear();
                    }
                    state.read.insert(pending.key.clone(), lines.clone());
                }
                match pending.then {
                    // Still the picture it was read for?
                    Use::Picture => {
                        if imp.live_on.get() && self.live_key() == Some(pending.key) {
                            imp.live_status.set_reveal_child(false);
                            self.live_show(lines);
                        }
                    }
                    then => {
                        imp.live_status.set_reveal_child(false);
                        self.pdf_text_read(&pending.key, then, lines);
                    }
                }
                // On to the next, if one is waiting.
                self.live_send();
                self.live_schedule_idle();
            }
            Reply::Fail(id, reason) => {
                let gone = id == GONE;
                if gone {
                    imp.live.borrow_mut().helper = None;
                }
                let ours = imp.live.borrow().reading.as_ref().is_some_and(|p| p.id == id || gone);
                if ours {
                    imp.live.borrow_mut().reading = None;
                    self.live_failed(&format!("Could not read the text: {reason}"));
                }
            }
        }
    }

    fn live_show(&self, lines: Vec<TextLine>) {
        let imp = self.imp();
        if lines.is_empty() {
            self.toast("No text found in this picture.");
        }
        imp.view.canvas().set_live_text(Some(LiveLayer::new(lines)));
    }

    fn live_failed(&self, message: &str) {
        let imp = self.imp();
        {
            let mut state = imp.live.borrow_mut();
            state.reading = None;
            state.waiting = None;
        }
        imp.live_status.set_reveal_child(false);
        self.toast(message);
        self.live_schedule_idle();
    }

    /// Let the helper go once it has been idle a while.
    fn live_schedule_idle(&self) {
        let imp = self.imp();
        let mut state = imp.live.borrow_mut();
        if let Some(timer) = state.idle.take() {
            timer.remove();
        }
        if state.helper.is_none() || state.reading.is_some() || state.waiting.is_some() {
            return;
        }
        state.idle = Some(glib::timeout_add_local_once(
            IDLE,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move || {
                    let mut state = window.imp().live.borrow_mut();
                    state.idle = None;
                    if state.reading.is_none() && state.waiting.is_none() {
                        state.helper = None;
                    }
                }
            ),
        ));
    }

    /// Let the reader go now, rather than after it has idled: its files are
    /// about to be taken away. What it read before is still remembered.
    pub(crate) fn live_forget_helper(&self) {
        let mut state = self.imp().live.borrow_mut();
        if let Some(timer) = state.idle.take() {
            timer.remove();
        }
        state.reading = None;
        state.waiting = None;
        state.helper = None;
        state.paths = None;
    }

    /// Read the text in a picture on a PDF's page: to show over it, or to
    /// copy straight away.
    pub(crate) fn read_pdf_image(&self, image: crate::pdf::PageImage, copy: bool) {
        self.with_live_text(move |window, paths| {
            window.imp().live.borrow_mut().paths = Some(paths);
            window.pdf_image_read(image, copy);
        });
    }

    fn pdf_image_read(&self, image: crate::pdf::PageImage, copy: bool) {
        let imp = self.imp();
        let Some(path) = imp.pdf_view.path() else { return };
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        let area = image.area.map(|v| (v * 100.0).round() as i64);
        let key = Key { path, frame: image.page, modified, area: Some(area) };
        let then = if copy { Use::PdfCopy } else { Use::PdfShow(image) };
        if let Some(lines) = imp.live.borrow().read.get(&key).cloned() {
            self.pdf_text_read(&key, then, lines);
            return;
        }
        let Some(program) = client::helper_path() else {
            self.toast("Live Text cannot start: its reader, glance-ocr, is not installed with Glance.");
            return;
        };
        if !self.live_ensure_helper(&program) {
            return;
        }
        let Some(pixels) = imp.pdf_view.image_pixels(&image, SEND_SIDE) else {
            self.toast("This picture could not be read.");
            self.live_schedule_idle();
            return;
        };
        let id = {
            let mut state = imp.live.borrow_mut();
            state.next_id += 1;
            state.next_id
        };
        let step = 1.0 / pixels.per_point;
        let pending =
            Pending { id, key, scale: (step, step), offset: (image.area[0], image.area[1]), then, lines: Vec::new() };
        imp.live.borrow_mut().waiting = Some(Waiting { pending, width: pixels.width, height: pixels.height, rgba: pixels.rgba });
        imp.live_status.set_reveal_child(true);
        self.live_send();
    }

    /// Text read from a picture on a PDF's page, shown or copied, if that
    /// PDF is still the one open.
    fn pdf_text_read(&self, key: &Key, then: Use, lines: Vec<TextLine>) {
        let imp = self.imp();
        if !imp.showing_pdf.get() || imp.pdf_view.path().as_ref() != Some(&key.path) {
            return;
        }
        match then {
            Use::PdfCopy if lines.is_empty() => self.toast("No text found in this picture."),
            Use::PdfCopy => {
                let text: Vec<String> = lines.iter().map(|line| line.chars.iter().collect()).collect();
                self.clipboard().set_text(&text.join("\n"));
                self.toast("Text copied.");
            }
            Use::PdfShow(_) if lines.is_empty() => self.toast("No text found in this picture."),
            Use::PdfShow(image) => {
                imp.pdf_view.set_image_text(image, Some(lines));
                self.toast("Drag across the text to select it, then copy it with Ctrl+C.");
            }
            Use::Picture => {}
        }
    }

    /// Copy the selected text, if any is selected.
    pub(crate) fn copy_live_text(&self, all: bool) {
        let canvas = self.imp().view.canvas();
        let text = if all { canvas.live_all_text() } else { canvas.live_selected_text() };
        match text {
            Some(text) => {
                self.clipboard().set_text(&text);
                self.toast(if all { "All the text copied." } else { "Text copied." });
            }
            None if all => self.toast("There is no text in this picture."),
            None => self.toast("Select some text first."),
        }
    }
}
