// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading a PDF: every page in one scrolling column, one page at a time, or
//! two side by side.
//!
//! Only the pages on screen are drawn, plus one either side so a scroll never
//! shows a blank page for long. Pages well off screen give their pixels back,
//! so a thousand-page document costs about what a ten-page one does.
//!
//! Zooming resizes the pages at once, stretching whatever was already drawn,
//! and draws them sharp again once the zoom has stopped moving. Drawing every
//! step of a pinch would mean drawing pages nobody sees.
//!
//! Text is selected by dragging across it: a double-click takes a word, a
//! triple-click a line. The highlight is drawn over the page rather than into
//! it, so selecting never waits for a page to be redrawn.
//!
//! Selected text can be highlighted, underlined or struck through, and notes,
//! speech bubbles, text boxes and drawings put on a page. They go into the
//! file itself (see `annots`), the pages they are on are redrawn from it, and
//! each change can be undone and redone. A note or bubble is opened by
//! clicking it, and any of them moved by dragging it.
//!
//! What a press on a page does depends on the tool: select text, draw with
//! one of the image editor's pens and shapes, or place and pick text boxes.
//!
//! Beside the pages, the sidebar finds the way round: thumbnails, the table
//! of contents, and bookmarks, which are kept apart from the file (see
//! `bookmarks`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, gio, glib, graphene, pango};

use crate::images::edit::draw;

use super::annots::{self, Annotation, Rgb};
use super::bookmark_list::Request;
use super::bookmarks::{self, Bookmark};
use super::document::{self, Allowed, Opened, Pixels, Source, Spot, Unit};
use super::editor::{Commit, Editor};
use super::layout::{self, Layout, Mode, Rotation};
use super::markup::{self, Mark, Style};
use super::ink::{self, Drawing};
use super::notes::{self, Note, TextBox, TextStyle};
use super::outline::{self, Heading};
use super::redact::Area as RedactArea;
use super::page::Page;
use super::render::{Job, Rendered, Renderer};
use super::search::{Found, Match, Searcher};
use super::sidebar::{Event as SidebarEvent, Sidebar, View as SidebarView};

/// Pages drawn ahead of the ones on screen, in each direction.
const PREFETCH: usize = 1;
/// Pages kept drawn either side of the screen before their pixels are freed.
/// A little more than `PREFETCH`, so scrolling back a short way is instant.
const KEEP: usize = 3;
/// How long a zoom has to stay still before pages are redrawn at the new size.
const SETTLE: Duration = Duration::from_millis(120);
/// Which page counts as the one being read: the one crossing this line, a
/// quarter of the way down the window. A page moved to the top of the window
/// is then the current page, as soon as it gets there.
const READING_LINE: f64 = 0.25;
/// Said when the author's permissions do not allow marking the file up.
pub const NOT_ANNOTATABLE: &str = "The author of this document does not allow adding to it.";
/// Room left above a heading gone to from the table of contents, in pixels.
const HEADING_ROOM: f64 = 12.0;
/// One page at a time: how hard to keep scrolling past a page's end before
/// the next one comes, for a wheel's notches and a touchpad's pixels, and how
/// long before another turn, so one flick does not fly through the document.
const PUSH_NOTCHES: f64 = 1.0;
const PUSH_PIXELS: f64 = 80.0;
const TURN_PAUSE: Duration = Duration::from_millis(350);
/// A drag shorter than this, in points, is a click.
const NUDGE: f64 = 1.5;

/// What the header shows: where you are, and at what size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Status {
    pub page: usize,
    pub pages: usize,
    pub percent: f64,
}

/// Where a search has got to, for the search bar's count.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SearchStatus {
    /// The match being looked at, counting from one.
    pub current: Option<usize>,
    pub total: usize,
    /// Every page has been searched. Until then the total can still grow.
    pub done: bool,
}

/// What became of a request to mark the selection.
#[derive(Clone, Debug, PartialEq)]
pub enum Marked {
    Done,
    /// No text selected, so nothing to mark.
    NothingSelected,
    /// The file could not be saved; nothing was changed.
    Failed(String),
}

/// What kind of thing to put on a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pinned {
    Note,
    Bubble,
}

/// One step to undo: annotations taken off and put on together. Recolouring a
/// highlight is both.
#[derive(Default)]
struct Change {
    removed: Vec<Annotation>,
    added: Vec<Annotation>,
}

/// A selection: where the drag started, where it is now, and by what unit.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Selection {
    anchor: Spot,
    head: Spot,
    unit: Unit,
}

/// What a press on a page does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// Select text; open notes; move notes and bubbles.
    Select,
    /// Draw with one of the image editor's pens or shapes.
    Draw(draw::Tool),
    /// Put a text box down, or pick one to restyle or move.
    Text,
}

/// How long typing or restyling a text box has to pause before the file is
/// written: one save, and one step of undo, per pause rather than per key.
const RESTYLE_PAUSE: Duration = Duration::from_millis(500);

/// A note or bubble being dragged, and how far so far, in points.
struct Moving {
    annotation: Annotation,
    from: Spot,
    by: (f64, f64),
}

pub struct PdfView {
    inner: Rc<Inner>,
}

/// A bookmark added or taken away, for the window to tell the reader about.
pub enum BookmarkEvent {
    /// `seen` when the bookmarks were on screen, where it shows for itself.
    Added { page: usize, seen: bool },
    /// What was removed, so it can be put back.
    Removed(Bookmark),
    Failed(String),
}

struct Inner {
    root: gtk::ScrolledWindow,
    /// Every page, placed where the layout says.
    column: gtk::Fixed,
    sidebar: Sidebar,
    editor: Editor,
    /// Page sizes in points, as the PDF gives them, before any turning.
    pages: RefCell<Vec<(f64, f64)>>,
    widgets: RefCell<Vec<Page>>,
    /// The device scale each page was last drawn at, or 0.0 for not drawn.
    rendered: RefCell<Vec<f64>>,
    layout: RefCell<Layout>,
    mode: Cell<Mode>,
    /// The page shown when they are shown one at a time.
    shown: Cell<usize>,
    rotation: Cell<Rotation>,
    night: Cell<bool>,
    /// Logical pixels per point.
    scale: Cell<f64>,
    /// Following the window's size, until the user zooms by hand.
    fit: Cell<bool>,
    /// A refit to a new window size is waiting for GTK to finish laying out.
    refit_queued: Cell<bool>,
    uri: RefCell<Option<String>>,
    /// What opened the document, if it is protected.
    password: RefCell<Option<String>>,
    /// What its author allows.
    allowed: Cell<Allowed>,
    renderer: RefCell<Option<Renderer>>,
    /// The document again, on this thread, for selecting text and reading and
    /// writing annotations. Opened the first time it is needed, not before.
    reader: RefCell<Option<poppler::Document>>,
    /// Bumped for every document, so pages drawn for the last one are ignored.
    document: Cell<u64>,
    settle: RefCell<Option<glib::SourceId>>,
    /// Where a zoom wants the view to end up, per axis, until GTK has laid out
    /// the new sizes and the position can actually be reached.
    pending: Cell<(Option<f64>, Option<f64>)>,
    /// A page asked for by number, and the scroll position it was shown at.
    /// Until the view moves, that page is the one reported, even when it is
    /// too near the end to reach the top of the window.
    jumped: Cell<Option<(usize, f64)>>,
    pointer: Cell<Option<(f64, f64)>>,
    /// Where the context menu was opened, in the window's coordinates.
    menu_point: Cell<Option<(f64, f64)>>,
    pinch_from: Cell<f64>,
    /// One page at a time: scrolling pushed past the page's end so far, and
    /// when the page last turned.
    push: Cell<f64>,
    turned_at: Cell<Option<Instant>>,
    selection: Cell<Option<Selection>>,
    /// Pages currently showing a highlight, so they can be cleared.
    highlighted: RefCell<Vec<usize>>,
    /// Consecutive presses in the same spot, for double and triple clicks.
    presses: Cell<(u32, Option<(Instant, f64, f64)>)>,
    /// The notes and bubbles on each page looked at, read from the file once
    /// per version of it.
    pinned: RefCell<HashMap<usize, Vec<Annotation>>>,
    moving: RefCell<Option<Moving>>,
    /// The page whose pointer is showing a hand over a note.
    hovering: Cell<Option<usize>>,
    status: RefCell<Option<Box<dyn Fn(Status)>>>,
    last_status: Cell<Option<Status>>,
    searcher: RefCell<Option<Searcher>>,
    /// Every match found so far, in page order.
    matches: RefCell<Vec<Match>>,
    current_match: Cell<Option<usize>>,
    search_done: Cell<bool>,
    /// The page being read when the search began: the first match shown is
    /// the first one from here on, not the first in the document.
    search_from: Cell<usize>,
    /// Bumped per search, so matches from a replaced one are ignored.
    search_id: Cell<u64>,
    /// Pages showing match marks, so they can be cleared.
    marked: RefCell<Vec<usize>>,
    search_status: RefCell<Option<Box<dyn Fn(SearchStatus)>>>,
    /// The version of the file on screen, counting changes made here. Pages
    /// drawn from an older one are out of date.
    revision: Cell<u64>,
    history: RefCell<Vec<Change>>,
    undone: RefCell<Vec<Change>>,
    on_history: RefCell<Option<Box<dyn Fn(bool, bool)>>>,
    /// For what goes wrong after the call that started it has returned: a
    /// note that could not be saved when its editor closed.
    on_error: RefCell<Option<Box<dyn Fn(String)>>>,
    tool: Cell<Tool>,
    /// The pen's colour and thickness, in points.
    ink: Cell<(gdk::RGBA, f64)>,
    /// A drawing being made, on a page, in page points.
    sketch: RefCell<Option<(usize, draw::Mark)>>,
    /// The page showing a finished drawing until it is redrawn with it.
    sketched: Cell<Option<usize>>,
    /// What the text controls say, for the next box and the chosen one.
    look: RefCell<(String, TextStyle)>,
    /// The text box picked with the text tool.
    chosen: RefCell<Option<TextBox>>,
    /// A change to the chosen box, waiting for typing to pause.
    restyle: RefCell<Option<glib::SourceId>>,
    /// Where a press with the text tool landed on empty page.
    placing: Cell<Option<Spot>>,
    on_chosen: RefCell<Option<Box<dyn Fn(Option<(String, TextStyle)>)>>>,
    outline: RefCell<Vec<Heading>>,
    /// The reader's bookmarks in this document, in page order, and which
    /// document they are kept under.
    bookmarks: RefCell<Vec<Bookmark>>,
    bookmark_key: RefCell<Option<bookmarks::Key>>,
    on_bookmarks: RefCell<Option<Box<dyn Fn(BookmarkEvent)>>>,
    on_sidebar_view: RefCell<Option<Box<dyn Fn(SidebarView)>>>,
    /// Areas marked for redaction, not yet applied: nothing in the file
    /// changes until they are.
    redactions: RefCell<Vec<RedactArea>>,
    on_redactions: RefCell<Option<Box<dyn Fn(usize)>>>,
}

