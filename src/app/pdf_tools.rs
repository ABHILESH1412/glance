// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Edit panel beside a PDF: the image editor's pens, shapes and text,
//! for a page, and marking areas for redaction.
//!
//! Laid out like the image editor's own sections, with the same tool icons
//! and colour wells, so moving between a picture and a document there is
//! nothing new to learn. Everything made here is saved into the PDF at once,
//! and Ctrl+Z takes it back.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, glib, pango};

use crate::app::window::Window;
use crate::images::edit::draw;
use crate::images::edit::panel::{colour_button, section_toggle};
use crate::pdf;

pub struct PdfTools {
    pub root: gtk::ScrolledWindow,
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
        let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
        column.set_margin_top(12);
        column.set_margin_bottom(12);
        column.set_margin_start(12);
        column.set_margin_end(12);

        let draw_toggle = section_toggle("applications-graphics-symbolic", "Draw");
        draw_toggle.set_tooltip_text(Some("Draw on the page"));
        let text_toggle = section_toggle("insert-text-symbolic", "Text");
        text_toggle.set_tooltip_text(Some("Write on the page"));
        let redact_toggle = section_toggle("view-conceal-symbolic", "Redact");
        redact_toggle.set_tooltip_text(Some("Black out parts of the document for good"));

        // -- drawing --
        let strokes = gtk::Box::new(gtk::Orientation::Vertical, 6);
        strokes.set_visible(false);
        let tool_grid = gtk::FlowBox::new();
        tool_grid.set_selection_mode(gtk::SelectionMode::None);
        tool_grid.set_max_children_per_line(2);
        tool_grid.set_row_spacing(4);
        tool_grid.set_column_spacing(4);
        let mut tools: Vec<(draw::Tool, gtk::ToggleButton)> = Vec::new();
        for tool in draw::TOOLS {
            let button = gtk::ToggleButton::new();
            let face = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            face.set_halign(gtk::Align::Center);
            face.append(&draw::ToolIcon::new(*tool));
            face.append(&gtk::Label::new(Some(tool.label())));
            button.set_child(Some(&face));
            button.set_tooltip_text(Some(tool.label()));
            if let Some((_, first)) = tools.first() {
                button.set_group(Some(first));
            }
            tool_grid.append(&button);
            tools.push((*tool, button));
        }
        if let Some((_, pen)) = tools.first() {
            pen.set_active(true);
        }
        strokes.append(&tool_grid);

        let width = gtk::SpinButton::with_range(0.5, 50.0, 0.5);
        width.set_digits(1);
        width.set_value(3.0);
        width.set_width_chars(4);
        width.set_tooltip_text(Some("Line thickness, in points"));
        let ink = colour_button(gdk::RGBA::new(0.9, 0.15, 0.15, 1.0));
        ink.set_tooltip_text(Some("Colour of the ink"));
        ink.set_title("Ink Colour");
        let stroke_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for (caption, widget) in [("Width", width.upcast_ref::<gtk::Widget>()), ("Colour", ink.upcast_ref())] {
            let label = gtk::Label::new(Some(caption));
            label.add_css_class("dim-label");
            stroke_row.append(&label);
            stroke_row.append(widget);
        }
        strokes.append(&stroke_row);
        strokes.append(&history_buttons());
        let draw_hint = hint(if pdf::can_draw() {
            "Pick a tool, then drag on the page."
        } else {
            "Drawing on a PDF needs Poppler 25.06 or newer."
        });
        tool_grid.set_sensitive(pdf::can_draw());
        stroke_row.set_sensitive(pdf::can_draw());
        strokes.append(&draw_hint);

