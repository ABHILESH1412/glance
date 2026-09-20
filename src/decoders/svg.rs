//! SVG, via resvg.
//!
//! SVG is the odd one out: it has no pixels until a size is chosen. So the
//! source is kept alongside the first rasterisation and the view re-renders it
//! whenever it needs more detail — that is the whole point of a vector image,
//! and rasterising once at load would throw it away.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use resvg::tiny_skia;
use resvg::usvg;

use crate::loader::{open_error, unsupported, LoadedImage};

/// Small vector art would open as a postage stamp, so give the first render
/// some room. Zooming in re-renders anyway.
const MIN_LONG_EDGE: f64 = 1024.0;
/// A single render is capped here so a pathological viewBox, or an enthusiastic
/// zoom, cannot ask for gigabytes of pixmap.
pub const MAX_LONG_EDGE: f64 = 8192.0;
pub const MAX_PIXELS: f64 = 40.0 * 1024.0 * 1024.0;

fn fontdb() -> Arc<usvg::fontdb::Database> {
    // Loading system fonts takes a moment, and the database is large enough
    // that copying it per parse would be worse than the parse itself.
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        Arc::new(db)
    })
    .clone()
}

/// A vector image kept in its source form, so it can be rasterised again
/// whenever the view needs more detail. Without this an SVG would be pinned to
/// whatever resolution it happened to be drawn at when it was opened, and
/// zooming in would go soft exactly like a photograph.
pub struct VectorSource {
    data: Vec<u8>,
    base_dir: Option<std::path::PathBuf>,
    /// Natural size in the image's own units, which is what the view lays out
    /// against; the rasterised pixel size is free to differ.
    pub width: f64,
    pub height: f64,
    /// Parsed once and shared. Re-parsing on every zoom step would dominate the
    /// cost for a large drawing.
    tree: OnceLock<Option<usvg::Tree>>,
}

impl VectorSource {
    fn tree(&self) -> Option<&usvg::Tree> {
        self.tree
            .get_or_init(|| parse(&self.data, self.base_dir.as_deref()))
            .as_ref()
    }
}

fn parse(data: &[u8], base_dir: Option<&Path>) -> Option<usvg::Tree> {
    let mut options = usvg::Options {
        // Lets `<image href="logo.png">` and friends resolve next to the SVG.
        resources_dir: base_dir.map(|p| p.to_path_buf()),
        ..Default::default()
    };
    options.fontdb = fontdb();
    usvg::Tree::from_data(data, &options).ok()
}

pub fn decode(path: &Path) -> Result<LoadedImage, String> {
    let data = std::fs::read(path).map_err(|e| open_error(path, &e))?;
    let base_dir = path.parent().map(|p| p.to_path_buf());

    let tree = parse(&data, base_dir.as_deref())
        .ok_or_else(|| unsupported(path, "could not be parsed as SVG"))?;

    let size = tree.size();
    let (natural_w, natural_h) = (f64::from(size.width()), f64::from(size.height()));
    if natural_w <= 0.0 || natural_h <= 0.0 {
        return Err(unsupported(path, "SVG has zero size"));
    }

    let cached = OnceLock::new();
    let _ = cached.set(Some(tree));
    let source = Arc::new(VectorSource {
        data,
        base_dir,
        width: natural_w,
        height: natural_h,
        tree: cached,
    });

    // A first pass at a comfortable size; the view sharpens it from here.
    let longest = natural_w.max(natural_h);
    let scale = (MIN_LONG_EDGE / longest).max(1.0);
    let mut image = source
        .tree()
        .and_then(|tree| render(&source, tree, scale))
        .ok_or_else(|| unsupported(path, "SVG is too large to rasterise"))?;
    image.label = "SVG".to_string();
    image.vector = Some(source);
    Ok(image)
}

