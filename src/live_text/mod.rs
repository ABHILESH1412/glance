// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Live Text: selecting and copying the text in a picture.
//!
//! The reading itself is done by `glance-ocr`, a helper program started only
//! when it is wanted (see `src/bin/glance-ocr`), so Glance never holds the
//! engine or its models. What it needs is downloaded the first time.

pub mod client;
pub mod install;
// The helper's half of the conversation goes unused on this side.
#[allow(dead_code)]
pub mod protocol;
