// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The controls that float over a picture: the filmstrip along the bottom,
//! an arrow either side, and the zoom in the corner.
//!
//! They come up when the pointer moves and fade once it has rested, so the
//! picture has the window to itself; while the pointer is on one of them they
//! stay. A PDF, a slideshow and the editor's unsaved picture have no use for
//! them, and do without.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};

use crate::app::window::Window;

/// How long they stay once the pointer has stopped moving.
const LINGER: Duration = Duration::from_millis(2200);
/// And once it has left the picture altogether.
const LEFT: Duration = Duration::from_millis(500);
/// Over the picture, but clear of its edges.
const MARGIN: i32 = 12;
/// Keeps the filmstrip clear of the zoom in the corner, either side so it
/// stays in the middle.
const STRIP_MARGIN: i32 = 132;

pub struct ViewerControls {
    /// What floats over the picture or page is put on this.
    pub overlay: gtk::Overlay,
    pub strip: gtk::Revealer,
    pub previous: gtk::Revealer,
    pub next: gtk::Revealer,
    pub zoom: gtk::Revealer,
    pub zoom_level: gtk::MenuButton,
    pub zoom_text: gtk::Label,
    /// Fades them once left alone.
    pub linger: RefCell<Option<glib::SourceId>>,
    /// How many of them the pointer is on: they stay while it is any.
    pub hovered: Cell<u32>,
    /// Where the pointer was last over the picture.
    pub pointer: Cell<Option<(f64, f64)>>,
    /// Says what a drop will do, over the whole window.
    pub drop_hint: gtk::Revealer,
    pub drop_label: gtk::Label,
}

impl Default for ViewerControls {
    fn default() -> Self {
        let revealer = || {
            gtk::Revealer::builder()
                .transition_type(gtk::RevealerTransitionType::Crossfade)
                .transition_duration(200)
                .reveal_child(false)
                .build()
        };
        Self {
            overlay: gtk::Overlay::new(),
            strip: revealer(),
            previous: revealer(),
            next: revealer(),
            zoom: revealer(),
            zoom_level: gtk::MenuButton::new(),
            zoom_text: gtk::Label::new(None),
            linger: RefCell::new(None),
            hovered: Cell::new(0),
            pointer: Cell::new(None),
            drop_hint: revealer(),
            drop_label: gtk::Label::new(None),
        }
    }
}

