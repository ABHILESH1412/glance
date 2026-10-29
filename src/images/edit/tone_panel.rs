// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Adjust and Levels sections of the edit panel, and keeping the picture
//! on screen in step with them.
//!
//! Adjust has the sliders, grouped as light, colour and detail. Levels has
//! the histogram with its three handles, the same three values typed, and
//! Auto Levels. Both feed one set of `Adjustments`, shown on the picture as
//! they change and written with it when it is saved; each section's Reset
//! puts back only its own.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};

use crate::app::window::Window;
use crate::images::canvas;
use crate::images::edit::adjust::{self, Adjustments, Levels};
use crate::images::edit::levels;
use crate::app::explain;
use crate::images::edit::panel::{button_row, card, note as panel_note};
use crate::images::edit::tone::{Detail, ToneWorker};

/// How long a slider has to rest before the whole picture is worked out, not
/// just the quick copy.
const REST_MS: u64 = 250;

/// A slider for something that only goes one way from nothing, like sepia.
pub(crate) fn one_sided_scale() -> gtk::Scale {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, adjust::RANGE, 1.0);
    scale.set_draw_value(true);
    scale.set_value_pos(gtk::PositionType::Right);
    scale.set_digits(0);
    // Nothing is at the start, marked like the others' middle, which also
    // gives it their height so the rows beside each other stay even.
    scale.add_mark(0.0, gtk::PositionType::Bottom, None);
    scale.add_css_class("tone");
    crate::images::edit::panel::wheel_scrolls_panel(&scale);
    scale
}

fn reset_button(action: &str, tooltip: &str) -> gtk::Button {
    let reset = gtk::Button::with_label("Reset");
    reset.set_action_name(Some(action));
    reset.set_tooltip_text(Some(tooltip));
    reset
}

