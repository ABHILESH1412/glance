// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Signatures, as the reader sees them: the pad a signature is written on,
//! bringing one in from a picture, the PDF toolbar's Signature button and
//! the image editor's Signature tool that put a kept one down, and the small pictures of them both of those
//! and the preferences show.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use image::RgbaImage;

use crate::app::window::Window;
use crate::images::edit::signature::{self, Ink, Signature};

/// The pad's size, and how thick its pen is, in its pixels.
const PAD: (i32, i32) = (520, 200);
const PEN: f64 = 3.0;
/// What is done with a signature chosen from a list of them.
type Place = Rc<dyn Fn(Arc<Signature>)>;
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
    let picture = signature.picture.as_ref().and_then(|picture| surface_of(&picture.pixels));
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
        if let Some(surface) = &picture {
            let _ = cr.save();
            cr.translate(frame[0], frame[1]);
            cr.scale(scale, scale);
            let _ = cr.set_source_surface(surface, 0.0, 0.0);
            cr.source().set_filter(gtk::cairo::Filter::Good);
            let _ = cr.paint();
            let _ = cr.restore();
            return;
        }
        let colour = signature.colour;
        cr.set_source_rgba(f64::from(colour.red()), f64::from(colour.green()), f64::from(colour.blue()), 1.0);
        cr.set_line_width((signature.pen * scale).max(1.0));
        trace(cr, &signature.fitted(frame));
    });
    area
}

/// A picture as Cairo draws it: premultiplied, in the machine's own order.
fn surface_of(pixels: &RgbaImage) -> Option<gtk::cairo::ImageSurface> {
    let (width, height) = (i32::try_from(pixels.width()).ok()?, i32::try_from(pixels.height()).ok()?);
    let mut surface = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, width, height).ok()?;
    let stride = surface.stride() as usize;
    {
        let mut data = surface.data().ok()?;
        for (y, row) in pixels.rows().enumerate() {
            for (x, p) in row.enumerate() {
                let a = u32::from(p[3]);
                let pre = |c: u8| (u32::from(c) * a + 127) / 255;
                let argb = (a << 24) | (pre(p[0]) << 16) | (pre(p[1]) << 8) | pre(p[2]);
                data[y * stride + x * 4..y * stride + x * 4 + 4].copy_from_slice(&argb.to_ne_bytes());
            }
        }
    }
    Some(surface)
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
    let undo = gtk::Button::builder().icon_name("glance-undo-symbolic").tooltip_text("Take back the last stroke (Ctrl+Z)").sensitive(false).build();
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

/// A picture of a signature is not read at more than this across: the
/// signature is kept smaller still, and a 50-megapixel photograph of a page
/// would only make the paper slower to find.
const IMPORT_SIDE: u32 = 2400;

/// Bring a signature in from a picture of it: a scan, a photograph of it on
/// paper, or a PNG made elsewhere. The paper is made see-through, and the
/// result shown to be kept, in the pen's own ink or another. `done` is given
/// it once it has been kept.
pub fn import_picture(window: &Window, done: impl Fn(Signature) + 'static) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Pictures"));
    filter.add_mime_type("image/*");
    for suffix in crate::app::playlist::EXTENSIONS {
        filter.add_suffix(suffix);
    }
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let chooser = gtk::FileDialog::builder()
        .title("Import a Signature")
        .accept_label("_Import")
        .modal(true)
        .filters(&filters)
        .default_filter(&filter)
        .build();
    let done: Rc<dyn Fn(Signature)> = Rc::new(done);
    chooser.open(
        Some(window),
        gio::Cancellable::NONE,
        glib::clone!(
            #[weak]
            window,
            move |result| {
                let path = match result {
                    Ok(file) => match file.path() {
                        Some(path) => path,
                        None => return window.toast("That location is not a local file."),
                    },
                    Err(error) => {
                        if !error.matches(gtk::DialogError::Dismissed) {
                            window.toast(&format!("Could not open the picture: {error}"));
                        }
                        return;
                    }
                };
                let (sender, receiver) = async_channel::bounded(1);
                std::thread::spawn(move || {
                    let found = read_scan(&path).and_then(|scan| {
                        Signature::from_picture(&scan, Ink::AsItIs).ok_or_else(|| "No signature was found in that picture.".to_string())
                    });
                    let _ = sender.send_blocking(found);
                });
                let done = done.clone();
                glib::spawn_future_local(async move {
                    match receiver.recv().await {
                        Ok(Ok(found)) => offer_import(&window, found, done),
                        Ok(Err(message)) => window.toast(&message),
                        Err(_) => window.toast("Reading the picture stopped unexpectedly."),
                    }
                });
            }
        ),
    );
}

