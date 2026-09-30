// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading a PDF: every page in one scrolling column.
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

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, glib};

use super::document::{self, Opened, Pixels, Spot, Unit};
use super::layout::{self, Layout, Rotation};
use super::page::Page;
use super::render::{Job, Rendered, Renderer};
use super::sidebar::Sidebar;

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

/// What the header shows: where you are, and at what size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Status {
    pub page: usize,
    pub pages: usize,
    pub percent: f64,
}

/// A selection: where the drag started, where it is now, and by what unit.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Selection {
    anchor: Spot,
    head: Spot,
    unit: Unit,
}

pub struct PdfView {
    inner: Rc<Inner>,
}

struct Inner {
    root: gtk::ScrolledWindow,
    column: gtk::Box,
    sidebar: Sidebar,
    /// Page sizes in points, as the PDF gives them, before any turning.
    pages: RefCell<Vec<(f64, f64)>>,
    widgets: RefCell<Vec<Page>>,
    /// The device scale each page was last drawn at, or 0.0 for not drawn.
    rendered: RefCell<Vec<f64>>,
    layout: RefCell<Layout>,
    rotation: Cell<Rotation>,
    /// Logical pixels per point.
    scale: Cell<f64>,
    /// Following the window's width, until the user zooms by hand.
    fit: Cell<bool>,
    /// A refit to a new window width is waiting for GTK to finish laying out.
    refit_queued: Cell<bool>,
    uri: RefCell<Option<String>>,
    renderer: RefCell<Option<Renderer>>,
    /// The document again, on this thread, for selecting text. Opened the
    /// first time anything is selected, not before.
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
    pinch_from: Cell<f64>,
    selection: Cell<Option<Selection>>,
    /// Pages currently showing a highlight, so they can be cleared.
    highlighted: RefCell<Vec<usize>>,
    /// Consecutive presses in the same spot, for double and triple clicks.
    presses: Cell<(u32, Option<(Instant, f64, f64)>)>,
    status: RefCell<Option<Box<dyn Fn(Status)>>>,
    last_status: Cell<Option<Status>>,
}

impl PdfView {
    pub fn new() -> Self {
        let column = gtk::Box::new(gtk::Orientation::Vertical, layout::SPACING as i32);
        column.set_halign(gtk::Align::Center);
        column.set_valign(gtk::Align::Start);
        let margin = layout::MARGIN as i32;
        column.set_margin_top(margin);
        column.set_margin_bottom(margin);
        column.set_margin_start(margin);
        column.set_margin_end(margin);

        let root = gtk::ScrolledWindow::builder().hexpand(true).vexpand(true).child(&column).build();

        let inner = Rc::new(Inner {
            root,
            column,
            sidebar: Sidebar::new(),
            pages: RefCell::default(),
            widgets: RefCell::default(),
            rendered: RefCell::default(),
            layout: RefCell::default(),
            rotation: Cell::new(Rotation::default()),
            scale: Cell::new(layout::ACTUAL),
            fit: Cell::new(true),
            refit_queued: Cell::new(false),
            uri: RefCell::default(),
            renderer: RefCell::default(),
            reader: RefCell::default(),
            document: Cell::new(0),
            settle: RefCell::default(),
            pending: Cell::new((None, None)),
            jumped: Cell::new(None),
            pointer: Cell::new(None),
            pinch_from: Cell::new(layout::ACTUAL),
            selection: Cell::new(None),
            highlighted: RefCell::default(),
            presses: Cell::new((0, None)),
            status: RefCell::default(),
            last_status: Cell::new(None),
        });
        Inner::connect(&inner);
        PdfView { inner }
    }

    pub fn widget(&self) -> &gtk::ScrolledWindow {
        &self.inner.root
    }

    /// The page thumbnails, for the window to put beside the reader.
    pub fn sidebar(&self) -> &gtk::ScrolledWindow {
        self.inner.sidebar.widget()
    }

    /// The sidebar is on screen, or not. It draws nothing while hidden.
    pub fn set_sidebar_active(&self, active: bool) {
        self.inner.sidebar.set_active(active);
    }

