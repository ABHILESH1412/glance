// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The frames of an animated picture, down a sidebar: every frame with its
//! number and how long it shows, the one on screen picked out, and buttons to
//! play, pause and step. Clicking a frame holds the animation on it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, glib};

/// How tall each frame's picture is in the list.
const THUMB_HEIGHT: i32 = 96;

pub struct FramesPane {
    pub root: gtk::Box,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    play: gtk::Button,
    position: gtk::Label,
    rows: RefCell<Vec<gtk::ListBoxRow>>,
    /// Set while the list is being put in step with the picture, so that
    /// picking the row does not echo back as a click.
    syncing: Cell<bool>,
    chosen: RefCell<Option<Box<dyn Fn(usize)>>>,
}

/// "0.10 s", or "100 ms" for the very short frames GIFs are full of.
pub fn describe_delay(delay: Duration) -> String {
    let ms = delay.as_millis();
    if ms < 1000 {
        format!("{ms} ms")
    } else {
        format!("{:.2} s", delay.as_secs_f64())
    }
}

impl FramesPane {
    pub fn new() -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("frames-pane");

        // The controls, over the list.
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        controls.set_margin_top(6);
        controls.set_margin_bottom(6);
        controls.set_margin_start(6);
        controls.set_margin_end(6);
        let back = gtk::Button::from_icon_name("media-skip-backward-symbolic");
        back.set_tooltip_text(Some("Previous Frame (,)"));
        back.set_action_name(Some("win.frame-previous"));
        back.add_css_class("flat");
        let play = gtk::Button::from_icon_name("media-playback-pause-symbolic");
        play.set_tooltip_text(Some("Pause (K)"));
        play.set_action_name(Some("win.frame-play"));
        play.add_css_class("flat");
        let forward = gtk::Button::from_icon_name("media-skip-forward-symbolic");
        forward.set_tooltip_text(Some("Next Frame (.)"));
        forward.set_action_name(Some("win.frame-next"));
        forward.add_css_class("flat");
        let position = gtk::Label::new(None);
        position.add_css_class("dim-label");
        position.add_css_class("numeric");
        position.set_hexpand(true);
        position.set_xalign(1.0);
        controls.append(&back);
        controls.append(&play);
        controls.append(&forward);
        controls.append(&position);
        root.append(&controls);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.add_css_class("navigation-sidebar");
        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        root.append(&scroller);

        let pane = Rc::new(FramesPane {
            root,
            list: list.clone(),
            scroller,
            play,
            position,
            rows: RefCell::default(),
            syncing: Cell::new(false),
            chosen: RefCell::default(),
        });
        let weak = Rc::downgrade(&pane);
        list.connect_row_activated(move |_, row| {
            let Some(pane) = weak.upgrade() else { return };
            if pane.syncing.get() {
                return;
            }
            let chosen = pane.chosen.borrow();
            if let Some(chosen) = chosen.as_ref() {
                chosen(row.index().max(0) as usize);
            }
        });
        pane
    }

    /// Told the frame clicked.
    pub fn connect_chosen(&self, f: impl Fn(usize) + 'static) {
        self.chosen.replace(Some(Box::new(f)));
    }

    /// List these frames, or none.
    pub fn set_frames(&self, frames: &[(gdk::Texture, Duration)]) {
        for row in self.rows.borrow_mut().drain(..) {
            self.list.remove(&row);
        }
        for (index, (texture, delay)) in frames.iter().enumerate() {
            let picture = gtk::Picture::for_paintable(texture);
            picture.set_content_fit(gtk::ContentFit::Contain);
            picture.set_can_shrink(true);
            picture.set_size_request(-1, THUMB_HEIGHT);
            picture.set_hexpand(true);
            let label = gtk::Label::new(Some(&format!("{} · {}", index + 1, describe_delay(*delay))));
            label.add_css_class("caption");
            label.add_css_class("numeric");
            let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
            column.set_margin_top(4);
            column.set_margin_bottom(4);
            column.append(&picture);
            column.append(&label);
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&column));
            row.update_property(&[gtk::accessible::Property::Label(&format!(
                "Frame {}, shown for {}",
                index + 1,
                describe_delay(*delay)
            ))]);
            self.list.append(&row);
            self.rows.borrow_mut().push(row);
        }
    }

    /// Pick out the frame on screen, and show whether it is playing. The
    /// list follows a frame stepped to, but not every frame of a playing
    /// animation, so it can be scrolled while it plays.
    pub fn set_current(&self, index: usize, playing: bool) {
        let count = self.rows.borrow().len();
        self.position.set_text(&if count > 0 { format!("{} / {count}", index + 1) } else { String::new() });
        self.play.set_icon_name(if playing { "media-playback-pause-symbolic" } else { "media-playback-start-symbolic" });
        self.play.set_tooltip_text(Some(if playing { "Pause (K)" } else { "Play (K)" }));
        let Some(row) = self.rows.borrow().get(index).cloned() else { return };
        self.syncing.set(true);
        self.list.select_row(Some(&row));
        self.syncing.set(false);
        if !playing {
            self.scroll_to(&row);
        }
    }

    fn scroll_to(&self, row: &gtk::ListBoxRow) {
        let row = row.clone();
        let scroller = self.scroller.clone();
        // After layout, when the row knows where it is.
        glib::idle_add_local_once(move || {
            let Some(point) = row.compute_point(&scroller.child().unwrap_or(scroller.clone().upcast()), &gtk::graphene::Point::new(0.0, 0.0)) else {
                return;
            };
            let adjustment = scroller.vadjustment();
            let top = f64::from(point.y());
            let bottom = top + f64::from(row.height());
            if top < adjustment.value() {
                adjustment.set_value(top);
            } else if bottom > adjustment.value() + adjustment.page_size() {
                adjustment.set_value(bottom - adjustment.page_size());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_are_said_in_the_unit_that_fits() {
        assert_eq!(describe_delay(Duration::from_millis(100)), "100 ms");
        assert_eq!(describe_delay(Duration::from_millis(1500)), "1.50 s");
    }
}
