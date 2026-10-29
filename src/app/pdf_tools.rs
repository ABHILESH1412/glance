// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Marking up a PDF: the image editor's pens, shapes and text, for a page,
//! signatures and pictures, and marking areas for redaction.
//!
//! A toolbar floats over the bottom of the page, with the tool in hand's
//! options on a card just above it, so the page keeps the whole width of the
//! window. The tools have the image editor's icons and colour wells, so moving between a picture and a document there is nothing new
//! to learn. Everything made here is saved into the PDF at once, and Ctrl+Z
//! takes it back.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, glib, pango};

use crate::app::window::Window;
use crate::images::edit::draw;
use crate::images::edit::panel::{colour_button, explain};
use crate::pdf;

pub struct PdfTools {
    /// The toolbar and the card of options above it, floating over the
    /// bottom of the page.
    pub root: gtk::Box,
    /// The tools along the toolbar, one in hand at a time.
    select: gtk::ToggleButton,
    draw: gtk::ToggleButton,
    text: gtk::ToggleButton,
    redact: gtk::ToggleButton,
}

/// The text controls, kept together so the chosen box can be shown in them.
#[derive(Clone)]
struct TextControls {
    entry: gtk::Entry,
    font: gtk::FontDialogButton,
    size: gtk::SpinButton,
    bold: gtk::ToggleButton,
    italic: gtk::ToggleButton,
    colour: crate::app::colour::ColourButton,
    background: crate::app::colour::ColourButton,
    remove: gtk::Button,
    hint: gtk::Label,
    /// Set while the controls are being filled from a chosen box, so that
    /// filling them is not taken for the user changing them.
    syncing: Rc<Cell<bool>>,
}

impl TextControls {
    fn style(&self) -> pdf::TextStyle {
        let background = self.background.rgba();
        pdf::TextStyle {
            // Without Poppler's font calls, its own font it is.
            family: pdf::can_style_text()
                .then(|| self.font.font_desc().and_then(|d| d.family()).map(|f| f.to_string()))
                .flatten()
                .or_else(|| pdf::can_style_text().then(|| "Sans".to_string())),
            size: self.size.value(),
            bold: self.bold.is_active(),
            italic: self.italic.is_active(),
            colour: pdf::Rgb::from_rgba(&self.colour.rgba()),
            // A see-through background is none at all.
            fill: (background.alpha() > 0.05).then(|| pdf::Rgb::from_rgba(&background)),
            border: false,
        }
    }

    fn show(&self, chosen: Option<(String, pdf::TextStyle)>) {
        self.remove.set_sensitive(chosen.is_some());
        self.hint.set_text(if chosen.is_some() {
            "Change it here, or drag it on the page to move it."
        } else {
            "Click the page where the text should go, or click a text box to change it."
        });
        let Some((text, style)) = chosen else { return };
        self.syncing.set(true);
        self.entry.set_text(&text);
        if let Some(family) = &style.family {
            self.font.set_font_desc(&pango::FontDescription::from_string(family));
        }
        self.size.set_value(style.size);
        self.bold.set_active(style.bold);
        self.italic.set_active(style.italic);
        self.colour.set_rgba(&style.colour.to_rgba());
        let mut fill = style.fill.map_or(gdk::RGBA::new(1.0, 1.0, 1.0, 0.0), |c| c.to_rgba());
        if style.fill.is_none() {
            fill.set_alpha(0.0);
        }
        self.background.set_rgba(&fill);
        self.syncing.set(false);
    }
}

