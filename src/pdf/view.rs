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

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, glib};

use super::document::{Opened, Pixels};
use super::layout::{self, Layout};
use super::render::{Job, Rendered, Renderer};

/// Pages drawn ahead of the ones on screen, in each direction.
const PREFETCH: usize = 1;
/// Pages kept drawn either side of the screen before their pixels are freed.
/// A little more than `PREFETCH`, so scrolling back a short way is instant.
const KEEP: usize = 3;
/// How long a zoom has to stay still before pages are redrawn at the new size.
const SETTLE: Duration = Duration::from_millis(120);

/// What the header shows: where you are, and at what size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Status {
    pub page: usize,
    pub pages: usize,
    pub percent: f64,
}

pub struct PdfView {
    inner: Rc<Inner>,
}

struct Inner {
    root: gtk::ScrolledWindow,
    column: gtk::Box,
    /// Page sizes in points.
    pages: RefCell<Vec<(f64, f64)>>,
    widgets: RefCell<Vec<page::Page>>,
    /// The device scale each page was last drawn at, or 0.0 for not drawn.
    rendered: RefCell<Vec<f64>>,
    layout: RefCell<Layout>,
    /// Logical pixels per point.
    scale: Cell<f64>,
    /// Following the window's width, until the user zooms by hand.
    fit: Cell<bool>,
    /// A refit to a new window width is waiting for GTK to finish laying out.
    refit_queued: Cell<bool>,
    renderer: RefCell<Option<Renderer>>,
    /// Bumped for every document, so pages drawn for the last one are ignored.
    document: Cell<u64>,
    settle: RefCell<Option<glib::SourceId>>,
    /// Where a zoom wants the view to end up, per axis, until GTK has laid out
    /// the new sizes and the position can actually be reached.
    pending: Cell<(Option<f64>, Option<f64>)>,
    pointer: Cell<Option<(f64, f64)>>,
    pinch_from: Cell<f64>,
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
            pages: RefCell::default(),
            widgets: RefCell::default(),
            rendered: RefCell::default(),
            layout: RefCell::default(),
            scale: Cell::new(layout::ACTUAL),
            fit: Cell::new(true),
            refit_queued: Cell::new(false),
            renderer: RefCell::default(),
            document: Cell::new(0),
            settle: RefCell::default(),
            pending: Cell::new((None, None)),
            pointer: Cell::new(None),
            pinch_from: Cell::new(layout::ACTUAL),
            status: RefCell::default(),
            last_status: Cell::new(None),
        });
        Inner::connect(&inner);
        PdfView { inner }
    }

    pub fn widget(&self) -> &gtk::ScrolledWindow {
        &self.inner.root
    }

    /// Lay out a freshly opened document, fitted to the window's width, and
    /// start drawing the first pages.
    pub fn show(&self, opened: Opened) {
        let inner = &self.inner;
        inner.clear();
        let id = inner.document.get();

        let count = opened.pages.len();
        let widgets: Vec<page::Page> = (0..count)
            .map(|_| {
                let widget = page::Page::new();
                inner.column.append(&widget);
                widget
            })
            .collect();
        *inner.pages.borrow_mut() = opened.pages;
        *inner.widgets.borrow_mut() = widgets;
        *inner.rendered.borrow_mut() = vec![0.0; count];

        inner.fit.set(true);
        let width = inner.root.hadjustment().page_size();
        let scale = if width > 0.0 {
            layout::fit_width(&inner.pages.borrow(), width)
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
        inner.set_scale(layout::fit_width(&inner.pages.borrow(), width), inner.centre());
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

    pub fn scroll_to_start(&self) {
        self.inner.root.vadjustment().set_value(0.0);
    }

    pub fn scroll_to_end(&self) {
        let v = self.inner.root.vadjustment();
        v.set_value(v.upper());
    }
}

impl Inner {
    fn connect(this: &Rc<Self>) {
        let v = this.root.vadjustment();
        let h = this.root.hadjustment();

        let weak = Rc::downgrade(this);
        v.connect_value_changed(move |_| {
            if let Some(inner) = weak.upgrade() {
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
                    let scale = layout::fit_width(&inner.pages.borrow(), width);
                    // Keep whatever is at the top of the window at the top.
                    inner.set_scale(scale, (0.0, 0.0));
                }
            });
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
        let layout = Layout::new(&self.pages.borrow(), scale);
        for (i, widget) in self.widgets.borrow().iter().enumerate() {
            let (w, h) = layout.size(i);
            widget.set_page_size(w as i32, h as i32);
        }
        *self.layout.borrow_mut() = layout;
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

        let target = self.device_scale();
        let jobs = order
            .into_iter()
            .filter(|&page| rendered[page] != target)
            .map(|page| Job { page, scale: target })
            .collect();
        renderer.want(jobs);
    }

    fn on_rendered(&self, rendered: Rendered) {
        // Drawn for a zoom that has since changed.
        if rendered.requested != self.device_scale() {
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
        let page = if v.value() <= 0.5 {
            1
        } else if v.value() + v.page_size() >= v.upper() - 0.5 {
            pages // Scrolled to the end: the last page, even if it is short.
        } else {
            self.layout.borrow().page_at(v.value() + v.page_size() / 2.0) + 1
        };
        let status = Status { page, pages, percent: self.scale.get() / layout::ACTUAL * 100.0 };
        if self.last_status.replace(Some(status)) != Some(status) {
            if let Some(callback) = self.status.borrow().as_ref() {
                callback(status);
            }
        }
    }

    fn clear(&self) {
        self.renderer.replace(None);
        if let Some(source) = self.settle.take() {
            source.remove();
        }
        self.document.set(self.document.get() + 1);
        // Taken out first, so nothing is borrowed while GTK removes them.
        let widgets = std::mem::take(&mut *self.widgets.borrow_mut());
        for widget in widgets {
            self.column.remove(&widget);
        }
        self.pages.borrow_mut().clear();
        self.rendered.borrow_mut().clear();
        *self.layout.borrow_mut() = Layout::default();
        self.pending.set((None, None));
        self.last_status.set(None);
    }
}

fn texture(pixels: Pixels) -> gdk::Texture {
    // Cairo's ARGB32 is one native-endian word per pixel.
    #[cfg(target_endian = "little")]
    let format = gdk::MemoryFormat::B8g8r8a8Premultiplied;
    #[cfg(target_endian = "big")]
    let format = gdk::MemoryFormat::A8r8g8b8Premultiplied;
    let bytes = glib::Bytes::from_owned(pixels.data);
    gdk::MemoryTexture::new(pixels.width, pixels.height, format, &bytes, pixels.stride).upcast()
}

/// One page: exactly the size it is told to be, whatever it is showing, with
/// the drawn page stretched to fill it.
///
/// A `gtk::Picture` would size itself from its texture, and a texture drawn
/// for a HiDPI screen is twice the size it should appear.
mod page {
    use std::cell::{Cell, RefCell};

    use gtk::prelude::*;
    use gtk::subclass::prelude::*;
    use gtk::{gdk, glib, graphene, gsk};

    mod imp {
        use super::*;

        #[derive(Default)]
        pub struct Page {
            pub size: Cell<(i32, i32)>,
            pub texture: RefCell<Option<gdk::Texture>>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for Page {
            const NAME: &'static str = "GlancePdfPage";
            type Type = super::Page;
            type ParentType = gtk::Widget;
        }

        impl ObjectImpl for Page {}

        impl WidgetImpl for Page {
            fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
                let (w, h) = self.size.get();
                let size = if orientation == gtk::Orientation::Horizontal { w } else { h };
                (size, size, -1, -1)
            }

            fn snapshot(&self, snapshot: &gtk::Snapshot) {
                let widget = self.obj();
                let bounds = graphene::Rect::new(0.0, 0.0, widget.width() as f32, widget.height() as f32);
                // A soft edge, so a white page still reads as paper on a light window.
                snapshot.append_outset_shadow(
                    &gsk::RoundedRect::from_rect(bounds, 0.0),
                    &gdk::RGBA::new(0.0, 0.0, 0.0, 0.3),
                    0.0,
                    1.0,
                    0.0,
                    4.0,
                );
                snapshot.append_color(&gdk::RGBA::WHITE, &bounds);
                if let Some(texture) = self.texture.borrow().as_ref() {
                    snapshot.append_scaled_texture(texture, gsk::ScalingFilter::Linear, &bounds);
                }
            }
        }
    }

    glib::wrapper! {
        pub struct Page(ObjectSubclass<imp::Page>)
            @extends gtk::Widget,
            @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
    }

    impl Page {
        pub fn new() -> Self {
            // Centred, not stretched: a vertical box widens every child to its
            // widest one, which would squash a portrait page to the width of a
            // landscape page beside it.
            glib::Object::builder().property("halign", gtk::Align::Center).build()
        }

        pub fn set_page_size(&self, width: i32, height: i32) {
            if self.imp().size.replace((width, height)) != (width, height) {
                self.queue_resize();
            }
        }

        pub fn set_texture(&self, texture: Option<gdk::Texture>) {
            self.imp().texture.replace(texture);
            self.queue_draw();
        }
    }
}
