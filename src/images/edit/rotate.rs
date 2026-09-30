// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Rotate & Flip section, and reading an angle typed into its box.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use crate::app::window::Window;

/// Bring an angle into the half turn either side of upright, the way angles
/// work rather than the way a bounded number works: 400 degrees is 40, -200 is
/// 160. Half a turn stays at 180 rather than becoming -180, because that is
/// what someone who typed 180 asked for.
pub(crate) fn wrap_degrees(degrees: f64) -> f64 {
    let wrapped = degrees.rem_euclid(360.0);
    if wrapped > 180.0 {
        wrapped - 360.0
    } else {
        wrapped
    }
}

/// True when two angles point the same way, whole turns and the two spellings
/// of half a turn (180 and -180) included.
pub(crate) fn same_angle(a: f64, b: f64) -> bool {
    wrap_degrees(a - b).abs() < 0.01
}

impl Window {
    /// The rotation controls, in two rows: the angle on its own line at the
    /// top, then the buttons. Squeezed onto one line the slider had barely a
    /// centimetre to cover half a turn, which made every angle a fight.
    pub(crate) fn build_rotation_bar(&self) -> &gtk::Box {
        let imp = self.imp();
        let bar = &imp.rotation_bar;
        // Hidden until its section is opened, like every other tool's options.
        bar.set_visible(false);

        let left = gtk::Button::from_icon_name("object-rotate-left-symbolic");
        left.set_tooltip_text(Some("Rotate Left 90°"));
        left.set_action_name(Some("win.rotate-left"));
        left.add_css_class("flat");

        let right = gtk::Button::from_icon_name("object-rotate-right-symbolic");
        right.set_tooltip_text(Some("Rotate Right 90°"));
        right.set_action_name(Some("win.rotate-right"));
        right.add_css_class("flat");

        let slider = &imp.rotation_scale;
        slider.set_hexpand(true);
        slider.set_draw_value(false);
        // The angle runs either side of zero, so a bar filling from the far
        // left would read as though 0 were most of the way along.
        slider.set_has_origin(false);
        slider.set_value(0.0);
        // Detents at the quarter turns and at upright, so the useful angles are
        // easy to find by eye.
        for mark in [-180.0, -90.0, 0.0, 90.0, 180.0] {
            slider.add_mark(mark, gtk::PositionType::Bottom, None);
        }

        let spin = &imp.rotation_spin;
        // Fixed width, otherwise the slider shuffles sideways as digits appear.
        spin.set_width_chars(5);
        spin.set_max_width_chars(6);
        spin.set_hexpand(false);
        spin.set_digits(0);
        spin.set_tooltip_text(Some("The angle. Type one, or step it a degree at a time"));
        // Angles wrap rather than stop, so stepping past half a turn comes
        // round the other side instead of sticking at the end.
        spin.set_wrap(true);
        // Not numeric: the box shows a degree sign, and refusing to let the
        // user type one back would be a trap. What is typed is parsed below.
        spin.set_numeric(false);

        // Draw the degree sign, so the number is never ambiguous.
        spin.connect_output(|spin| {
            spin.set_text(&format!("{:.0}°", spin.value()));
            glib::Propagation::Stop
        });

        // Read what was typed. A whole turn is 360 degrees and then it starts
        // again, so a number past the end is not something to refuse or clamp:
        // 400 is 40, -200 is 160, 720 is straight.
        //
        // Text that is not a number at all puts the old angle back, by handing
        // it straight back as the answer. GTK's own way of saying "that was not
        // a number" turns the picture upright instead of leaving it alone,
        // which loses work over a typo.
        spin.connect_input(|spin| {
            let text = spin.text();
            let typed = text.trim().trim_end_matches('\u{00b0}').trim();
            Some(Ok(match typed.parse::<f64>() {
                Ok(degrees) if degrees.is_finite() => wrap_degrees(degrees),
                _ => spin.value(),
            }))
        });

        spin.connect_value_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |spin| {
                if window.imp().syncing.get() {
                    return;
                }
                window.imp().view.canvas().set_rotation(spin.value());
            }
        ));

        let flip_h = &imp.flip_h_button;
        flip_h.set_icon_name("object-flip-horizontal-symbolic");
        flip_h.set_tooltip_text(Some("Flip Horizontally"));
        flip_h.set_action_name(Some("win.flip-horizontal"));
        flip_h.add_css_class("flat");

        let flip_v = &imp.flip_v_button;
        flip_v.set_icon_name("object-flip-vertical-symbolic");
        flip_v.set_tooltip_text(Some("Flip Vertically"));
        flip_v.set_action_name(Some("win.flip-vertical"));
        flip_v.add_css_class("flat");

        let reset = gtk::Button::from_icon_name("edit-undo-symbolic");
        reset.set_tooltip_text(Some("Reset Rotation"));
        reset.set_action_name(Some("win.rotate-reset"));
        reset.add_css_class("flat");

        // The slider gets the whole width of the panel to itself. Sharing a
        // line with four buttons left it a centimetre to cover half a turn,
        // which is what made every angle a fight.
        let angle = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let angle_label = gtk::Label::new(Some("Angle"));
        angle_label.set_xalign(0.0);
        angle_label.set_hexpand(true);
        angle_label.add_css_class("dim-label");
        angle.append(&angle_label);
        angle.append(spin);

        // ...and the things that jump to a particular angle underneath.
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.append(&left);
        buttons.append(&right);
        buttons.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        buttons.append(flip_h);
        buttons.append(flip_v);
        // Pushed to the far end: undoing the lot should not sit a slip away
        // from the buttons used all the time.
        reset.set_hexpand(true);
        reset.set_halign(gtk::Align::End);
        buttons.append(&reset);

        bar.append(slider);
        bar.append(&angle);
        bar.append(&buttons);

        slider.connect_value_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |scale| {
                if window.imp().syncing.get() {
                    return;
                }
                window.imp().view.canvas().set_rotation(scale.value());
            }
        ));

        imp.view.canvas().connect_flip_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |h, v| {
                // Reflect state without re-triggering the actions.
                window.imp().flip_h_button.set_active(h);
                window.imp().flip_v_button.set_active(v);
            }
        ));

        imp.view.canvas().connect_rotation_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |degrees| window.show_rotation(degrees)
        ));

        bar
    }

    /// Show or hide the transform options. While they are open the header
    /// carries only the close button, so the controls have the user's full
    /// attention and there is one obvious way back.
    /// Show the rotate and flip controls, which means opening the edit panel
    /// at that section.
    pub(crate) fn set_transform_open(&self, open: bool) {
        let imp = self.imp();
        if open && !imp.edit_button.is_sensitive() {
            return;
        }
        if open {
            imp.edit_button.set_active(true);
        }
        imp.transform_toggle.set_active(open);
    }

    /// Push the canvas's angle back into the slider and the typed readout.
    fn show_rotation(&self, degrees: f64) {
        let imp = self.imp();
        imp.syncing.set(true);
        // Half a turn is the same angle whichever way it is written, so a
        // readout showing 180 is left alone rather than flipped to -180 under
        // the user's fingers.
        if !same_angle(imp.rotation_scale.value(), degrees) {
            imp.rotation_scale.set_value(degrees);
        }
        if !same_angle(imp.rotation_spin.value(), degrees) {
            imp.rotation_spin.set_value(degrees);
        }
        imp.syncing.set(false);
    }
}