/// Rasterise just the region `(x, y, w, h)` of the image, in its own units, at
/// `scale` pixels per unit.
///
/// Only the visible part is drawn, which is what makes deep zoom affordable:
/// re-rendering the whole image at 30x would need gigapixels, while the part
/// actually on screen is never bigger than the window.
pub fn rasterise_region(
    source: &VectorSource,
    region: (f64, f64, f64, f64),
    scale: f64,
) -> Option<(LoadedImage, f64)> {
    let (x, y, w, h) = region;
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    // Clamp so one tile can never exceed the pixel budget.
    let scale = scale
        .min(MAX_LONG_EDGE / w.max(1e-6))
        .min(MAX_LONG_EDGE / h.max(1e-6))
        .min((MAX_PIXELS / (w * h).max(1e-6)).sqrt())
        .max(0.01);

    let width = (w * scale).round().max(1.0) as u32;
    let height = (h * scale).round().max(1.0) as u32;

    let tree = source.tree()?;
    let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
    // Maps image units into the tile: scale, then shift the region's corner to
    // the pixmap's origin.
    let transform = tiny_skia::Transform::from_row(
        scale as f32,
        0.0,
        0.0,
        scale as f32,
        (-x * scale) as f32,
        (-y * scale) as f32,
    );
    resvg::render(tree, transform, &mut pixmap.as_mut());

    Some((
        LoadedImage {
            width,
            height,
            rgba: pixmap.take(),
            premultiplied: true,
            label: String::new(),
            animation: Vec::new(),
            vector: None,
        },
        scale,
    ))
}

fn render(source: &VectorSource, tree: &usvg::Tree, scale: f64) -> Option<LoadedImage> {
    let longest = source.width.max(source.height).max(1.0);
    let limit = (MAX_LONG_EDGE / longest)
        .min((MAX_PIXELS / (source.width * source.height).max(1.0)).sqrt())
        .max(0.01);
    let scale = scale.clamp(0.01, limit);
    let width = (source.width * scale).round().max(1.0) as u32;
    let height = (source.height * scale).round().max(1.0) as u32;

    let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
    resvg::render(
        tree,
        tiny_skia::Transform::from_scale(scale as f32, scale as f32),
        &mut pixmap.as_mut(),
    );

    Some(LoadedImage {
        width,
        height,
        // tiny-skia works in premultiplied alpha throughout.
        rgba: pixmap.take(),
        premultiplied: true,
        label: String::new(),
        animation: Vec::new(),
        vector: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders a filtered SVG straight through resvg at a high scale, so a
    /// blocky result can be blamed on the renderer rather than on compositing.
    #[test]
    #[ignore]
    fn dump_region_render() {
        let path = std::path::Path::new(
            &std::env::var("SV_TEST_SVG").expect("set SV_TEST_SVG"),
        )
        .to_path_buf();
        let data = std::fs::read(&path).unwrap();
        let tree = parse(&data, path.parent()).unwrap();
        let size = tree.size();
        let cached = OnceLock::new();
        let _ = cached.set(Some(tree));
        let source = VectorSource {
            data,
            base_dir: path.parent().map(|p| p.to_path_buf()),
            width: f64::from(size.width()),
            height: f64::from(size.height()),
            tree: cached,
        };
        let scale: f64 = std::env::var("SV_TEST_SCALE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20.0);
        // A small window, the way the viewer would ask for it. Centre defaults
        // to the middle but can be aimed at an edge where the glow lives.
        let w = 900.0 / scale;
        let h = 900.0 / scale;
        let cx: f64 = std::env::var("SV_TEST_X").ok().and_then(|v| v.parse().ok())
            .unwrap_or(source.width / 2.0);
        let cy: f64 = std::env::var("SV_TEST_Y").ok().and_then(|v| v.parse().ok())
            .unwrap_or(source.height / 2.0);
        let region = (cx - w / 2.0, cy - h / 2.0, w, h);
        let (image, used) = rasterise_region(&source, region, scale).unwrap();
        eprintln!("region {region:?} scale {scale} -> used {used}, {}x{}", image.width, image.height);
        let out = std::env::var("SV_TEST_OUT").unwrap_or_else(|_| "/tmp/svg_region.png".into());
        image::RgbaImage::from_raw(image.width, image.height, image.rgba)
            .unwrap()
            .save(&out)
            .unwrap();
        eprintln!("wrote {out}");
    }
}
