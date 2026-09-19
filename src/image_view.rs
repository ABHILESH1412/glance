//! The body of the window: a stack that swaps between the empty state, a
//! spinner while decoding runs, and the picture itself.

use adw::prelude::*;
use gtk::{gdk, glib};

use crate::loader::LoadedImage;

pub struct ImageView {
    stack: gtk::Stack,
    picture: gtk::Picture,
}

impl ImageView {
    pub fn new() -> Self {
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(150)
            .build();

        // Empty state. The button drives the same action as Ctrl+O, so there is
        // only one open path to maintain.
        let open_button = gtk::Button::builder()
            .label("Open Image…")
            .halign(gtk::Align::Center)
            .action_name("win.open")
            .build();
        open_button.add_css_class("pill");
        open_button.add_css_class("suggested-action");

        let empty = adw::StatusPage::builder()
            .icon_name("image-x-generic-symbolic")
            .title("No Image Open")
            .description("Open an image to start viewing.")
            .child(&open_button)
            .build();

        let spinner = adw::Spinner::builder()
            .width_request(48)
            .height_request(48)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();

        // ScaleDown rather than Contain: large images shrink to fit, but a small
        // image stays at its natural size instead of being blown up and blurry.
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::ScaleDown)
            .can_shrink(true)
            .build();

        stack.add_named(&empty, Some("empty"));
        stack.add_named(&spinner, Some("loading"));
        stack.add_named(&picture, Some("image"));
        stack.set_visible_child_name("empty");

        Self { stack, picture }
    }

    pub fn widget(&self) -> &gtk::Stack {
        &self.stack
    }

    pub fn show_loading(&self) {
        self.stack.set_visible_child_name("loading");
    }

    /// Falls back to the empty state unless an image is already on screen, so a
    /// failed second open does not blank out the picture you were looking at.
    pub fn show_idle(&self) {
        if self.picture.paintable().is_none() {
            self.stack.set_visible_child_name("empty");
        } else {
            self.stack.set_visible_child_name("image");
        }
    }

    pub fn show_image(&self, image: LoadedImage) {
        // Taken by value: the buffer is width * height * 4 bytes, so a clone here
        // would mean a second 200 MB allocation on a large photo.
        let bytes = glib::Bytes::from_owned(image.rgba);
        // Mismatching this against the decoder leaves dark halos around
        // anti-aliased transparent edges.
        let format = if image.premultiplied {
            gdk::MemoryFormat::R8g8b8a8Premultiplied
        } else {
            gdk::MemoryFormat::R8g8b8a8
        };
        let texture = gdk::MemoryTexture::new(
            image.width as i32,
            image.height as i32,
            format,
            &bytes,
            image.width as usize * 4,
        );

        self.picture.set_paintable(Some(&texture));
        self.stack.set_visible_child_name("image");
    }
}
