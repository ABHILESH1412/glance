// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Document Info window: everything a PDF says about itself.
//!
//! It opens at once with a spinner and fills in when the details arrive,
//! since listing a long document's fonts takes a moment. Every value can be
//! selected and copied.

use std::path::PathBuf;

use adw::prelude::*;
use gtk::glib;

use crate::pdf;

pub fn present(parent: &impl IsA<gtk::Widget>, path: PathBuf, current_page: usize) {
    let spinner = gtk::Spinner::builder().spinning(true).width_request(32).height_request(32).build();
    let waiting = gtk::Box::builder().valign(gtk::Align::Center).halign(gtk::Align::Center).vexpand(true).build();
    waiting.append(&spinner);
    let stack = gtk::Stack::new();
    stack.add_named(&waiting, Some("waiting"));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&stack));
    let dialog = adw::Dialog::builder()
        .title("Document Info")
        .content_width(520)
        .content_height(640)
        .child(&toolbar)
        .build();
    dialog.present(Some(parent));

    let (sender, receiver) = async_channel::bounded(1);
    std::thread::Builder::new()
        .name("glance-info".into())
        .spawn(move || {
            let _ = sender.send_blocking(pdf::document_info(&path, current_page));
        })
        .expect("the system refused to start a thread");
    glib::spawn_future_local(async move {
        let Ok(info) = receiver.recv().await else { return };
        let content: gtk::Widget = match info {
            Ok(info) => sections(&info).upcast(),
            Err(error) => adw::StatusPage::builder()
                .icon_name("dialog-warning-symbolic")
                .title("Could Not Read the Document")
                .description(glib::markup_escape_text(&error).as_str())
                .build()
                .upcast(),
        };
        stack.add_named(&content, Some("info"));
        stack.set_visible_child(&content);
    });
}

fn sections(info: &pdf::Info) -> gtk::ScrolledWindow {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 24);
    column.set_margin_top(12);
    column.set_margin_bottom(24);
    column.set_margin_start(12);
    column.set_margin_end(12);
    for section in &info.sections {
        let group = adw::PreferencesGroup::builder().title(section.title.as_str()).build();
        for (label, value) in &section.rows {
            let row = adw::ActionRow::builder()
                .title(label.as_str())
                .subtitle(glib::markup_escape_text(value).as_str())
                .subtitle_selectable(true)
                .subtitle_lines(0)
                .build();
            // The value is what matters: shown as the row's main text.
            row.add_css_class("property");
            group.add(&row);
        }
        column.append(&group);
    }
    let clamp = adw::Clamp::builder().maximum_size(600).child(&column).build();
    gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&clamp).build()
}
