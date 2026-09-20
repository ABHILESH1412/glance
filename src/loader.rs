//! Loading happens here, off the main thread.
//!
//! The result is a plain RGBA8 buffer rather than a GDK type on purpose: GDK
//! objects are not `Send`, so decoding straight into one would pin this work to
//! the main loop and freeze the window on large files.

use std::cell::Cell;
use std::fmt::Display;
use std::io::ErrorKind;
use std::path::Path;

use crate::decoders;
use crate::format::{self, Format};

pub struct LoadedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// tiny-skia (SVG) hands back premultiplied alpha; the others do not.
    /// Getting this wrong shows up as dark fringes around transparent edges.
    pub premultiplied: bool,
    /// Shown in the header bar, e.g. "PNG" or "RAW (preview)".
    pub label: String,
}

pub fn decode(path: &Path) -> Result<LoadedImage, String> {
    match format::detect(path) {
        Format::Raster => decoders::raster::decode(path),
        Format::Heif { avif } => decoders::heif::decode(path, avif),
        Format::Svg => decoders::svg::decode(path),
        Format::Raw => decoders::raw::decode(path),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "this file".to_string())
}

pub fn open_error(path: &Path, error: &std::io::Error) -> String {
    let name = file_name(path);
    match error.kind() {
        ErrorKind::NotFound => format!("“{name}” no longer exists."),
        ErrorKind::PermissionDenied => format!("You do not have permission to read “{name}”."),
        _ => format!("Could not open “{name}”."),
    }
}

thread_local! {
    /// Set while a thumbnail is being made. A neighbouring file that will not
    /// decode is not something the user asked to see, so browsing a folder
    /// should not fill the log with complaints about it. Opening that file
    /// deliberately still reports as usual.
    static QUIET: Cell<bool> = const { Cell::new(false) };
}

/// Run `f` with decode failures kept out of the log.
pub fn quietly<T>(f: impl FnOnce() -> T) -> T {
    QUIET.with(|quiet| quiet.set(true));
    let result = f();
    QUIET.with(|quiet| quiet.set(false));
    result
}

/// The toast stays short and human. The technical detail goes to stderr, where
/// it helps when debugging without being shoved in the user's face.
pub fn unsupported(path: &Path, detail: impl Display) -> String {
    if !QUIET.with(|quiet| quiet.get()) {
        eprintln!("simple-viewer: {}: {detail}", path.display());
    }
    format!(
        "“{}” is not a supported image, or the file is damaged.",
        file_name(path)
    )
}
