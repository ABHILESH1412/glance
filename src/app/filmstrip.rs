// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The strip of neighbouring images, floating over the bottom of the picture.
//!
//! It asks for as much width as its thumbnails need and no more, so it sits as
//! a small pill in the middle; how many thumbnails fit is then decided from the
//! width actually handed to it, so a narrow window gets fewer. The current image sits in the middle, and the strip never
//! shows the same file twice: a folder of three images gets three slots, not a
//! repeating carousel.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, glib};

/// Big enough to recognise a photo, small enough to keep out of its way.
const THUMB_W: i32 = 56;
const THUMB_H: i32 = 42;
const SPACING: i32 = 4;
/// Past this the thumbnails stop helping and start costing decodes.
const MAX_SLOTS: usize = 11;

/// Thumbnails are generated filling the slot, so every slot claims identical
/// width no matter the shape of the picture; at twice its size, so they stay
/// sharp on a high-resolution screen.
pub const SLOT_W: u32 = THUMB_W as u32 * 2;
pub const SLOT_H: u32 = THUMB_H as u32 * 2;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct FilmStrip {
        /// The visible bar. This widget exists only to wrap it, because a
        /// `GtkBox` subclass never receives `size_allocate` -- GTK calls its
        /// layout manager instead -- and the slot count has to come from the
        /// width actually granted.
        pub root: gtk::Box,
        /// Holds the thumbnails, and is what gets measured.
        ///
        /// A scroller rather than a plain box: a box's minimum width is the sum
        /// of its slots, which would make the strip dictate the window's
        /// minimum width and stop it ever being narrow enough to need fewer
        /// thumbnails. A scroller asks for almost nothing and simply clips
        /// during the frame before the slot count catches up.
        pub inner: gtk::ScrolledWindow,
        /// Holds the slots, centred inside `inner`.
        pub slots_box: gtk::Box,
        pub counter: gtk::Label,
        pub buttons: RefCell<Vec<gtk::Button>>,
        pub pictures: RefCell<Vec<gtk::Picture>>,
        pub files: RefCell<Vec<PathBuf>>,
        /// Whether the window will have the strip on screen at all.
        ///
        /// The strip rebuilds itself whenever the folder changes, and used to
        /// decide its own visibility from the file count — which meant that
        /// saving into the folder being viewed brought it back mid-edit,
        /// because the rescan happened after the window had hidden it. The
        /// window's intent is recorded here so a rebuild cannot override it.
        pub allowed: Cell<bool>,
        pub index: Cell<usize>,
        /// Slots currently built, which is what `size_allocate` compares against.
        pub slots: Cell<usize>,
        pub cache: RefCell<HashMap<PathBuf, gdk::Texture>>,
        /// Which file each slot is showing, so a late thumbnail can be matched
        /// to a slot that may since have moved on.
        pub showing: RefCell<Vec<PathBuf>>,
        pub on_select: RefCell<Option<Box<dyn Fn(usize)>>>,
        pub on_need: RefCell<Option<Box<dyn Fn(PathBuf)>>>,
        pub pending_relayout: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FilmStrip {
        const NAME: &'static str = "SvFilmStrip";
        type Type = super::FilmStrip;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for FilmStrip {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().build();
            self.root.set_parent(&*self.obj());
        }

        fn dispose(&self) {
            self.root.unparent();
        }
    }

    impl WidgetImpl for FilmStrip {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let (minimum, natural, baseline_min, baseline_nat) = self.root.measure(orientation, for_size);
            if orientation != gtk::Orientation::Horizontal {
                return (minimum, natural, baseline_min, baseline_nat);
            }
            // Room for every thumbnail the folder could fill, beside the
            // counter: what is wanted, not what is there yet.
            let wanted = self.files.borrow().len().min(MAX_SLOTS) as i32;
            (minimum, natural.max(minimum + wanted * (THUMB_W + SPACING)), -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.root.allocate(width, height, baseline, None);
            let wanted = self.obj().slots_that_fit();
            if wanted != self.slots.get() && !self.pending_relayout.get() {
                // Rebuilding children from inside an allocation is a layout
                // loop, so do it once the frame is done.
                self.pending_relayout.set(true);
                glib::idle_add_local_once(glib::clone!(
                    #[weak(rename_to = strip)]
                    self.obj(),
                    move || {
                        strip.imp().pending_relayout.set(false);
                        strip.rebuild();
                    }
                ));
            }
        }
    }

}