        // -- text --
        let words = gtk::Box::new(gtk::Orientation::Vertical, 6);
        words.set_visible(false);
        let controls = TextControls {
            entry: gtk::Entry::builder().placeholder_text("Type something").build(),
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
        words.append(&controls.entry);
        words.append(&controls.font);
        let style_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let size_label = gtk::Label::new(Some("Size"));
        size_label.add_css_class("dim-label");
        style_row.append(&size_label);
        style_row.append(&controls.size);
        for (button, css, tip) in [(&controls.bold, "text-bold", "Bold"), (&controls.italic, "text-italic", "Italic")] {
            button.add_css_class(css);
            button.set_tooltip_text(Some(tip));
            style_row.append(button);
        }
        words.append(&style_row);
        let colour_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for (caption, button, tip) in [
            ("Text", &controls.colour, "Colour of the letters"),
            ("Behind", &controls.background, "Box behind the letters — set its opacity to zero for none"),
        ] {
            let label = gtk::Label::new(Some(caption));
            label.add_css_class("dim-label");
            button.set_tooltip_text(Some(tip));
            colour_row.append(&label);
            colour_row.append(button);
        }
        words.append(&colour_row);
        // Without Poppler's font calls only the words themselves can be set.
        for widget in [controls.font.upcast_ref::<gtk::Widget>(), style_row.upcast_ref(), controls.colour.upcast_ref()] {
            widget.set_sensitive(pdf::can_style_text());
        }
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.set_homogeneous(true);
        let add = gtk::Button::with_label("Add");
        add.add_css_class("suggested-action");
        add.set_tooltip_text(Some("Put a text box near the top of this page"));
        controls.remove.set_tooltip_text(Some("Delete the chosen text box"));
        controls.remove.set_sensitive(false);
        actions.append(&add);
        actions.append(&controls.remove);
        words.append(&actions);
        controls.show(None);
        if !pdf::can_style_text() {
            controls.hint.set_text("Fonts, sizes and colours for text need Poppler 24.12 or newer.");
        }
        words.append(&controls.hint);

        // -- redaction --
        let marking = gtk::Box::new(gtk::Orientation::Vertical, 6);
        marking.set_visible(false);
        // Two ways to mark: a box over anything, or text as it is selected.
        let by_area = gtk::ToggleButton::with_label("Area");
        by_area.set_tooltip_text(Some("Drag a box over anything: text, pictures, drawings"));
        let by_text = gtk::ToggleButton::with_label("Text");
        by_text.set_tooltip_text(Some("Select text to mark it, word by word"));
        by_text.set_group(Some(&by_area));
        by_area.set_active(true);
        let ways = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        ways.add_css_class("linked");
        ways.set_homogeneous(true);
        ways.append(&by_area);
        ways.append(&by_text);
        marking.append(&ways);
        let redact_hint = hint("Drag over what should go.");
        marking.append(&redact_hint);
        let redact_actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        redact_actions.set_homogeneous(true);
        let unmark = gtk::Button::with_label("Unmark All");
        unmark.set_action_name(Some("win.unredact-all"));
        unmark.set_tooltip_text(Some("Take every mark away"));
        let apply = gtk::Button::with_label("Apply…");
        apply.add_css_class("suggested-action");
        apply.set_action_name(Some("win.apply-redactions"));
        apply.set_tooltip_text(Some("Black out what is marked, in a copy or the original"));
        redact_actions.append(&unmark);
        redact_actions.append(&apply);
        marking.append(&redact_actions);
        marking.append(&hint("Marked areas stay see-through, so you can check them. Nothing is blacked out until you apply them."));

        // Its name and a way out, now that nothing in the header opens it.
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let title = gtk::Label::builder().label("Edit").xalign(0.0).hexpand(true).css_classes(["heading"]).build();
        let close = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Close (Ctrl+E)")
            .action_name("win.draw-panel")
            .css_classes(["flat", "circular"])
            .build();
        top.append(&title);
        top.append(&close);
        column.append(&top);
        column.append(&draw_toggle);
        column.append(&strokes);
        column.append(&text_toggle);
        column.append(&words);
        column.append(&redact_toggle);
        column.append(&marking);
        // One width whichever section is open, or the page would be refitted,
        // and jump, every time the other one is opened.
        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .width_request(300)
            // The title row stretches to push the close button right; said
            // here, so that stretch stays inside the panel instead of taking
            // half the window.
            .hexpand(false)
            .child(&column)
            .visible(false)
            .build();

        // -- wiring --
        let tools = Rc::new(tools);
        let push_tool = {
            let window = window.downgrade();
            let (draw_toggle, text_toggle, tools) = (draw_toggle.clone(), text_toggle.clone(), tools.clone());
            let (redact_toggle, by_text) = (redact_toggle.clone(), by_text.clone());
            let (width, ink, controls) = (width.clone(), ink.clone(), controls.clone());
            Rc::new(move || {
                let Some(window) = window.upgrade() else { return };
                let view = &window.imp().pdf_view;
                view.set_ink(ink.rgba(), width.value());
                let tool = if draw_toggle.is_active() {
                    tools.iter().find(|(_, b)| b.is_active()).map_or(pdf::Tool::Select, |(t, _)| pdf::Tool::Draw(*t))
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
                if tool == pdf::Tool::Text {
                    view.set_text_look(controls.entry.text().to_string(), controls.style());
                }
            })
        };
        let sections = [(draw_toggle.clone(), strokes.clone()), (text_toggle.clone(), words.clone()), (redact_toggle.clone(), marking.clone())];
        for (toggle, options) in &sections {
            let others: Vec<gtk::ToggleButton> =
                sections.iter().map(|(t, _)| t.clone()).filter(|t| t != toggle).collect();
            let (options, push_tool) = (options.clone(), push_tool.clone());
            toggle.connect_toggled(move |toggle| {
                // One section open at a time, as in the image editor.
                if toggle.is_active() {
                    for other in others.iter().filter(|o| o.is_active()) {
                        other.set_active(false);
                    }
                }
                options.set_visible(toggle.is_active());
                push_tool();
            });
        }
        for (_, button) in tools.iter() {
            let push_tool = push_tool.clone();
            button.connect_toggled(move |_| push_tool());
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
            width.connect_value_changed(move |_| push_tool());
        }
        {
            let push_tool = push_tool.clone();
            ink.connect_rgba_notify(move |_| push_tool());
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

        PdfTools { root, draw: draw_toggle, text: text_toggle, redact: redact_toggle }
    }

    /// Show the panel, or put it away; put away, a press on the page selects
    /// text again.
    pub fn set_open(&self, open: bool) {
        self.root.set_visible(open);
        if !open {
            self.draw.set_active(false);
            self.text.set_active(false);
            self.redact.set_active(false);
        } else if !self.draw.is_active() && !self.text.is_active() && !self.redact.is_active() {
            // Opened to draw, most likely: start with the pen in hand.
            self.draw.set_active(true);
        }
    }
}

fn hint(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("dim-label");
    label.set_xalign(0.0);
    label.set_wrap(true);
    label
}

/// Undo and Redo, which here take back drawings and text as they do marks.
fn history_buttons() -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.set_homogeneous(true);
    for (label, action, tip) in [
        ("Undo", "win.undo-mark", "Take back the last change to the page (Ctrl+Z)"),
        ("Redo", "win.redo-mark", "Put it back (Ctrl+Shift+Z)"),
    ] {
        let button = gtk::Button::with_label(label);
        button.set_action_name(Some(action));
        button.set_tooltip_text(Some(tip));
        row.append(&button);
    }
    row
}
