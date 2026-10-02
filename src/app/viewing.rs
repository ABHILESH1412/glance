// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Looking at pictures, rather than changing them: the inspector, an
//! animation's frames in the sidebar, and the slideshow.
//!
//! The slideshow is the folder, full screen, one picture after another. Each
//! picture's time starts once it is on screen, so a slow one is not cut
//! short. Moving the pointer brings up its controls, which fade out again
//! along with the pointer; Space pauses; Escape, or leaving full screen, ends
//! it and puts the window back as it was.

use std::time::Duration;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};

use crate::app::inspector::Picture;
use crate::app::window::Window;

/// How long the slideshow's controls stay after the pointer stops.
const CONTROLS_LINGER: Duration = Duration::from_millis(2500);

/// The choices of how long each picture stays up, in seconds.
pub const INTERVALS: [u32; 6] = [2, 3, 5, 10, 20, 30];

impl Window {
    pub(crate) fn install_viewing_actions(&self) {
        let imp = self.imp();

        let inspector = gio::SimpleAction::new_stateful("inspector", None, &false.to_variant());
        inspector.set_enabled(false);
        inspector.connect_change_state(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |action, state| {
                let Some(show) = state.and_then(|state| state.get::<bool>()) else { return };
                action.set_state(&show.to_variant());
                window.imp().inspector.root.set_visible(show);
                if show {
                    window.refresh_inspector();
                }
            }
        ));
        self.add_action(&inspector);