/// A picture file as straight RGBA, no bigger than `IMPORT_SIDE` across.
fn read_scan(path: &std::path::Path) -> Result<RgbaImage, String> {
    let decoded = crate::images::loader::decode(path)?;
    let mut rgba = RgbaImage::from_raw(decoded.width, decoded.height, decoded.rgba).ok_or("The picture could not be read.")?;
    if decoded.premultiplied {
        for pixel in rgba.pixels_mut() {
            let a = u32::from(pixel[3]);
            if a > 0 && a < 255 {
                for c in &mut pixel.0[..3] {
                    *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
    }
    let side = rgba.width().max(rgba.height());
    if side > IMPORT_SIDE {
        let scale = f64::from(IMPORT_SIDE) / f64::from(side);
        let to = |n: u32| ((f64::from(n) * scale).round() as u32).max(1);
        rgba = image::imageops::resize(&rgba, to(rgba.width()), to(rgba.height()), image::imageops::FilterType::Triangle);
    }
    Ok(rgba)
}

/// The signature found in a picture, shown on paper to be kept, with the
/// pen's own ink or one of the pad's.
fn offer_import(window: &Window, found: Signature, done: Rc<dyn Fn(Signature)>) {
    let Some(original) = found.picture.as_ref().map(|picture| picture.pixels.clone()) else { return };
    let recoloured = move |ink: Ink| -> Signature {
        let mut pixels = original.clone();
        if let Ink::Colour(rgb) = ink {
            for pixel in pixels.pixels_mut() {
                pixel.0[..3].copy_from_slice(&rgb);
            }
        }
        Signature::from_picture(&pixels, Ink::AsItIs).unwrap_or_else(|| found.clone())
    };
    let chosen = Rc::new(RefCell::new(recoloured(Ink::AsItIs)));

    let preview = gtk::Box::new(gtk::Orientation::Vertical, 0);
    preview.set_size_request(PAD.0, PAD.1);
    let show = {
        let preview = preview.clone();
        move |signature: &Signature| {
            while let Some(child) = preview.first_child() {
                preview.remove(&child);
            }
            let face = thumbnail(signature, PAD.0, PAD.1);
            face.set_hexpand(true);
            face.set_vexpand(true);
            preview.append(&face);
        }
    };
    show(&chosen.borrow());

    let caption = gtk::Label::builder()
        .label("The paper is made see-through, so only the ink is put down.")
        .wrap(true)
        .css_classes(["dim-label"])
        .build();
    let inks = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    inks.set_halign(gtk::Align::Center);
    let as_it_is = gtk::ToggleButton::with_label("As It Is");
    as_it_is.set_active(true);
    inks.append(&as_it_is);
    let recoloured = Rc::new(recoloured);
    let mut buttons = vec![(as_it_is.clone(), Ink::AsItIs)];
    for &(name, (r, g, b)) in INKS {
        let button = gtk::ToggleButton::with_label(name);
        button.set_group(Some(&as_it_is));
        inks.append(&button);
        let rgb = [r, g, b].map(|c| (c * 255.0).round() as u8);
        buttons.push((button, Ink::Colour(rgb)));
    }
    for (button, ink) in buttons {
        let (chosen, recoloured, show) = (chosen.clone(), recoloured.clone(), show.clone());
        button.connect_toggled(move |button| {
            if button.is_active() {
                chosen.replace(recoloured(ink));
                show(&chosen.borrow());
            }
        });
    }

    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.set_margin_top(12);
    column.set_margin_bottom(18);
    column.set_margin_start(18);
    column.set_margin_end(18);
    column.append(&preview);
    column.append(&caption);
    let ink_label = gtk::Label::builder().label("Ink").css_classes(["heading"]).build();
    column.append(&ink_label);
    column.append(&inks);

    let cancel = gtk::Button::with_mnemonic("_Cancel");
    let keep = gtk::Button::with_mnemonic("_Keep");
    keep.add_css_class("suggested-action");
    let header = adw::HeaderBar::builder().show_end_title_buttons(false).show_start_title_buttons(false).build();
    header.pack_start(&cancel);
    header.pack_end(&keep);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&column));
    let dialog = adw::Dialog::builder().title("Import Signature").child(&toolbar).follows_content_size(true).build();
    cancel.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            dialog.close();
        }
    ));
    keep.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| match signature::save(&chosen.borrow()) {
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
    ));
    dialog.present(Some(window));
}