impl PdfView {
    pub fn new() -> Self {
        let column = gtk::Fixed::new();
        column.set_halign(gtk::Align::Center);
        column.set_valign(gtk::Align::Start);
        let root = gtk::ScrolledWindow::builder().hexpand(true).vexpand(true).child(&column).build();
        let editor = Editor::new(&root);

        let inner = Rc::new(Inner {
            root,
            column,
            sidebar: Sidebar::new(),
            editor,
            pages: RefCell::default(),
            widgets: RefCell::default(),
            rendered: RefCell::default(),
            layout: RefCell::default(),
            mode: Cell::new(Mode::default()),
            shown: Cell::new(0),
            rotation: Cell::new(Rotation::default()),
            night: Cell::new(false),
            scale: Cell::new(layout::ACTUAL),
            fit: Cell::new(true),
            refit_queued: Cell::new(false),
            uri: RefCell::default(),
            password: RefCell::default(),
            allowed: Cell::new(Allowed::default()),
            renderer: RefCell::default(),
            reader: RefCell::default(),
            document: Cell::new(0),
            settle: RefCell::default(),
            pending: Cell::new((None, None)),
            jumped: Cell::new(None),
            pointer: Cell::new(None),
            menu_point: Cell::new(None),
            pinch_from: Cell::new(layout::ACTUAL),
            push: Cell::new(0.0),
            turned_at: Cell::new(None),
            selection: Cell::new(None),
            highlighted: RefCell::default(),
            presses: Cell::new((0, None)),
            pinned: RefCell::default(),
            moving: RefCell::default(),
            hovering: Cell::new(None),
            status: RefCell::default(),
            last_status: Cell::new(None),
            searcher: RefCell::default(),
            matches: RefCell::default(),
            current_match: Cell::new(None),
            search_done: Cell::new(true),
            search_from: Cell::new(0),
            search_id: Cell::new(0),
            marked: RefCell::default(),
            search_status: RefCell::default(),
            revision: Cell::new(0),
            history: RefCell::default(),
            undone: RefCell::default(),
            on_history: RefCell::default(),
            on_error: RefCell::default(),
            tool: Cell::new(Tool::Select),
            ink: Cell::new((gdk::RGBA::new(0.9, 0.15, 0.15, 1.0), 3.0)),
            sketch: RefCell::default(),
            sketched: Cell::new(None),
            look: RefCell::new(("Text".to_string(), TextStyle::bubble())),
            chosen: RefCell::default(),
            restyle: RefCell::default(),
            placing: Cell::new(None),
            on_chosen: RefCell::default(),
            outline: RefCell::default(),
            bookmarks: RefCell::default(),
            bookmark_key: RefCell::default(),
            on_bookmarks: RefCell::default(),
            on_sidebar_view: RefCell::default(),
            redactions: RefCell::default(),
            on_redactions: RefCell::default(),
        });
        Inner::connect(&inner);
        PdfView { inner }
    }

    pub fn widget(&self) -> &gtk::ScrolledWindow {
        &self.inner.root
    }

    /// Pages, contents and bookmarks, for the window to put beside the reader.
    pub fn sidebar(&self) -> &gtk::Box {
        self.inner.sidebar.widget()
    }

    /// Which of them the sidebar shows.
    pub fn set_sidebar_view(&self, view: SidebarView) {
        self.inner.sidebar.set_view(view);
    }

    /// Called when the reader picks another of them.
    pub fn connect_sidebar_view(&self, callback: impl Fn(SidebarView) + 'static) {
        self.inner.on_sidebar_view.replace(Some(Box::new(callback)));
    }

    /// Whether the page being read is bookmarked.
    pub fn is_bookmarked(&self) -> bool {
        let page = self.inner.reported_page();
        self.inner.bookmarks.borrow().iter().any(|mark| mark.page == page)
    }

    /// Bookmark the page being read, or take its bookmark away.
    pub fn toggle_bookmark(&self) {
        let inner = &self.inner;
        if inner.uri.borrow().is_none() {
            return;
        }
        let page = inner.reported_page();
        if inner.bookmarks.borrow().iter().any(|mark| mark.page == page) {
            inner.remove_bookmark(page);
        } else {
            inner.add_bookmark(page);
        }
    }

    /// Put back a bookmark that was removed.
    pub fn restore_bookmark(&self, mark: Bookmark) {
        let inner = &self.inner;
        if mark.page >= inner.pages.borrow().len() {
            return;
        }
        let mut marks = inner.bookmarks.borrow().clone();
        marks.retain(|m| m.page != mark.page);
        marks.push(mark);
        inner.set_bookmarks(marks);
    }

    pub fn connect_bookmarks(&self, callback: impl Fn(BookmarkEvent) + 'static) {
        self.inner.on_bookmarks.replace(Some(Box::new(callback)));
    }

    /// The sidebar is on screen, or not. It draws nothing while hidden.
    pub fn set_sidebar_active(&self, active: bool) {
        self.inner.sidebar.set_active(active);
    }

    /// The file on screen.
    pub fn path(&self) -> Option<PathBuf> {
        self.inner.path()
    }

    /// Mark the selected text for redaction. Nothing is changed yet.
    pub fn mark_redaction(&self) -> Marked {
        let inner = &self.inner;
        let Some(selection) = inner.selection.get() else { return Marked::NothingSelected };
        let Some(reader) = inner.reader() else { return Marked::NothingSelected };
        let spans = document::spans(selection.anchor, selection.head, &inner.pages.borrow());
        let areas: Vec<RedactArea> = spans
            .into_iter()
            .flat_map(|(page, span)| {
                document::highlights(&reader, page, span, selection.unit)
                    .into_iter()
                    .map(move |rect| RedactArea { page, rect })
            })
            .collect();
        if areas.is_empty() {
            return Marked::NothingSelected;
        }
        inner.set_selection(None);
        inner.add_redactions(areas);
        Marked::Done
    }

    /// The areas marked for redaction.
    pub fn redactions(&self) -> Vec<RedactArea> {
        self.inner.redactions.borrow().clone()
    }

    /// Unmark everything.
    pub fn clear_redactions(&self) {
        self.inner.set_redactions(Vec::new());
    }

    /// Unmark the most recent area. False if there was none.
    pub fn unmark_last_redaction(&self) -> bool {
        let mut areas = self.inner.redactions.borrow().clone();
        let removed = areas.pop().is_some();
        self.inner.set_redactions(areas);
        removed
    }

    /// Unmark the areas where the context menu was opened. False if it was
    /// opened on none.
    pub fn unmark_redaction_here(&self) -> bool {
        let inner = &self.inner;
        let Some(spot) = inner.menu_spot() else { return false };
        let mut areas = inner.redactions.borrow().clone();
        let before = areas.len();
        areas.retain(|area| {
            let [x, y, w, h] = area.rect;
            !(area.page == spot.page && spot.x >= x && spot.x <= x + w && spot.y >= y && spot.y <= y + h)
        });
        let removed = areas.len() != before;
        inner.set_redactions(areas);
        removed
    }

    /// Called with how many areas are marked, whenever that changes.
    pub fn connect_redactions(&self, callback: impl Fn(usize) + 'static) {
        self.inner.on_redactions.replace(Some(Box::new(callback)));
    }

    /// What the document's author allows.
    pub fn allowed(&self) -> Allowed {
        self.inner.allowed.get()
    }

    /// The password the document was opened with, if it needed one.
    pub fn password(&self) -> Option<String> {
        self.inner.password.borrow().clone()
    }

    /// The page being read, counting from zero.
    pub fn current_page(&self) -> usize {
        self.inner.current_page()
    }

    /// Lay out a freshly opened document, fitted to the window, and start
    /// drawing the first pages.
    pub fn show(&self, opened: Opened) {
        let inner = &self.inner;
        inner.clear();
        let id = inner.document.get();

        let count = opened.pages.len();
        let widgets: Vec<Page> = (0..count)
            .map(|_| {
                let widget = Page::new();
                widget.set_cursor_from_name(Some(inner.cursor()));
                widget.set_night(inner.night.get());
                inner.column.put(&widget, 0.0, 0.0);
                widget
            })
            .collect();
        let source = Source { uri: opened.uri.clone(), password: opened.password.clone() };
        inner.sidebar.show_document(source.clone(), opened.pages.clone(), &opened.outline);
        let key = bookmarks::Key { uri: opened.uri.clone(), id: opened.id.clone() };
        let mut marks = bookmarks::load(&key);
        marks.retain(|mark| mark.page < count);
        inner.sidebar.set_bookmarks(&marks, 0);
        inner.bookmarks.replace(marks);
        inner.bookmark_key.replace(Some(key));
        inner.outline.replace(opened.outline);
        *inner.pages.borrow_mut() = opened.pages;
        *inner.widgets.borrow_mut() = widgets;
        *inner.rendered.borrow_mut() = vec![0.0; count];
        inner.uri.replace(Some(opened.uri.clone()));
        inner.password.replace(opened.password.clone());
        inner.allowed.set(opened.allowed);

        inner.fit.set(true);
        inner.apply_scale(inner.fitted().unwrap_or(layout::ACTUAL));
        inner.scroll_to(Some(0.0), Some(0.0));

        let (sender, receiver) = async_channel::bounded(4);
        inner.renderer.replace(Some(Renderer::start(source, inner.revision.get(), sender)));
        let weak = Rc::downgrade(inner);
        glib::spawn_future_local(async move {
            while let Ok(rendered) = receiver.recv().await {
                let Some(inner) = weak.upgrade() else { break };
                if inner.document.get() != id {
                    break;
                }
                inner.on_rendered(rendered);
            }
        });

        inner.refresh(true);
        inner.emit_status();
    }

    /// Stop drawing and let go of every page, so leaving a PDF frees it.
    pub fn clear(&self) {
        self.inner.clear();
    }

    pub fn connect_status(&self, callback: impl Fn(Status) + 'static) {
        self.inner.status.replace(Some(Box::new(callback)));
    }

    pub fn zoom_by(&self, factor: f64) {
        let inner = &self.inner;
        inner.fit.set(false);
        inner.set_scale(inner.scale.get() * factor, inner.centre());
    }

    /// Fit the document to the window, and keep fitting as it resizes: the
    /// width of a column or a spread, or one whole page.
    pub fn zoom_fit(&self) {
        let inner = &self.inner;
        inner.fit.set(true);
        if let Some(scale) = inner.fitted() {
            inner.set_scale(scale, inner.centre());
        }
    }

    /// Life size: a centimetre on the page is a centimetre on the screen.
    pub fn zoom_actual(&self) {
        let inner = &self.inner;
        inner.fit.set(false);
        inner.set_scale(layout::ACTUAL, inner.centre());
    }

    /// Continuous, one page at a time, or two side by side. The page being
    /// read stays in view.
    pub fn set_mode(&self, mode: Mode) {
        self.inner.set_mode(mode);
    }

    /// Swap light and dark on the pages, for reading at night.
    pub fn set_night(&self, night: bool) {
        let inner = &self.inner;
        inner.night.set(night);
        for widget in inner.widgets.borrow().iter() {
            widget.set_night(night);
        }
        inner.sidebar.set_night(night);
    }

