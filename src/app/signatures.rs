// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Signatures, as the reader sees them: the pad a signature is written on,
//! the Signature button in the Draw panels that puts a kept one down, and
//! the small pictures of them both of those and the preferences show.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gdk, glib};

use crate::app::window::Window;
use crate::images::edit::signature::{self, Signature};

/// The pad's size, and how thick its pen is, in its pixels.
const PAD: (i32, i32) = (520, 200);
const PEN: f64 = 3.0;
/// What has been written so far, a stroke to each lift of the pen.
type Strokes = Vec<Vec<(f64, f64)>>;
/// The inks offered: dark enough to pass for a pen's on any paper.
const INKS: &[(&str, (f32, f32, f32))] = &[
    ("Black", (0.08, 0.08, 0.10)),
    ("Blue", (0.10, 0.24, 0.66)),
    ("Dark Blue", (0.05, 0.11, 0.36)),
];

/// A signature drawn small, on white, as on paper: a black one would vanish
/// on a dark popover otherwise.
pub fn thumbnail(signature: &Signature, width: i32, height: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder().content_width(width).content_height(height).build();
    let signature = signature.clone();
    area.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (f64::from(w), f64::from(h));
        rounded(cr, w, h, 6.0);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        let _ = cr.fill();
        let pad = 6.0;
        let (room_w, room_h) = ((w - pad * 2.0).max(1.0), (h - pad * 2.0).max(1.0));
        let scale = (room_w / signature.width.max(1.0)).min(room_h / signature.height.max(1.0));
        let (sw, sh) = (signature.width * scale, signature.height * scale);
        let frame = [(w - sw) / 2.0, (h - sh) / 2.0, sw, sh];
        let colour = signature.colour;
        cr.set_source_rgba(f64::from(colour.red()), f64::from(colour.green()), f64::from(colour.blue()), 1.0);
        cr.set_line_width((signature.pen * scale).max(1.0));
        trace(cr, &signature.fitted(frame));
    });
    area
}

fn rounded(cr: &gtk::cairo::Context, w: f64, h: f64, r: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(w - r, r, r, -FRAC_PI_2, 0.0);
    cr.arc(w - r, h - r, r, 0.0, FRAC_PI_2);
    cr.arc(r, h - r, r, FRAC_PI_2, PI);
    cr.arc(r, r, r, PI, PI + FRAC_PI_2);
    cr.close_path();
}

/// Stroke polylines with a pen's round ends.
fn trace(cr: &gtk::cairo::Context, strokes: &[Vec<(f64, f64)>]) {
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    cr.set_line_join(gtk::cairo::LineJoin::Round);
    for stroke in strokes {
        let Some((first, rest)) = stroke.split_first() else { continue };
        cr.move_to(first.0, first.1);
        if rest.is_empty() {
            cr.line_to(first.0, first.1);
        }
        for p in rest {
            cr.line_to(p.0, p.1);
        }
    }
    let _ = cr.stroke();
}

