// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The editing sidebar: its sections, and keeping each section's controls in
//! step with the canvas.
//!
//! The widgets are fields of the window in `app::window`, because a GObject's
//! state has to live in one struct. What they do lives here.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, gio, glib};
use crate::images::edit::adjust;
use crate::images::edit::compress;
use crate::images::edit::draw;
use crate::images::edit::export;
use crate::app::window::Window;

/// A tool section's header: an icon beside its name, so the panel can be
/// scanned by shape rather than read line by line.
pub(crate) fn section_toggle(icon: &str, label: &str) -> gtk::ToggleButton {
    let button = gtk::ToggleButton::new();
    button.set_child(Some(
        &adw::ButtonContent::builder()
            .icon_name(icon)
            .label(label)
            .build(),
    ));
    button
}

/// A colour well that lets the alpha channel be set, so "no background" is a
/// colour you can choose rather than a separate switch.
/// How many sides a polygon has and how many points a star: one box beside
/// the pens, shown only for those two, remembering each count apart.
#[derive(Clone)]
pub struct Corners {
    pub row: gtk::Box,
    caption: gtk::Label,
    spin: gtk::SpinButton,
    sides: std::rc::Rc<std::cell::Cell<u8>>,
    points: std::rc::Rc<std::cell::Cell<u8>>,
    /// Which count is on show, the polygon's or the star's.
    showing: std::rc::Rc<std::cell::Cell<Option<draw::Tool>>>,
    /// Set while the box is being filled in, not changed by hand.
    filling: std::rc::Rc<std::cell::Cell<bool>>,
}

impl Corners {
    pub fn new() -> Self {
        let caption = gtk::Label::new(None);
        caption.add_css_class("dim-label");
        let spin = gtk::SpinButton::with_range(f64::from(draw::FEWEST_CORNERS), f64::from(draw::MOST_CORNERS), 1.0);
        spin.set_width_chars(3);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.append(&caption);
        row.append(&spin);
        row.set_visible(false);
        let corners = Corners {
            row,
            caption,
            spin,
            sides: std::rc::Rc::new(std::cell::Cell::new(draw::DEFAULT_SIDES)),
            points: std::rc::Rc::new(std::cell::Cell::new(draw::DEFAULT_POINTS)),
            showing: std::rc::Rc::default(),
            filling: std::rc::Rc::default(),
        };
        let kept = corners.clone();
        corners.spin.connect_value_changed(move |spin| {
            if kept.filling.get() {
                return;
            }
            let n = spin.value().round() as u8;
            match kept.showing.get() {
                Some(draw::Tool::Polygon(_)) => kept.sides.set(n),
                Some(draw::Tool::Star(_)) => kept.points.set(n),
                _ => {}
            }
        });
        corners
    }

    /// A polygon or star tool with the count set here; any other as it is.
    pub fn tool(&self, tool: draw::Tool) -> draw::Tool {
        match tool {
            draw::Tool::Polygon(_) => tool.with_corners(self.sides.get()),
            draw::Tool::Star(_) => tool.with_corners(self.points.get()),
            other => other,
        }
    }

    /// Show the box for a polygon or star, with its count; hide it for
    /// anything else.
    pub fn show(&self, tool: Option<draw::Tool>) {
        let tool = tool.filter(|t| t.corners().is_some());
        self.showing.set(tool);
        self.row.set_visible(tool.is_some());
        let Some(tool) = tool else { return };
        self.caption.set_text(if matches!(tool, draw::Tool::Star(_)) { "Points" } else { "Sides" });
        self.spin.set_tooltip_text(Some(if matches!(tool, draw::Tool::Star(_)) {
            "How many points the star has"
        } else {
            "How many sides the polygon has"
        }));
        self.filling.set(true);
        self.spin.set_value(f64::from(tool.corners().unwrap_or(draw::DEFAULT_SIDES)));
        self.filling.set(false);
    }

    /// Called with the count when it is changed by hand.
    pub fn connect_changed(&self, f: impl Fn(u8) + 'static) {
        let filling = self.filling.clone();
        self.spin.connect_value_changed(move |spin| {
            if !filling.get() {
                f(spin.value().round() as u8);
            }
        });
    }
}

pub(crate) fn colour_button(initial: gdk::RGBA) -> crate::app::colour::ColourButton {
    crate::app::colour::ColourButton::new(initial, true)
}

/// A pixel-dimension entry. Wide range, typed or stepped.
pub(crate) fn dimension_spin() -> gtk::SpinButton {
    let spin = gtk::SpinButton::with_range(1.0, 30_000.0, 1.0);
    spin.set_numeric(true);
    spin.set_snap_to_ticks(true);
    // Sized to its digits. Letting it expand drags the whole sidebar wider
    // than the picture it is meant to sit beside.
    spin.set_hexpand(false);
    spin.set_width_chars(5);
    spin.set_max_width_chars(6);
    spin
}

/// One tone slider: centred on zero, with a mark there so the neutral point
/// can be found by feel, and its own number drawn beside it.
pub(crate) fn tone_scale() -> gtk::Scale {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, -adjust::RANGE, adjust::RANGE, 1.0);
    scale.set_draw_value(true);
    scale.set_value_pos(gtk::PositionType::Right);
    scale.set_digits(0);
    scale.add_mark(0.0, gtk::PositionType::Bottom, None);
    // The middle is "untouched", so a bar filling from the far left would
    // read as though something had been done.
    scale.set_has_origin(false);
    scale.add_css_class("tone");
    wheel_scrolls_panel(&scale);
    // A range starting at its minimum would show -100 on a picture nothing had
    // been done to.
    scale.set_value(0.0);
    scale
}

/// Let the mouse wheel scroll the panel past `widget` rather than change it.
///
/// A slider or number box takes the wheel for itself, so scrolling down a
/// panel of them nudges whichever one passes under the pointer: a photo's
/// highlights changed by reading the list. Dragging, clicking and the keys
/// still change it.
pub(crate) fn wheel_scrolls_panel(widget: &impl IsA<gtk::Widget>) {
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = widget.as_ref().downgrade();
    scroll.connect_scroll(move |controller, _, dy| {
        let Some(scroller) = weak
            .upgrade()
            .and_then(|widget| widget.ancestor(gtk::ScrolledWindow::static_type()))
            .and_downcast::<gtk::ScrolledWindow>()
        else {
            return glib::Propagation::Proceed;
        };
        let adjustment = scroller.vadjustment();
        // A wheel's notch moves as far as GTK's own scrolling would; a
        // touchpad's movement is already in pixels.
        let step = match controller.unit() {
            gdk::ScrollUnit::Wheel => adjustment.page_size().powf(2.0 / 3.0),
            _ => 1.0,
        };
        adjustment.set_value(adjustment.value() + dy * step);
        glib::Propagation::Stop
    });
    widget.add_controller(scroll);
}

/// What the width and height boxes count in.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum SizeUnit {
    Pixels,
    Percent,
    Inches,
    Centimetres,
}