    /// Move by most of a screen, keeping a strip of the old view for context.
    /// One page at a time, past the page's end is the next page.
    pub fn scroll_pages(&self, direction: f64) {
        self.inner.scroll_by(direction, 0.9);
    }

    pub fn scroll_lines(&self, direction: f64) {
        self.inner.scroll_by(direction, 0.1);
    }

    /// Sideways, for a page zoomed wider than the window.
    pub fn scroll_across(&self, direction: f64) {
        let h = self.inner.root.hadjustment();
        h.set_value(h.value() + direction * h.page_size() * 0.1);
    }

    pub fn scroll_to_start(&self) {
        let inner = &self.inner;
        if inner.mode.get() == Mode::Single {
            inner.show_page(0, false);
        } else {
            inner.root.vadjustment().set_value(0.0);
        }
    }

    pub fn scroll_to_end(&self) {
        let inner = &self.inner;
        if inner.mode.get() == Mode::Single {
            let last = inner.pages.borrow().len().saturating_sub(1);
            inner.show_page(last, true);
        } else {
            let v = inner.root.vadjustment();
            v.set_value(v.upper());
        }
    }

    /// Bring a page to the top of the window. Counts from zero.
    pub fn go_to_page(&self, index: usize) {
        self.inner.go_to_page(index);
    }

    /// Turn every page a quarter turn per step, clockwise for positive steps.
    /// Only how they are shown: the file is left as it is.
    pub fn rotate_by(&self, quarters: i32) {
        let inner = &self.inner;
        if inner.pages.borrow().is_empty() {
            return;
        }
        let current = inner.current_page();
        let rotation = inner.rotation.get().turned(quarters);
        inner.rotation.set(rotation);
        // A selection is made of positions on the page; turning moves them.
        inner.set_selection(None);
        // Everything drawn so far is the wrong way round now.
        for (widget, rendered) in inner.widgets.borrow().iter().zip(inner.rendered.borrow_mut().iter_mut()) {
            widget.set_texture(None);
            *rendered = 0.0;
        }
        let scale = if inner.fit.get() { inner.fitted().unwrap_or(inner.scale.get()) } else { inner.scale.get() };
        inner.apply_scale(scale);
        inner.scroll_to(None, Some(inner.row_top(current)));
        inner.settle_then_render();
        inner.sidebar.set_rotation(rotation);
        // Matches are positions on the page; turning moves where they show.
        inner.mark_all();
        let marked: Vec<usize> = inner.redactions.borrow().iter().map(|a| a.page).collect();
        for page in marked {
            inner.show_redactions(page);
        }
        inner.emit_status();
    }

    /// Search the document for `query`, replacing any search already running.
    /// An empty query clears the marks.
    pub fn search(&self, query: &str) {
        self.inner.search(query);
    }

    /// Move to the next match, or the one before, wrapping round the ends.
    pub fn search_step(&self, forward: bool) {
        self.inner.search_step(forward);
    }

    /// Stop searching and take the marks off the pages.
    pub fn clear_search(&self) {
        self.inner.search("");
    }

    pub fn connect_search_status(&self, callback: impl Fn(SearchStatus) + 'static) {
        self.inner.search_status.replace(Some(Box::new(callback)));
    }

    /// Highlight, underline or strike through the selected text, and save it
    /// into the file. A highlight is in `colour`; the lines are always red.
    /// Marking text exactly as it is already marked takes the mark off again;
    /// highlighting it in another colour changes the colour.
    pub fn mark(&self, style: Style, colour: Rgb) -> Marked {
        self.inner.mark(style, colour)
    }

    /// Open a new note or speech bubble for writing: where the context menu
    /// was opened if `here`, otherwise at the selection, or failing that on
    /// the page being read.
    pub fn pin(&self, kind: Pinned, here: bool) {
        self.inner.pin(kind, here);
    }

    /// Where the context menu is being opened, in the reader's coordinates.
    pub fn set_menu_point(&self, x: f64, y: f64) {
        self.inner.menu_point.set(Some((x, y)));
    }

    /// Take back the last change, or put it back. Ok if there was nothing to
    /// take back.
    pub fn undo(&self) -> Result<(), String> {
        self.inner.step_history(true)
    }

    pub fn redo(&self) -> Result<(), String> {
        self.inner.step_history(false)
    }

    /// Called with whether there is anything to undo, and to redo.
    pub fn connect_history(&self, callback: impl Fn(bool, bool) + 'static) {
        self.inner.on_history.replace(Some(Box::new(callback)));
    }

    /// Called with a message when a change made in the note editor could not
    /// be saved.
    pub fn connect_error(&self, callback: impl Fn(String) + 'static) {
        self.inner.on_error.replace(Some(Box::new(callback)));
    }

    /// What a press on a page does from now on.
    pub fn set_tool(&self, tool: Tool) {
        self.inner.set_tool(tool);
    }

    /// The pen's colour, and its thickness in points.
    pub fn set_ink(&self, colour: gdk::RGBA, width: f64) {
        self.inner.ink.set((colour, width));
    }

    /// What the text controls say: the words and look for the next text box,
    /// and for the chosen one, which follows once typing pauses.
    pub fn set_text_look(&self, text: String, style: TextStyle) {
        self.inner.set_text_look(text, style);
    }

    /// Put a new text box near the top of the page being read.
    pub fn add_text_box(&self) {
        let inner = &self.inner;
        let page = inner.current_page();
        if let Some(&(w, h)) = inner.pages.borrow().get(page) {
            inner.place_text(Spot { page, x: w * 0.12, y: h * 0.12 });
        }
    }

    /// Delete the chosen text box.
    pub fn remove_chosen(&self) {
        self.inner.remove_chosen();
    }

    /// Called with the chosen text box's words and look, or `None` when none
    /// is chosen.
    pub fn connect_chosen(&self, callback: impl Fn(Option<(String, TextStyle)>) + 'static) {
        self.inner.on_chosen.replace(Some(Box::new(callback)));
    }

    /// Copy the selected text to the clipboard. False if nothing is selected.
    pub fn copy_selection(&self) -> bool {
        let inner = &self.inner;
        if !inner.allowed.get().copy {
            return false;
        }
        let Some(text) = inner.selected_text().filter(|text| !text.trim().is_empty()) else {
            return false;
        };
        inner.root.clipboard().set_text(&text);
        true
    }
}