/// The signature pad: write a signature with the mouse, touchpad or a pen,
/// and keep it. `done` is given it once it has been kept.
pub fn write_new(parent: &impl IsA<gtk::Widget>, done: impl Fn(Signature) + 'static) {
    let strokes: Rc<RefCell<Strokes>> = Rc::default();
    let ink = Rc::new(std::cell::Cell::new(INKS[0].1));

    let pad = gtk::DrawingArea::builder().content_width(PAD.0).content_height(PAD.1).build();
    pad.set_cursor_from_name(Some("crosshair"));
    pad.update_property(&[gtk::accessible::Property::Label("Signature pad")]);
    {
        let (strokes, ink) = (strokes.clone(), ink.clone());
        pad.set_draw_func(move |_, cr, w, h| {
            let (w, h) = (f64::from(w), f64::from(h));
            rounded(cr, w, h, 10.0);
            cr.set_source_rgb(1.0, 1.0, 1.0);
            let _ = cr.fill_preserve();
            // Edged, or it melts into a light dialog.
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.18);
            cr.set_line_width(1.0);
            let _ = cr.stroke();
            // The line to sign on, and the cross that says where.
            let line = h * 0.74;
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.25);
            cr.set_line_width(1.0);
            cr.move_to(24.0, line + 0.5);
            cr.line_to(w - 24.0, line + 0.5);
            let _ = cr.stroke();
            cr.set_line_width(1.5);
            cr.move_to(26.0, line - 14.0);
            cr.line_to(36.0, line - 4.0);
            cr.move_to(36.0, line - 14.0);
            cr.line_to(26.0, line - 4.0);
            let _ = cr.stroke();
            let (r, g, b) = ink.get();
            cr.set_source_rgb(f64::from(r), f64::from(g), f64::from(b));
            cr.set_line_width(PEN);
            trace(cr, &strokes.borrow());
        });
    }

    let cancel = gtk::Button::with_mnemonic("_Cancel");
    let save = gtk::Button::builder().label("_Save").use_underline(true).css_classes(["suggested-action"]).sensitive(false).build();
    let header = adw::HeaderBar::new();
    header.set_show_start_title_buttons(false);
    header.set_show_end_title_buttons(false);
    header.pack_start(&cancel);
    header.pack_end(&save);

    let hint = gtk::Label::builder()
        .label("Sign on the line with the mouse, the touchpad or a pen. It is sized to fit wherever it is put.")
        .wrap(true)
        .xalign(0.0)
        .css_classes(["dim-label"])
        .build();

    let inks = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let caption = gtk::Label::new(Some("Ink"));
    caption.add_css_class("dim-label");
    inks.append(&caption);
    let mut first: Option<gtk::ToggleButton> = None;
    for &(name, (r, g, b)) in INKS {
        // Small inside its button, so the one chosen shows as pressed in.
        let swatch = gtk::DrawingArea::builder()
            .content_width(16)
            .content_height(16)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        swatch.set_draw_func(move |_, cr, w, h| {
            let (w, h) = (f64::from(w), f64::from(h));
            cr.arc(w / 2.0, h / 2.0, w.min(h) / 2.0 - 0.5, 0.0, std::f64::consts::TAU);
            cr.set_source_rgb(f64::from(r), f64::from(g), f64::from(b));
            let _ = cr.fill_preserve();
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.4);
            cr.set_line_width(1.0);
            let _ = cr.stroke();
        });
        let button = gtk::ToggleButton::builder().child(&swatch).tooltip_text(name).css_classes(["flat", "circular"]).build();
        button.update_property(&[gtk::accessible::Property::Label(name)]);
        match &first {
            Some(first) => button.set_group(Some(first)),
            None => {
                button.set_active(true);
                first = Some(button.clone());
            }
        }
        let (ink, pad) = (ink.clone(), pad.clone());
        button.connect_toggled(move |button| {
            if button.is_active() {
                ink.set((r, g, b));
                pad.queue_draw();
            }
        });
        inks.append(&button);
    }
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    inks.append(&spacer);
    let undo = gtk::Button::builder().icon_name("edit-undo-symbolic").tooltip_text("Take back the last stroke (Ctrl+Z)").sensitive(false).build();
    let clear = gtk::Button::builder().label("C_lear").use_underline(true).sensitive(false).build();
    inks.append(&undo);
    inks.append(&clear);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.set_margin_top(6);
    body.set_margin_bottom(18);
    body.set_margin_start(18);
    body.set_margin_end(18);
    body.append(&hint);
    body.append(&pad);
    body.append(&inks);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&body));
    let dialog = adw::Dialog::builder().title("New Signature").child(&toolbar).follows_content_size(true).build();

    // Whether there is anything yet, for the buttons that need something.
    let refresh = {
        let (strokes, save, undo, clear, pad) = (strokes.clone(), save.clone(), undo.clone(), clear.clone(), pad.clone());
        Rc::new(move || {
            let strokes = strokes.borrow();
            save.set_sensitive(Signature::from_strokes(&strokes, PEN, gdk::RGBA::BLACK).is_some());
            undo.set_sensitive(!strokes.is_empty());
            clear.set_sensitive(!strokes.is_empty());
            pad.queue_draw();
        })
    };

    let drag = gtk::GestureDrag::new();
    drag.set_button(gdk::BUTTON_PRIMARY);
    {
        let (strokes, refresh) = (strokes.clone(), refresh.clone());
        drag.connect_drag_begin(move |_, x, y| {
            strokes.borrow_mut().push(vec![(x, y)]);
            refresh();
        });
    }
    {
        let (strokes, pad) = (strokes.clone(), pad.clone());
        drag.connect_drag_update(move |gesture, dx, dy| {
            let Some((x, y)) = gesture.start_point() else { return };
            if let Some(stroke) = strokes.borrow_mut().last_mut() {
                stroke.push((x + dx, y + dy));
            }
            pad.queue_draw();
        });
    }
    {
        let refresh = refresh.clone();
        drag.connect_drag_end(move |_, _, _| refresh());
    }
    pad.add_controller(drag);

    {
        let (strokes, refresh) = (strokes.clone(), refresh.clone());
        undo.connect_clicked(move |_| {
            strokes.borrow_mut().pop();
            refresh();
        });
    }
    {
        let (strokes, refresh) = (strokes.clone(), refresh.clone());
        clear.connect_clicked(move |_| {
            strokes.borrow_mut().clear();
            refresh();
        });
    }
    let keys = gtk::EventControllerKey::new();
    {
        let (strokes, refresh) = (strokes.clone(), refresh.clone());
        keys.connect_key_pressed(move |_, key, _, state| {
            if key == gdk::Key::z && state.contains(gdk::ModifierType::CONTROL_MASK) {
                strokes.borrow_mut().pop();
                refresh();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    dialog.add_controller(keys);

    cancel.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            dialog.close();
        }
    ));
    save.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            let (r, g, b) = ink.get();
            let Some(written) = Signature::from_strokes(&strokes.borrow(), PEN, gdk::RGBA::new(r, g, b, 1.0)) else {
                return;
            };
            match signature::save(&written) {
                Ok(kept) => {
                    dialog.close();
                    done(kept);
                }
                Err(error) => {
                    let alert = adw::AlertDialog::new(Some("Could Not Keep the Signature"), Some(&error));
                    alert.add_response("close", "_Close");
                    alert.present(Some(&dialog));
                }
            }
        }
    ));
    dialog.present(Some(parent));
}

