// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The frames of an animated picture, down a sidebar: every frame with its
//! number and how long it shows, the one on screen picked out, and buttons to
//! play, pause and step. Clicking a frame holds the animation on it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

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
        // A click, Enter, or moving up and down the list with the arrow
        // keys: whichever frame is picked is the one on screen.
        let weak = Rc::downgrade(&pane);
        list.connect_row_activated(move |_, row| {
            if let Some(pane) = weak.upgrade() {
                pane.choose(row.index());
            }
        });
        let weak = Rc::downgrade(&pane);
        list.connect_row_selected(move |_, row| {
            if let (Some(pane), Some(row)) = (weak.upgrade(), row) {
                pane.choose(row.index());
            }
        });

        // Right-click a frame to copy it.
        let menu = gio::Menu::new();
        menu.append(Some("_Copy Frame"), Some("win.copy"));
        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        // On the pane, not the list: a list box takes every child for a row,
        // and on closing would try to remove the menu as one, fail, and try
        // again forever, so the window went but the program never ended.
        popover.set_parent(&pane.root);
        popover.set_has_arrow(false);
        popover.set_halign(gtk::Align::Start);
        pane.root.connect_destroy(glib::clone!(
            #[weak]
            popover,
            move |_| popover.unparent()
        ));
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(&pane);
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(pane) = weak.upgrade() else { return };
            let Some(row) = pane.list.row_at_y(y as i32) else { return };
            gesture.set_state(gtk::EventSequenceState::Claimed);
            // The frame the menu is about is the one on screen.
            pane.list.select_row(Some(&row));
            pane.choose(row.index());
            let at = pane
                .list
                .compute_point(&pane.root, &gtk::graphene::Point::new(x as f32, y as f32))
                .unwrap_or(gtk::graphene::Point::new(x as f32, y as f32));
            popover.set_pointing_to(Some(&gdk::Rectangle::new(at.x() as i32, at.y() as i32, 1, 1)));
            popover.popup();
        });
        list.add_controller(click);
        pane
    }

    /// The person picked frame `index`; not the list being put in step.
    fn choose(&self, index: i32) {
        if self.syncing.get() || index < 0 {
            return;
        }
        let chosen = self.chosen.borrow();
        if let Some(chosen) = chosen.as_ref() {
            chosen(index as usize);
        }
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