impl Window {
    /// The sliders, with what each controls.
    fn tone_sliders(&self) -> [(&'static str, &gtk::Scale); 10] {
        let imp = self.imp();
        [
            ("Exposure", &imp.exposure_scale),
            ("Brightness", &imp.brightness_scale),
            ("Contrast", &imp.contrast_scale),
            ("Highlights", &imp.highlights_scale),
            ("Shadows", &imp.shadows_scale),
            ("Saturation", &imp.saturation_scale),
            ("Temperature", &imp.temperature_scale),
            ("Tint", &imp.tint_scale),
            ("Sepia", &imp.sepia_scale),
            ("Sharpness", &imp.sharpness_scale),
        ]
    }

    pub(crate) fn build_tone_sections(&self, tools: &gtk::Box) {
        let imp = self.imp();

        // -- adjust --
        let toggle = &imp.adjust_toggle;
        toggle.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                if button.is_active() {
                    window.close_other_sections(button);
                }
                window.imp().adjust_options.set_visible(button.is_active());
            }
        ));

        let options = &imp.adjust_options;
        options.set_visible(false);
        // Which slider tells what, in a word, beside the name.
        let tips = [
            ("Exposure", "As if more or less light had come in, up to two stops either way"),
            ("Highlights", "Lighten or recover the bright parts, leaving the dark alone"),
            ("Shadows", "Lighten or deepen the dark parts, leaving the bright alone"),
            ("Temperature", "Cooler towards blue, warmer towards orange"),
            ("Tint", "Towards green or towards magenta"),
            ("Sepia", "Towards the brown of an old print"),
            ("Sharpness", "Crisper edges, or softer below zero"),
        ];
        let mut group = options.clone();
        // The names share one width, so the sliders beside them line up.
        let names = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);
        for (name, scale) in self.tone_sliders() {
            match name {
                "Exposure" => group = card(options, Some("Light")),
                "Saturation" => group = card(options, Some("Colour")),
                "Sharpness" => group = card(options, Some("Detail")),
                _ => {}
            }
            let label = gtk::Label::new(Some(name));
            label.set_xalign(0.0);
            names.add_widget(&label);
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.append(&label);
            scale.set_hexpand(true);
            row.append(scale);
            group.append(&row);
            match tips.iter().find(|(n, _)| *n == name) {
                Some((_, tip)) => explain(scale, name, tip),
                None => scale.update_property(&[gtk::accessible::Property::Label(name)]),
            }
            // The two white balance sliders show where they lead.
            match name {
                "Temperature" => scale.add_css_class("temperature"),
                "Tint" => scale.add_css_class("tint"),
                _ => {}
            }
            scale.connect_value_changed(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_| window.tone_changed()
            ));
        }
        options.append(&button_row(&[&reset_button("win.adjust-reset", "Put every slider back to the middle")]));
        tools.append(options);

        // -- levels --
        let toggle = &imp.levels_toggle;
        toggle.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                let open = button.is_active();
                if open {
                    window.close_other_sections(button);
                }
                window.imp().levels_options.set_visible(open);
                if open {
                    window.sync_levels();
                    window.refresh_tone();
                }
            }
        ));

        let options = &imp.levels_options;
        options.set_visible(false);
        let graph = card(options, None);

        let channel_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let channel_label = gtk::Label::new(Some("Channel"));
        channel_label.set_hexpand(true);
        channel_label.set_xalign(0.0);
        channel_row.append(&channel_label);
        let channel = &imp.levels_channel;
        channel.set_model(Some(&gtk::StringList::new(&levels::CHANNELS)));
        channel.set_selected(0);
        channel.update_property(&[gtk::accessible::Property::Label("Channel")]);
        channel.connect_selected_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.sync_levels()
        ));
        channel_row.append(channel);
        graph.append(&channel_row);

        imp.levels_graph.set_margin_top(4);
        imp.levels_graph.connect_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |levels| window.levels_changed(levels)
        ));
        graph.append(&imp.levels_graph);

        // The same three values, to type or step exactly, each beside its
        // name like the sliders of Adjust.
        let fields = gtk::Box::new(gtk::Orientation::Vertical, 6);
        fields.set_margin_top(4);
        for (label, spin) in [
            ("Black", &imp.levels_black),
            ("Midtones", &imp.levels_gamma),
            ("White", &imp.levels_white),
        ] {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let name = gtk::Label::new(Some(label));
            name.set_xalign(0.0);
            name.set_hexpand(true);
            row.append(&name);
            spin.set_numeric(true);
            spin.set_width_chars(4);
            crate::images::edit::panel::wheel_scrolls_panel(spin);
            spin.update_property(&[gtk::accessible::Property::Label(label)]);
            spin.connect_value_changed(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_| window.levels_typed()
            ));
            row.append(spin);
            fields.append(&row);
        }
        imp.levels_gamma.set_digits(2);
        explain(&imp.levels_gamma, "Midtones", "Above 1 lightens the middle greys, below 1 darkens them");
        graph.append(&fields);

        // Auto Levels works on each colour, which the RGB handles do not show.
        let note = &imp.levels_note;
        note.set_label("Red, green and blue have levels of their own too. Choose one above to see them.");
        panel_note(note);
        note.set_visible(false);
        graph.append(note);

        let auto = &imp.levels_auto;
        auto.set_label("Auto Levels");
        auto.set_tooltip_text(Some(
            "Stretch each colour so the darkest and brightest parts reach black and white, which also takes out a colour cast",
        ));
        auto.connect_clicked(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.auto_levels()
        ));
        options.append(&button_row(&[
            auto,
            &reset_button("win.levels-reset", "Put black, midtones and white back as they were"),
        ]));
        tools.append(options);
    }

    /// The channel levels is showing: 0 for all, then red, green, blue.
    fn levels_channel(&self) -> usize {
        (self.imp().levels_channel.selected() as usize).min(3)
    }

    /// A slider moved: show the new values. Matrices show at once; anything
    /// else is worked out on the tone thread.
    fn tone_changed(&self) {
        let imp = self.imp();
        if imp.syncing_panel.get() {
            return;
        }
        let canvas = imp.view.canvas();
        let mut adjust = canvas.adjustments();
        adjust.exposure = imp.exposure_scale.value();
        adjust.brightness = imp.brightness_scale.value();
        adjust.contrast = imp.contrast_scale.value();
        adjust.highlights = imp.highlights_scale.value();
        adjust.shadows = imp.shadows_scale.value();
        adjust.saturation = imp.saturation_scale.value();
        adjust.temperature = imp.temperature_scale.value();
        adjust.tint = imp.tint_scale.value();
        adjust.sepia = imp.sepia_scale.value();
        adjust.sharpness = imp.sharpness_scale.value();
        canvas.set_adjustments(adjust);
        // A new change: what undo put back to nothing is gone for good.
        imp.tone_redo.set(None);
        self.refresh_tone();
        self.update_tone_actions();
    }

    /// A handle was dragged.
    fn levels_changed(&self, levels: Levels) {
        let imp = self.imp();
        imp.tone_redo.set(None);
        let canvas = imp.view.canvas();
        let mut adjust = canvas.adjustments();
        adjust.levels[self.levels_channel()] = levels;
        canvas.set_adjustments(adjust);
        self.sync_levels();
        self.refresh_tone();
        self.update_tone_actions();
    }

    /// A value was typed or stepped.
    fn levels_typed(&self) {
        let imp = self.imp();
        if imp.syncing_panel.get() {
            return;
        }
        let black = imp.levels_black.value() / 255.0;
        let white = imp.levels_white.value() / 255.0;
        let levels = Levels {
            black: black.min(white - 2.0 / 255.0).max(0.0),
            gamma: imp.levels_gamma.value().clamp(Levels::GAMMA_MIN, Levels::GAMMA_MAX),
            white: white.max(black + 2.0 / 255.0).min(1.0),
        };
        self.levels_changed(levels);
    }

    fn auto_levels(&self) {
        let imp = self.imp();
        let Some(histogram) = imp.levels_graph.histogram() else {
            self.toast("Still measuring the picture.");
            return;
        };
        let canvas = imp.view.canvas();
        let mut adjust = canvas.adjustments();
        adjust.levels = histogram.auto_levels();
        imp.tone_redo.set(None);
        if adjust.levels_are_identity() {
            self.toast("The picture already runs from black to white.");
        }
        canvas.set_adjustments(adjust);
        self.sync_levels();
        self.refresh_tone();
        self.update_tone_actions();
    }

    /// Put the shown channel's levels into the handles and the fields.
    fn sync_levels(&self) {
        let imp = self.imp();
        let channel = self.levels_channel();
        let all = imp.view.canvas().adjustments().levels;
        let levels = all[channel];
        imp.levels_graph.show(channel, levels);
        imp.levels_note.set_visible(channel == 0 && all[1..].iter().any(|l| !l.is_identity()));
        let was = imp.syncing_panel.replace(true);
        imp.levels_black.set_value((levels.black * 255.0).round());
        imp.levels_gamma.set_value(levels.gamma);
        imp.levels_white.set_value((levels.white * 255.0).round());
        imp.syncing_panel.set(was);
    }

    /// Put the canvas's values back into the sliders and levels, after
    /// something baked them into the pixels and reset them.
    pub(crate) fn sync_tone_panel(&self) {
        let imp = self.imp();
        let adjust = imp.view.canvas().adjustments();
        let values = [
            adjust.exposure,
            adjust.brightness,
            adjust.contrast,
            adjust.highlights,
            adjust.shadows,
            adjust.saturation,
            adjust.temperature,
            adjust.tint,
            adjust.sepia,
            adjust.sharpness,
        ];
        imp.syncing_panel.set(true);
        for ((_, scale), value) in self.tone_sliders().into_iter().zip(values) {
            scale.set_value(value);
        }
        imp.syncing_panel.set(false);
        self.sync_levels();
        self.update_tone_actions();
    }

    fn update_tone_actions(&self) {
        // Undo can take the sliders back now, or has nothing of theirs left.
        self.update_edit_state();
        let adjust = self.imp().view.canvas().adjustments();
        for (name, enabled) in [
            ("adjust-reset", !adjust.sliders_are_identity()),
            ("levels-reset", !adjust.levels_are_identity()),
        ] {
            if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(enabled);
            }
        }
    }

    /// Put the sliders back to nothing, leaving levels.
    pub(crate) fn reset_sliders(&self) {
        self.imp().tone_redo.set(None);
        let canvas = self.imp().view.canvas();
        let levels = canvas.adjustments().levels;
        canvas.set_adjustments(Adjustments { levels, ..Default::default() });
        self.sync_tone_panel();
        self.refresh_tone();
    }

    /// Put levels back to nothing, leaving the sliders.
    pub(crate) fn reset_levels(&self) {
        self.imp().tone_redo.set(None);
        let canvas = self.imp().view.canvas();
        let adjust = Adjustments { levels: Default::default(), ..canvas.adjustments() };
        canvas.set_adjustments(adjust);
        self.sync_tone_panel();
        self.refresh_tone();
    }

    /// Ask the tone thread for whatever the screen needs now: the picture,
    /// if the colour matrix cannot show it, and the histogram, if levels is
    /// open.
    pub(crate) fn refresh_tone(&self) {
        let imp = self.imp();
        if let Some(timer) = imp.tone_timer.take() {
            timer.remove();
        }
        let adjust = imp.view.canvas().adjustments();
        let histogram = imp.levels_toggle.is_active();
        let pixels = !adjust.shows_on_gpu();
        if !pixels && !histogram {
            return;
        }
        if !self.start_tone_worker() {
            return;
        }
        let worker = imp.tone_worker.borrow();
        let Some(worker) = worker.as_ref() else { return };
        if !pixels {
            worker.ask(adjust, Detail::None, true);
            return;
        }
        worker.ask(adjust, Detail::Quick, histogram);
        if worker.is_small() {
            return;
        }
        // Once the slider rests, the whole picture, so zooming in shows it.
        let timer = glib::timeout_add_local_once(
            std::time::Duration::from_millis(REST_MS),
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move || {
                    let imp = window.imp();
                    imp.tone_timer.replace(None);
                    let now = imp.view.canvas().adjustments();
                    if now == adjust {
                        if let Some(worker) = imp.tone_worker.borrow().as_ref() {
                            worker.ask(adjust, Detail::Full, false);
                        }
                    }
                }
            ),
        );
        imp.tone_timer.replace(Some(timer));
    }

    /// Start the tone thread on the working pixels, if it is not running.
    /// False while there are no working pixels yet.
    fn start_tone_worker(&self) -> bool {
        let imp = self.imp();
        if imp.tone_worker.borrow().is_some() {
            return true;
        }
        let Some(working) = imp.working.borrow().clone() else {
            return false;
        };
        let weak = self.downgrade();
        let worker = ToneWorker::start(working, move |toned| {
            let Some(window) = weak.upgrade() else { return };
            let imp = window.imp();
            let canvas = imp.view.canvas();
            if let Some((width, height, pixels)) = toned.pixels {
                // The matrix may have taken over since this was asked for.
                if !canvas.adjustments().shows_on_gpu() {
                    canvas.set_toned(Some(canvas::texture_from(width, height, false, pixels)));
                }
            }
            if let Some(histogram) = toned.histogram {
                imp.levels_graph.set_histogram(Some(histogram));
            }
        });
        imp.tone_worker.replace(Some(worker));
        true
    }

    /// Stop the tone thread: the pixels it works from are no longer the ones
    /// being edited.
    pub(crate) fn stop_tone_worker(&self) {
        let imp = self.imp();
        if let Some(timer) = imp.tone_timer.take() {
            timer.remove();
        }
        imp.tone_worker.replace(None);
        imp.levels_graph.set_histogram(None);
    }
}
