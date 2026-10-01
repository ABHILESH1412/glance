// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The little window a note or speech bubble is written in, opening beside it.
//!
//! Whatever is typed is kept when it closes, however it closes — clicking
//! away, Escape, or Done — as a sticky note would. Undo takes it back.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

/// What became of the text.
pub enum Commit {
    Text(String),
    Delete,
}

type OnCommit = Box<dyn Fn(Commit)>;

pub struct Editor {
    popover: gtk::Popover,
    title: gtk::Label,
    text: gtk::TextView,
    delete: gtk::Button,
    state: Rc<State>,
}

#[derive(Default)]
struct State {
    /// Called once, when the editor closes; taken, so it cannot run twice.
    on_commit: RefCell<Option<OnCommit>>,
    original: RefCell<String>,
    deleting: std::cell::Cell<bool>,
}

impl Editor {
    pub fn new(parent: &impl IsA<gtk::Widget>) -> Self {
        let title = gtk::Label::builder().xalign(0.0).hexpand(true).css_classes(["heading"]).build();
        let delete = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text("Delete")
            .css_classes(["flat", "destructive-action"])
            .build();
        let done = gtk::Button::builder().label("Done").css_classes(["suggested-action"]).build();
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        top.append(&title);
        top.append(&delete);
        top.append(&done);

        let text = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .accepts_tab(false)
            .top_margin(6)
            .bottom_margin(6)
            .left_margin(6)
            .right_margin(6)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            // A size request, not a minimum content width: with sideways
            // scrolling off, that would be ignored.
            .width_request(280)
            .min_content_height(110)
            .max_content_height(260)
            .propagate_natural_height(true)
            .child(&text)
            .css_classes(["card"])
            .build();
        let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
        body.append(&top);
        body.append(&scroller);

        let popover = gtk::Popover::builder().child(&body).position(gtk::PositionType::Bottom).build();
        popover.set_parent(parent);
        // Not a child its parent knows about, so it is let go of by hand, or
        // GTK complains when the window closes.
        parent.as_ref().connect_destroy(glib::clone!(
            #[weak]
            popover,
            move |_| popover.unparent()
        ));

        let state = Rc::new(State::default());
        let editor = Editor { popover, title, text, delete, state };

        done.connect_clicked(glib::clone!(
            #[weak(rename_to = popover)]
            editor.popover,
            move |_| popover.popdown()
        ));
        editor.delete.connect_clicked(glib::clone!(
            #[weak(rename_to = popover)]
            editor.popover,
            #[weak(rename_to = state)]
            editor.state,
            move |_| {
                state.deleting.set(true);
                popover.popdown();
            }
        ));
        // Ctrl+Enter finishes, as in a chat box; Enter alone starts a line.
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(glib::clone!(
            #[weak(rename_to = popover)]
            editor.popover,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, modifiers| {
                let enter = matches!(key, gdk::Key::Return | gdk::Key::KP_Enter);
                if enter && modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
                    popover.popdown();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        ));
        editor.text.add_controller(keys);

        editor.popover.connect_closed(glib::clone!(
            #[weak(rename_to = text)]
            editor.text,
            #[weak(rename_to = state)]
            editor.state,
            move |_| {
                let Some(on_commit) = state.on_commit.take() else { return };
                if state.deleting.replace(false) {
                    on_commit(Commit::Delete);
                    return;
                }
                let buffer = text.buffer();
                let typed = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).trim_end().to_string();
                if typed != *state.original.borrow() {
                    on_commit(Commit::Text(typed));
                }
            }
        ));
        editor
    }

    /// Open beside `at`, in the parent's coordinates, showing `text`. A note
    /// already on the page can be deleted from here; a new one cannot.
    pub fn open(&self, at: gdk::Rectangle, title: &str, text: &str, existing: bool, on_commit: impl Fn(Commit) + 'static) {
        // Anything still open is finished first, with what it was given.
        self.popover.popdown();
        self.title.set_text(title);
        self.delete.set_visible(existing);
        self.text.buffer().set_text(text);
        self.state.original.replace(text.to_string());
        self.state.deleting.set(false);
        self.state.on_commit.replace(Some(Box::new(on_commit)));
        self.popover.set_pointing_to(Some(&at));
        self.popover.popup();
        self.text.grab_focus();
        let buffer = self.text.buffer();
        buffer.place_cursor(&buffer.end_iter());
    }
}