impl PdfTools {
    pub fn new(window: &Window) -> Self {
        // -- the toolbar's tools: one in hand at a time --
        let select = tool_toggle(draw::SELECT_ICON, "Select", "Pick up a drawing or picture, to move, resize or remove it");
        let draw_toggle = tool_toggle(draw::Tool::Pen.icon_name(), "Draw", "Pens and shapes on the page; the arrow beside it picks which");
        let text_toggle = tool_toggle("glance-text-symbolic", "Text", "Write on the page");
        let redact_toggle = tool_toggle("glance-redact-symbolic", "Redact", "Black out parts of the document for good");
        for other in [&draw_toggle, &text_toggle, &redact_toggle] {
            other.set_group(Some(&select));
        }

        // -- drawing: which pen or shape, in a menu beside the Draw button --
        let tool_grid = gtk::FlowBox::new();
        tool_grid.set_selection_mode(gtk::SelectionMode::None);
        tool_grid.set_min_children_per_line(2);
        tool_grid.set_max_children_per_line(2);
        tool_grid.set_row_spacing(4);
        tool_grid.set_column_spacing(4);
        let mut tools: Vec<(draw::Tool, gtk::ToggleButton)> = Vec::new();
        for tool in draw::TOOLS {
            let button = gtk::ToggleButton::new();
            button.set_child(Some(&draw::tool_face(tool.icon_name(), tool.label())));
            button.set_tooltip_text(Some(tool.description()));
            button.add_css_class("flat");
            if let Some((_, first)) = tools.first() {
                button.set_group(Some(first));
            }
            tool_grid.append(&button);
            tools.push((*tool, button));
        }
        if let Some((_, pen)) = tools.first() {
            pen.set_active(true);
        }
        let tool_popover = gtk::Popover::builder().child(&tool_grid).build();
        let choose = gtk::MenuButton::builder()
            .popover(&tool_popover)
            .icon_name("glance-choose-symbolic")
            .direction(gtk::ArrowType::Up)
            .build();
        explain(&choose, "Pens and Shapes", "Choose what Draw puts on the page");
        choose.add_css_class("flat");
        choose.add_css_class("tool-choice");
        let drawing = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        drawing.append(&draw_toggle);
        drawing.append(&choose);
        drawing.set_sensitive(pdf::can_draw());

        // A kept signature, put down whole and picked up to move.
        let sign = {
            let (weak, select) = (window.downgrade(), select.clone());
            crate::app::signatures::button(window, move |signature| {
                let Some(window) = weak.upgrade() else { return };
                select.set_active(true);
                window.imp().pdf_view.place_signature(&signature);
            })
        };
        sign.set_icon_name("glance-signature-symbolic");
        sign.set_always_show_arrow(false);
        explain(&sign, "Signature", "Put a signature down, or write a new one");
        sign.add_css_class("flat");
        // Written signatures need ink; ones from a picture, only pictures.
        sign.set_sensitive(pdf::can_draw() || pdf::can_place_pictures());
        // A picture from a file, put down whole and picked up to move.
        let picture = icon_button("glance-picture-symbolic", "Picture", "Add a picture from a file, to move and size on the page", "win.add-picture");
        picture.set_sensitive(pdf::can_place_pictures());

        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        for class in ["osd", "viewer-pill", "markup-bar"] {
            toolbar.add_css_class(class);
        }
        toolbar.set_halign(gtk::Align::Center);
        toolbar.append(&select);
        toolbar.append(&drawing);
        toolbar.append(&text_toggle);
        toolbar.append(&redact_toggle);
        toolbar.append(&divider());
        toolbar.append(&sign);
        toolbar.append(&picture);
        toolbar.append(&divider());
        toolbar.append(&icon_button("glance-undo-symbolic", "Undo", "Take back the last mark (Ctrl+Z)", "win.undo-mark"));
        toolbar.append(&icon_button("glance-redo-symbolic", "Redo", "Put back what was undone (Ctrl+Shift+Z)", "win.redo-mark"));
        toolbar.append(&divider());
        toolbar.append(&icon_button("glance-cross-symbolic", "Done", "Put the markup tools away (Ctrl+E)", "win.draw-panel"));

        // -- drawing's options --
        let strokes = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let width = gtk::SpinButton::with_range(0.5, 50.0, 0.5);
        width.set_digits(1);
        width.set_value(3.0);
        width.set_width_chars(4);
        width.set_tooltip_text(Some("Line thickness, in points"));
        let ink = colour_button(gdk::RGBA::new(0.9, 0.15, 0.15, 1.0));
        ink.set_tooltip_text(Some("Colour of the ink"));
        ink.set_title("Ink Colour");
        // Inside a rectangle or ellipse. See-through, as it starts, is none.
        let fill = colour_button(gdk::RGBA::new(0.9, 0.15, 0.15, 0.0));
        fill.set_tooltip_text(Some("Inside colour for rectangles and ellipses — set its opacity to zero for none"));
        fill.set_title("Fill Colour");
        let stroke_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for (caption, widget) in [("Width", width.upcast_ref::<gtk::Widget>()), ("Colour", ink.upcast_ref())] {
            stroke_row.append(&caption_label(caption));
            stroke_row.append(widget);
        }
        let fill_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        fill_row.append(&caption_label("Fill"));
        fill_row.append(&fill);
        fill_row.set_sensitive(pdf::can_draw());
        // A polygon's sides and a star's points.
        let corners = crate::images::edit::panel::Corners::new();
        let ink_row = gtk::Box::new(gtk::Orientation::Horizontal, 18);
        ink_row.append(&stroke_row);
        ink_row.append(&fill_row);
        ink_row.append(&corners.row);
        strokes.append(&ink_row);
        let draw_hint = hint(if pdf::can_draw() {
            "Pick a tool, then drag on the page."
        } else {
            "Drawing on a PDF needs Poppler 25.06 or newer."
        });
        stroke_row.set_sensitive(pdf::can_draw());
        strokes.append(&draw_hint);
        // Set while the controls are being filled from a picked drawing.
        let showing_picked = Rc::new(Cell::new(false));

        // -- text's options --
        let words = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let controls = TextControls {
            entry: gtk::Entry::builder().placeholder_text("Type something").hexpand(true).build(),
            font: gtk::FontDialogButton::new(Some(gtk::FontDialog::new())),
            size: gtk::SpinButton::with_range(4.0, 200.0, 1.0),
            bold: gtk::ToggleButton::with_label("B"),
            italic: gtk::ToggleButton::with_label("I"),
            colour: colour_button(gdk::RGBA::BLACK),
            background: colour_button(gdk::RGBA::new(1.0, 1.0, 1.0, 0.0)),
            remove: gtk::Button::with_label("Remove"),
            hint: hint(""),
            syncing: Rc::new(Cell::new(false)),
        };
        controls.colour.set_title("Text Colour");
        controls.background.set_title("Background Colour");
        controls.font.set_level(gtk::FontLevel::Family);
        controls.font.set_use_font(true);
        controls.font.set_font_desc(&pango::FontDescription::from_string("Sans"));
        controls.size.set_value(18.0);
        controls.size.set_width_chars(4);
        controls.size.set_tooltip_text(Some("Font size, in points"));
        let add = gtk::Button::with_label("Add");
        add.add_css_class("suggested-action");
        add.set_tooltip_text(Some("Put a text box near the top of this page"));
        controls.remove.set_tooltip_text(Some("Delete the chosen text box"));
        controls.remove.set_sensitive(false);
        let typing_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        typing_row.append(&controls.entry);
        typing_row.append(&add);
        typing_row.append(&controls.remove);
        words.append(&typing_row);
        let style_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        style_row.append(&controls.font);
        style_row.append(&caption_label("Size"));
        style_row.append(&controls.size);
        let emphasis = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        emphasis.add_css_class("linked");
        for (button, css, tip) in [(&controls.bold, "text-bold", "Bold"), (&controls.italic, "text-italic", "Italic")] {
            button.add_css_class(css);
            button.set_tooltip_text(Some(tip));
            emphasis.append(button);
        }
        style_row.append(&emphasis);
        let colour_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for (caption, button, tip) in [
            ("Text", &controls.colour, "Colour of the letters"),
            ("Behind", &controls.background, "Box behind the letters — set its opacity to zero for none"),
        ] {
            button.set_tooltip_text(Some(tip));
            colour_row.append(&caption_label(caption));
            colour_row.append(button);
        }
        let look_row = gtk::Box::new(gtk::Orientation::Horizontal, 18);
        look_row.append(&style_row);
        look_row.append(&colour_row);
        words.append(&look_row);
        // Without Poppler's font calls only the words themselves can be set.
        for widget in [controls.font.upcast_ref::<gtk::Widget>(), style_row.upcast_ref(), controls.colour.upcast_ref()] {
            widget.set_sensitive(pdf::can_style_text());
        }
        controls.show(None);
        if !pdf::can_style_text() {
            controls.hint.set_text("Fonts, sizes and colours for text need Poppler 24.12 or newer.");
        }
        words.append(&controls.hint);

        // -- redaction's options --
        let marking = gtk::Box::new(gtk::Orientation::Vertical, 6);
        // Two ways to mark: a box over anything, or text as it is selected.
        let by_area = gtk::ToggleButton::with_label("Area");
        by_area.set_tooltip_text(Some("Drag a box over anything: text, pictures, drawings"));
        let by_text = gtk::ToggleButton::with_label("Text");
        by_text.set_tooltip_text(Some("Select text to mark it, word by word"));
        by_text.set_group(Some(&by_area));
        by_area.set_active(true);
        let ways = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        ways.add_css_class("linked");
        ways.append(&by_area);
        ways.append(&by_text);
        let unmark = gtk::Button::with_label("Unmark All");
        unmark.set_action_name(Some("win.unredact-all"));
        unmark.set_tooltip_text(Some("Take every mark away"));
        let apply = gtk::Button::with_label("Apply…");
        apply.add_css_class("suggested-action");
        apply.set_action_name(Some("win.apply-redactions"));
        apply.set_tooltip_text(Some("Black out what is marked, in a copy or the original"));
        let redact_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        ways.set_hexpand(true);
        ways.set_halign(gtk::Align::Start);
        redact_row.append(&ways);
        redact_row.append(&unmark);
        redact_row.append(&apply);
        marking.append(&redact_row);
        let redact_hint = hint("Drag over what should go.");
        marking.append(&redact_hint);
        marking.append(&hint("Marked areas stay see-through, so you can check them. Nothing is blacked out until you apply them."));

        // -- the card above the toolbar: the options of the tool in hand --
        let options = gtk::Stack::new();
        options.set_hhomogeneous(false);
        options.set_vhomogeneous(false);
        options.set_interpolate_size(true);
        options.set_transition_type(gtk::StackTransitionType::Crossfade);
        options.add_named(&strokes, Some("draw"));
        options.add_named(&words, Some("text"));
        options.add_named(&marking, Some("redact"));
        options.add_css_class("markup-options");
        // Wide enough for the text's options; the hints wrap to it.
        options.set_width_request(420);
        let card = gtk::Revealer::builder()
            .child(&options)
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .transition_duration(150)
            .reveal_child(false)
            .halign(gtk::Align::Center)
            .build();

        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.set_halign(gtk::Align::Center);
        root.set_valign(gtk::Align::End);
        root.set_margin_bottom(12);
        root.set_margin_start(12);
        root.set_margin_end(12);
        root.append(&card);
        root.append(&toolbar);
        root.set_visible(false);

        // -- wiring --
        let tools = Rc::new(tools);
        let push_tool = {
            let window = window.downgrade();
            let (draw_toggle, text_toggle, tools) = (draw_toggle.clone(), text_toggle.clone(), tools.clone());
            let (redact_toggle, by_text) = (redact_toggle.clone(), by_text.clone());
            let (width, ink, fill, controls) = (width.clone(), ink.clone(), fill.clone(), controls.clone());
            let corners = corners.clone();
            Rc::new(move || {
                let Some(window) = window.upgrade() else { return };
                let view = &window.imp().pdf_view;
                view.set_ink(pdf::Ink { colour: ink.rgba(), width: width.value(), fill: fill.rgba() });
                let tool = if draw_toggle.is_active() {
                    tools
                        .iter()
                        .find(|(_, b)| b.is_active())
                        .map_or(pdf::Tool::Select, |(t, _)| pdf::Tool::Draw(corners.tool(*t)))
                } else if text_toggle.is_active() {
                    pdf::Tool::Text
                } else if redact_toggle.is_active() && by_text.is_active() {
                    pdf::Tool::RedactText
                } else if redact_toggle.is_active() {
                    pdf::Tool::Draw(draw::Tool::Redact)
                } else {
                    pdf::Tool::Select
                };
                view.set_tool(tool);
                corners.show(match tool {
                    pdf::Tool::Draw(tool) => Some(tool),
                    _ => None,
                });
                if tool == pdf::Tool::Text {
                    view.set_text_look(controls.entry.text().to_string(), controls.style());
                }
            })
        };
        // The card shows what the tool in hand can be told; Select picks a
        // drawing up to change its ink, so it has the drawing's options.
        let toggles = [(select.clone(), "draw"), (draw_toggle.clone(), "draw"), (text_toggle.clone(), "text"), (redact_toggle.clone(), "redact")];
        for (toggle, page) in &toggles {
            let all: Vec<gtk::ToggleButton> = toggles.iter().map(|(t, _)| t.clone()).collect();
            let (options, card, push_tool, page) = (options.clone(), card.clone(), push_tool.clone(), *page);
            toggle.connect_toggled(move |toggle| {
                if toggle.is_active() {
                    options.set_visible_child_name(page);
                }
                card.set_reveal_child(all.iter().any(|t| t.is_active()));
                push_tool();
            });
        }
        // Picking a pen or shape puts it in hand, on the Draw button's face.
        for (tool, button) in tools.iter() {
            let (tool, draw_toggle, popover) = (*tool, draw_toggle.clone(), tool_popover.clone());
            button.connect_toggled(move |button| {
                if button.is_active() {
                    draw_toggle.set_icon_name(tool.icon_name());
                    draw_toggle.set_active(true);
                    popover.popdown();
                }
            });
        }
        let pick_hint = {
            let (draw_hint, select) = (draw_hint.clone(), select.clone());
            Rc::new(move |picked: bool, picture: bool| {
                if !pdf::can_draw() && !picture {
                    return;
                }
                draw_hint.set_text(if picture {
                    "Drag it to move it, or drag a handle to resize it; Shift lets a corner stretch it. \
                     Delete removes it."
                } else if picked {
                    "Drag it to move it, or drag a handle to resize it; Shift keeps a corner in proportion. \
                     Width, Colour and Fill change it, and Delete removes it."
                } else if select.is_active() {
                    "Click a drawing to pick it up."
                } else {
                    "Pick a tool, then drag on the page."
                });
            })
        };
        for button in tools.iter().map(|(_, b)| b).chain([&select]) {
            let (push_tool, pick_hint) = (push_tool.clone(), pick_hint.clone());
            button.connect_toggled(move |_| {
                push_tool();
                pick_hint(false, false);
            });
        }
        {
            let push_tool = push_tool.clone();
            by_text.connect_toggled(move |by_text| {
                redact_hint.set_text(if by_text.is_active() {
                    "Select text to mark it. A word touched anywhere is redacted whole."
                } else {
                    "Drag over what should go."
                });
                push_tool();
            });
        }
        {
            let push_tool = push_tool.clone();
            corners.connect_changed(move |_| push_tool());
        }
        // The pen's colour and thickness, and the picked drawing's too.
        let reink = {
            let window = window.downgrade();
            let (width, ink, fill, showing_picked) = (width.clone(), ink.clone(), fill.clone(), showing_picked.clone());
            Rc::new(move || {
                if showing_picked.get() {
                    return;
                }
                if let Some(window) = window.upgrade() {
                    window.imp().pdf_view.reink_picked(pdf::Ink { colour: ink.rgba(), width: width.value(), fill: fill.rgba() });
                }
            })
        };
        {
            let (push_tool, reink) = (push_tool.clone(), reink.clone());
            width.connect_value_changed(move |_| {
                push_tool();
                reink();
            });
        }
        {
            let (push_tool, reink) = (push_tool.clone(), reink.clone());
            ink.connect_rgba_notify(move |_| {
                push_tool();
                reink();
            });
        }
        {
            let (push_tool, reink) = (push_tool.clone(), reink.clone());
            fill.connect_rgba_notify(move |_| {
                push_tool();
                reink();
            });
        }
        {
            let (width, ink, fill) = (width.clone(), ink.clone(), fill.clone());
            let weak = window.downgrade();
            window.imp().pdf_view.connect_picked(move |picked| {
                // A picture has no ink: the controls keep what they had.
                let picture = weak.upgrade().is_some_and(|window| window.imp().pdf_view.picked_is_picture());
                if let Some(picked) = picked.filter(|_| !picture) {
                    showing_picked.set(true);
                    width.set_value(picked.width);
                    ink.set_rgba(&picked.colour);
                    fill.set_rgba(&picked.fill);
                    showing_picked.set(false);
                }
                pick_hint(picked.is_some(), picture);
            });
        }

        // Any change to the text controls: the next box looks like this, and
        // the chosen one becomes it.
        let push_look = {
            let window = window.downgrade();
            let controls = controls.clone();
            Rc::new(move || {
                if controls.syncing.get() {
                    return;
                }
                if let Some(window) = window.upgrade() {
                    window.imp().pdf_view.set_text_look(controls.entry.text().to_string(), controls.style());
                }
            })
        };
        {
            let push_look = push_look.clone();
            controls.entry.connect_changed(move |_| push_look());
        }
        {
            let push_look = push_look.clone();
            controls.font.connect_font_desc_notify(move |_| push_look());
        }
        {
            let push_look = push_look.clone();
            controls.size.connect_value_changed(move |_| push_look());
        }
        for button in [&controls.bold, &controls.italic] {
            let push_look = push_look.clone();
            button.connect_toggled(move |_| push_look());
        }
        for button in [&controls.colour, &controls.background] {
            let push_look = push_look.clone();
            button.connect_rgba_notify(move |_| push_look());
        }
        add.connect_clicked(glib::clone!(
            #[weak]
            window,
            move |_| window.imp().pdf_view.add_text_box()
        ));
        controls.remove.connect_clicked(glib::clone!(
            #[weak]
            window,
            move |_| window.imp().pdf_view.remove_chosen()
        ));
        let shown = controls.clone();
        window.imp().pdf_view.connect_chosen(move |chosen| shown.show(chosen));

        PdfTools { root, select, draw: draw_toggle, text: text_toggle, redact: redact_toggle }
    }

