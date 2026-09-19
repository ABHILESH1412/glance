//! SVG, via resvg.
//!
//! SVG is the odd one out: it has no pixels until we pick a size. We rasterise
//! once at load time, which is fine while the viewer only ever scales down. Once
//! zoom exists this should re-render at the zoom level instead, or zooming in
//! will go soft the same way a bitmap would.

use std::path::Path;
use std::sync::OnceLock;

use resvg::tiny_skia;
use resvg::usvg;

use crate::loader::{open_error, unsupported, LoadedImage};

/// Small vector art would rasterise into a postage stamp, so give it room.
const MIN_LONG_EDGE: f32 = 1024.0;
/// Keep a pathological viewBox from allocating gigabytes.
const MAX_LONG_EDGE: f32 = 8192.0;

fn fontdb() -> &'static usvg::fontdb::Database {
    // Loading system fonts takes a moment, so do it once for the process.
    static DB: OnceLock<usvg::fontdb::Database> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        db
    })
}

pub fn decode(path: &Path) -> Result<LoadedImage, String> {
    let data = std::fs::read(path).map_err(|e| open_error(path, &e))?;

    let mut options = usvg::Options::default();
    // Lets `<image href="logo.png">` and friends resolve next to the SVG.
    options.resources_dir = path.parent().map(|p| p.to_path_buf());
    options.fontdb = std::sync::Arc::new(fontdb().clone());

    let tree = usvg::Tree::from_data(&data, &options).map_err(|e| unsupported(path, &e))?;

    let size = tree.size();
    let longest = size.width().max(size.height());
    if longest <= 0.0 {
        return Err(unsupported(path, "SVG has zero size"));
    }

    let scale = (MIN_LONG_EDGE / longest).max(1.0).min(MAX_LONG_EDGE / longest);
    let width = (size.width() * scale).round().max(1.0) as u32;
    let height = (size.height() * scale).round().max(1.0) as u32;

    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| unsupported(path, "SVG is too large to rasterise"))?;

    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    Ok(LoadedImage {
        width,
        height,
        // tiny-skia works in premultiplied alpha throughout.
        rgba: pixmap.take(),
        premultiplied: true,
        label: "SVG".to_string(),
    })
}
