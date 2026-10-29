// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The body of the window: a stack that swaps between the empty state, a
//! spinner while decoding runs, and the image itself.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;

use crate::images::canvas::{self, ImageCanvas};
use crate::images::loader::LoadedImage;

/// How many of the files opened last the empty window offers again.
const RECENT: usize = 4;

pub struct ImageView {
    stack: gtk::Stack,
    canvas: ImageCanvas,
    /// The files opened last, under the buttons while nothing is open.
    recent: gtk::Box,
    recent_list: gtk::ListBox,
    recent_clear: gtk::Button,
    recent_paths: Rc<RefCell<Vec<PathBuf>>>,
}

impl ImageView {
    pub fn new() -> Self {
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(150)
            .build();

        // The button drives the same action as Ctrl+O, so there is only one
        // open path to maintain.
        let open_button = gtk::Button::builder()
            .label("Open…")
            .halign(gtk::Align::Center)
            .action_name("win.open")
            .build();
        open_button.add_css_class("pill");
        open_button.add_css_class("suggested-action");

        let combine_button = gtk::Button::builder()
            .label("Combine into PDF…")
            .halign(gtk::Align::Center)
            .action_name("win.combine")
            .tooltip_text("Put pages from PDFs and pictures together as one PDF")
            .build();
        combine_button.add_css_class("pill");
        // The same width, one above the other.
        open_button.set_halign(gtk::Align::Fill);
        combine_button.set_halign(gtk::Align::Fill);
        let choices = gtk::Box::new(gtk::Orientation::Vertical, 12);
        choices.set_halign(gtk::Align::Center);
        choices.set_homogeneous(true);
        choices.append(&open_button);
        choices.append(&combine_button);

        // Opened last, to pick up where you were.
        let heading = gtk::Label::builder().label("Recent").xalign(0.0).hexpand(true).css_classes(["heading"]).build();
        let recent_clear = gtk::Button::builder().label("Clear").tooltip_text("Forget the files opened lately").build();
        recent_clear.add_css_class("flat");
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.append(&heading);
        header.append(&recent_clear);
        let recent_list = gtk::ListBox::new();
        recent_list.add_css_class("boxed-list");
        recent_list.set_selection_mode(gtk::SelectionMode::None);
        let recent = gtk::Box::new(gtk::Orientation::Vertical, 6);
        recent.append(&header);
        recent.append(&recent_list);
        recent.set_visible(false);
        let clamp = adw::Clamp::builder().maximum_size(420).child(&recent).build();

        let body = gtk::Box::new(gtk::Orientation::Vertical, 36);
        body.append(&choices);
        body.append(&clamp);
        let empty = adw::StatusPage::builder()
            .icon_name(crate::APP_ID)
            .title("Open a Picture or PDF")
            .description("Drag one in, or choose one. Several at once can be put together as one PDF.")
            .child(&body)
            .build();

        // GTK's spinner rather than libadwaita's, which would otherwise be the
        // one thing in the whole program requiring libadwaita 1.6 and so lock
        // out Ubuntu's current long-term release. It is deprecated in favour of
        // libadwaita's, but a deprecated spinner that everyone can run beats a
        // current one that half the distributions cannot.
        #[allow(deprecated)]
        let spinner = gtk::Spinner::builder()
            .spinning(true)
            .width_request(48)
            .height_request(48)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();

        let canvas = ImageCanvas::new();

        stack.add_named(&empty, Some("empty"));
        stack.add_named(&spinner, Some("loading"));
        stack.add_named(&canvas, Some("image"));
        stack.set_visible_child_name("empty");

        Self { stack, canvas, recent, recent_list, recent_clear, recent_paths: Rc::default() }
    }