/// The Signature button of the PDF markup toolbar: kept signatures to put
/// down, one click each, and the way to write a new one or tidy them up.
pub fn button(window: &Window, place: impl Fn(Arc<Signature>) + 'static) -> gtk::MenuButton {
    let content = adw::ButtonContent::builder().icon_name("glance-signature-symbolic").label("Signature").build();
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
    let place: Place = Rc::new(place);
    let done: Rc<dyn Fn()> = Rc::new(glib::clone!(
        #[weak]
        popover,
        move || popover.popdown()
    ));
    // Filled each time it opens, so it always shows what is kept.
    popover.connect_show(glib::clone!(
        #[weak]
        window,
        #[weak]
        list,
        move |_| fill(&list, &window, place.clone(), done.clone(), Rc::new(|| ()))
    ));
    button
}

/// The image editor's Signature tool: the kept signatures, one click each to
/// put down, and the way to write a new one or tidy them up, laid out in
/// the editor's settings pane rather than a menu.
pub fn pane(window: &Window, place: impl Fn(Arc<Signature>) + 'static) -> gtk::Box {
    let list = gtk::Box::new(gtk::Orientation::Vertical, 6);
    // Putting one down fills the list again, so a signature just written or
    // imported is there to use a second time; so does coming back from
    // Preferences, where some may have gone.
    let placing: Rc<std::cell::OnceCell<Place>> = Rc::default();
    let refill = refill_again(&list, window, &placing);
    let after = refill.clone();
    let _ = placing.set(Rc::new(move |kept| {
        place(kept);
        after();
    }));
    // Filled each time the tool is opened, so it always shows what is kept.
    // This handler is what keeps `placing` alive, for as long as the list.
    list.connect_map(move |_| {
        let _ = &placing;
        refill();
    });
    list
}

/// A way to fill `list` again later, holding nothing alive.
fn refill_again(list: &gtk::Box, window: &Window, cell: &Rc<std::cell::OnceCell<Place>>) -> Rc<dyn Fn()> {
    let (list, window, cell) = (list.downgrade(), window.downgrade(), Rc::downgrade(cell));
    Rc::new(move || {
        if let (Some(list), Some(window), Some(cell)) = (list.upgrade(), window.upgrade(), cell.upgrade()) {
            if let Some(place) = cell.get() {
                fill(&list, &window, place.clone(), Rc::new(|| ()), refill_again(&list, &window, &cell));
            }
        }
    })
}

/// Put the kept signatures in `list`, each a button that hands it to
/// `place`, then the ways to write, import and manage them. `done` is
/// called when one is chosen, to put a menu away, and `refresh` when
/// Preferences closes, to show what is left.
fn fill(list: &gtk::Box, window: &Window, place: Place, done: Rc<dyn Fn()>, refresh: Rc<dyn Fn()>) {
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
        let (place, done) = (place.clone(), done.clone());
        choice.connect_clicked(move |_| {
            done();
            place(kept.clone());
        });
        list.append(&choice);
    }
    list.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let new = gtk::Button::builder().label("_New Signature…").use_underline(true).css_classes(["flat"]).build();
    let (place_new, done_new) = (place.clone(), done.clone());
    new.connect_clicked(glib::clone!(
        #[weak]
        window,
        move |_| {
            done_new();
            let place = place_new.clone();
            write_new(&window, move |kept| place(Arc::new(kept)));
        }
    ));
    list.append(&new);
    let import = gtk::Button::builder().label("_Import from Picture…").use_underline(true).css_classes(["flat"]).build();
    let done_import = done.clone();
    import.connect_clicked(glib::clone!(
        #[weak]
        window,
        move |_| {
            done_import();
            let place = place.clone();
            import_picture(&window, move |kept| place(Arc::new(kept)));
        }
    ));
    list.append(&import);
    let manage = gtk::Button::builder().label("_Manage Signatures…").use_underline(true).css_classes(["flat"]).build();
    manage.connect_clicked(glib::clone!(
        #[weak]
        window,
        move |_| {
            done();
            let refresh = refresh.clone();
            window.show_preferences_at(Some("signatures")).connect_closed(move |_| refresh());
        }
    ));
    list.append(&manage);
}
