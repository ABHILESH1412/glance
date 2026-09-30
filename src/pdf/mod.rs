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

mod document;
mod layout;
mod markup;
mod page;
mod render;
mod search;
mod sidebar;
mod view;

pub use document::{is_pdf, open, Opened};
pub use markup::Style;
pub use view::{Marked, PdfView, SearchStatus, Status};