    /// The files opened last, newest first; none puts the list away.
    pub fn set_recent(&self, paths: Vec<PathBuf>) {
        while let Some(row) = self.recent_list.row_at_index(0) {
            self.recent_list.remove(&row);
        }
        let home = glib_home();
        for path in paths.iter().take(RECENT) {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let folder = path.parent().map(|folder| tidy_folder(folder, home.as_deref())).unwrap_or_default();
            let icon = if crate::app::playlist::is_image(path) { "glance-photo-symbolic" } else { "glance-document-symbolic" };
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&name).as_str())
                .subtitle(glib::markup_escape_text(&folder).as_str())
                .activatable(true)
                .build();
            row.add_prefix(&gtk::Image::from_icon_name(icon));
            row.add_suffix(&gtk::Image::from_icon_name("glance-next-symbolic"));
            self.recent_list.append(&row);
        }
        self.recent.set_visible(!paths.is_empty());
        self.recent_paths.replace(paths.into_iter().take(RECENT).collect());
    }

    /// Called with a recent file when its row is clicked, or with `None` when
    /// the list is to be forgotten.
    pub fn connect_recent(&self, f: impl Fn(Option<PathBuf>) + 'static) {
        let f = Rc::new(f);
        let paths = self.recent_paths.clone();
        let open = f.clone();
        self.recent_list.connect_row_activated(move |_, row| {
            let path = usize::try_from(row.index()).ok().and_then(|i| paths.borrow().get(i).cloned());
            if path.is_some() {
                open(path);
            }
        });
        self.recent_clear.connect_clicked(move |_| f(None));
    }

    pub fn widget(&self) -> &gtk::Stack {
        &self.stack
    }

    pub fn canvas(&self) -> &ImageCanvas {
        &self.canvas
    }

    pub fn show_loading(&self) {
        self.stack.set_visible_child_name("loading");
    }

    /// Falls back to the empty state unless an image is already on screen, so a
    /// failed second open does not blank out the picture you were looking at.
    pub fn show_idle(&self) {
        if self.canvas.has_image() {
            self.stack.set_visible_child_name("image");
        } else {
            self.stack.set_visible_child_name("empty");
        }
    }

    pub fn show_image(&self, mut image: LoadedImage) {
        let (width, height, premultiplied) = (image.width, image.height, image.premultiplied);
        let vector = image.vector.take();

        if let Some(source) = vector {
            // Kept in vector form so zooming re-renders instead of enlarging
            // pixels.
            let texture = canvas::texture_from(width, height, premultiplied, image.rgba);
            self.canvas.set_vector(source, texture);
        } else if image.animation.len() > 1 {
            let frames = image
                .animation
                .into_iter()
                .map(|frame| {
                    (
                        canvas::texture_from(width, height, premultiplied, frame.rgba),
                        frame.delay,
                    )
                })
                .collect();
            self.canvas.set_animation(frames);
        } else {
            // Taken by value: the buffer is width * height * 4 bytes, so a clone
            // here would mean a second 200 MB allocation on a large photo.
            let texture = canvas::texture_from(width, height, premultiplied, image.rgba);
            self.canvas.set_texture(Some(texture));
        }
        self.stack.set_visible_child_name("image");
    }
}

use gtk::glib;

fn glib_home() -> Option<PathBuf> {
    Some(glib::home_dir()).filter(|home| !home.as_os_str().is_empty())
}

/// A folder as people say it: `~/Pictures/Holiday` rather than the whole path.
fn tidy_folder(folder: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| folder.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => folder.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_under_home_are_said_from_it() {
        let home = Path::new("/home/someone");
        assert_eq!(tidy_folder(Path::new("/home/someone/Pictures/Trip"), Some(home)), "~/Pictures/Trip");
        assert_eq!(tidy_folder(Path::new("/home/someone"), Some(home)), "~");
        assert_eq!(tidy_folder(Path::new("/media/disk"), Some(home)), "/media/disk");
        assert_eq!(tidy_folder(Path::new("/home/someoneelse"), Some(home)), "/home/someoneelse");
    }
}