impl Window {
    /// The picture with its controls over it, to stand in its place.
    pub(crate) fn build_viewer_controls(&self, picture: &impl IsA<gtk::Widget>) -> gtk::Overlay {
        let imp = self.imp();
        let controls = &imp.controls;
        let overlay = controls.overlay.clone();
        overlay.set_child(Some(picture));

        let strip = &controls.strip;
        strip.set_child(Some(&imp.strip));
        strip.set_halign(gtk::Align::Center);
        strip.set_valign(gtk::Align::End);
        strip.set_margin_bottom(MARGIN);
        strip.set_margin_start(STRIP_MARGIN);
        strip.set_margin_end(STRIP_MARGIN);
        // The strip puts itself away for a single picture, or while editing;
        // the arrows go with it.
        imp.strip.connect_visible_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.sync_viewer_controls()
        ));

        let arrow = |revealer: &gtk::Revealer, icon: &str, (name, what): (&str, &str), action: &str, side: gtk::Align| {
            let button = gtk::Button::from_icon_name(icon);
            crate::app::explain(&button, name, what);
            button.set_action_name(Some(action));
            for class in ["osd", "circular", "viewer-arrow"] {
                button.add_css_class(class);
            }
            revealer.set_child(Some(&button));
            revealer.set_halign(side);
            revealer.set_valign(gtk::Align::Center);
            revealer.set_margin_start(MARGIN);
            revealer.set_margin_end(MARGIN);
        };
        arrow(&controls.previous, "glance-previous-symbolic", ("Previous Image", "The one before it in the folder (←)"), "win.previous-image", gtk::Align::Start);
        arrow(&controls.next, "glance-next-symbolic", ("Next Image", "The one after it in the folder (→)"), "win.next-image", gtk::Align::End);

        let zoom = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        for class in ["osd", "viewer-pill", "zoom-pill"] {
            zoom.add_css_class(class);
        }
        let step = |icon: &str, name: &str, what: &str, action: &str| {
            let button = gtk::Button::from_icon_name(icon);
            crate::app::explain(&button, name, what);
            button.set_action_name(Some(action));
            button.add_css_class("flat");
            button.add_css_class("circular");
            button
        };
        let menu = gio::Menu::new();
        menu.append(Some("Fit to Window"), Some("win.zoom-fit"));
        menu.append(Some("Actual Size"), Some("win.zoom-actual"));
        let level = &controls.zoom_level;
        level.set_menu_model(Some(&menu));
        level.set_direction(gtk::ArrowType::Up);
        // A child of its own rather than a label, which would bring an arrow.
        level.set_child(Some(&controls.zoom_text));
        // Its menu is off the picture, so the pointer leaves for it: the
        // controls wait for it to close before they fade.
        level.connect_active_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.fade_viewer_controls(LINGER)
        ));
        crate::app::explain(level, "Zoom", "How large the picture is shown; click for Fit to Window or Actual Size");
        level.add_css_class("flat");
        level.add_css_class("numeric");
        zoom.append(&step("glance-remove-symbolic", "Zoom Out", "Show the picture smaller (−)", "win.zoom-out"));
        zoom.append(level);
        zoom.append(&step("glance-add-symbolic", "Zoom In", "Show the picture larger (+)", "win.zoom-in"));
        let corner = &controls.zoom;
        corner.set_child(Some(&zoom));
        corner.set_halign(gtk::Align::End);
        corner.set_valign(gtk::Align::End);
        corner.set_margin_end(MARGIN);
        corner.set_margin_bottom(MARGIN);

        for revealer in [strip, &controls.previous, &controls.next, corner] {
            overlay.add_overlay(revealer);
            if let Some(child) = revealer.child() {
                child.add_controller(self.hover_keeps_controls());
            }
        }

        // Any movement over the picture brings them up.
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, x, y| {
                // GTK says the pointer moved whenever what is under it
                // changes, the controls fading included; only a real move
                // brings them back.
                let pointer = &window.imp().controls.pointer;
                if pointer.replace(Some((x, y))) != Some((x, y)) {
                    window.wake_viewer_controls();
                }
            }
        ));
        motion.connect_leave(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| {
                window.imp().controls.pointer.set(None);
                window.fade_viewer_controls(LEFT);
            }
        ));
        overlay.add_controller(motion);
        self.sync_viewer_controls();
        overlay
    }

    /// While the pointer is on a control, they all stay.
    fn hover_keeps_controls(&self) -> gtk::EventControllerMotion {
        let hover = gtk::EventControllerMotion::new();
        hover.connect_enter(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _, _| {
                let hovered = &window.imp().controls.hovered;
                hovered.set(hovered.get() + 1);
                window.wake_viewer_controls();
            }
        ));
        hover.connect_leave(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| {
                // Only the count down from here: a control being put away
                // hears the pointer leave too, and waking them all again
                // then would keep them up for ever.
                let hovered = &window.imp().controls.hovered;
                hovered.set(hovered.get().saturating_sub(1));
                window.fade_viewer_controls(LINGER);
            }
        ));
        hover
    }

    /// Whether there is a picture for them to be over.
    fn viewer_controls_wanted(&self) -> bool {
        let imp = self.imp();
        !imp.showing_pdf.get() && !imp.slideshow.get() && imp.view.canvas().has_image()
    }

    /// Which of them belong on screen at all: the arrows only with a folder
    /// to step through, which is when the filmstrip is there too.
    pub(crate) fn sync_viewer_controls(&self) {
        let imp = self.imp();
        let controls = &imp.controls;
        let wanted = self.viewer_controls_wanted();
        // Its own wish: `is_visible` would also ask the revealer around it,
        // which is hidden until the strip is there to show.
        let stepping = wanted && imp.strip.get_visible();
        controls.strip.set_visible(stepping);
        controls.previous.set_visible(stepping);
        controls.next.set_visible(stepping);
        controls.zoom.set_visible(wanted);
        if !wanted {
            // Put away under the pointer, they may never hear it leave.
            controls.hovered.set(0);
            self.reveal_viewer_controls(false);
        }
    }

    /// Bring them up, and fade them once left alone.
    pub(crate) fn wake_viewer_controls(&self) {
        self.sync_viewer_controls();
        if !self.viewer_controls_wanted() {
            return;
        }
        self.reveal_viewer_controls(true);
        self.fade_viewer_controls(LINGER);
    }

    /// Whether the pointer is on one of them, or the zoom's menu is open.
    fn viewer_controls_in_use(&self) -> bool {
        let controls = &self.imp().controls;
        controls.hovered.get() > 0 || controls.zoom_level.is_active()
    }

    /// Fade them after `after`, unless they are in use by then.
    fn fade_viewer_controls(&self, after: Duration) {
        let imp = self.imp();
        if let Some(timer) = imp.controls.linger.take() {
            timer.remove();
        }
        if self.viewer_controls_in_use() {
            return;
        }
        let timer = glib::timeout_add_local_once(
            after,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move || {
                    let imp = window.imp();
                    imp.controls.linger.replace(None);
                    if !window.viewer_controls_in_use() {
                        window.reveal_viewer_controls(false);
                    }
                }
            ),
        );
        imp.controls.linger.replace(Some(timer));
    }

    fn reveal_viewer_controls(&self, shown: bool) {
        let imp = self.imp();
        let controls = &imp.controls;
        for revealer in [&controls.strip, &controls.previous, &controls.next, &controls.zoom] {
            revealer.set_reveal_child(shown);
        }
        // Full screen, a resting pointer would sit over the picture: it goes
        // with the controls. A slideshow sees to its own.
        if !imp.slideshow.get() {
            imp.view.canvas().set_cursor_hidden(!shown && self.is_fullscreen());
        }
    }

    /// The zoom in the corner, as it changes.
    pub(crate) fn show_zoom_level(&self, percent: f64) {
        self.imp().controls.zoom_text.set_text(&format!("{percent:.0}%"));
    }

    /// The hint over the window while files are dragged over it; `None`
    /// takes it away.
    pub(crate) fn build_drop_hint(&self) -> &gtk::Revealer {
        let controls = &self.imp().controls;
        let label = &controls.drop_label;
        label.add_css_class("title-2");
        label.set_wrap(true);
        label.set_justify(gtk::Justification::Center);
        let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        frame.add_css_class("drop-hint");
        frame.set_margin_top(MARGIN);
        frame.set_margin_bottom(MARGIN);
        frame.set_margin_start(MARGIN);
        frame.set_margin_end(MARGIN);
        label.set_vexpand(true);
        label.set_valign(gtk::Align::Center);
        frame.append(label);
        let hint = &controls.drop_hint;
        hint.set_child(Some(&frame));
        // Never in the way of the drop itself.
        hint.set_can_target(false);
        hint.set_transition_duration(150);
        hint
    }

    pub(crate) fn show_drop_hint(&self, hint: Option<&str>) {
        let controls = &self.imp().controls;
        if let Some(text) = hint {
            controls.drop_label.set_text(text);
        }
        controls.drop_hint.set_reveal_child(hint.is_some());
    }
}