impl SizeUnit {
    pub(crate) const ALL: [SizeUnit; 4] = [SizeUnit::Pixels, SizeUnit::Percent, SizeUnit::Inches, SizeUnit::Centimetres];

    fn name(self) -> &'static str {
        match self {
            SizeUnit::Pixels => "pixels",
            SizeUnit::Percent => "percent",
            SizeUnit::Inches => "inches",
            SizeUnit::Centimetres => "cm",
        }
    }

    /// A length on paper rather than a count of pixels.
    fn is_print(self) -> bool {
        matches!(self, SizeUnit::Inches | SizeUnit::Centimetres)
    }

    /// `pixels` in this unit, for a side `natural` pixels long at `dpi`.
    pub(crate) fn in_unit(self, pixels: f64, natural: f64, dpi: f64) -> f64 {
        match self {
            SizeUnit::Pixels => pixels,
            SizeUnit::Percent => pixels / natural.max(1.0) * 100.0,
            SizeUnit::Inches => pixels / dpi,
            SizeUnit::Centimetres => pixels / dpi * 2.54,
        }
    }

    /// The other way: `value` in this unit, as pixels.
    pub(crate) fn pixels(self, value: f64, natural: f64, dpi: f64) -> f64 {
        match self {
            SizeUnit::Pixels => value,
            SizeUnit::Percent => value / 100.0 * natural,
            SizeUnit::Inches => value * dpi,
            SizeUnit::Centimetres => value / 2.54 * dpi,
        }
    }

    /// Range, step and decimals for the boxes.
    fn spin(self) -> (f64, f64, f64, u32) {
        match self {
            SizeUnit::Pixels => (1.0, 30_000.0, 1.0, 0),
            SizeUnit::Percent => (0.1, 10_000.0, 1.0, 1),
            SizeUnit::Inches => (0.01, 10_000.0, 0.1, 2),
            SizeUnit::Centimetres => (0.01, 25_000.0, 0.1, 2),
        }
    }
}

