// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Choosing a colour.
//!
//! GTK's own colour dialog opens as a window sized for its palette, and when
//! "+" swaps the palette for the custom-colour editor, which is narrower and
//! taller, the window keeps its old size: a strip left empty at the side, and
//! the editor cut off at the bottom behind a scrollbar. Here the same palette
//! and editor sit in a libadwaita dialog that follows the size of what it
//! shows, so each view fits exactly, and changes size smoothly between them.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, glib};

/// Ask for a colour, starting from `initial`; `done` is called with the one
/// chosen, and not at all if the dialog is cancelled.
// GtkColorChooserWidget is deprecated in favour of GtkColorDialog — the very
// dialog this replaces, which shows this same widget inside its own window.
#[allow(deprecated)]
pub fn choose(
    parent: &impl IsA<gtk::Widget>,
    title: &str,
    initial: &gdk::RGBA,
    with_alpha: bool,
    done: impl Fn(gdk::RGBA) + 'static,
) {
    let chooser = gtk::ColorChooserWidget::new();
    chooser.set_use_alpha(with_alpha);
    chooser.set_rgba(initial);
    chooser.set_margin_top(6);
    chooser.set_margin_bottom(18);
    chooser.set_margin_start(18);
    chooser.set_margin_end(18);

    let cancel = gtk::Button::with_label("Cancel");
    let select = gtk::Button::builder().label("Select").css_classes(["suggested-action"]).build();
    let header = adw::HeaderBar::new();
    header.set_show_start_title_buttons(false);
    header.set_show_end_title_buttons(false);
    header.pack_start(&cancel);
    header.pack_end(&select);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&chooser));

    let dialog = adw::Dialog::builder().title(title).child(&toolbar).follows_content_size(true).build();
    let done: Rc<dyn Fn(gdk::RGBA)> = Rc::new(done);

    cancel.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            dialog.close();
        }
    ));
    select.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        #[weak]
        chooser,
        #[strong]
        done,
        move |_| {
            done(chooser.rgba());
            dialog.close();
        }
    ));
    // A double-click on a colour chooses it straight away, as GTK's own does.
    chooser.connect_color_activated(glib::clone!(
        #[weak]
        dialog,
        #[strong]
        done,
        move |_, rgba| {
            done(*rgba);
            dialog.close();
        }
    ));
    dialog.present(Some(parent));
}

mod imp {
    use super::*;

    pub struct ColourButton {
        pub rgba: Cell<gdk::RGBA>,
        pub with_alpha: Cell<bool>,
        pub title: RefCell<String>,
        pub swatch: gtk::DrawingArea,
    }

    impl Default for ColourButton {
        fn default() -> Self {
            ColourButton {
                rgba: Cell::new(gdk::RGBA::BLACK),
                with_alpha: Cell::new(true),
                title: RefCell::new("Choose a Colour".to_string()),
                swatch: gtk::DrawingArea::new(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ColourButton {
        const NAME: &'static str = "GlanceColourButton";
        type Type = super::ColourButton;
        type ParentType = gtk::Button;
    }

    impl ObjectImpl for ColourButton {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: std::sync::OnceLock<Vec<glib::ParamSpec>> = std::sync::OnceLock::new();
            PROPERTIES.get_or_init(|| vec![glib::ParamSpecBoxed::builder::<gdk::RGBA>("rgba").build()])
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "rgba" => self.rgba.get().to_value(),
                _ => unreachable!(),
            }
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            if pspec.name() == "rgba" {
                if let Ok(rgba) = value.get::<gdk::RGBA>() {
                    self.rgba.set(rgba);
                    self.swatch.queue_draw();
                }
            }
        }

        fn constructed(&self) {
            self.parent_constructed();
            let button = self.obj();
            self.swatch.set_content_width(28);
            self.swatch.set_content_height(18);
            let weak = button.downgrade();
            self.swatch.set_draw_func(move |_, cr, w, h| {
                let Some(button) = weak.upgrade() else { return };
                let rgba = button.imp().rgba.get();
                paint_swatch(cr, f64::from(w), f64::from(h), &rgba);
            });
            button.set_child(Some(&self.swatch));
            button.connect_clicked(|button| {
                let imp = button.imp();
                let weak = button.downgrade();
                choose(button, &imp.title.borrow(), &imp.rgba.get(), imp.with_alpha.get(), move |rgba| {
                    if let Some(button) = weak.upgrade() {
                        button.set_rgba(&rgba);
                    }
                });
            });
        }
    }

    impl WidgetImpl for ColourButton {}
    impl ButtonImpl for ColourButton {}
}

/// A rounded patch of the colour, over a chequerboard so see-through shows as
/// see-through.
fn paint_swatch(cr: &gtk::cairo::Context, w: f64, h: f64, rgba: &gdk::RGBA) {
    let radius = 4.0;
    let rounded = |cr: &gtk::cairo::Context| {
        cr.new_sub_path();
        cr.arc(w - radius, radius, radius, -std::f64::consts::FRAC_PI_2, 0.0);
        cr.arc(w - radius, h - radius, radius, 0.0, std::f64::consts::FRAC_PI_2);
        cr.arc(radius, h - radius, radius, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
        cr.arc(radius, radius, radius, std::f64::consts::PI, 3.0 * std::f64::consts::FRAC_PI_2);
        cr.close_path();
    };
    rounded(cr);
    cr.clip();
    if rgba.alpha() < 1.0 {
        let cell = 5.0;
        for row in 0..=(h / cell) as i32 {
            for col in 0..=(w / cell) as i32 {
                let light = (row + col) % 2 == 0;
                cr.set_source_rgb(if light { 0.8 } else { 0.55 }, if light { 0.8 } else { 0.55 }, if light { 0.8 } else { 0.55 });
                cr.rectangle(f64::from(col) * cell, f64::from(row) * cell, cell, cell);
                let _ = cr.fill();
            }
        }
    }
    cr.set_source_rgba(f64::from(rgba.red()), f64::from(rgba.green()), f64::from(rgba.blue()), f64::from(rgba.alpha()));
    let _ = cr.paint();
    cr.reset_clip();
    rounded(cr);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.25);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
}

glib::wrapper! {
    /// A button showing a colour, which opens the chooser when clicked.
    pub struct ColourButton(ObjectSubclass<imp::ColourButton>)
        @extends gtk::Button, gtk::Widget,
        @implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}

impl ColourButton {
    pub fn new(initial: gdk::RGBA, with_alpha: bool) -> Self {
        let button: Self = glib::Object::new();
        button.imp().with_alpha.set(with_alpha);
        button.set_rgba(&initial);
        button
    }

    pub fn rgba(&self) -> gdk::RGBA {
        self.imp().rgba.get()
    }

    pub fn set_rgba(&self, rgba: &gdk::RGBA) {
        if self.imp().rgba.get() != *rgba {
            self.set_property("rgba", rgba);
        }
    }

    /// The chooser's title, when this button opens it.
    pub fn set_title(&self, title: &str) {
        self.imp().title.replace(title.to_string());
    }

    pub fn connect_rgba_notify(&self, f: impl Fn(&Self) + 'static) -> glib::SignalHandlerId {
        self.connect_notify_local(Some("rgba"), move |button, _| f(button))
    }
}
