// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The window every kind of document is shown in.
//!
//! The header bar, the filmstrip, stepping through a folder, opening and
//! deleting files, and the About dialog. None of it draws a document itself.
//! Today the window only opens images; `Window::load` is where a PDF or a 3D
//! model will be handed to its own module instead.

pub mod closing;
pub mod colour;
pub mod combine;
pub mod doc_info;
pub mod filmstrip;
pub mod frames;
pub mod inspector;
pub mod live_text;
pub mod locked;
pub mod menus;
pub mod pdf_tools;
pub mod playlist;
pub mod preferences;
pub mod prefs;
pub mod print;
pub mod protect;
pub mod redact;
pub mod shortcuts;
pub mod shrink;
pub mod signatures;
pub mod viewing;
pub mod window;