impl Inner {
    fn connect(this: &Rc<Self>) {
        let v = this.root.vadjustment();
        let h = this.root.hadjustment();

        let weak = Rc::downgrade(this);
        v.connect_value_changed(move |v| {
            if let Some(inner) = weak.upgrade() {
                // Scrolled away from a page that was asked for by number.
                if inner.jumped.get().is_some_and(|(_, at)| (v.value() - at).abs() > 0.5) {
                    inner.jumped.set(None);
                }
                // While a zoom settles, pages are resized but not redrawn.
                let settled = inner.settle.borrow().is_none();
                inner.refresh(settled);
                inner.emit_status();
            }
        });

        for (adjustment, horizontal) in [(&v, false), (&h, true)] {
            let weak = Rc::downgrade(this);
            adjustment.connect_changed(move |_| {
                if let Some(inner) = weak.upgrade() {
                    inner.apply_pending(horizontal);
                }
            });
        }

        // A new window size means a new fitted size: its width always, and
        // its height too for a whole page at a time. This fires while GTK is
        // in the middle of laying the window out, and a resize requested from
        // inside that pass is lost: GTK finishes the pass and forgets it,
        // leaving the scrollable height a page or more short of the pages.
        // So the refit waits until the pass is over.
        for (adjustment, height) in [(&h, false), (&v, true)] {
            let weak = Rc::downgrade(this);
            adjustment.connect_page_size_notify(move |_| {
                let Some(inner) = weak.upgrade() else { return };
                if height && inner.mode.get() != Mode::Single {
                    return;
                }
                inner.queue_refit();
            });
        }

        let weak = Rc::downgrade(this);
        this.sidebar.connect_event(move |event| {
            let Some(inner) = weak.upgrade() else { return };
            match event {
                SidebarEvent::Go(page, y) => inner.go_to(page, y),
                SidebarEvent::Switched(view) => {
                    if let Some(callback) = inner.on_sidebar_view.borrow().as_ref() {
                        callback(view);
                    }
                }
                SidebarEvent::Bookmarks(Request::Go(page)) => inner.go_to_page(page),
                SidebarEvent::Bookmarks(Request::Add) => {
                    let page = inner.reported_page();
                    if !inner.bookmarks.borrow().iter().any(|mark| mark.page == page) {
                        inner.add_bookmark(page);
                    }
                }
                SidebarEvent::Bookmarks(Request::Remove(page)) => inner.remove_bookmark(page),
                SidebarEvent::Bookmarks(Request::Rename(page, name)) => {
                    let mut marks = inner.bookmarks.borrow().clone();
                    if let Some(mark) = marks.iter_mut().find(|mark| mark.page == page) {
                        mark.name = name;
                    }
                    inner.set_bookmarks(marks);
                }
            }
        });

        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(this);
        motion.connect_motion(move |_, x, y| {
            if let Some(inner) = weak.upgrade() {
                inner.pointer.set(Some((x, y)));
            }
        });
        let weak = Rc::downgrade(this);
        motion.connect_leave(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.pointer.set(None);
            }
        });
        this.root.add_controller(motion);

        // A hand over a note or bubble, which a click opens and a drag moves.
        let hover = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(this);
        hover.connect_motion(move |_, x, y| {
            if let Some(inner) = weak.upgrade() {
                inner.hover(x, y);
            }
        });
        this.column.add_controller(hover);

        // The wheel and two fingers scroll, as in every reader; with Ctrl they
        // zoom about the pointer. Caught before the scrolled window sees it,
        // or it would scroll as well.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(this);
        scroll.connect_scroll(move |controller, _, dy| {
            let Some(inner) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let smooth = controller
                .current_event()
                .and_then(|event| event.downcast::<gdk::ScrollEvent>().ok())
                .is_some_and(|event| event.unit() == gdk::ScrollUnit::Surface);
            if !controller.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
                return inner.scroll_past_page(dy, smooth);
            }
            // A touchpad reports distance; a wheel reports notches.
            let factor = if smooth { (-dy * 0.01).exp() } else { 1.1f64.powf(-dy) };
            inner.fit.set(false);
            let anchor = inner.pointer.get().unwrap_or_else(|| inner.centre());
            inner.set_scale(inner.scale.get() * factor, anchor);
            glib::Propagation::Stop
        });
        this.root.add_controller(scroll);

        let pinch = gtk::GestureZoom::new();
        let weak = Rc::downgrade(this);
        pinch.connect_begin(move |_, _| {
            if let Some(inner) = weak.upgrade() {
                inner.pinch_from.set(inner.scale.get());
            }
        });
        let weak = Rc::downgrade(this);
        pinch.connect_scale_changed(move |gesture, delta| {
            let Some(inner) = weak.upgrade() else { return };
            inner.fit.set(false);
            let anchor = gesture.bounding_box_center().unwrap_or_else(|| inner.centre());
            inner.set_scale(inner.pinch_from.get() * delta, anchor);
        });
        this.root.add_controller(pinch);

        // Selecting text, or moving a note. Positions are in the column's own
        // coordinates, which are the layout's.
        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        let weak = Rc::downgrade(this);
        drag.connect_drag_begin(move |_, x, y| {
            let Some(inner) = weak.upgrade() else { return };
            let Some(spot) = inner.hit(x, y) else { return };
            match inner.tool.get() {
                Tool::Draw(tool) => {
                    inner.start_sketch(spot, tool);
                    return;
                }
                Tool::Text => {
                    inner.set_selection(None);
                    match inner.pinned_at(spot) {
                        Some(annotation) => {
                            if let Annotation::TextBox(text_box) = &annotation {
                                inner.choose(Some(text_box.clone()));
                            }
                            inner.moving.replace(Some(Moving { annotation, from: spot, by: (0.0, 0.0) }));
                        }
                        None => {
                            inner.choose(None);
                            inner.placing.set(Some(spot));
                        }
                    }
                    return;
                }
                Tool::Select => {}
            }
            let unit = inner.count_press(x, y);
            if unit == Unit::Glyph {
                if let Some(annotation) = inner.pinned_at(spot) {
                    inner.set_selection(None);
                    inner.moving.replace(Some(Moving { annotation, from: spot, by: (0.0, 0.0) }));
                    return;
                }
            }
            inner.set_selection(Some(Selection { anchor: spot, head: spot, unit }));
        });
        let weak = Rc::downgrade(this);
        drag.connect_drag_update(move |gesture, dx, dy| {
            let Some(inner) = weak.upgrade() else { return };
            let Some((x, y)) = gesture.start_point() else { return };
            let Some(head) = inner.hit(x + dx, y + dy) else { return };
            if inner.sketch.borrow().is_some() {
                inner.extend_sketch(head);
                return;
            }
            // Dragged off where it was pressed: not a click to place text.
            if inner.placing.get().is_some() && dx.hypot(dy) > 4.0 {
                inner.placing.set(None);
            }
            if inner.moving.borrow().is_some() {
                inner.drag_pinned(head);
                return;
            }
            if let Some(selection) = inner.selection.get() {
                if head != selection.head {
                    inner.set_selection(Some(Selection { head, ..selection }));
                }
            }
        });
        let weak = Rc::downgrade(this);
        drag.connect_drag_end(move |_, _, _| {
            let Some(inner) = weak.upgrade() else { return };
            if inner.sketch.borrow().is_some() {
                inner.finish_sketch();
                return;
            }
            if let Some(spot) = inner.placing.take() {
                inner.place_text(spot);
                return;
            }
            if let Some(moving) = inner.moving.take() {
                inner.drop_pinned(moving);
                return;
            }
            // A plain click with no drag selects nothing: it clears.
            if let Some(selection) = inner.selection.get() {
                if selection.unit == Unit::Glyph && selection.anchor == selection.head {
                    inner.set_selection(None);
                }
            }
        });
        this.column.add_controller(drag);
    }

    /// Page sizes as they are shown: turned, if the view has been turned.
    fn turned_pages(&self) -> Vec<(f64, f64)> {
        let rotation = self.rotation.get();
        self.pages.borrow().iter().map(|&(w, h)| rotation.size(w, h)).collect()
    }

    fn centre(&self) -> (f64, f64) {
        let (h, v) = (self.root.hadjustment(), self.root.vadjustment());
        (h.page_size() / 2.0, v.page_size() / 2.0)
    }

    /// The scale that fits the window in the current mode, or `None` before
    /// the window has a size.
    fn fitted(&self) -> Option<f64> {
        let (width, height) = (self.root.hadjustment().page_size(), self.root.vadjustment().page_size());
        (width > 0.0).then(|| layout::fit(&self.turned_pages(), self.mode.get(), (width, height)))
    }

    fn queue_refit(self: &Rc<Self>) {
        if !self.fit.get() || self.refit_queued.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            let Some(inner) = weak.upgrade() else { return };
            inner.refit_queued.set(false);
            if inner.fit.get() && !inner.pages.borrow().is_empty() {
                if let Some(scale) = inner.fitted() {
                    // Keep whatever is at the top of the window at the top.
                    inner.set_scale(scale, (0.0, 0.0));
                }
            }
        });
    }

    /// Device pixels per point: the zoom, times the screen's own scale.
    fn device_scale(&self) -> f64 {
        let screen = self
            .root
            .native()
            .and_then(|native| native.surface())
            .map(|surface| surface.scale())
            .unwrap_or_else(|| f64::from(self.root.scale_factor()));
        self.scale.get() * screen.max(1.0)
    }

    /// Zoom to `wanted`, keeping the document point under `anchor` — a
    /// position in the window — under it afterwards.
    fn set_scale(self: &Rc<Self>, wanted: f64, anchor: (f64, f64)) {
        let scale = layout::clamp_scale(wanted);
        if self.pages.borrow().is_empty() || (scale - self.scale.get()).abs() < 1e-9 {
            return;
        }
        let (h, v) = (self.root.hadjustment(), self.root.vadjustment());
        let (page, fraction) = self.layout.borrow().locate(v.value() + anchor.1);
        let old_width = self.layout.borrow().extent().0;
        let across = if old_width > 0.0 { (h.value() + anchor.0) / old_width } else { 0.5 };

        self.apply_scale(scale);

        let layout = self.layout.borrow();
        let x = across * layout.extent().0 - anchor.0;
        let y = layout.position(page, fraction) - anchor.1;
        drop(layout);
        self.scroll_to(Some(x), Some(y));
        self.settle_then_render();
        self.emit_status();
    }

    /// Lay the pages out again at `scale`, in the current mode, and put every
    /// widget where the layout says. Pages not laid out are hidden.
    fn apply_scale(&self, scale: f64) {
        self.scale.set(scale);
        let layout = Layout::new(&self.turned_pages(), scale, self.mode.get(), self.shown.get());
        for (i, widget) in self.widgets.borrow().iter().enumerate() {
            let shown = layout.contains(i);
            widget.set_visible(shown);
            if shown {
                let (w, h) = layout.size(i);
                widget.set_page_size(w as i32, h as i32);
                self.column.move_(widget, layout.left(i), layout.top(i));
            }
        }
        let (w, h) = layout.extent();
        self.column.set_size_request(w as i32, h as i32);
        *self.layout.borrow_mut() = layout;
    }

    fn set_mode(self: &Rc<Self>, mode: Mode) {
        if self.mode.get() == mode {
            return;
        }
        // Read in the old arrangement, before it changes.
        let current = self.current_page();
        self.mode.set(mode);
        // One page at a time sits in the middle of the window.
        self.column.set_valign(if mode == Mode::Single { gtk::Align::Center } else { gtk::Align::Start });
        if self.pages.borrow().is_empty() {
            return;
        }
        self.shown.set(current);
        let scale = if self.fit.get() { self.fitted().unwrap_or(self.scale.get()) } else { self.scale.get() };
        self.apply_scale(scale);
        self.jumped.set(None);
        self.go_to_page(current);
        self.settle_then_render();
    }

    /// Go to a page, and as far down it as `y` says, in points from its top:
    /// a heading in the table of contents is brought near the top of the
    /// window, not just its page.
    fn go_to(&self, index: usize, y: Option<f64>) {
        let Some(&(width, height)) = self.pages.borrow().get(index) else { return };
        // Turned a quarter, the heading's height on the page is its distance
        // across the screen; the page's top is the best there is.
        let y = y.and_then(|y| match self.rotation.get().quarters() {
            0 | 2 => Some(self.rotation.get().apply(0.0, y, width, height).1),
            _ => None,
        });
        let Some(y) = y else {
            self.go_to_page(index);
            return;
        };
        // A little above the heading, so it is not hard against the edge.
        let offset = (y * self.scale.get() - HEADING_ROOM).max(0.0);
        if self.mode.get() == Mode::Single {
            self.show_page(index, false);
            self.scroll_to(None, Some(offset));
            return;
        }
        let v = self.root.vadjustment();
        let top = self.layout.borrow().top(index);
        v.set_value(if offset > 0.0 { top + offset } else { self.row_top(index) });
        self.jumped.set(Some((index, v.value())));
        self.emit_status();
    }

    fn go_to_page(&self, index: usize) {
        if index >= self.pages.borrow().len() {
            return;
        }
        if self.mode.get() == Mode::Single {
            self.show_page(index, false);
            return;
        }
        let v = self.root.vadjustment();
        v.set_value(self.row_top(index));
        self.jumped.set(Some((index, v.value())));
        self.emit_status();
    }

    /// Scrolled to put a page's row just under the top of the window.
    fn row_top(&self, page: usize) -> f64 {
        (self.layout.borrow().position(page, 0.0) - layout::SPACING / 2.0).max(0.0)
    }

    /// One page at a time: show another, from its top, or from its end when
    /// going back.
    fn show_page(&self, index: usize, from_end: bool) {
        let count = self.pages.borrow().len();
        if count == 0 {
            return;
        }
        let index = index.min(count - 1);
        self.shown.set(index);
        self.apply_scale(self.scale.get());
        let bottom = (self.layout.borrow().extent().1 - self.root.vadjustment().page_size()).max(0.0);
        self.scroll_to(None, Some(if from_end { bottom } else { 0.0 }));
        self.refresh(true);
        self.emit_status();
    }

    /// Scroll by a share of the window; one page at a time, turn the page
    /// instead at its end.
    fn scroll_by(&self, direction: f64, share: f64) {
        let v = self.root.vadjustment();
        if self.mode.get() == Mode::Single {
            let (at_top, at_bottom) = (v.value() <= 0.5, v.value() + v.page_size() >= v.upper() - 0.5);
            let shown = self.shown.get();
            if direction > 0.0 && at_bottom {
                if shown + 1 < self.pages.borrow().len() {
                    self.show_page(shown + 1, false);
                }
                return;
            }
            if direction < 0.0 && at_top {
                if shown > 0 {
                    self.show_page(shown - 1, true);
                }
                return;
            }
        }
        v.set_value(v.value() + direction * v.page_size() * share);
    }

    /// One page at a time, scrolling on past the end of a page turns it.
    fn scroll_past_page(&self, dy: f64, smooth: bool) -> glib::Propagation {
        if self.mode.get() != Mode::Single || dy == 0.0 {
            return glib::Propagation::Proceed;
        }
        let v = self.root.vadjustment();
        let at_edge = if dy > 0.0 { v.value() + v.page_size() >= v.upper() - 0.5 } else { v.value() <= 0.5 };
        if !at_edge {
            self.push.set(0.0);
            return glib::Propagation::Proceed;
        }
        let push = self.push.get() + dy;
        self.push.set(push);
        let needed = if smooth { PUSH_PIXELS } else { PUSH_NOTCHES };
        let rested = self.turned_at.get().is_none_or(|at| at.elapsed() >= TURN_PAUSE);
        if push.abs() >= needed && rested {
            self.push.set(0.0);
            self.turned_at.set(Some(Instant::now()));
            self.scroll_by(dy.signum(), 0.9);
        }
        glib::Propagation::Stop
    }

    /// The page being read: the one shown, or the one crossing the reading
    /// line.
    fn current_page(&self) -> usize {
        if self.mode.get() == Mode::Single {
            return self.shown.get();
        }
        let v = self.root.vadjustment();
        self.layout.borrow().page_at(v.value() + v.page_size() * READING_LINE)
    }

    /// Scroll now as far as the current layout allows, and again once GTK has
    /// laid out the new page sizes, when the rest becomes reachable.
    fn scroll_to(&self, x: Option<f64>, y: Option<f64>) {
        self.pending.set((x, y));
        if let Some(x) = x {
            self.root.hadjustment().set_value(x);
        }
        if let Some(y) = y {
            self.root.vadjustment().set_value(y);
        }
    }

    fn apply_pending(&self, horizontal: bool) {
        let (x, y) = self.pending.get();
        let (want, adjustment) = if horizontal {
            (x, self.root.hadjustment())
        } else {
            (y, self.root.vadjustment())
        };
        let Some(want) = want else { return };
        adjustment.set_value(want);
        // Reached: done. Otherwise the layout is not there yet; try again on
        // the next change, and give up when the zoom settles.
        if (adjustment.value() - want).abs() < 0.5 {
            self.pending.set(if horizontal { (None, y) } else { (x, None) });
        }
    }

    fn settle_then_render(self: &Rc<Self>) {
        if let Some(source) = self.settle.take() {
            source.remove();
        }
        let weak = Rc::downgrade(self);
        let source = glib::timeout_add_local_once(SETTLE, move || {
            let Some(inner) = weak.upgrade() else { return };
            inner.settle.replace(None);
            inner.pending.set((None, None));
            inner.refresh(true);
        });
        self.settle.replace(Some(source));
    }

    /// Free pages far from the screen and, if `render`, ask for the ones near
    /// it that are missing or were drawn at another size.
    fn refresh(&self, render: bool) {
        let v = self.root.vadjustment();
        let (top, bottom) = (v.value(), v.value() + v.page_size());
        let layout = self.layout.borrow();
        let Some((first, last)) = layout.visible(top, bottom) else { return };

        let widgets = self.widgets.borrow();
        let mut rendered = self.rendered.borrow_mut();
        for (i, widget) in widgets.iter().enumerate() {
            let near = i + KEEP >= first && i <= last + KEEP;
            if !near && rendered[i] != 0.0 {
                widget.set_texture(None);
                rendered[i] = 0.0;
            }
        }
        if !render {
            return;
        }
        let renderer = self.renderer.borrow();
        let Some(renderer) = renderer.as_ref() else { return };

        // On-screen pages first, nearest the middle of the window first; then
        // the next row's worth either side, so turning a page finds it drawn.
        let centre = (top + bottom) / 2.0;
        let middle = |i: usize| layout.top(i) + layout.size(i).1 / 2.0;
        let mut order: Vec<usize> = (first..=last).collect();
        order.sort_by(|&a, &b| (middle(a) - centre).abs().total_cmp(&(middle(b) - centre).abs()));
        let ahead = if self.mode.get() == Mode::Double { 2 * PREFETCH } else { PREFETCH };
        order.extend((last + 1)..=(last + ahead).min(layout.len() - 1));
        order.extend((first.saturating_sub(ahead)..first).rev());

        let (target, rotation) = (self.device_scale(), self.rotation.get());
        let jobs = order
            .into_iter()
            .filter(|&page| rendered[page] != target)
            .map(|page| Job { page, scale: target, rotation })
            .collect();
        renderer.want(jobs);
    }

    fn on_rendered(&self, rendered: Rendered) {
        // Drawn for a zoom, a turn or a version of the file that has since
        // changed.
        if rendered.requested != self.device_scale()
            || rendered.rotation != self.rotation.get()
            || rendered.revision != self.revision.get()
        {
            return;
        }
        let v = self.root.vadjustment();
        let visible = self.layout.borrow().visible(v.value(), v.value() + v.page_size());
        let Some((first, last)) = visible else { return };
        // Scrolled far away while it was being drawn: not worth holding.
        if rendered.page + KEEP < first || rendered.page > last + KEEP {
            return;
        }
        let widgets = self.widgets.borrow();
        let Some(widget) = widgets.get(rendered.page) else { return };
        widget.set_texture(Some(texture(rendered.pixels)));
        self.rendered.borrow_mut()[rendered.page] = rendered.requested;
        // The drawing is in the page now; the sketch of it can go.
        if self.sketched.get() == Some(rendered.page) {
            widget.set_sketch(None);
            self.sketched.set(None);
        }
    }

    fn emit_status(&self) {
        let pages = self.pages.borrow().len();
        if pages == 0 {
            return;
        }
        let v = self.root.vadjustment();
        let page = if self.mode.get() == Mode::Single {
            self.shown.get() + 1
        } else if let Some((page, _)) = self.jumped.get() {
            page + 1
        } else if v.value() <= 0.5 {
            1
        } else if v.value() + v.page_size() >= v.upper() - 0.5 {
            // Scrolled to the end: the last page, even if it is short, or the
            // first of the last pair.
            self.layout.borrow().page_at(v.upper()) + 1
        } else {
            self.current_page() + 1
        };
        let status = Status { page, pages, percent: self.scale.get() / layout::ACTUAL * 100.0 };
        if self.last_status.replace(Some(status)) != Some(status) {
            self.sidebar.set_current(page - 1);
            if let Some(callback) = self.status.borrow().as_ref() {
                callback(status);
            }
        }
    }

    /// Where a point in the column falls, in the PDF's own page coordinates.
    /// A point between pages belongs to the nearest.
    fn hit(&self, x: f64, y: f64) -> Option<Spot> {
        let layout = self.layout.borrow();
        if layout.len() == 0 {
            return None;
        }
        let page = layout.page_at_point(x, y);
        let (w, h) = layout.size(page);
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        let local_x = (x - layout.left(page)).clamp(0.0, w);
        let local_y = (y - layout.top(page)).clamp(0.0, h);
        let (page_w, page_h) = self.pages.borrow()[page];
        let rotation = self.rotation.get();
        let (turned_w, turned_h) = rotation.size(page_w, page_h);
        let (px, py) = rotation.undo(local_x / w * turned_w, local_y / h * turned_h, page_w, page_h);
        Some(Spot { page, x: px, y: py })
    }

    /// An area of a page, x1, y1, x2, y2 in points, as a rectangle in the
    /// reader's own coordinates, for pointing a popover at.
    fn on_screen(&self, page: usize, area: [f64; 4]) -> Option<gdk::Rectangle> {
        let size = *self.pages.borrow().get(page)?;
        let [x1, y1, x2, y2] = area;
        let [fx, fy, fw, fh] = shown([x1, y1, x2 - x1, y2 - y1], size, self.rotation.get());
        let layout = self.layout.borrow();
        let (w, h) = layout.size(page);
        let corner = graphene::Point::new((layout.left(page) + fx * w) as f32, (layout.top(page) + fy * h) as f32);
        let at = self.column.compute_point(&self.root, &corner)?;
        Some(gdk::Rectangle::new(at.x() as i32, at.y() as i32, (fw * w).ceil() as i32, (fh * h).ceil() as i32))
    }

    /// Count presses landing close together in time and place, the way the
    /// desktop's own double-click settings say to.
    fn count_press(&self, x: f64, y: f64) -> Unit {
        let settings = gtk::Settings::default();
        let time = settings.as_ref().map_or(400, |s| s.gtk_double_click_time());
        let distance = settings.as_ref().map_or(5, |s| s.gtk_double_click_distance());
        let now = Instant::now();
        let (count, last) = self.presses.get();
        let count = match last {
            Some((at, lx, ly))
                if now.duration_since(at) <= Duration::from_millis(u64::try_from(time).unwrap_or(400))
                    && (x - lx).hypot(y - ly) <= f64::from(distance) =>
            {
                count + 1
            }
            _ => 1,
        };
        self.presses.set((count, Some((now, x, y))));
        match count {
            1 => Unit::Glyph,
            2 => Unit::Word,
            _ => Unit::Line,
        }
    }

    /// The document on this thread, opened the first time it is needed.
    fn reader(&self) -> Option<poppler::Document> {
        if self.reader.borrow().is_none() {
            let source = self.source()?;
            self.reader.replace(source.load().ok());
        }
        self.reader.borrow().clone()
    }

    fn path(&self) -> Option<PathBuf> {
        self.uri.borrow().as_deref().and_then(|uri| gio::File::for_uri(uri).path())
    }

    fn source(&self) -> Option<Source> {
        Some(Source { uri: self.uri.borrow().clone()?, password: self.password.borrow().clone() })
    }

    /// The page the header says is being read, counting from zero.
    fn reported_page(&self) -> usize {
        self.last_status.get().map_or_else(|| self.current_page(), |status| status.page.saturating_sub(1))
    }

    /// A new bookmark, named for the heading the page is under, if any.
    fn add_bookmark(&self, page: usize) {
        if page >= self.pages.borrow().len() {
            return;
        }
        let name = outline::covering(&self.outline.borrow(), page)
            .map_or_else(|| format!("Page {}", page + 1), |heading| heading.title.clone());
        let seen = self.sidebar.showing_bookmarks();
        let mut marks = self.bookmarks.borrow().clone();
        marks.push(Bookmark { page, name });
        if self.set_bookmarks(marks) {
            self.emit_bookmark(BookmarkEvent::Added { page, seen });
        }
    }

    fn remove_bookmark(&self, page: usize) {
        let mut marks = self.bookmarks.borrow().clone();
        let Some(i) = marks.iter().position(|mark| mark.page == page) else { return };
        let removed = marks.remove(i);
        if self.set_bookmarks(marks) {
            self.emit_bookmark(BookmarkEvent::Removed(removed));
        }
    }

    /// Keep a new set of bookmarks, and show it. False if it could not be
    /// saved, in which case nothing changes.
    fn set_bookmarks(&self, mut marks: Vec<Bookmark>) -> bool {
        let Some(key) = self.bookmark_key.borrow().clone() else { return false };
        marks.sort_by_key(|mark| mark.page);
        if let Err(error) = bookmarks::save(&key, &marks) {
            self.emit_bookmark(BookmarkEvent::Failed(format!("Could not save the bookmarks: {error}")));
            return false;
        }
        self.sidebar.set_bookmarks(&marks, self.reported_page());
        self.bookmarks.replace(marks);
        true
    }

    fn emit_bookmark(&self, event: BookmarkEvent) {
        if let Some(callback) = self.on_bookmarks.borrow().as_ref() {
            callback(event);
        }
    }

    fn set_selection(&self, selection: Option<Selection>) {
        self.selection.set(selection);
        let mut now = Vec::new();
        if let (Some(selection), Some(reader)) = (selection, selection.and_then(|_| self.reader())) {
            let pages = self.pages.borrow();
            let widgets = self.widgets.borrow();
            let rotation = self.rotation.get();
            for (page, span) in document::spans(selection.anchor, selection.head, &pages) {
                let areas = document::highlights(&reader, page, span, selection.unit)
                    .into_iter()
                    .map(|area| shown(area, pages[page], rotation))
                    .collect();
                widgets[page].set_highlights(areas);
                now.push(page);
            }
        }
        let widgets = self.widgets.borrow();
        for page in self.highlighted.replace(now.clone()) {
            if !now.contains(&page) {
                if let Some(widget) = widgets.get(page) {
                    widget.set_highlights(Vec::new());
                }
            }
        }
    }

    fn search(self: &Rc<Self>, query: &str) {
        self.searcher.replace(None);
        self.matches.borrow_mut().clear();
        self.current_match.set(None);
        let id = self.search_id.get() + 1;
        self.search_id.set(id);
        self.mark_all();

        let query = query.trim();
        let (Some(source), false) = (self.source(), query.is_empty()) else {
            self.search_done.set(true);
            self.emit_search_status();
            return;
        };
        self.search_done.set(false);
        self.search_from.set(self.current_page());
        let (sender, receiver) = async_channel::bounded(16);
        self.searcher.replace(Some(Searcher::start(source, query.to_string(), sender)));
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(found) = receiver.recv().await {
                let Some(inner) = weak.upgrade() else { break };
                if inner.search_id.get() != id {
                    break;
                }
                match found {
                    Found::Page(matches) => inner.add_matches(matches),
                    Found::Done => {
                        inner.search_done.set(true);
                        // Nothing from where the reader was to the end: wrap
                        // round to the first match in the document.
                        if inner.current_match.get().is_none() && !inner.matches.borrow().is_empty() {
                            inner.current_match.set(Some(0));
                            inner.mark_page(inner.matches.borrow()[0].page);
                            inner.reveal_current();
                        }
                    }
                }
                inner.emit_search_status();
            }
        });
        self.emit_search_status();
    }

    fn add_matches(&self, found: Vec<Match>) {
        let Some(page) = found.first().map(|m| m.page) else { return };
        let first_new = self.matches.borrow().len();
        self.matches.borrow_mut().extend(found);
        // The first match from where the reader was: show it straight away,
        // without waiting for the rest of the document.
        if self.current_match.get().is_none() && page >= self.search_from.get() {
            self.current_match.set(Some(first_new));
            self.mark_page(page);
            self.reveal_current();
        } else {
            self.mark_page(page);
        }
    }

    fn search_step(&self, forward: bool) {
        let total = self.matches.borrow().len();
        if total == 0 {
            return;
        }
        let old = self.current_match.get();
        let new = match old {
            None if forward => 0,
            None => total - 1,
            Some(current) if forward => (current + 1) % total,
            Some(current) => (current + total - 1) % total,
        };
        self.current_match.set(Some(new));
        let page_of = |i: usize| self.matches.borrow()[i].page;
        if let Some(old) = old {
            self.mark_page(page_of(old));
        }
        self.mark_page(page_of(new));
        self.reveal_current();
        self.emit_search_status();
    }

    /// Mark one page's matches, the current one in its own colour.
    fn mark_page(&self, page: usize) {
        let pages = self.pages.borrow();
        let Some(&size) = pages.get(page) else { return };
        let rotation = self.rotation.get();
        let current = self.current_match.get();
        let (mut all, mut here) = (Vec::new(), Vec::new());
        for (i, found) in self.matches.borrow().iter().enumerate().filter(|(_, m)| m.page == page) {
            let areas = found.areas.iter().map(|&area| shown(area, size, rotation));
            if Some(i) == current {
                here.extend(areas);
            } else {
                all.extend(areas);
            }
        }
        let marked = !all.is_empty() || !here.is_empty();
        if let Some(widget) = self.widgets.borrow().get(page) {
            widget.set_matches(all, here);
        }
        let mut list = self.marked.borrow_mut();
        if marked && !list.contains(&page) {
            list.push(page);
        }
    }

    /// Redo every page's marks: after a turn, or to clear them all.
    fn mark_all(&self) {
        let mut pages: Vec<usize> = std::mem::take(&mut *self.marked.borrow_mut());
        pages.extend(self.matches.borrow().iter().map(|m| m.page));
        pages.sort_unstable();
        pages.dedup();
        for page in pages {
            self.mark_page(page);
        }
    }

    /// Scroll the current match into view, a third of the way down the
    /// window, if it is not already on screen.
    fn reveal_current(&self) {
        let Some(current) = self.current_match.get() else { return };
        let (page, area) = {
            let matches = self.matches.borrow();
            let Some(found) = matches.get(current) else { return };
            let Some(&area) = found.areas.first() else { return };
            (found.page, area)
        };
        self.reveal(page, area);
    }

    /// Bring an area of a page — x, y, width, height in points — into view,
    /// turning to its page first if pages are shown one at a time.
    fn reveal(&self, page: usize, area: [f64; 4]) {
        let Some(&size) = self.pages.borrow().get(page) else { return };
        if self.mode.get() == Mode::Single && self.shown.get() != page {
            self.show_page(page, false);
        }
        let [fx, fy, fw, fh] = shown(area, size, self.rotation.get());
        // Worked out from the layout rather than from where GTK last put the
        // page, which is out of date just after a page is turned.
        let layout = self.layout.borrow();
        let (w, h) = layout.size(page);
        let (x, y) = (layout.left(page) + fx * w, layout.top(page) + fy * h);
        let (mw, mh) = (fw * w, fh * h);
        drop(layout);
        let (hadj, vadj) = (self.root.hadjustment(), self.root.vadjustment());
        let want_y = (y < vadj.value() || y + mh > vadj.value() + vadj.page_size())
            .then(|| (y - vadj.page_size() * 0.3).max(0.0));
        let want_x = (x < hadj.value() || x + mw > hadj.value() + hadj.page_size())
            .then(|| (x - hadj.page_size() * 0.3).max(0.0));
        if want_x.is_some() || want_y.is_some() {
            self.scroll_to(want_x, want_y);
        }
    }

    fn emit_search_status(&self) {
        let status = SearchStatus {
            current: self.current_match.get().map(|i| i + 1),
            total: self.matches.borrow().len(),
            done: self.search_done.get(),
        };
        if let Some(callback) = self.search_status.borrow().as_ref() {
            callback(status);
        }
    }

    fn mark(&self, style: Style, colour: Rgb) -> Marked {
        let Some(selection) = self.selection.get() else { return Marked::NothingSelected };
        let Some(reader) = self.reader() else { return Marked::NothingSelected };
        let colour = if style == Style::Highlight { colour } else { style.default_colour() };
        let marks: Vec<Mark> = {
            let pages = self.pages.borrow();
            document::spans(selection.anchor, selection.head, &pages)
                .into_iter()
                .map(|(page, span)| Mark {
                    page,
                    style,
                    lines: markup::selected_lines(&reader, page, span, selection.unit),
                    colour,
                })
                .filter(|mark| !mark.lines.is_empty())
                .collect()
        };
        if marks.is_empty() {
            return Marked::NothingSelected; // Only space between words.
        }
        let existing: Vec<Option<Rgb>> = marks.iter().map(|mark| markup::existing_colour(&reader, mark)).collect();
        let mut change = Change::default();
        if existing.iter().all(|&found| found == Some(colour)) {
            // All of it marked just so already: the same again takes it off,
            // like bold in a word processor.
            change.removed = marks.into_iter().map(Annotation::Mark).collect();
        } else {
            // Mark what is not yet; what is marked in another colour changes
            // colour, remembering the old one for undo.
            for (mark, found) in marks.into_iter().zip(existing) {
                match found {
                    Some(old) if old == colour => {}
                    Some(old) => {
                        change.removed.push(Annotation::Mark(Mark { colour: old, ..mark.clone() }));
                        change.added.push(Annotation::Mark(mark));
                    }
                    None => change.added.push(Annotation::Mark(mark)),
                }
            }
        }
        if let Err(error) = self.commit(change) {
            return Marked::Failed(error);
        }
        // Out of the way, so the mark just made shows.
        self.set_selection(None);
        Marked::Done
    }

    /// Make a change, save it, and make it the one undo takes back.
    fn commit(&self, change: Change) -> Result<(), String> {
        let Some(reader) = self.reader() else { return Ok(()) };
        self.apply(&reader, &change, true)?;
        self.history.borrow_mut().push(change);
        self.undone.borrow_mut().clear();
        self.emit_history();
        Ok(())
    }

    /// Undo the last change, or redo the last one undone.
    fn step_history(&self, back: bool) -> Result<(), String> {
        let (from, to) = if back { (&self.history, &self.undone) } else { (&self.undone, &self.history) };
        self.flush_restyle();
        let Some(change) = from.borrow_mut().pop() else { return Ok(()) };
        let Some(reader) = self.reader() else { return Ok(()) };
        // What was chosen may be what is about to be taken away.
        self.choose(None);
        if let Err(error) = self.apply(&reader, &change, !back) {
            from.borrow_mut().push(change);
            return Err(error);
        }
        self.reveal_change(&change);
        to.borrow_mut().push(change);
        self.emit_history();
        Ok(())
    }

    /// Make a change, or take it back, and save the file. If it cannot be
    /// saved the document is put back as it was, so what is on screen never
    /// differs from the file.
    fn apply(&self, reader: &poppler::Document, change: &Change, forwards: bool) -> Result<(), String> {
        let (take, put) = if forwards { (&change.removed, &change.added) } else { (&change.added, &change.removed) };
        let edit = |take: &[Annotation], put: &[Annotation]| {
            for annotation in take {
                annotation.remove(reader);
            }
            for annotation in put {
                annotation.add(reader);
            }
        };
        let path = self.path().ok_or_else(|| "Only a file on this computer can be marked up.".to_string())?;
        if !self.allowed.get().annotate {
            return Err(NOT_ANNOTATABLE.to_string());
        }
        let mut pages: Vec<usize> = take.iter().chain(put).map(Annotation::page).collect();
        pages.sort_unstable();
        pages.dedup();
        edit(take, put);
        annots::settle(reader, &pages);
        if let Err(error) = annots::save(reader, &path) {
            edit(put, take);
            return Err(error);
        }

        let revision = self.revision.get() + 1;
        self.revision.set(revision);
        self.pinned.borrow_mut().clear();
        if let Some(renderer) = self.renderer.borrow().as_ref() {
            renderer.reload(revision);
        }
        {
            let mut rendered = self.rendered.borrow_mut();
            for &page in &pages {
                if let Some(scale) = rendered.get_mut(page) {
                    *scale = 0.0;
                }
            }
        }
        // The old drawing stays up until the new one replaces it, so the page
        // does not blink.
        self.refresh(true);
        self.sidebar.reload(&pages, revision);
        Ok(())
    }

    /// Bring an undone or redone change into view, if it is off screen.
    fn reveal_change(&self, change: &Change) {
        let Some(page) = change.added.iter().chain(&change.removed).map(Annotation::page).next() else { return };
        let visible = if self.mode.get() == Mode::Single {
            self.shown.get() == page
        } else {
            let v = self.root.vadjustment();
            let range = self.layout.borrow().visible(v.value(), v.value() + v.page_size());
            range.is_some_and(|(first, last)| (first..=last).contains(&page))
        };
        if !visible {
            self.go_to_page(page);
        }
    }

    fn emit_history(&self) {
        let can_undo = !self.history.borrow().is_empty();
        let can_redo = !self.undone.borrow().is_empty();
        if let Some(callback) = self.on_history.borrow().as_ref() {
            callback(can_undo, can_redo);
        }
    }

    fn report(&self, error: String) {
        if let Some(callback) = self.on_error.borrow().as_ref() {
            callback(error);
        }
    }

    /// The note or bubble under a spot, if any.
    fn pinned_at(&self, spot: Spot) -> Option<Annotation> {
        let reader = self.reader()?;
        let mut pinned = self.pinned.borrow_mut();
        let here = pinned.entry(spot.page).or_insert_with(|| notes::on_page(&reader, spot.page));
        notes::under(here, spot.x, spot.y).cloned()
    }

    /// A hand over a note or bubble, the text cursor elsewhere.
    fn hover(&self, x: f64, y: f64) {
        // A pen draws over notes like anything else on the page.
        if matches!(self.tool.get(), Tool::Draw(_)) {
            return;
        }
        let over = self.hit(x, y).filter(|&spot| self.pinned_at(spot).is_some()).map(|spot| spot.page);
        if over == self.hovering.get() {
            return;
        }
        let widgets = self.widgets.borrow();
        if let Some(widget) = self.hovering.get().and_then(|page| widgets.get(page)) {
            widget.set_cursor_from_name(Some(self.cursor()));
        }
        if let Some(widget) = over.and_then(|page| widgets.get(page)) {
            widget.set_cursor_from_name(Some("pointer"));
        }
        self.hovering.set(over);
    }

    fn drag_pinned(&self, to: Spot) {
        let mut moving = self.moving.borrow_mut();
        let Some(moving) = moving.as_mut() else { return };
        // A note stays on its own page.
        if to.page != moving.from.page {
            return;
        }
        moving.by = (to.x - moving.from.x, to.y - moving.from.y);
        let page = moving.from.page;
        let Some(&size) = self.pages.borrow().get(page) else { return };
        let [x1, y1, x2, y2] = area_of(&moved(&moving.annotation, moving.by, size));
        let ghost = shown([x1, y1, x2 - x1, y2 - y1], size, self.rotation.get());
        if let Some(widget) = self.widgets.borrow().get(page) {
            widget.set_ghost(Some(ghost));
        }
    }

    /// A note or bubble let go of: moved, if it was dragged, or opened, if it
    /// was only clicked.
    fn drop_pinned(self: &Rc<Self>, moving: Moving) {
        let page = moving.from.page;
        if let Some(widget) = self.widgets.borrow().get(page) {
            widget.set_ghost(None);
        }
        let Some(&size) = self.pages.borrow().get(page) else { return };
        let text_tool = self.tool.get() == Tool::Text;
        if moving.by.0.hypot(moving.by.1) < NUDGE {
            // With the text tool a click picks a text box, to restyle in the
            // panel; a note still opens.
            if !(text_tool && matches!(moving.annotation, Annotation::TextBox(_))) {
                self.open_pinned(moving.annotation);
            }
            return;
        }
        self.flush_restyle();
        let after = moved(&moving.annotation, moving.by, size);
        let change = Change { removed: vec![moving.annotation], added: vec![after.clone()] };
        match self.commit(change) {
            Ok(()) if text_tool => {
                if let Annotation::TextBox(text_box) = after {
                    self.choose(Some(text_box));
                }
            }
            Ok(()) => {}
            Err(error) => self.report(error),
        }
    }

    /// Open a note or bubble already on the page, to read, change or delete.
    fn open_pinned(self: &Rc<Self>, annotation: Annotation) {
        let page = annotation.page();
        let (title, text) = match &annotation {
            Annotation::Note(note) => ("Note", note.text.clone()),
            Annotation::TextBox(text_box) if text_box.tip.is_some() => ("Speech Bubble", text_box.text.clone()),
            Annotation::TextBox(text_box) => ("Text Box", text_box.text.clone()),
            Annotation::Mark(_) | Annotation::Ink(_) => return,
        };
        let Some(at) = self.on_screen(page, area_of(&annotation)) else { return };
        let size = self.pages.borrow()[page];
        let weak = Rc::downgrade(self);
        self.editor.open(at, title, &text, true, move |commit| {
            let Some(inner) = weak.upgrade() else { return };
            let change = match commit {
                Commit::Delete => Change { removed: vec![annotation.clone()], added: Vec::new() },
                // Emptied is as good as deleted.
                Commit::Text(text) if text.trim().is_empty() => {
                    Change { removed: vec![annotation.clone()], added: Vec::new() }
                }
                Commit::Text(text) => {
                    let after = match &annotation {
                        Annotation::Note(note) => Annotation::Note(Note { text, ..note.clone() }),
                        Annotation::TextBox(text_box) => {
                            let fits = inner.box_size(&text, &text_box.style, size.0);
                            Annotation::TextBox(text_box.changed(text, text_box.style.clone(), fits, size))
                        }
                        Annotation::Mark(_) | Annotation::Ink(_) => return,
                    };
                    Change { removed: vec![annotation.clone()], added: vec![after] }
                }
            };
            if let Err(error) = inner.commit(change) {
                inner.report(error);
            }
        });
    }

    /// Where on a page the context menu was opened.
    fn menu_spot(&self) -> Option<Spot> {
        self.menu_point.get().and_then(|(x, y)| {
            let point = self.root.compute_point(&self.column, &graphene::Point::new(x as f32, y as f32))?;
            self.hit(f64::from(point.x()), f64::from(point.y()))
        })
    }

    fn add_redactions(&self, more: Vec<RedactArea>) {
        let mut areas = self.redactions.borrow().clone();
        areas.extend(more);
        self.set_redactions(areas);
    }

    /// Keep and show a new set of marked areas.
    fn set_redactions(&self, areas: Vec<RedactArea>) {
        let mut pages: Vec<usize> = self.redactions.borrow().iter().chain(&areas).map(|a| a.page).collect();
        pages.sort_unstable();
        pages.dedup();
        let count = areas.len();
        let changed = *self.redactions.borrow() != areas;
        self.redactions.replace(areas);
        for page in pages {
            self.show_redactions(page);
        }
        if changed {
            if let Some(callback) = self.on_redactions.borrow().as_ref() {
                callback(count);
            }
        }
    }

    /// Show a page's marked areas, the way the page is turned.
    fn show_redactions(&self, page: usize) {
        let Some(&size) = self.pages.borrow().get(page) else { return };
        let rotation = self.rotation.get();
        let areas = self
            .redactions
            .borrow()
            .iter()
            .filter(|area| area.page == page)
            .map(|area| shown(area.rect, size, rotation))
            .collect();
        if let Some(widget) = self.widgets.borrow().get(page) {
            widget.set_redactions(areas);
        }
    }

    /// Open a new note or bubble for writing, and put it on the page once
    /// something is written.
    fn pin(self: &Rc<Self>, kind: Pinned, here: bool) {
        let spot = if here { self.menu_spot() } else { None };
        // Otherwise just after the selected text, or near the top of the page
        // being read.
        let spot = spot.or_else(|| self.selection_end()).or_else(|| {
            let page = self.current_page();
            let (w, h) = *self.pages.borrow().get(page)?;
            Some(Spot { page, x: w / 2.0, y: h / 4.0 })
        });
        let Some(spot) = spot else { return };
        let Some(&size) = self.pages.borrow().get(spot.page) else { return };
        let (title, point) = match kind {
            Pinned::Note => ("New Note", [spot.x, spot.y, spot.x + notes::NOTE_SIZE, spot.y + notes::NOTE_SIZE]),
            Pinned::Bubble => ("New Speech Bubble", [spot.x, spot.y, spot.x + 1.0, spot.y + 1.0]),
        };
        self.set_selection(None);
        let Some(at) = self.on_screen(spot.page, point) else { return };
        let weak = Rc::downgrade(self);
        self.editor.open(at, title, "", false, move |commit| {
            let Some(inner) = weak.upgrade() else { return };
            let Commit::Text(text) = commit else { return };
            if text.trim().is_empty() {
                return;
            }
            let annotation = match kind {
                Pinned::Note => Annotation::Note(Note::new(spot.page, (spot.x, spot.y), size, text)),
                Pinned::Bubble => Annotation::TextBox(TextBox::bubble(spot.page, (spot.x, spot.y), size, text)),
            };
            if let Err(error) = inner.commit(Change { removed: Vec::new(), added: vec![annotation] }) {
                inner.report(error);
            }
        });
    }

    fn set_tool(&self, tool: Tool) {
        if self.tool.replace(tool) == tool {
            return;
        }
        self.flush_restyle();
        self.choose(None);
        for widget in self.widgets.borrow().iter() {
            widget.set_cursor_from_name(Some(self.cursor()));
        }
        self.hovering.set(None);
    }

    /// The pointer over a page, for the tool in hand: a drawing tool takes
    /// over the press, so the text cursor would lie.
    fn cursor(&self) -> &'static str {
        match self.tool.get() {
            Tool::Draw(_) => "crosshair",
            Tool::Text => "default",
            Tool::Select => "text",
        }
    }

    /// Start a drawing where the pointer went down.
    fn start_sketch(&self, spot: Spot, tool: draw::Tool) {
        // A redaction is not ink, and needs nothing newer from Poppler.
        if tool != draw::Tool::Redact && !ink::available() {
            self.report("Drawing on a PDF needs Poppler 25.06 or newer.".to_string());
            return;
        }
        let (colour, width) = self.ink.get();
        let mark = draw::Mark { tool, points: vec![(spot.x, spot.y)], colour, width, sequence: 0 };
        // A new drawing replaces any sketch still waiting for its page.
        if let Some(page) = self.sketched.take() {
            if let Some(widget) = self.widgets.borrow().get(page) {
                widget.set_sketch(None);
            }
        }
        self.sketch.replace(Some((spot.page, mark)));
        self.show_sketch();
    }

    fn extend_sketch(&self, to: Spot) {
        {
            let mut sketch = self.sketch.borrow_mut();
            let Some((page, mark)) = sketch.as_mut() else { return };
            // A drawing stays on the page it was started on.
            if to.page != *page {
                return;
            }
            mark.extend((to.x, to.y));
        }
        self.show_sketch();
    }

    /// Draw the sketch on its page, converted to the page widget's pixels.
    fn show_sketch(&self) {
        let sketch = self.sketch.borrow();
        let Some((page, mark)) = sketch.as_ref() else { return };
        let Some(&(pw, ph)) = self.pages.borrow().get(*page) else { return };
        let rotation = self.rotation.get();
        let (tw, th) = rotation.size(pw, ph);
        let (w, h) = self.layout.borrow().size(*page);
        let points = mark
            .points
            .iter()
            .map(|&(x, y)| {
                let (x, y) = rotation.apply(x, y, pw, ph);
                (x / tw * w, y / th * h)
            })
            .collect();
        let shown = draw::Mark { points, width: mark.width * w / tw, ..mark.clone() };
        if let Some(widget) = self.widgets.borrow().get(*page) {
            widget.set_sketch(Some(shown));
        }
    }

    /// The pointer let go: save the drawing. Its sketch stays up until the
    /// page has been redrawn with it, so it never blinks out.
    fn finish_sketch(&self) {
        let Some((page, mark)) = self.sketch.take() else { return };
        // Marked, not applied: it joins the list waiting to be applied.
        if mark.tool == draw::Tool::Redact {
            if let Some(widget) = self.widgets.borrow().get(page) {
                widget.set_sketch(None);
            }
            if let (true, Some((x, y, w, h))) = (mark.is_worth_keeping(), mark.rect()) {
                self.add_redactions(vec![RedactArea { page, rect: [x, y, w, h] }]);
            }
            return;
        }
        let saved = match Drawing::from_mark(page, &mark) {
            Some(drawing) => match self.commit(Change { removed: Vec::new(), added: vec![Annotation::Ink(drawing)] }) {
                Ok(()) => true,
                Err(error) => {
                    self.report(error);
                    false
                }
            },
            None => false,
        };
        if saved {
            self.sketched.set(Some(page));
        } else if let Some(widget) = self.widgets.borrow().get(page) {
            widget.set_sketch(None);
        }
    }

    /// The text box a piece of text in a style needs, `page_width` wide at
    /// most. Measured with Pango in that very font, the one Poppler will find.
    fn box_size(&self, text: &str, style: &TextStyle, page_width: f64) -> (f64, f64) {
        let measured = style.family.as_ref().map(|family| {
            let layout = self.root.create_pango_layout(Some(text));
            let mut desc = pango::FontDescription::new();
            desc.set_family(family);
            desc.set_absolute_size(style.size * f64::from(pango::SCALE));
            desc.set_weight(if style.bold { pango::Weight::Bold } else { pango::Weight::Normal });
            desc.set_style(if style.italic { pango::Style::Italic } else { pango::Style::Normal });
            layout.set_font_description(Some(&desc));
            // Absolute sizes make Pango's pixels points here.
            layout.set_width(((page_width * 0.8) * f64::from(pango::SCALE)) as i32);
            layout.set_wrap(pango::WrapMode::WordChar);
            let (width, _) = layout.pixel_size();
            (f64::from(width), usize::try_from(layout.line_count()).unwrap_or(1))
        });
        notes::box_size(text, style, measured)
    }

    /// Put a new text box down, with the words and look the controls show.
    fn place_text(&self, spot: Spot) {
        let Some(&page_size) = self.pages.borrow().get(spot.page) else { return };
        let (text, style) = self.look.borrow().clone();
        let text = if text.trim().is_empty() { "Text".to_string() } else { text };
        let size = self.box_size(&text, &style, page_size.0);
        let text_box = TextBox::new(spot.page, (spot.x, spot.y), size, page_size, text, style);
        match self.commit(Change { removed: Vec::new(), added: vec![Annotation::TextBox(text_box.clone())] }) {
            Ok(()) => self.choose(Some(text_box)),
            Err(error) => self.report(error),
        }
    }

    /// Pick a text box, or none: outlined on its page, and shown in the
    /// controls.
    fn choose(&self, text_box: Option<TextBox>) {
        let widgets = self.widgets.borrow();
        if let Some(old) = self.chosen.borrow().as_ref() {
            if let Some(widget) = widgets.get(old.page) {
                widget.set_outline(None);
            }
        }
        if let Some(text_box) = &text_box {
            if let (Some(widget), Some(&size)) = (widgets.get(text_box.page), self.pages.borrow().get(text_box.page)) {
                let [x1, y1, x2, y2] = text_box.area;
                widget.set_outline(Some(shown([x1, y1, x2 - x1, y2 - y1], size, self.rotation.get())));
            }
        }
        drop(widgets);
        let look = text_box.as_ref().map(|t| (t.text.clone(), t.style.clone()));
        self.chosen.replace(text_box);
        if let Some(callback) = self.on_chosen.borrow().as_ref() {
            callback(look);
        }
    }

    fn set_text_look(self: &Rc<Self>, text: String, style: TextStyle) {
        self.look.replace((text, style));
        if self.chosen.borrow().is_none() {
            return;
        }
        if let Some(source) = self.restyle.take() {
            source.remove();
        }
        let weak = Rc::downgrade(self);
        let source = glib::timeout_add_local_once(RESTYLE_PAUSE, move || {
            if let Some(inner) = weak.upgrade() {
                inner.restyle.replace(None);
                inner.apply_restyle();
            }
        });
        self.restyle.replace(Some(source));
    }

    /// Save a change to the chosen box now, rather than when typing pauses.
    fn flush_restyle(&self) {
        if let Some(source) = self.restyle.take() {
            source.remove();
            self.apply_restyle();
        }
    }

    fn apply_restyle(&self) {
        let Some(old) = self.chosen.borrow().clone() else { return };
        let (text, style) = self.look.borrow().clone();
        // Emptied while typing is not a delete; Remove is for that.
        if text.trim().is_empty() {
            return;
        }
        let Some(&page_size) = self.pages.borrow().get(old.page) else { return };
        let size = self.box_size(&text, &style, page_size.0);
        let new = old.changed(text, style, size, page_size);
        if new == old {
            return;
        }
        match self.commit(Change { removed: vec![Annotation::TextBox(old)], added: vec![Annotation::TextBox(new.clone())] }) {
            Ok(()) => {
                // Still chosen, now as it is in the file; the controls already
                // show it, so they are not told.
                let callback = self.on_chosen.take();
                self.choose(Some(new));
                self.on_chosen.replace(callback);
            }
            Err(error) => self.report(error),
        }
    }

    fn remove_chosen(&self) {
        if let Some(source) = self.restyle.take() {
            source.remove();
        }
        let Some(text_box) = self.chosen.borrow().clone() else { return };
        self.choose(None);
        if let Err(error) = self.commit(Change { removed: vec![Annotation::TextBox(text_box)], added: Vec::new() }) {
            self.report(error);
        }
    }

    /// The top right corner of the last line of the selection: where the
    /// text chosen ends, whichever way it was dragged.
    fn selection_end(&self) -> Option<Spot> {
        let selection = self.selection.get()?;
        let reader = self.reader()?;
        let pages = self.pages.borrow();
        let (page, span) = document::spans(selection.anchor, selection.head, &pages).pop()?;
        let areas = document::highlights(&reader, page, span, selection.unit);
        // The last line is the lowest; the end of it, its rightmost box.
        let bottom = areas.iter().map(|a| a[1]).fold(f64::MIN, f64::max);
        let last = areas.iter().filter(|a| (a[1] - bottom).abs() < 1.0).max_by(|a, b| (a[0] + a[2]).total_cmp(&(b[0] + b[2])))?;
        Some(Spot { page, x: last[0] + last[2], y: last[1] })
    }

    fn selected_text(&self) -> Option<String> {
        let selection = self.selection.get()?;
        let reader = self.reader()?;
        let pages = self.pages.borrow();
        let parts: Vec<String> = document::spans(selection.anchor, selection.head, &pages)
            .into_iter()
            .map(|(page, span)| document::selected_text(&reader, page, span, selection.unit))
            .filter(|text| !text.is_empty())
            .collect();
        Some(parts.join("\n"))
    }

    fn clear(&self) {
        self.renderer.replace(None);
        self.reader.replace(None);
        if let Some(source) = self.settle.take() {
            source.remove();
        }
        self.document.set(self.document.get() + 1);
        self.selection.set(None);
        self.highlighted.borrow_mut().clear();
        self.pinned.borrow_mut().clear();
        self.moving.replace(None);
        self.hovering.set(None);
        if let Some(source) = self.restyle.take() {
            source.remove();
        }
        self.chosen.replace(None);
        self.sketch.replace(None);
        self.sketched.set(None);
        self.placing.set(None);
        // Taken out first, so nothing is borrowed while GTK removes them.
        let widgets = std::mem::take(&mut *self.widgets.borrow_mut());
        for widget in widgets {
            self.column.remove(&widget);
        }
        self.pages.borrow_mut().clear();
        self.rendered.borrow_mut().clear();
        *self.layout.borrow_mut() = Layout::default();
        self.shown.set(0);
        self.rotation.set(Rotation::default());
        self.uri.replace(None);
        self.password.replace(None);
        self.allowed.set(Allowed::default());
        self.pending.set((None, None));
        self.jumped.set(None);
        self.last_status.set(None);
        self.searcher.replace(None);
        self.search_id.set(self.search_id.get() + 1);
        self.matches.borrow_mut().clear();
        self.current_match.set(None);
        self.search_done.set(true);
        self.marked.borrow_mut().clear();
        self.revision.set(0);
        self.history.borrow_mut().clear();
        self.undone.borrow_mut().clear();
        self.emit_history();
        self.sidebar.clear();
        self.outline.borrow_mut().clear();
        self.bookmarks.borrow_mut().clear();
        self.set_redactions(Vec::new());
        self.bookmark_key.replace(None);
    }
}