    /// Lay out a freshly opened document, fitted to the window's width, and
    /// start drawing the first pages.
    pub fn show(&self, opened: Opened) {
        let inner = &self.inner;
        inner.clear();
        let id = inner.document.get();

        let count = opened.pages.len();
        let widgets: Vec<Page> = (0..count)
            .map(|_| {
                let widget = Page::new();
                widget.set_cursor_from_name(Some("text"));
                inner.column.append(&widget);
                widget
            })
            .collect();
        inner.sidebar.show_document(opened.uri.clone(), opened.pages.clone());
        *inner.pages.borrow_mut() = opened.pages;
        *inner.widgets.borrow_mut() = widgets;
        *inner.rendered.borrow_mut() = vec![0.0; count];
        inner.uri.replace(Some(opened.uri.clone()));

        inner.fit.set(true);
        let width = inner.root.hadjustment().page_size();
        let scale = if width > 0.0 {
            layout::fit_width(&inner.turned_pages(), width)
        } else {
            // Not on screen yet; fitted properly once it has a width.
            layout::ACTUAL
        };
        inner.apply_scale(scale);
        inner.scroll_to(Some(0.0), Some(0.0));

        let (sender, receiver) = async_channel::bounded(4);
        inner.renderer.replace(Some(Renderer::start(opened.uri, sender)));
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

    /// Fit the widest page to the window, and keep fitting as it resizes.
    pub fn zoom_fit(&self) {
        let inner = &self.inner;
        inner.fit.set(true);
        let width = inner.root.hadjustment().page_size();
        inner.set_scale(layout::fit_width(&inner.turned_pages(), width), inner.centre());
    }

    /// Life size: a centimetre on the page is a centimetre on the screen.
    pub fn zoom_actual(&self) {
        let inner = &self.inner;
        inner.fit.set(false);
        inner.set_scale(layout::ACTUAL, inner.centre());
    }

    /// Move by most of a screen, keeping a strip of the old view for context.
    pub fn scroll_pages(&self, direction: f64) {
        let v = self.inner.root.vadjustment();
        v.set_value(v.value() + direction * v.page_size() * 0.9);
    }

    pub fn scroll_lines(&self, direction: f64) {
        let v = self.inner.root.vadjustment();
        v.set_value(v.value() + direction * v.page_size() * 0.1);
    }

    /// Sideways, for a page zoomed wider than the window.
    pub fn scroll_across(&self, direction: f64) {
        let h = self.inner.root.hadjustment();
        h.set_value(h.value() + direction * h.page_size() * 0.1);
    }

    pub fn scroll_to_start(&self) {
        self.inner.root.vadjustment().set_value(0.0);
    }

    pub fn scroll_to_end(&self) {
        let v = self.inner.root.vadjustment();
        v.set_value(v.upper());
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
        let scale = if inner.fit.get() {
            layout::fit_width(&inner.turned_pages(), inner.root.hadjustment().page_size())
        } else {
            inner.scale.get()
        };
        inner.apply_scale(scale);
        let top = inner.layout.borrow().top(current) - layout::SPACING / 2.0;
        inner.scroll_to(None, Some(top.max(0.0)));
        inner.settle_then_render();
        inner.sidebar.set_rotation(rotation);
        inner.emit_status();
    }

    /// Copy the selected text to the clipboard. False if nothing is selected.
    pub fn copy_selection(&self) -> bool {
        let inner = &self.inner;
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

        // A new window width means a new fitted size. This fires while GTK is
        // in the middle of laying the window out, and a resize requested from
        // inside that pass is lost: GTK finishes the pass and forgets it,
        // leaving the scrollable height a page or more short of the pages.
        // So the refit waits until the pass is over.
        let weak = Rc::downgrade(this);
        h.connect_page_size_notify(move |_| {
            let Some(inner) = weak.upgrade() else { return };
            if !inner.fit.get() || inner.refit_queued.replace(true) {
                return;
            }
            let weak = Rc::downgrade(&inner);
            glib::idle_add_local_once(move || {
                let Some(inner) = weak.upgrade() else { return };
                inner.refit_queued.set(false);
                if inner.fit.get() && !inner.pages.borrow().is_empty() {
                    let width = inner.root.hadjustment().page_size();
                    let scale = layout::fit_width(&inner.turned_pages(), width);
                    // Keep whatever is at the top of the window at the top.
                    inner.set_scale(scale, (0.0, 0.0));
                }
            });
        });

        let weak = Rc::downgrade(this);
        this.sidebar.connect_pick(move |index| {
            if let Some(inner) = weak.upgrade() {
                inner.go_to_page(index);
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
            if !controller.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
                return glib::Propagation::Proceed;
            }
            let smooth = controller
                .current_event()
                .and_then(|event| event.downcast::<gdk::ScrollEvent>().ok())
                .is_some_and(|event| event.unit() == gdk::ScrollUnit::Surface);
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

        // Selecting text. Positions are in the column's own coordinates.
        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        let weak = Rc::downgrade(this);
        drag.connect_drag_begin(move |_, x, y| {
            let Some(inner) = weak.upgrade() else { return };
            let Some(spot) = inner.hit(x, y) else { return };
            let unit = inner.count_press(x, y);
            inner.set_selection(Some(Selection { anchor: spot, head: spot, unit }));
        });
        let weak = Rc::downgrade(this);
        drag.connect_drag_update(move |gesture, dx, dy| {
            let Some(inner) = weak.upgrade() else { return };
            let (Some((x, y)), Some(selection)) = (gesture.start_point(), inner.selection.get()) else {
                return;
            };
            if let Some(head) = inner.hit(x + dx, y + dy) {
                if head != selection.head {
                    inner.set_selection(Some(Selection { head, ..selection }));
                }
            }
        });
        let weak = Rc::downgrade(this);
        drag.connect_drag_end(move |_, _, _| {
            let Some(inner) = weak.upgrade() else { return };
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

    fn apply_scale(&self, scale: f64) {
        self.scale.set(scale);
        let layout = Layout::new(&self.turned_pages(), scale);
        for (i, widget) in self.widgets.borrow().iter().enumerate() {
            let (w, h) = layout.size(i);
            widget.set_page_size(w as i32, h as i32);
        }
        *self.layout.borrow_mut() = layout;
    }

    fn go_to_page(&self, index: usize) {
        let top = {
            let layout = self.layout.borrow();
            if index >= layout.len() {
                return;
            }
            (layout.top(index) - layout::SPACING / 2.0).max(0.0)
        };
        let v = self.root.vadjustment();
        v.set_value(top);
        self.jumped.set(Some((index, v.value())));
        self.emit_status();
    }

    /// The page being read: the one crossing the reading line.
    fn current_page(&self) -> usize {
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
        // one page either side.
        let centre = (top + bottom) / 2.0;
        let middle = |i: usize| layout.top(i) + layout.size(i).1 / 2.0;
        let mut order: Vec<usize> = (first..=last).collect();
        order.sort_by(|&a, &b| (middle(a) - centre).abs().total_cmp(&(middle(b) - centre).abs()));
        let ahead = (last + PREFETCH).min(layout.len() - 1);
        order.extend((last + 1)..=ahead);
        order.extend((first.saturating_sub(PREFETCH)..first).rev());

        let (target, rotation) = (self.device_scale(), self.rotation.get());
        let jobs = order
            .into_iter()
            .filter(|&page| rendered[page] != target)
            .map(|page| Job { page, scale: target, rotation })
            .collect();
        renderer.want(jobs);
    }

    fn on_rendered(&self, rendered: Rendered) {
        // Drawn for a zoom or a turn that has since changed.
        if rendered.requested != self.device_scale() || rendered.rotation != self.rotation.get() {
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
    }

    fn emit_status(&self) {
        let pages = self.pages.borrow().len();
        if pages == 0 {
            return;
        }
        let v = self.root.vadjustment();
        let page = if let Some((page, _)) = self.jumped.get() {
            page + 1
        } else if v.value() <= 0.5 {
            1
        } else if v.value() + v.page_size() >= v.upper() - 0.5 {
            pages // Scrolled to the end: the last page, even if it is short.
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
    /// A point between pages belongs to the end of the page above it.
    fn hit(&self, x: f64, y: f64) -> Option<Spot> {
        let page = {
            let layout = self.layout.borrow();
            if layout.len() == 0 {
                return None;
            }
            // The column's own coordinates start below its top margin.
            layout.page_at(y + layout::MARGIN)
        };
        let widgets = self.widgets.borrow();
        let bounds = widgets.get(page)?.compute_bounds(&self.column)?;
        let (w, h) = (f64::from(bounds.width()), f64::from(bounds.height()));
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        let local_x = (x - f64::from(bounds.x())).clamp(0.0, w);
        let local_y = (y - f64::from(bounds.y())).clamp(0.0, h);
        let (page_w, page_h) = self.pages.borrow()[page];
        let rotation = self.rotation.get();
        let (turned_w, turned_h) = rotation.size(page_w, page_h);
        let (px, py) = rotation.undo(local_x / w * turned_w, local_y / h * turned_h, page_w, page_h);
        Some(Spot { page, x: px, y: py })
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
            let uri = self.uri.borrow().clone()?;
            self.reader.replace(poppler::Document::from_file(&uri, None).ok());
        }
        self.reader.borrow().clone()
    }

    fn set_selection(&self, selection: Option<Selection>) {
        self.selection.set(selection);
        let mut now = Vec::new();
        if let (Some(selection), Some(reader)) = (selection, selection.and_then(|_| self.reader())) {
            let pages = self.pages.borrow();
            let widgets = self.widgets.borrow();
            let rotation = self.rotation.get();
            for (page, span) in document::spans(selection.anchor, selection.head, &pages) {
                let (w, h) = pages[page];
                let (turned_w, turned_h) = rotation.size(w, h);
                // Turn both corners of each area, then take the box around
                // them, as fractions of the page as it is shown.
                let areas = document::highlights(&reader, page, span, selection.unit)
                    .into_iter()
                    .map(|[x, y, aw, ah]| {
                        let (x1, y1) = rotation.apply(x, y, w, h);
                        let (x2, y2) = rotation.apply(x + aw, y + ah, w, h);
                        [
                            x1.min(x2) / turned_w,
                            y1.min(y2) / turned_h,
                            (x1 - x2).abs() / turned_w,
                            (y1 - y2).abs() / turned_h,
                        ]
                    })
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
        // Taken out first, so nothing is borrowed while GTK removes them.
        let widgets = std::mem::take(&mut *self.widgets.borrow_mut());
        for widget in widgets {
            self.column.remove(&widget);
        }
        self.pages.borrow_mut().clear();
        self.rendered.borrow_mut().clear();
        *self.layout.borrow_mut() = Layout::default();
        self.rotation.set(Rotation::default());
        self.uri.replace(None);
        self.pending.set((None, None));
        self.jumped.set(None);
        self.last_status.set(None);
        self.sidebar.clear();
    }
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