#[cfg(test)]
mod tests {
    use super::{same_angle, wrap_degrees};

    #[test]
    fn a_typed_angle_past_the_end_comes_round_rather_than_sticking() {
        // Someone typing 400 means 40, not "as far as it goes".
        assert!((wrap_degrees(400.0) - 40.0).abs() < 1e-9);
        assert!((wrap_degrees(-200.0) - 160.0).abs() < 1e-9);
        assert!(wrap_degrees(720.0).abs() < 1e-9);
        assert!((wrap_degrees(-365.0) - -5.0).abs() < 1e-9);
    }

    #[test]
    fn half_a_turn_stays_the_way_it_was_typed() {
        assert!((wrap_degrees(180.0) - 180.0).abs() < 1e-9);
        assert!((wrap_degrees(-180.0) - 180.0).abs() < 1e-9);
        assert!(same_angle(180.0, -180.0));
    }

    #[test]
    fn ordinary_angles_are_left_alone() {
        for degrees in [-179.0, -90.0, -0.5, 0.0, 1.0, 45.0, 90.0, 179.5] {
            assert!((wrap_degrees(degrees) - degrees).abs() < 1e-9, "{degrees}");
        }
    }

    #[test]
    fn upside_down_is_not_the_same_as_upright() {
        assert!(!same_angle(0.0, 180.0));
        assert!(!same_angle(45.0, -135.0));
        assert!(same_angle(45.0, 405.0));
    }
}