/// Where a note or bubble sits: x1, y1, x2, y2 in points.
fn area_of(annotation: &Annotation) -> [f64; 4] {
    match annotation {
        Annotation::Note(note) => note.area,
        Annotation::TextBox(text_box) => text_box.area,
        Annotation::Ink(drawing) => drawing.area(),
        Annotation::Mark(mark) => mark.lines.first().map_or([0.0; 4], |line| line.area),
    }
}

fn moved(annotation: &Annotation, by: (f64, f64), page_size: (f64, f64)) -> Annotation {
    match annotation {
        Annotation::Note(note) => Annotation::Note(note.moved(by, page_size)),
        Annotation::TextBox(text_box) => Annotation::TextBox(text_box.moved(by, page_size)),
        Annotation::Mark(_) | Annotation::Ink(_) => annotation.clone(),
    }
}

/// An area on a page, in points from its top-left corner, as fractions of the
/// page as it is shown: turned, if the view is turned. Both corners are
/// turned, then the box around them taken.
fn shown(area: [f64; 4], size: (f64, f64), rotation: Rotation) -> [f64; 4] {
    let [x, y, w, h] = area;
    let (page_w, page_h) = size;
    let (turned_w, turned_h) = rotation.size(page_w, page_h);
    let (x1, y1) = rotation.apply(x, y, page_w, page_h);
    let (x2, y2) = rotation.apply(x + w, y + h, page_w, page_h);
    [x1.min(x2) / turned_w, y1.min(y2) / turned_h, (x1 - x2).abs() / turned_w, (y1 - y2).abs() / turned_h]
}

pub(super) fn texture(pixels: Pixels) -> gdk::Texture {
    // Cairo's ARGB32 is one native-endian word per pixel.
    #[cfg(target_endian = "little")]
    let format = gdk::MemoryFormat::B8g8r8a8Premultiplied;
    #[cfg(target_endian = "big")]
    let format = gdk::MemoryFormat::A8r8g8b8Premultiplied;
    let bytes = glib::Bytes::from_owned(pixels.data);
    gdk::MemoryTexture::new(pixels.width, pixels.height, format, &bytes, pixels.stride).upcast()
}
