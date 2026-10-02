// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Everything specific to images: decoding every supported format, the
//! zoomable canvas, filmstrip thumbnails, colour management, and the editor.

pub mod canvas;
pub mod colour;
pub mod decoders;
pub mod exif;
pub mod edit;
pub mod format;
pub mod loader;
pub mod resolution;
pub mod scene;
pub mod thumbs;
pub mod view;
