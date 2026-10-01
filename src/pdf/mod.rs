// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! PDF documents, read with Poppler — the engine GNOME's own document viewers
//! use, and the only one that covers the rest of what is planned here: text
//! search and selection, links, forms and annotations.
//!
//! Poppler is C++, not Rust, so it is kept behind this module: nothing outside
//! it touches a Poppler type. That keeps the door open to moving it into a
//! sandboxed helper process later without the rest of the program noticing.
//!
//! Anything in `crate::images` can be used from here directly — the drawing
//! and text tools in `images::edit` are the obvious ones for annotating a page.

mod annots;
mod bookmark_list;
mod bookmarks;
mod contents;
mod document;
mod editor;
mod info;
mod ink;
mod layout;
mod markup;
mod newer;
mod notes;
mod outline;
mod page;
mod qpdf;
mod render;
mod rewrite;
mod search;
mod sidebar;
mod thumbnails;
mod view;

pub use document::{is_pdf, open, Allowed, OpenError, Opened};
pub use annots::Rgb;
pub use info::{gather as document_info, Info};
pub use layout::Mode;
pub use markup::Style;
pub use notes::TextStyle;
pub use qpdf::{Permissions, Protection};
pub use rewrite::{
    install, keep_as, opens_with, permissions as current_permissions, protect, shrink, temporary_beside, Level,
    Report,
};
pub use sidebar::View as SidebarView;
pub use view::{NOT_ANNOTATABLE, BookmarkEvent, Marked, PdfView, Pinned, SearchStatus, Status, Tool};

/// Whether this Poppler can draw on pages: ink needs 25.06.
pub fn can_draw() -> bool {
    ink::available()
}

/// Whether this Poppler can set a text box's font, size and colour: 24.12.
pub fn can_style_text() -> bool {
    newer::get().fonts.is_some() && newer::get().border.is_some()
}