glib::wrapper! {
    pub struct FilmStrip(ObjectSubclass<imp::FilmStrip>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for FilmStrip {
    fn default() -> Self {
        let strip: Self = glib::Object::new();
        // No objection until the window raises one, so the strip behaves as it
        // always did unless something deliberately puts it away.
        strip.imp().allowed.set(true);
        strip
    }
}

impl FilmStrip {
    pub fn new() -> Self {
        Self::default()
    }

    fn build(&self) {
        let imp = self.imp();
        // The thumbnails, then where this one is among them, in one pill.
        let root = &imp.root;
        root.set_orientation(gtk::Orientation::Horizontal);
        root.set_spacing(6);
        root.add_css_class("osd");
        root.add_css_class("viewer-pill");
        root.add_css_class("filmstrip");
        self.set_visible(false);

        imp.slots_box.set_orientation(gtk::Orientation::Horizontal);
        imp.slots_box.set_spacing(SPACING);
        // Centring belongs on the inner child: `inner` itself must fill, or
        // measuring it would report the width of the slots already there and
        // the strip could never grow past one.
        imp.slots_box.set_halign(gtk::Align::Center);

        imp.inner.set_hexpand(true);
        imp.inner.set_valign(gtk::Align::Center);
        imp.slots_box.set_valign(gtk::Align::Center);
        imp.inner.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
        imp.inner.set_kinetic_scrolling(false);
        imp.inner.set_propagate_natural_width(false);
        imp.inner.set_child(Some(&imp.slots_box));

        imp.counter.add_css_class("numeric");
        imp.counter.add_css_class("dim-label");
        // Fixed width, so the pill does not jitter as the digits change.
        imp.counter.set_width_chars(7);

        root.append(&imp.inner);
        root.append(&imp.counter);
    }

    /// Point the strip at a folder listing and a position within it.
    pub fn set_playlist(&self, files: &[PathBuf], index: usize) {
        let imp = self.imp();
        imp.files.replace(files.to_vec());
        imp.index.set(index);
        self.rebuild();
    }

    /// Move the highlight without re-cloning the folder listing, which is what
    /// stepping through the same folder does.
    pub fn set_index(&self, index: usize) {
        self.imp().index.set(index);
        self.rebuild();
    }

    pub fn clear(&self) {
        let imp = self.imp();
        imp.files.replace(Vec::new());
        imp.index.set(0);
        self.rebuild();
    }

    /// Called with the absolute playlist index of a clicked thumbnail.
    pub fn connect_selected(&self, f: impl Fn(usize) + 'static) {
        self.imp().on_select.replace(Some(Box::new(f)));
    }

    /// Called when a thumbnail is needed and not cached.
    pub fn connect_needs_thumbnail(&self, f: impl Fn(PathBuf) + 'static) {
        self.imp().on_need.replace(Some(Box::new(f)));
    }

    pub fn set_thumbnail(&self, path: &Path, texture: &gdk::Texture) {
        let imp = self.imp();
        imp.cache
            .borrow_mut()
            .insert(path.to_path_buf(), texture.clone());
        // The strip may have moved on while this was being generated, so only
        // fill the slots still showing this file.
        for (slot, showing) in imp.showing.borrow().iter().enumerate() {
            if showing == path {
                if let Some(picture) = imp.pictures.borrow().get(slot) {
                    picture.set_paintable(Some(texture));
                }
            }
        }
    }

    fn slots_that_fit(&self) -> usize {
        let available = self.imp().inner.width();
        if available <= 0 {
            return 1;
        }
        let per_slot = THUMB_W + SPACING;
        let fits = ((available + SPACING) / per_slot).max(1) as usize;
        fits.min(MAX_SLOTS)
    }

    /// Show the strip, or keep it away regardless of what the folder holds.
    pub fn set_allowed(&self, allowed: bool) {
        if self.imp().allowed.get() == allowed {
            return;
        }
        self.imp().allowed.set(allowed);
        self.rebuild();
    }

    fn rebuild(&self) {
        let imp = self.imp();
        let files = imp.files.borrow().clone();
        // How wide it would like to be follows the folder.
        self.queue_resize();

        // One image, or none, or the window wants it out of the way.
        if files.len() < 2 || !imp.allowed.get() {
            self.set_visible(false);
            imp.showing.replace(Vec::new());
            return;
        }
        self.set_visible(true);

        // Never show the same file twice, so a short folder gets a short strip.
        let slots = self.slots_that_fit().min(files.len());
        self.ensure_slots(slots);
        imp.slots.set(slots);

        let centre = slots / 2;
        let index = imp.index.get();
        let mut showing = Vec::with_capacity(slots);

        for slot in 0..slots {
            let offset = slot as isize - centre as isize;
            let target = (index as isize + offset).rem_euclid(files.len() as isize) as usize;
            let path = files[target].clone();

            let buttons = imp.buttons.borrow();
            let pictures = imp.pictures.borrow();
            let (button, picture) = (&buttons[slot], &pictures[slot]);

            button.set_tooltip_text(path.file_name().and_then(|n| n.to_str()));
            // Remembered so the click handler does not need to recompute it.
            unsafe { button.set_data("sv-index", target) };

            if target == index {
                button.add_css_class("filmstrip-current");
            } else {
                button.remove_css_class("filmstrip-current");
            }

            match imp.cache.borrow().get(&path) {
                Some(texture) => picture.set_paintable(Some(texture)),
                None => {
                    picture.set_paintable(gdk::Paintable::NONE);
                    if let Some(request) = imp.on_need.borrow().as_ref() {
                        request(path.clone());
                    }
                }
            }
            showing.push(path);
        }

        imp.showing.replace(showing);
        imp.counter
            .set_text(&format!("{} / {}", index + 1, files.len()));
    }

    /// Grow the pool of slot widgets to `count`, hiding any spares.
    fn ensure_slots(&self, count: usize) {
        let imp = self.imp();
        while imp.buttons.borrow().len() < count {
            let picture = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Cover)
                .can_shrink(true)
                .overflow(gtk::Overflow::Hidden)
                .build();
            let button = gtk::Button::builder()
                .child(&picture)
                .width_request(THUMB_W)
                .height_request(THUMB_H)
                .build();
            button.add_css_class("flat");
            button.add_css_class("filmstrip-slot");

            button.connect_clicked(glib::clone!(
                #[weak(rename_to = strip)]
                self,
                move |button| {
                    let target = unsafe { button.data::<usize>("sv-index") };
                    if let Some(target) = target {
                        let target = unsafe { *target.as_ref() };
                        if let Some(select) = strip.imp().on_select.borrow().as_ref() {
                            select(target);
                        }
                    }
                }
            ));

            imp.slots_box.append(&button);
            imp.buttons.borrow_mut().push(button);
            imp.pictures.borrow_mut().push(picture);
        }

        for (slot, button) in imp.buttons.borrow().iter().enumerate() {
            button.set_visible(slot < count);
        }
    }
}
