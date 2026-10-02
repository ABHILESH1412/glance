// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pictures of a PDF's pages for code outside this module: the Combine into
//! PDF grid. Drawn on a thread of their own, as the sidebar's are, most
//! wanted first.

use std::path::Path;

use gtk::{gdk, glib};

use super::document::{self, Source};
use super::layout::Rotation;
use super::render::{Job, Renderer};
use super::view::texture;

/// The pages of one document, drawn on request. Stops when dropped.
pub struct PageImages {
    renderer: Renderer,
}

impl PageImages {
    /// Start, calling `ready` with each page drawn: its number, the scale it
    /// was asked for, and the picture, upright as the file has it.
    pub fn start(path: &Path, password: Option<String>, ready: impl Fn(usize, f64, gdk::Texture) + 'static) -> Self {
        let source = Source { uri: document::uri(path), password };
        let (sender, receiver) = async_channel::bounded(8);
        let renderer = Renderer::start(source, 0, sender);
        glib::spawn_future_local(async move {
            while let Ok(rendered) = receiver.recv().await {
                ready(rendered.page, rendered.requested, texture(rendered.pixels));
            }
        });
        PageImages { renderer }
    }

    /// Draw these pages, at these device pixels per point, in this order,
    /// in place of anything still waiting.
    pub fn want(&self, pages: &[(usize, f64)]) {
        self.renderer.want(
            pages.iter().map(|&(page, scale)| Job { page, scale, rotation: Rotation::default() }).collect(),
        );
    }
}