        let slideshow = gio::SimpleAction::new("slideshow", None);
        slideshow.set_enabled(false);
        slideshow.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                if window.imp().slideshow.get() {
                    window.stop_slideshow();
                } else {
                    window.start_slideshow();
                }
            }
        ));
        self.add_action(&slideshow);

        let pause = gio::SimpleAction::new("slideshow-pause", None);
        pause.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.toggle_slideshow_pause()
        ));
        self.add_action(&pause);

        for (name, step) in [("frame-previous", -1isize), ("frame-next", 1)] {
            let action = gio::SimpleAction::new(name, None);
            action.set_enabled(false);
            action.connect_activate(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, _| window.imp().view.canvas().step_frame(step)
            ));
            self.add_action(&action);
        }
        let play = gio::SimpleAction::new("frame-play", None);
        play.set_enabled(false);
        play.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                let canvas = window.imp().view.canvas();
                canvas.set_playing(!canvas.is_playing());
            }
        ));
        self.add_action(&play);

        // The frames list and the picture keep each other in step.
        let frames = imp.frames.clone();
        imp.view.canvas().connect_frame_changed(move |index, playing| frames.set_current(index, playing));
        imp.frames.connect_chosen(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |index| window.imp().view.canvas().show_frame(index)
        ));

        self.build_slideshow_controls();
        self.build_picture_menu();
    }

    /// Right-click on the picture: copy it, and the things to do with it
    /// that are otherwise in the main menu.
    fn build_picture_menu(&self) {
        let canvas = self.imp().view.canvas().clone();
        // One for a still picture and one for an animation, each made with
        // its menu: a menu swapped into a popover after it is made comes up
        // a row too short.
        let popovers = [false, true].map(|animated| {
            let popover = gtk::PopoverMenu::from_model(Some(&Self::picture_menu(animated)));
            popover.set_parent(&canvas);
            popover.set_has_arrow(false);
            popover.set_halign(gtk::Align::Start);
            canvas.connect_destroy(glib::clone!(
                #[weak]
                popover,
                move |_| popover.unparent()
            ));
            popover
        });
        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_SECONDARY);
        click.connect_pressed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |gesture, _, x, y| {
                let imp = window.imp();
                let canvas = imp.view.canvas();
                if !canvas.has_image() || imp.slideshow.get() {
                    return;
                }
                gesture.set_state(gtk::EventSequenceState::Claimed);
                let popover = &popovers[usize::from(canvas.frame_count() > 1)];
                popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                popover.popup();
            }
        ));
        canvas.add_controller(click);
    }

    /// The menu for a picture: "Copy This Frame" for an animation, which is
    /// what Copy then copies.
    fn picture_menu(animated: bool) -> gio::Menu {
        let copy = gio::Menu::new();
        copy.append(Some(if animated { "_Copy This Frame" } else { "_Copy Image" }), Some("win.copy"));
        let look = gio::Menu::new();
        look.append(Some("Image _Info"), Some("win.inspector"));
        if animated {
            look.append(Some("Show F_rames"), Some("win.show-pages"));
        }
        look.append(Some("_Slideshow"), Some("win.slideshow"));
        let turn = gio::Menu::new();
        turn.append(Some("Rotate _Left"), Some("win.rotate-left"));
        turn.append(Some("Rotate _Right"), Some("win.rotate-right"));
        let change = gio::Menu::new();
        change.append(Some("_Edit…"), Some("win.edit"));
        change.append(Some("_Print…"), Some("win.print"));
        let menu = gio::Menu::new();
        for section in [&copy, &look, &turn, &change] {
            menu.append_section(None, section);
        }
        menu
    }

    /// A picture has just been put on screen.
    pub(crate) fn picture_shown(&self, picture: Picture) {
        let imp = self.imp();
        imp.picture.replace(Some(picture));
        if let Some(action) = self.lookup_action("inspector").and_downcast::<gio::SimpleAction>() {
            action.set_enabled(true);
        }
        self.refresh_inspector();

        // An animation gets its frames in the sidebar; a still picture has
        // no sidebar.
        let canvas = imp.view.canvas();
        let frames = canvas.frames();
        let animated = frames.len() > 1;
        imp.frames.set_frames(if animated { &frames } else { &[] });
        imp.frames.set_current(canvas.frame(), canvas.is_playing());
        for name in ["frame-previous", "frame-next", "frame-play"] {
            if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(animated);
            }
        }
        imp.sidebar_button.set_visible(animated);
        imp.sidebar_button.set_tooltip_text(Some("Frames (F9)"));
        if animated {
            imp.split.set_sidebar(Some(&imp.frames.root));
        }
        if let Some(action) = self.lookup_action("show-pages").and_downcast::<gio::SimpleAction>() {
            action.set_enabled(animated);
            let open = animated && imp.frames_wanted.get() && !imp.slideshow.get();
            action.change_state(&open.to_variant());
        }

        if imp.slideshow.get() {
            self.update_slideshow_position();
            self.schedule_slide();
        }
    }

    /// Nothing is on screen, or a PDF is: the picture tools have nothing to
    /// work on.
    pub(crate) fn picture_gone(&self) {
        let imp = self.imp();
        imp.picture.replace(None);
        imp.inspector.clear();
        imp.frames.set_frames(&[]);
        for name in ["inspector", "frame-previous", "frame-next", "frame-play"] {
            if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                if name == "inspector" {
                    action.change_state(&false.to_variant());
                }
                action.set_enabled(false);
            }
        }
        if imp.slideshow.get() {
            self.stop_slideshow();
        }
    }

    /// Fill the inspector for the picture on screen, if it is open.
    pub(crate) fn refresh_inspector(&self) {
        let imp = self.imp();
        if !imp.inspector.root.is_visible() {
            return;
        }
        let path = imp.current.borrow().clone();
        let picture = imp.picture.borrow().clone();
        match (path, picture) {
            (Some(path), Some(picture)) if !imp.showing_pdf.get() => imp.inspector.show(&path, picture),
            _ => imp.inspector.clear(),
        }
    }

    // -- slideshow ----------------------------------------------------------

    fn build_slideshow_controls(&self) {
        let imp = self.imp();
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.add_css_class("osd");
        bar.add_css_class("toolbar");
        bar.add_css_class("slideshow-controls");

        let button = |icon: &str, tip: &str| {
            let button = gtk::Button::from_icon_name(icon);
            button.set_tooltip_text(Some(tip));
            button.add_css_class("circular");
            button
        };
        let previous = button("go-previous-symbolic", "Previous (←)");
        previous.set_action_name(Some("win.previous-image"));
        let play = &imp.slideshow_play;
        play.set_icon_name("media-playback-pause-symbolic");
        play.set_tooltip_text(Some("Pause (Space)"));
        play.add_css_class("circular");
        play.set_action_name(Some("win.slideshow-pause"));
        let next = button("go-next-symbolic", "Next (→)");
        next.set_action_name(Some("win.next-image"));
        let position = &imp.slideshow_position;
        position.add_css_class("numeric");
        position.set_margin_start(6);
        position.set_margin_end(6);

        let every = &imp.slideshow_every;
        let names: Vec<String> = INTERVALS.iter().map(|s| format!("Every {s} s")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        every.set_model(Some(&gtk::StringList::new(&names)));
        let chosen = imp.reader_prefs.get().slideshow;
        let index = INTERVALS.iter().position(|&s| s == chosen).unwrap_or(2);
        every.set_selected(index as u32);
        every.set_tooltip_text(Some("How long each picture stays up"));
        every.connect_selected_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |drop| {
                let seconds = INTERVALS.get(drop.selected() as usize).copied().unwrap_or(5);
                window.update_prefs(|prefs| prefs.slideshow = seconds);
                if window.imp().slideshow.get() {
                    window.schedule_slide();
                }
            }
        ));

        let close = button("view-restore-symbolic", "End Slideshow (Esc)");
        close.set_action_name(Some("win.slideshow"));

        bar.append(&previous);
        bar.append(play);
        bar.append(&next);
        bar.append(position);
        bar.append(every);
        bar.append(&close);

        // While the pointer is on the controls they stay.
        let hover = gtk::EventControllerMotion::new();
        hover.connect_enter(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _, _| {
                window.imp().slideshow_hovered.set(true);
                window.wake_slideshow_controls();
            }
        ));
        hover.connect_leave(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| {
                window.imp().slideshow_hovered.set(false);
                window.wake_slideshow_controls();
            }
        ));
        bar.add_controller(hover);

        let revealer = &imp.slideshow_controls;
        revealer.set_transition_type(gtk::RevealerTransitionType::Crossfade);
        revealer.set_child(Some(&bar));
        revealer.set_halign(gtk::Align::Center);
        revealer.set_valign(gtk::Align::End);
        revealer.set_margin_bottom(24);
        revealer.set_reveal_child(false);
        revealer.set_visible(false);
    }

    fn slideshow_seconds(&self) -> u32 {
        INTERVALS.get(self.imp().slideshow_every.selected() as usize).copied().unwrap_or(5)
    }

    fn start_slideshow(&self) {
        let imp = self.imp();
        if imp.playlist.borrow().is_none() || imp.showing_pdf.get() || imp.edit_button.is_active() {
            return;
        }
        // To put back afterwards.
        imp.slideshow_restore.set((
            imp.inspector.root.is_visible(),
            imp.split.shows_sidebar(),
            self.is_fullscreen(),
        ));
        imp.slideshow.set(true);
        imp.slideshow_paused.set(false);
        for name in ["inspector", "show-pages"] {
            if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.change_state(&false.to_variant());
            }
        }
        self.fullscreen();
        self.sync_fullscreen();
        self.refresh_accels();
        // Whatever the picture was zoomed to, the slideshow shows it whole.
        imp.view.canvas().zoom_fit();
        imp.slideshow_controls.set_visible(true);
        self.sync_slideshow_play();
        self.update_slideshow_position();
        self.wake_slideshow_controls();
        self.schedule_slide();
    }

    pub(crate) fn stop_slideshow(&self) {
        let imp = self.imp();
        if !imp.slideshow.replace(false) {
            return;
        }
        for timer in [&imp.slideshow_timer, &imp.slideshow_linger] {
            if let Some(timer) = timer.take() {
                timer.remove();
            }
        }
        imp.slideshow_controls.set_reveal_child(false);
        imp.slideshow_controls.set_visible(false);
        imp.view.canvas().set_cursor_hidden(false);
        self.refresh_accels();
        let (inspector, sidebar, fullscreen) = imp.slideshow_restore.get();
        if !fullscreen && self.is_fullscreen() {
            self.unfullscreen();
        }
        self.sync_fullscreen();
        if inspector {
            if let Some(action) = self.lookup_action("inspector").and_downcast::<gio::SimpleAction>() {
                action.change_state(&true.to_variant());
            }
        }
        if sidebar {
            if let Some(action) = self.lookup_action("show-pages").and_downcast::<gio::SimpleAction>() {
                if action.is_enabled() {
                    action.change_state(&true.to_variant());
                }
            }
        }
    }

    fn toggle_slideshow_pause(&self) {
        let imp = self.imp();
        if !imp.slideshow.get() {
            return;
        }
        let paused = !imp.slideshow_paused.get();
        imp.slideshow_paused.set(paused);
        self.sync_slideshow_play();
        self.wake_slideshow_controls();
        if paused {
            if let Some(timer) = imp.slideshow_timer.take() {
                timer.remove();
            }
        } else {
            self.schedule_slide();
        }
    }

    fn sync_slideshow_play(&self) {
        let imp = self.imp();
        let paused = imp.slideshow_paused.get();
        imp.slideshow_play.set_icon_name(if paused { "media-playback-start-symbolic" } else { "media-playback-pause-symbolic" });
        imp.slideshow_play.set_tooltip_text(Some(if paused { "Carry On (Space)" } else { "Pause (Space)" }));
    }

    fn update_slideshow_position(&self) {
        let imp = self.imp();
        let text = imp
            .playlist
            .borrow()
            .as_ref()
            .map(|list| format!("{} / {}", list.position(), list.len()))
            .unwrap_or_default();
        imp.slideshow_position.set_text(&text);
    }

    /// Start the picture on screen's time afresh.
    fn schedule_slide(&self) {
        let imp = self.imp();
        if let Some(timer) = imp.slideshow_timer.take() {
            timer.remove();
        }
        if !imp.slideshow.get() || imp.slideshow_paused.get() {
            return;
        }
        let timer = glib::timeout_add_local_once(
            Duration::from_secs(u64::from(self.slideshow_seconds())),
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move || {
                    let imp = window.imp();
                    imp.slideshow_timer.replace(None);
                    // On to the next, and round to the first after the last.
                    let next = imp.playlist.borrow_mut().as_mut().map(|list| list.step(1));
                    match next {
                        Some(path) => window.load(path, false),
                        None => window.stop_slideshow(),
                    }
                }
            ),
        );
        imp.slideshow_timer.replace(Some(timer));
    }

    /// Show the controls and the pointer, and hide them again once both
    /// have been left alone a while.
    pub(crate) fn wake_slideshow_controls(&self) {
        let imp = self.imp();
        if !imp.slideshow.get() {
            return;
        }
        imp.slideshow_controls.set_reveal_child(true);
        imp.view.canvas().set_cursor_hidden(false);
        if let Some(timer) = imp.slideshow_linger.take() {
            timer.remove();
        }
        if imp.slideshow_hovered.get() {
            return;
        }
        let timer = glib::timeout_add_local_once(
            CONTROLS_LINGER,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move || {
                    let imp = window.imp();
                    imp.slideshow_linger.replace(None);
                    if imp.slideshow.get() && !imp.slideshow_hovered.get() {
                        imp.slideshow_controls.set_reveal_child(false);
                        imp.view.canvas().set_cursor_hidden(true);
                    }
                }
            ),
        );
        imp.slideshow_linger.replace(Some(timer));
    }

    /// A picture failed to open mid-slideshow: carry on past it.
    pub(crate) fn slideshow_skip(&self) {
        if self.imp().slideshow.get() {
            self.schedule_slide();
        }
    }
}