impl Window {
    /// The edit sidebar: tools at the top, output at the bottom.
    pub(crate) fn build_edit_panel(&self) -> &gtk::Box {
        let imp = self.imp();
        let panel = &imp.edit_panel;
        panel.set_width_request(300);
        panel.set_hexpand(false);
        panel.set_margin_top(12);
        panel.set_margin_bottom(12);
        panel.set_margin_start(12);
        panel.set_margin_end(12);
        panel.set_visible(false);

        // The tools live in a scroller. There are enough of them now that an
        // expanded section overflows a short window, and without this GTK
        // squeezes the whole column — sliding every button out from under the
        // pointer that was about to press one.
        let tools = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let scroller = gtk::ScrolledWindow::builder()
            .child(&tools)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .vexpand(true)
            .build();
        panel.append(&scroller);

        let heading = gtk::Label::new(Some("Edit"));
        heading.add_css_class("title-4");
        heading.set_xalign(0.0);
        tools.append(&heading);

        let history_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        history_row.set_homogeneous(true);
        imp.undo_button.set_tooltip_text(Some("Undo (Ctrl+Z)"));
        imp.undo_button.set_action_name(Some("win.undo"));
        imp.undo_button.set_sensitive(false);
        imp.redo_button.set_tooltip_text(Some("Redo (Ctrl+Shift+Z)"));
        imp.redo_button.set_action_name(Some("win.redo"));
        imp.redo_button.set_sensitive(false);
        history_row.append(&imp.undo_button);
        history_row.append(&imp.redo_button);
        tools.append(&history_row);

        // -- rotate and flip --
        let turning = &imp.transform_toggle;
        turning.set_child(Some(
            &adw::ButtonContent::builder()
                .icon_name("object-rotate-right-symbolic")
                .label("Rotate & Flip")
                .build(),
        ));
        turning.set_tooltip_text(Some("Turn or mirror the picture (Ctrl+T)"));
        turning.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                if button.is_active() {
                    window.close_other_sections(button);
                }
                window.imp().rotation_bar.set_visible(button.is_active());
            }
        ));
        tools.append(turning);
        tools.append(self.build_rotation_bar());

        // -- crop tool --
        let crop = &imp.crop_toggle;
        crop.set_tooltip_text(Some("Choose the part of the image to keep"));
        crop.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                if window.imp().syncing_panel.get() {
                    return;
                }
                if button.is_active() {
                    window.close_other_sections(button);
                }
                window.set_cropping(button.is_active());
            }
        ));
        tools.append(crop);

        let options = &imp.crop_options;
        // Hidden until the crop tool is picked, so the panel stays quiet.
        options.set_visible(false);

        let aspect_label = gtk::Label::new(Some("Aspect ratio"));
        aspect_label.add_css_class("dim-label");
        aspect_label.set_xalign(0.0);
        options.append(&aspect_label);

        // The ratios people actually crop to, plus unconstrained.
        let ratios: [(&str, f64); 8] = [
            ("Free", 0.0),
            ("1:1", 1.0),
            ("4:5", 4.0 / 5.0),
            ("5:4", 5.0 / 4.0),
            ("3:2", 3.0 / 2.0),
            ("2:3", 2.0 / 3.0),
            ("16:9", 16.0 / 9.0),
            ("9:16", 9.0 / 16.0),
        ];
        let grid = gtk::FlowBox::new();
        grid.set_selection_mode(gtk::SelectionMode::None);
        grid.set_max_children_per_line(4);
        grid.set_row_spacing(4);
        grid.set_column_spacing(4);
        let mut first: Option<gtk::ToggleButton> = None;
        for (label, ratio) in ratios {
            let button = gtk::ToggleButton::with_label(label);
            match &first {
                // One group, so picking a ratio releases the last one.
                Some(anchor) => button.set_group(Some(anchor)),
                None => {
                    button.set_active(true);
                    first = Some(button.clone());
                }
            }
            button.connect_toggled(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |button| {
                    if !button.is_active() || window.imp().syncing_panel.get() {
                        return;
                    }
                    window.imp().freehand_toggle.set_active(false);
                    window.imp().view.canvas().set_aspect(ratio);
                }
            ));
            grid.append(&button);
        }
        options.append(&grid);

        let freehand = &imp.freehand_toggle;
        freehand.set_tooltip_text(Some("Draw the area to keep"));
        freehand.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                if window.imp().syncing_panel.get() {
                    return;
                }
                window.imp().view.canvas().set_freehand(button.is_active());
            }
        ));
        options.append(freehand);

        imp.crop_size.add_css_class("dim-label");
        imp.crop_size.add_css_class("numeric");
        imp.crop_size.set_xalign(0.0);
        options.append(&imp.crop_size);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.set_homogeneous(true);
        let reset = gtk::Button::with_label("Reset");
        reset.set_action_name(Some("win.crop-reset"));
        let confirm = gtk::Button::with_label("OK");
        confirm.add_css_class("suggested-action");
        confirm.set_action_name(Some("win.crop-apply"));
        actions.append(&reset);
        actions.append(&confirm);
        options.append(&actions);
        tools.append(options);

        // -- resize --
        let sizing = &imp.resize_toggle;
        sizing.set_tooltip_text(Some("Change the size, in pixels, percent or on paper, and the resolution"));
        sizing.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                let open = button.is_active();
                if open {
                    window.close_other_sections(button);
                }
                window.imp().resize_options.set_visible(open);
                // The handles belong to the tool, so they come and go with it.
                window.imp().view.canvas().set_resizing(open);
                if open {
                    window.sync_resize_panel();
                }
            }
        ));
        tools.append(sizing);

        let sizes = &imp.resize_options;
        sizes.set_visible(false);

        // Side by side with the caption above each, so the pair reads as one
        // measurement and the panel keeps its width.
        let fields = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        fields.set_homogeneous(true);
        for (label, spin) in [("Width", &imp.width_spin), ("Height", &imp.height_spin)] {
            let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
            let caption = gtk::Label::new(Some(label));
            caption.add_css_class("dim-label");
            caption.set_xalign(0.0);
            column.append(&caption);
            column.append(spin);
            fields.append(&column);
            wheel_scrolls_panel(spin);
            spin.connect_value_changed(glib::clone!(
                #[weak(rename_to = window)]
                self,
                #[strong]
                label,
                move |_| window.size_typed(label == "Width")
            ));
        }
        sizes.append(&fields);

        // What the two boxes count in.
        let unit_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let unit_label = gtk::Label::new(Some("In"));
        unit_label.add_css_class("dim-label");
        unit_label.set_hexpand(true);
        unit_label.set_xalign(0.0);
        unit_row.append(&unit_label);
        let names: Vec<&str> = SizeUnit::ALL.iter().map(|unit| unit.name()).collect();
        let unit = &imp.resize_unit;
        unit.set_model(Some(&gtk::StringList::new(&names)));
        unit.set_selected(0);
        unit.update_property(&[gtk::accessible::Property::Label("Width and height in")]);
        unit.connect_selected_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.size_unit_changed()
        ));
        unit_row.append(unit);
        sizes.append(&unit_row);

        let keep = &imp.keep_aspect;
        // On by default: stretching a photograph out of shape is almost never
        // what someone reaching for a resize wants. The canvas has to be told
        // separately — setting the box before its handler exists tells nobody,
        // and the handles would then ignore the lock the box is showing.
        keep.set_active(true);
        imp.view.canvas().set_keep_aspect(true);
        keep.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                let keep = button.is_active();
                window.imp().view.canvas().set_keep_aspect(keep);
                // Ticking it should put a stretched picture straight, not just
                // promise to hold the stretch from here on.
                if keep && !window.imp().syncing_panel.get() {
                    window.size_typed(true);
                }
            }
        ));
        sizes.append(keep);

        // Resolution: how many pixels make an inch on paper.
        let resolution_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        resolution_row.set_margin_top(6);
        let resolution_label = gtk::Label::new(Some("Resolution"));
        resolution_label.add_css_class("dim-label");
        resolution_label.set_xalign(0.0);
        resolution_label.set_hexpand(true);
        resolution_row.append(&resolution_label);
        let resolution = &imp.resolution_spin;
        resolution.set_numeric(true);
        resolution.set_width_chars(5);
        resolution.set_tooltip_text(Some("Pixels per inch: how big the picture prints, not how it looks on screen"));
        resolution.update_property(&[gtk::accessible::Property::Label("Resolution, in pixels per inch")]);
        wheel_scrolls_panel(resolution);
        resolution.connect_value_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.resolution_typed()
        ));
        resolution_row.append(resolution);
        let ppi = gtk::Label::new(Some("ppi"));
        ppi.add_css_class("dim-label");
        resolution_row.append(&ppi);
        sizes.append(&resolution_row);

        let resample = &imp.resample;
        resample.set_label(Some("Resample image"));
        resample.set_tooltip_text(Some(
            "On: a new size or resolution changes the number of pixels. \
             Off: every pixel is kept, and only the size on paper and the resolution change",
        ));
        // On, as in every editor: a resize is what the section is for.
        resample.set_active(true);
        resample.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.sync_resize_panel()
        ));
        sizes.append(resample);

        imp.natural_label.add_css_class("dim-label");
        imp.natural_label.set_xalign(0.0);
        imp.natural_label.set_wrap(true);
        sizes.append(&imp.natural_label);

        let size_actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        size_actions.set_homogeneous(true);
        let size_reset = gtk::Button::with_label("Reset");
        size_reset.set_action_name(Some("win.resize-reset"));
        let size_apply = gtk::Button::with_label("Apply");
        size_apply.add_css_class("suggested-action");
        size_apply.set_tooltip_text(Some("Resample the image to this size, so it becomes an undo step"));
        size_apply.set_action_name(Some("win.resize-apply"));
        size_actions.append(&size_reset);
        size_actions.append(&size_apply);
        sizes.append(&size_actions);
        tools.append(sizes);

        // -- tone, and levels --
        self.build_tone_sections(&tools);

        // -- drawing --
        let pens = &imp.draw_toggle;
        pens.set_tooltip_text(Some("Draw on the picture"));
        pens.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                let open = button.is_active();
                if open {
                    window.close_other_sections(button);
                }
                window.imp().draw_options.set_visible(open);
                window.sync_draw_tool();
                if !open {
                    window.imp().view.canvas().set_draw_tool(None);
                }
            }
        ));
        tools.append(pens);

        let strokes = &imp.draw_options;
        strokes.set_visible(false);

        // One tool at a time, so picking one lets the last go.
        let tool_grid = gtk::FlowBox::new();
        tool_grid.set_selection_mode(gtk::SelectionMode::None);
        // Two across: three named buttons side by side made the whole sidebar
        // wider than the picture needed it to be.
        tool_grid.set_min_children_per_line(2);
        tool_grid.set_max_children_per_line(2);
        tool_grid.set_row_spacing(4);
        tool_grid.set_column_spacing(4);
        // Select first: for picking up what has already been drawn.
        let select = &imp.draw_select;
        select.set_child(Some(&draw::ToolIcon::select_face()));
        select.set_tooltip_text(Some("Select a drawing, to move, resize or remove it"));
        select.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.sync_draw_tool()
        ));
        tool_grid.append(select);
        let mut anchor: Option<gtk::ToggleButton> = Some(select.clone());
        for tool in draw::TOOLS {
            let button = gtk::ToggleButton::new();
            // The icon is the mark the tool makes, drawn by the same code that
            // draws it on the picture. Adwaita has no line, rectangle, ellipse
            // or arrow, and the nearest stock icons read as other things.
            let face = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            face.set_halign(gtk::Align::Center);
            face.append(&draw::ToolIcon::new(*tool));
            face.append(&gtk::Label::new(Some(tool.label())));
            button.set_child(Some(&face));
            button.set_tooltip_text(Some(tool.description()));
            match &anchor {
                Some(first) => button.set_group(Some(first)),
                None => anchor = Some(button.clone()),
            }
            button.connect_toggled(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_| window.sync_draw_tool()
            ));
            tool_grid.append(&button);
            imp.draw_tools.borrow_mut().push(button);
        }
        strokes.append(&tool_grid);

        // A kept signature, put down whole and picked up to move into place.
        let sign = crate::app::signatures::button(
            self,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |signature| window.place_signature_on_picture(&signature)
            ),
        );
        strokes.append(&sign);

        let stroke_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let width_caption = gtk::Label::new(Some("Width"));
        width_caption.add_css_class("dim-label");
        imp.draw_width.set_value(6.0);
        imp.draw_width.set_width_chars(4);
        imp.draw_width.set_tooltip_text(Some("Line thickness, in the image's own pixels"));
        imp.draw_width.connect_value_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |spin| {
                let canvas = window.imp().view.canvas();
                canvas.set_draw_width(spin.value());
                if !window.imp().syncing_panel.get() {
                    canvas.restyle_picked(None, Some(spin.value()), None);
                }
            }
        ));
        let colour_caption = gtk::Label::new(Some("Colour"));
        colour_caption.add_css_class("dim-label");
        imp.draw_colour.set_tooltip_text(Some("Colour of the ink"));
        imp.draw_colour.set_title("Ink Colour");
        imp.draw_colour.connect_rgba_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                let canvas = window.imp().view.canvas();
                canvas.set_draw_colour(button.rgba());
                if !window.imp().syncing_panel.get() {
                    canvas.restyle_picked(Some(button.rgba()), None, None);
                }
            }
        ));
        stroke_row.append(&width_caption);
        stroke_row.append(&imp.draw_width);
        stroke_row.append(&colour_caption);
        stroke_row.append(&imp.draw_colour);
        strokes.append(&stroke_row);

        // Inside a rectangle or ellipse. See-through, as it starts, is none.
        let fill_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let fill_caption = gtk::Label::new(Some("Fill"));
        fill_caption.add_css_class("dim-label");
        imp.draw_fill.set_tooltip_text(Some(
            "Inside colour for rectangles and ellipses — set its opacity to zero for none",
        ));
        imp.draw_fill.set_title("Fill Colour");
        imp.draw_fill.connect_rgba_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                let canvas = window.imp().view.canvas();
                canvas.set_draw_fill(button.rgba());
                if !window.imp().syncing_panel.get() {
                    canvas.restyle_picked(None, None, Some(button.rgba()));
                }
            }
        ));
        let fill_note = gtk::Label::new(Some("Shapes with an inside"));
        fill_note.add_css_class("dim-label");
        fill_note.add_css_class("caption");
        fill_row.append(&fill_caption);
        fill_row.append(&imp.draw_fill);
        fill_row.append(&fill_note);
        strokes.append(&fill_row);

        let corners = &imp.draw_corners;
        corners.connect_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |n| {
                // The picked one first: the panel is then refreshed from it.
                window.imp().view.canvas().recorner_picked(n);
                window.sync_draw_tool();
            }
        ));
        strokes.append(&corners.row);

        let draw_actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        draw_actions.set_homogeneous(true);
        let undo_stroke = gtk::Button::with_label("Undo stroke");
        undo_stroke.set_tooltip_text(Some("Take back the last thing laid over the picture (Ctrl+Z)"));
        undo_stroke.set_action_name(Some("win.undo"));
        let clear = gtk::Button::with_label("Clear");
        clear.set_tooltip_text(Some("Remove every stroke"));
        clear.set_action_name(Some("win.draw-clear"));
        draw_actions.append(&undo_stroke);
        draw_actions.append(&clear);
        strokes.append(&draw_actions);

        imp.draw_hint.add_css_class("dim-label");
        imp.draw_hint.set_xalign(0.0);
        imp.draw_hint.set_wrap(true);
        imp.draw_hint.set_text("Pick a tool, then drag on the picture.");
        strokes.append(&imp.draw_hint);
        tools.append(strokes);

        // -- text --
        let text = &imp.text_toggle;
        text.set_tooltip_text(Some("Lay words over the picture"));
        text.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                let open = button.is_active();
                if open {
                    window.close_other_sections(button);
                }
                window.imp().text_options.set_visible(open);
                // The grab handles belong to the tool, so they go with it.
                window.imp().view.canvas().set_text_tool(open);
                if open {
                    window.sync_text_panel();
                }
            }
        ));
        tools.append(text);

        let words = &imp.text_options;
        words.set_visible(false);

        imp.text_entry.set_placeholder_text(Some("Type something"));
        imp.text_entry.connect_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |entry| {
                let content = entry.text().to_string();
                window.edit_text(move |item| item.content = content.clone());
            }
        ));
        words.append(&imp.text_entry);

        imp.text_font.set_level(gtk::FontLevel::Family);
        imp.text_font.set_use_font(true);
        imp.text_font.connect_font_desc_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                let family = button
                    .font_desc()
                    .and_then(|desc| desc.family())
                    .map(|f| f.to_string());
                if let Some(family) = family {
                    window.edit_text(move |item| item.family = family.clone());
                }
            }
        ));
        words.append(&imp.text_font);

        let style_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let size_label = gtk::Label::new(Some("Size"));
        size_label.add_css_class("dim-label");
        imp.text_size.set_value(48.0);
        imp.text_size.set_width_chars(4);
        imp.text_size.set_tooltip_text(Some("Font size, in the image's own pixels"));
        imp.text_size.connect_value_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |spin| {
                let size = spin.value();
                window.edit_text(move |item| item.size = size);
            }
        ));
        style_row.append(&size_label);
        style_row.append(&imp.text_size);

        // B, I and U, drawn as what they do.
        for (button, css, tip) in [
            (&imp.text_bold, "text-bold", "Bold"),
            (&imp.text_italic, "text-italic", "Italic"),
            (&imp.text_underline, "text-underline", "Underline"),
        ] {
            button.add_css_class(css);
            button.set_tooltip_text(Some(tip));
            button.connect_toggled(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_| window.push_text_style()
            ));
            style_row.append(button);
        }
        words.append(&style_row);

        let colour_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for (label, button, tip) in [
            ("Text", &imp.text_colour, "Colour of the letters"),
            ("Behind", &imp.text_background, "Plate behind the letters — set its opacity to zero for none"),
        ] {
            let caption = gtk::Label::new(Some(label));
            caption.add_css_class("dim-label");
            button.set_tooltip_text(Some(tip));
            button.connect_rgba_notify(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_| window.push_text_style()
            ));
            colour_row.append(&caption);
            colour_row.append(button);
        }
        words.append(&colour_row);
        imp.text_colour.set_title("Text Colour");
        imp.text_background.set_title("Background Colour");

        let text_actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        text_actions.set_homogeneous(true);
        let add = gtk::Button::with_label("Add");
        add.add_css_class("suggested-action");
        add.set_action_name(Some("win.text-add"));
        let remove = gtk::Button::with_label("Remove");
        remove.set_action_name(Some("win.text-remove"));
        text_actions.append(&add);
        text_actions.append(&remove);
        words.append(&text_actions);

        imp.text_hint.add_css_class("dim-label");
        imp.text_hint.set_xalign(0.0);
        imp.text_hint.set_wrap(true);
        words.append(&imp.text_hint);
        tools.append(words);

        // -- redaction --
        let redact = &imp.redact_toggle;
        redact.set_tooltip_text(Some("Black out parts of the picture for good"));
        redact.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                let open = button.is_active();
                if open {
                    window.close_other_sections(button);
                }
                window.imp().redact_options.set_visible(open);
                window.sync_draw_tool();
            }
        ));
        tools.append(redact);
        let marking = &imp.redact_options;
        marking.set_visible(false);
        let explain = gtk::Label::new(Some(
            "Drag over what should go. Marked areas stay see-through until you apply them, \
             so you can check what each one covers.",
        ));
        explain.add_css_class("dim-label");
        explain.set_xalign(0.0);
        explain.set_wrap(true);
        marking.append(&explain);
        let redact_actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        redact_actions.set_homogeneous(true);
        let unmark = gtk::Button::with_label("Undo");
        unmark.set_action_name(Some("win.undo"));
        unmark.set_tooltip_text(Some("Take back the last mark (Ctrl+Z)"));
        let apply = gtk::Button::with_label("Apply…");
        apply.add_css_class("suggested-action");
        apply.set_action_name(Some("win.apply-redactions"));
        apply.set_tooltip_text(Some("Black out what is marked, in a copy or the original"));
        redact_actions.append(&unmark);
        redact_actions.append(&apply);
        marking.append(&redact_actions);
        tools.append(marking);

        imp.pending_crop.add_css_class("dim-label");
        imp.pending_crop.set_xalign(0.0);
        imp.pending_crop.set_wrap(true);
        imp.pending_crop.set_visible(false);
        tools.append(&imp.pending_crop);

        // -- output --
        // Writing a copy is a section like the tools, so the sidebar shows one
        // group of controls at a time instead of all of them at once. Only the
        // two ways out stay pinned below.
        let sending = &imp.export_toggle;
        sending.set_tooltip_text(Some("Format, size and quality for a copy"));
        sending.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                if button.is_active() {
                    window.close_other_sections(button);
                }
                window.imp().export_options.set_visible(button.is_active());
            }
        ));
        tools.append(sending);
        let sending_box = &imp.export_options;
        sending_box.set_visible(false);
        tools.append(sending_box);

        // Export writes a copy in another format, at whatever size the resize
        // tool is showing. Grouped with the other writers, above the way out.
        let export_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        // Names only: the caveats go in the line below, where they can be read
        // without making the sidebar wide enough to hold them.
        let formats = gtk::StringList::new(&[]);
        for target in export::TARGETS {
            formats.append(target.label);
        }
        let drop = &imp.format_drop;
        drop.set_model(Some(&formats));
        drop.set_selected(0);
        drop.set_hexpand(true);
        drop.set_tooltip_text(Some(
            "A drawing can be written as pixels, but pixels cannot be written \
             as a drawing — so SVG is not offered here, whatever the original was.",
        ));
        drop.connect_selected_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| {
                window.describe_format();
                window.describe_target_size();
            }
        ));
        // The format goes at the very top of the section so it cannot move:
        // choosing a lossy one adds a caveat and a quality dial below it, and a
        // picker that slides away as you use it is a picker you fight.
        let format_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        format_row.append(drop);
        sending_box.append(&format_row);

        let export_button = gtk::Button::with_label("Export…");
        export_button.set_hexpand(true);
        export_button.set_tooltip_text(Some(
            "Write a copy somewhere else, in the format and at the size chosen \
             here (Ctrl+Shift+S)",
        ));
        export_button.set_action_name(Some("win.export"));
        export_row.append(&export_button);
        // The note goes above the row, not below it. Everything from here down
        // is anchored to the bottom of the panel, so a note appearing when the
        // format changes would otherwise shove the Export button out from under
        // the pointer that just picked the format.
        imp.format_note.add_css_class("dim-label");
        imp.format_note.set_xalign(0.0);
        imp.format_note.set_wrap(true);
        // Both notes go above every control in this block. Everything from
        // here down is anchored to the bottom of the panel, so a note that
        // grows to a second line would otherwise slide the controls above it
        // out from under the pointer. Only the notes themselves move.

        // How hard the lossy formats squeeze, when no size is being aimed at.
        let quality_caption = gtk::Label::new(Some("Quality"));
        quality_caption.add_css_class("dim-label");
        let quality = &imp.quality_scale;
        wheel_scrolls_panel(quality);
        quality.set_draw_value(true);
        quality.set_value_pos(gtk::PositionType::Right);
        quality.set_digits(0);
        quality.set_hexpand(true);
        quality.set_value(f64::from(export::DEFAULT_QUALITY));
        quality.set_tooltip_text(Some(
            "How much detail JPEG, HEIC and JPEG 2000 keep. Higher is a bigger file, and \
             100 is lossless for HEIC and JPEG 2000; the lossless formats ignore it.",
        ));
        imp.quality_row.append(&quality_caption);
        imp.quality_row.append(quality);
        sending_box.append(&imp.quality_row);

        // Aiming at a file size: the thing people otherwise go to an
        // advertising-funded website for.
        imp.size_wanted.set_tooltip_text(Some(
            "Squeeze the file down to this, or pad it up to it if it is smaller",
        ));
        imp.size_wanted.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.describe_target_size()
        ));
        // Visible only once the panel knows the format; set after both exist.
        imp.size_value.set_value(500.0);
        imp.size_value.set_width_chars(5);
        imp.size_value.set_hexpand(true);
        imp.size_value.connect_value_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.describe_target_size()
        ));
        let units = gtk::StringList::new(&["KB", "MB"]);
        imp.size_unit.set_model(Some(&units));
        imp.size_unit.set_selected(0);
        imp.size_unit.connect_selected_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.describe_target_size()
        ));
        for control in [
            imp.size_value.upcast_ref::<gtk::Widget>(),
            imp.size_unit.upcast_ref::<gtk::Widget>(),
        ] {
            imp.size_wanted
                .bind_property("active", control, "sensitive")
                .sync_create()
                .build();
        }
        let size_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        size_row.append(&imp.size_value);
        size_row.append(&imp.size_unit);
        imp.size_note.add_css_class("dim-label");
        imp.size_note.set_xalign(0.0);
        imp.size_note.set_wrap(true);

        sending_box.append(&imp.size_wanted);
        sending_box.append(&size_row);
        sending_box.append(&export_row);
        // Both notes after every control, for the same reason.
        sending_box.append(&imp.format_note);
        sending_box.append(&imp.size_note);
        self.describe_format();
        self.describe_target_size();

        // The way out that keeps nothing. Above the save buttons rather than
        // beside them, so leaving and committing are never one slip apart.
        let cancel = gtk::Button::with_label("Cancel");
        cancel.set_tooltip_text(Some("Leave the editor without saving (Esc)"));
        cancel.set_action_name(Some("win.edit-cancel"));
        panel.append(&cancel);

        // Two ways out with pixels, not three: write back over the original,
        // or write a copy with the format, size and place of your choosing.
        // Export covers everything the old Save As did and more.
        let save = gtk::Button::with_label("Save");
        save.add_css_class("suggested-action");
        save.set_tooltip_text(Some(
            "Write these changes back over the original file (Ctrl+S)",
        ));
        save.set_action_name(Some("win.save"));
        panel.append(&save);

        // Clicking or dragging a piece of text has to reach the panel, the
        // same way typing in the panel reaches the picture.
        imp.view.canvas().connect_text_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move || {
                window.sync_text_panel();
                // A stroke or a line of text is now something undo can take
                // back, so the action has to be told it has work to do.
                window.update_edit_state();
            }
        ));

        imp.view.canvas().connect_picked(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |picked| window.show_picked(picked)
        ));

        // Dragging a handle has to reach the numbers, the same way typing a
        // number reaches the handles.
        imp.view.canvas().connect_resize_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.sync_resize_panel()
        ));

        imp.view.canvas().connect_crop_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |size| {
                window.imp().crop_size.set_text(&match size {
                    Some((w, h)) => format!("Selection: {w} × {h} px"),
                    None => "Draw an area to keep".to_string(),
                });
            }
        ));

        panel
    }

    /// The unit the size boxes are in.
    pub(crate) fn size_unit(&self) -> SizeUnit {
        SizeUnit::ALL
            .get(self.imp().resize_unit.selected() as usize)
            .copied()
            .unwrap_or(SizeUnit::Pixels)
    }

    /// The resolution in effect: chosen here, read from the file, or assumed.
    pub(crate) fn resolution(&self) -> f64 {
        self.imp().dpi.get().unwrap_or(crate::images::resolution::ASSUMED)
    }

    /// A number was typed or stepped: push it at the canvas, which redraws the
    /// picture at that size without resampling anything.
    fn size_typed(&self, width_led: bool) {
        let imp = self.imp();
        if imp.syncing_panel.get() {
            return;
        }
        let canvas = imp.view.canvas();
        let (Some((nw, nh)), Some((tw, th))) = (canvas.natural_size(), canvas.target_size()) else {
            return;
        };
        let (nw, nh) = (f64::from(nw), f64::from(nh));
        let unit = self.size_unit();
        let dpi = self.resolution();

        if !imp.resample.is_active() {
            // The pixels stay as they are, so a length on paper can only
            // change how many of them make an inch.
            if unit.is_print() {
                let typed = if width_led { imp.width_spin.value() } else { imp.height_spin.value() };
                let pixels = f64::from(if width_led { tw } else { th });
                let inches = SizeUnit::Inches.in_unit(unit.pixels(typed, 1.0, 1.0), 1.0, 1.0);
                if inches > 0.0 {
                    // Whole numbers, as files record it, so the size on paper
                    // shown is the size the saved file will say.
                    imp.dpi.set(Some((pixels / inches).round().clamp(1.0, 65_535.0)));
                }
            }
            self.sync_resize_panel();
            return;
        }

        let mut width = unit.pixels(imp.width_spin.value(), nw, dpi).round().max(1.0);
        let mut height = unit.pixels(imp.height_spin.value(), nh, dpi).round().max(1.0);
        if imp.keep_aspect.is_active() {
            let ratio = nw / nh.max(1e-9);
            // Whichever box was touched leads; the other follows.
            if width_led {
                height = (width / ratio).round().max(1.0);
            } else {
                width = (height * ratio).round().max(1.0);
            }
        }
        // Writing the size back would rewrite the box being typed into, under
        // the cursor, so only the other one is touched and the echo from the
        // canvas is suppressed for the duration.
        imp.syncing_panel.set(true);
        canvas.set_target_size(width as u32, height as u32);
        if width_led {
            imp.height_spin.set_value(unit.in_unit(height, nh, dpi));
        } else {
            imp.width_spin.set_value(unit.in_unit(width, nw, dpi));
        }
        imp.syncing_panel.set(false);
        self.describe_size();
    }

    /// The resolution was typed or stepped. Resampling keeps the size on
    /// paper and changes the pixels to fill it; otherwise the pixels stay
    /// and the size on paper changes.
    fn resolution_typed(&self) {
        let imp = self.imp();
        if imp.syncing_panel.get() {
            return;
        }
        let new = imp.resolution_spin.value().max(1.0);
        let old = self.resolution();
        imp.dpi.set(Some(new));
        if imp.resample.is_active() && (new - old).abs() > 1e-9 {
            let canvas = imp.view.canvas();
            if let Some((tw, th)) = canvas.target_size() {
                let scale = new / old;
                let width = (f64::from(tw) * scale).round().clamp(1.0, 30_000.0);
                let height = (f64::from(th) * scale).round().clamp(1.0, 30_000.0);
                canvas.set_target_size(width as u32, height as u32);
            }
        }
        self.sync_resize_panel();
    }

    /// Show the boxes in the newly chosen unit.
    fn size_unit_changed(&self) {
        let (low, high, step, digits) = self.size_unit().spin();
        let imp = self.imp();
        imp.syncing_panel.set(true);
        for spin in [&imp.width_spin, &imp.height_spin] {
            // Snapping counts its steps from the bottom of the range, so
            // only whole pixels can have it: 50% would become 50.1%.
            spin.set_snap_to_ticks(self.size_unit() == SizeUnit::Pixels);
            spin.set_digits(digits);
            spin.set_range(low, high);
            spin.set_increments(step, step * 10.0);
        }
        imp.syncing_panel.set(false);
        self.sync_resize_panel();
    }

    /// One tool at a time. Five sections open at once made a sidebar taller
    /// than the window, and you can only use one of them anyway.
    pub(crate) fn close_other_sections(&self, keep: &gtk::ToggleButton) {
        let imp = self.imp();
        for section in [
            &imp.transform_toggle,
            &imp.crop_toggle,
            &imp.resize_toggle,
            &imp.adjust_toggle,
            &imp.levels_toggle,
            &imp.draw_toggle,
            &imp.text_toggle,
            &imp.redact_toggle,
            &imp.export_toggle,
        ] {
            if section != keep && section.is_active() {
                section.set_active(false);
            }
        }
    }

    /// Hand the canvas whichever tool is pressed in, or none.
    pub(crate) fn sync_draw_tool(&self) {
        let imp = self.imp();
        let chosen = if imp.draw_toggle.is_active() {
            imp.draw_tools
                .borrow()
                .iter()
                .position(|button| button.is_active())
                .and_then(|index| draw::TOOLS.get(index).copied())
                .map(|tool| imp.draw_corners.tool(tool))
        } else if imp.redact_toggle.is_active() {
            Some(draw::Tool::Redact)
        } else {
            None
        };
        let selecting = imp.draw_toggle.is_active() && imp.draw_select.is_active();
        let canvas = imp.view.canvas();
        canvas.set_draw_colour(imp.draw_colour.rgba());
        canvas.set_draw_width(imp.draw_width.value());
        canvas.set_draw_fill(imp.draw_fill.rgba());
        canvas.set_draw_tool(chosen);
        canvas.set_mark_select(selecting);
        self.sync_corners();
        self.sync_draw_hint();
    }

    /// Put a signature on the picture, picked up with the Select tool so it
    /// can be dragged into place and sized at once.
    fn place_signature_on_picture(&self, signature: &std::sync::Arc<crate::images::edit::signature::Signature>) {
        let imp = self.imp();
        imp.draw_select.set_active(true);
        if imp.view.canvas().place_signature(signature) {
            self.sync_draw_hint();
        }
    }

    /// The sides or points box, for the polygon or star in hand or picked up.
    fn sync_corners(&self) {
        let imp = self.imp();
        let canvas = imp.view.canvas();
        let shown = canvas.draw_tool().or_else(|| canvas.picked_mark().map(|mark| mark.tool));
        imp.draw_corners.show(shown.filter(|_| imp.draw_toggle.is_active()));
    }

    /// What the tool in hand does, under the drawing tools.
    pub(crate) fn sync_draw_hint(&self) {
        let imp = self.imp();
        let canvas = imp.view.canvas();
        imp.draw_hint.set_text(match canvas.draw_tool() {
            Some(draw::Tool::Pen | draw::Tool::Highlighter) => "Drag on the picture to draw.",
            Some(_) => "Drag on the picture from one corner to the other.",
            None if canvas.picked_mark().is_some() => {
                "Drag it to move it, or drag a handle to resize it; Shift keeps a corner in proportion. \
                 Width, Colour, Fill and a polygon's sides change it, and Delete removes it."
            }
            None if imp.draw_select.is_active() => "Click a drawing to pick it up.",
            None => "Pick a tool, then drag on the picture.",
        });
    }

    /// A mark picked up shows its own thickness and colour, so changing
    /// them starts from what it has.
    pub(crate) fn show_picked(&self, picked: Option<draw::Mark>) {
        let imp = self.imp();
        if let Some(mark) = picked {
            imp.syncing_panel.set(true);
            imp.draw_width.set_value(mark.width);
            imp.draw_colour.set_rgba(&mark.colour);
            if mark.tool.fillable() {
                let mut none = mark.colour;
                none.set_alpha(0.0);
                imp.draw_fill.set_rgba(&mark.fill.unwrap_or(none));
            }
            imp.syncing_panel.set(false);
        }
        self.sync_corners();
        self.sync_draw_hint();
    }

    /// Change the selected item, unless the panel is only echoing the canvas
    /// back at itself.
    fn edit_text(&self, change: impl FnOnce(&mut crate::images::edit::text::TextItem)) {
        if self.imp().syncing_panel.get() {
            return;
        }
        self.imp().view.canvas().update_selected_text(change);
    }

    /// The three style toggles and the two colours, pushed together: they all
    /// come from the same widgets and it is not worth a closure each.
    fn push_text_style(&self) {
        let imp = self.imp();
        if imp.syncing_panel.get() {
            return;
        }
        let (bold, italic, underline) = (
            imp.text_bold.is_active(),
            imp.text_italic.is_active(),
            imp.text_underline.is_active(),
        );
        let (colour, background) = (imp.text_colour.rgba(), imp.text_background.rgba());
        imp.view.canvas().update_selected_text(move |item| {
            item.bold = bold;
            item.italic = italic;
            item.underline = underline;
            item.colour = colour;
            item.background = background;
        });
    }

    /// Show whichever item is selected. With none, the controls keep whatever
    /// they were set to and become the recipe for the next one added.
    pub(crate) fn sync_text_panel(&self) {
        let imp = self.imp();
        let canvas = imp.view.canvas();
        let selected = canvas.selected_text();
        imp.text_hint.set_text(&match (&selected, canvas.has_text()) {
            (Some(_), _) => "Drag it into place on the picture.".to_string(),
            (None, true) => "Click a piece of text on the picture to select it.".to_string(),
            (None, false) => "Add lays a new line in the middle of the view.".to_string(),
        });
        let Some(item) = selected else {
            self.update_text_actions();
            return;
        };
        imp.syncing_panel.set(true);
        imp.text_entry.set_text(&item.content);
        imp.text_size.set_value(item.size);
        imp.text_bold.set_active(item.bold);
        imp.text_italic.set_active(item.italic);
        imp.text_underline.set_active(item.underline);
        imp.text_colour.set_rgba(&item.colour);
        imp.text_background.set_rgba(&item.background);
        imp.syncing_panel.set(false);
        self.update_text_actions();
    }

    fn update_text_actions(&self) {
        let selected = self.imp().view.canvas().selected_text().is_some();
        if let Some(action) = self
            .lookup_action("text-remove")
            .and_downcast::<gio::SimpleAction>()
        {
            action.set_enabled(selected);
        }
    }

    /// A new item takes its look from whatever the controls currently show, so
    /// adding two in a row gives two that match.
    pub(crate) fn text_from_panel(&self) -> crate::images::edit::text::TextItem {
        let imp = self.imp();
        let content = imp.text_entry.text().to_string();
        crate::images::edit::text::TextItem {
            content: if content.is_empty() {
                "Text".to_string()
            } else {
                content
            },
            size: imp.text_size.value(),
            family: imp
                .text_font
                .font_desc()
                .and_then(|desc| desc.family())
                .map(|f| f.to_string())
                .unwrap_or_else(|| "Sans".to_string()),
            bold: imp.text_bold.is_active(),
            italic: imp.text_italic.is_active(),
            underline: imp.text_underline.is_active(),
            colour: imp.text_colour.rgba(),
            background: imp.text_background.rgba(),
            ..Default::default()
        }
    }

    /// The size an export should aim for, if one was asked for.
    pub(crate) fn size_wanted(&self) -> Option<u64> {
        let imp = self.imp();
        if !imp.size_wanted.is_active() {
            return None;
        }
        let unit = if imp.size_unit.selected() == 1 {
            compress::MB
        } else {
            compress::KB
        };
        Some((imp.size_value.value().max(1.0) as u64).saturating_mul(unit))
    }

    /// The quality to write with, where the format has a dial and nothing
    /// else is choosing it.
    pub(crate) fn quality(&self) -> Option<u8> {
        let imp = self.imp();
        self.chosen_target()
            .filter(|target| target.lossy())
            .map(|_| imp.quality_scale.value().round().clamp(1.0, 100.0) as u8)
    }

    fn chosen_target(&self) -> Option<&'static export::Target> {
        export::TARGETS.get(self.imp().format_drop.selected() as usize)
    }

    /// The dial is only meaningful for a lossy format, and only when a size
    /// target is not already choosing the quality for you.
    fn update_quality_row(&self) {
        let imp = self.imp();
        let lossy = self.chosen_target().map(|t| t.lossy()).unwrap_or(false);
        // Always present, so choosing a format cannot slide the controls
        // below it; just inert when there is no quality to choose.
        imp.quality_scale.set_sensitive(lossy && !imp.size_wanted.is_active());
    }

    /// What aiming at a size will mean for this format, or what the last
    /// attempt achieved.
    pub(crate) fn describe_target_size(&self) {
        let imp = self.imp();
        self.update_quality_row();
        let Some(wanted) = self.size_wanted() else {
            let current = imp
                .current
                .borrow()
                .as_ref()
                .and_then(|path| std::fs::metadata(path).ok())
                .map(|meta| meta.len());
            imp.size_note.set_visible(current.is_some());
            if let Some(size) = current {
                imp.size_note
                    .set_text(&format!("This file is {} on disk.", compress::describe(size)));
            }
            return;
        };
        let lossy = self.chosen_target().map(|target| target.lossy()).unwrap_or(false);
        imp.size_note.set_visible(true);
        imp.size_note.set_text(&if lossy {
            format!(
                "Quality will be dialled to land just under {}, and the picture \
                 scaled down only if quality alone cannot get there.",
                compress::describe(wanted)
            )
        } else {
            format!(
                "This format has no quality dial, so {} can only be reached by \
                 scaling the picture down. JPEG or HEIC will hold more detail at a size.",
                compress::describe(wanted)
            )
        });
    }

    /// What the chosen format will cost the picture, if anything.
    fn describe_format(&self) {
        let imp = self.imp();
        let note = export::TARGETS
            .get(imp.format_drop.selected() as usize)
            .and_then(|target| target.caveat);
        imp.format_note.set_visible(note.is_some());
        imp.format_note.set_text(note.unwrap_or_default());
    }

    /// The lines under the boxes, and whether Reset and Apply have anything
    /// to act on.
    fn describe_size(&self) {
        let imp = self.imp();
        let canvas = imp.view.canvas();
        let (Some((w, h)), Some((nw, nh))) = (canvas.target_size(), canvas.natural_size()) else {
            return;
        };
        let percent = f64::from(w) / f64::from(nw).max(1e-9) * 100.0;
        let mut lines = vec![if (w, h) == (nw, nh) {
            format!("Original size, {nw} × {nh} pixels")
        } else {
            format!("{w} × {h} pixels, from {nw} × {nh} — {percent:.0}% of the width")
        }];
        let dpi = self.resolution();
        let inches = |pixels: u32| f64::from(pixels) / dpi;
        lines.push(format!(
            "Prints at {:.2} × {:.2} in ({:.1} × {:.1} cm)",
            inches(w),
            inches(h),
            inches(w) * 2.54,
            inches(h) * 2.54
        ));
        if imp.dpi.get().is_none() {
            lines.push(format!("The file does not record a resolution, so {dpi:.0} ppi is assumed."));
        }
        let extension = imp
            .current
            .borrow()
            .as_ref()
            .and_then(|path| path.extension())
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_default();
        if self.resolution_changed() && !crate::images::resolution::can_store(&extension) {
            lines.push("This kind of file cannot record a resolution. Export as JPEG, PNG or BMP to keep it.".into());
        } else if self.resolution_changed() && !canvas.has_resize() {
            lines.push("The new resolution is saved with the picture.".into());
        }
        imp.natural_label.set_text(&lines.join("\n"));
        self.update_resize_actions();
    }

    /// Whether the resolution differs from what the file records.
    pub(crate) fn resolution_changed(&self) -> bool {
        let imp = self.imp();
        match (imp.dpi.get(), imp.dpi_file.get()) {
            (Some(now), Some(file)) => (now - file).abs() > 1e-6,
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// Put the canvas's size back into the boxes, after a handle drag or a
    /// bake changed it behind their back.
    pub(crate) fn sync_resize_panel(&self) {
        let imp = self.imp();
        let canvas = imp.view.canvas();
        let (Some((w, h)), Some((nw, nh))) = (canvas.target_size(), canvas.natural_size()) else {
            return;
        };
        let unit = self.size_unit();
        let dpi = self.resolution();
        let resample = imp.resample.is_active();
        imp.syncing_panel.set(true);
        imp.width_spin.set_value(unit.in_unit(f64::from(w), f64::from(nw), dpi));
        imp.height_spin.set_value(unit.in_unit(f64::from(h), f64::from(nh), dpi));
        imp.resolution_spin.set_value(dpi);
        // Without resampling the pixel count is fixed: only a length on paper
        // can be typed, and the shape cannot change.
        let editable = resample || unit.is_print();
        imp.width_spin.set_sensitive(editable);
        imp.height_spin.set_sensitive(editable);
        imp.keep_aspect.set_sensitive(resample);
        imp.syncing_panel.set(false);
        self.describe_size();
    }

    fn update_resize_actions(&self) {
        let resized = self.imp().view.canvas().has_resize();
        for (name, enabled) in [("resize-reset", resized || self.resolution_changed()), ("resize-apply", resized)] {
            if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(enabled);
            }
        }
    }

    /// Enter or leave the interactive crop.
    pub(crate) fn set_cropping(&self, active: bool) {
        let imp = self.imp();
        imp.crop_options.set_visible(active);
        imp.view.canvas().set_cropping(active);
        if active {
            imp.pending_crop.set_visible(false);
        }
        self.update_crop_actions();
    }

    pub(crate) fn update_crop_actions(&self) {
        let cropping = self.imp().view.canvas().is_cropping();
        for name in ["crop-apply", "crop-reset"] {
            if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(cropping);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SizeUnit;

    #[test]
    fn every_unit_converts_there_and_back() {
        for unit in SizeUnit::ALL {
            for pixels in [1.0, 640.0, 4000.0] {
                let value = unit.in_unit(pixels, 4000.0, 300.0);
                assert!((unit.pixels(value, 4000.0, 300.0) - pixels).abs() < 1e-9, "{unit:?}");
            }
        }
    }

    #[test]
    fn units_mean_what_they_say() {
        assert_eq!(SizeUnit::Percent.in_unit(2000.0, 4000.0, 300.0), 50.0);
        assert_eq!(SizeUnit::Inches.in_unit(3000.0, 4000.0, 300.0), 10.0);
        assert!((SizeUnit::Centimetres.in_unit(300.0, 4000.0, 300.0) - 2.54).abs() < 1e-12);
    }
}
