//! The body of the window: a stack that swaps between the empty state, a
//! spinner while decoding runs, and the image itself.

use adw::prelude::*;
use gtk::{gdk, glib};

use crate::canvas::ImageCanvas;
use crate::loader::LoadedImage;

pub struct ImageView {
    stack: gtk::Stack,
    canvas: ImageCanvas,
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
            .label("Open Image…")
            .halign(gtk::Align::Center)
            .action_name("win.open")
            .build();
        open_button.add_css_class("pill");
        open_button.add_css_class("suggested-action");

        let empty = adw::StatusPage::builder()
            .icon_name("image-x-generic-symbolic")
            .title("No Image Open")
            .description("Drop an image here, or open one to start viewing.")
            .child(&open_button)
            .build();

        let spinner = adw::Spinner::builder()
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

        Self { stack, canvas }
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

    fn texture_for(width: u32, height: u32, premultiplied: bool, rgba: Vec<u8>) -> gdk::Texture {
        let bytes = glib::Bytes::from_owned(rgba);
        // Mismatching this against the decoder leaves dark halos around
        // anti-aliased transparent edges.
        let format = if premultiplied {
            gdk::MemoryFormat::R8g8b8a8Premultiplied
        } else {
            gdk::MemoryFormat::R8g8b8a8
        };
        gdk::MemoryTexture::new(
            width as i32,
            height as i32,
            format,
            &bytes,
            width as usize * 4,
        )
        .upcast()
    }

    pub fn show_image(&self, image: LoadedImage) {
        let (width, height, premultiplied) = (image.width, image.height, image.premultiplied);

        if image.animation.len() > 1 {
            let frames = image
                .animation
                .into_iter()
                .map(|frame| {
                    (
                        Self::texture_for(width, height, premultiplied, frame.rgba),
                        frame.delay,
                    )
                })
                .collect();
            self.canvas.set_animation(frames);
        } else {
            // Taken by value: the buffer is width * height * 4 bytes, so a clone
            // here would mean a second 200 MB allocation on a large photo.
            let texture = Self::texture_for(width, height, premultiplied, image.rgba);
            self.canvas.set_texture(Some(texture));
        }
        self.stack.set_visible_child_name("image");
    }
}