    /// The Select tool in hand, for moving and sizing what was just put on
    /// the page.
    pub fn hold_select(&self) {
        self.select.set_active(true);
    }

    /// Show the toolbar, or put it away; put away, a press on the page
    /// selects text again.
    pub fn set_open(&self, open: bool) {
        self.root.set_visible(open);
        let tools = [&self.select, &self.draw, &self.text, &self.redact];
        if !open {
            for tool in tools {
                tool.set_active(false);
            }
        } else if !tools.iter().any(|tool| tool.is_active()) {
            // Opened to draw, most likely: start with the pen in hand, or
            // with Select where drawing is not to be had.
            if self.draw.is_sensitive() && pdf::can_draw() {
                self.draw.set_active(true);
            } else {
                self.select.set_active(true);
            }
        }
    }
}

fn hint(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("dim-label");
    label.set_xalign(0.0);
    label.set_wrap(true);
    // The card is as wide as its controls; the words fit to that.
    label.set_max_width_chars(60);
    label
}

fn caption_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("dim-label");
    label
}

/// A tool on the toolbar: held until another is taken up.
fn tool_toggle(icon: &str, name: &str, what: &str) -> gtk::ToggleButton {
    let button = gtk::ToggleButton::builder().icon_name(icon).build();
    button.add_css_class("flat");
    explain(&button, name, what);
    button
}

/// A button on the toolbar that does something once.
fn icon_button(icon: &str, name: &str, what: &str, action: &str) -> gtk::Button {
    let button = gtk::Button::builder().icon_name(icon).action_name(action).build();
    button.add_css_class("flat");
    explain(&button, name, what);
    button
}

/// A thin line between the toolbar's groups.
fn divider() -> gtk::Separator {
    let line = gtk::Separator::new(gtk::Orientation::Vertical);
    line.set_margin_top(6);
    line.set_margin_bottom(6);
    line.set_margin_start(4);
    line.set_margin_end(4);
    line
}