/// The Signature button of a Draw panel: kept signatures to put down, one
/// click each, and the way to write a new one or tidy them up.
pub fn button(window: &Window, place: impl Fn(Arc<Signature>) + 'static) -> gtk::MenuButton {
    let content = adw::ButtonContent::builder().icon_name("document-edit-symbolic").label("Signature").build();
    let button = gtk::MenuButton::builder()
        .child(&content)
        .tooltip_text("Put a signature down, or write a new one")
        .always_show_arrow(true)
        .build();
    let list = gtk::Box::new(gtk::Orientation::Vertical, 6);
    list.set_margin_top(6);
    list.set_margin_bottom(6);
    list.set_margin_start(6);
    list.set_margin_end(6);
    let popover = gtk::Popover::builder().child(&list).build();
    button.set_popover(Some(&popover));
    let place: Rc<dyn Fn(Arc<Signature>)> = Rc::new(place);
    // Filled each time it opens, so it always shows what is kept.
    popover.connect_show(glib::clone!(
        #[weak]
        window,
        #[weak]
        popover,
        #[weak]
        list,
        move |_| {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            let kept = signature::all();
            if kept.is_empty() {
                let none = gtk::Label::builder()
                    .label("No signatures yet. Write one once, then put it down anywhere.")
                    .wrap(true)
                    .max_width_chars(24)
                    .xalign(0.0)
                    .css_classes(["dim-label"])
                    .build();
                none.set_margin_start(6);
                none.set_margin_end(6);
                none.set_margin_top(6);
                none.set_margin_bottom(6);
                list.append(&none);
            }
            for (n, kept) in kept.into_iter().enumerate() {
                let face = thumbnail(&kept, 200, 64);
                let choice = gtk::Button::builder().child(&face).tooltip_text("Put this signature down").css_classes(["flat"]).build();
                choice.update_property(&[gtk::accessible::Property::Label(&format!("Signature {}", n + 1))]);
                let kept = Arc::new(kept);
                let place = place.clone();
                choice.connect_clicked(glib::clone!(
                    #[weak]
                    popover,
                    move |_| {
                        popover.popdown();
                        place(kept.clone());
                    }
                ));
                list.append(&choice);
            }
            list.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            let new = gtk::Button::builder().label("_New Signature…").use_underline(true).css_classes(["flat"]).build();
            let place = place.clone();
            new.connect_clicked(glib::clone!(
                #[weak]
                window,
                #[weak]
                popover,
                move |_| {
                    popover.popdown();
                    let place = place.clone();
                    write_new(&window, move |kept| place(Arc::new(kept)));
                }
            ));
            list.append(&new);
            let manage = gtk::Button::builder().label("_Manage Signatures…").use_underline(true).css_classes(["flat"]).build();
            manage.connect_clicked(glib::clone!(
                #[weak]
                window,
                #[weak]
                popover,
                move |_| {
                    popover.popdown();
                    window.show_preferences_at(Some("signatures"));
                }
            ));
            list.append(&manage);
        }
    ));
    button
}
