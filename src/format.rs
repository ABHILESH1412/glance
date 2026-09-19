//! Deciding which decoder a file belongs to.
//!
//! Content sniffing comes first and extensions only break ties, because the
//! extension is the least trustworthy thing about a file. The one place order
//! really matters is raw: most raw formats are TIFF containers, so a `.dng`
//! would happily start decoding as a TIFF and produce garbage if the raster
//! path saw it first.

use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// PNG, JPEG, GIF, WebP, TIFF, BMP and friends, via the `image` crate.
    Raster,
    /// HEIC/HEIF and AVIF, via libheif. Both use the same container and the
    /// same decoder; only the codec inside differs, so the flag exists purely
    /// to label them correctly in the UI.
    Heif { avif: bool },
    /// SVG, via resvg.
    Svg,
    /// Camera raw, via rawler.
    Raw,
}

/// Raw extensions worth routing away from the TIFF decoder.
const RAW_EXTENSIONS: &[&str] = &[
    "3fr", "arw", "cr2", "cr3", "crw", "dcr", "dng", "erf", "fff", "iiq", "kdc", "mef", "mos",
    "mrw", "nef", "nrw", "orf", "pef", "raf", "raw", "rw2", "rwl", "sr2", "srf", "srw", "x3f",
];

/// HEIF-family brands that appear in the `ftyp` box.
const HEIF_BRANDS: &[&[u8; 4]] = &[
    b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx", b"mif1", b"msf1", b"avif", b"avis",
];

/// The subset of those brands that mean AV1 rather than HEVC.
const AVIF_BRANDS: &[&[u8; 4]] = &[b"avif", b"avis"];

pub fn detect(path: &Path) -> Format {
    let head = read_head(path);
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    if let Some(avif) = heif_kind(&head) {
        return Format::Heif { avif };
    }
    // Checked before Raster so TIFF-based raw containers are not mistaken for
    // ordinary TIFFs.
    if RAW_EXTENSIONS.contains(&extension.as_str()) {
        return Format::Raw;
    }
    if is_svg(&head, &extension) {
        return Format::Svg;
    }
    Format::Raster
}

fn read_head(path: &Path) -> Vec<u8> {
    let mut buffer = vec![0u8; 1024];
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    match file.read(&mut buffer) {
        Ok(n) => {
            buffer.truncate(n);
            buffer
        }
        Err(_) => Vec::new(),
    }
}

/// An ISO base-media file: bytes 4..8 are `ftyp`, and the brand follows.
/// Returns whether the file is AVIF, or `None` if it is not HEIF-family at all.
fn heif_kind(head: &[u8]) -> Option<bool> {
    if head.len() < 12 || &head[4..8] != b"ftyp" {
        return None;
    }
    // The major brand can be generic (`mif1`), with the specific one listed in
    // the compatible-brands array that follows, so consider both.
    let major = &head[8..12];
    let brands: Vec<&[u8]> = std::iter::once(major)
        .chain(head[12..].chunks_exact(4).take(8))
        .collect();

    if !brands
        .iter()
        .any(|c| HEIF_BRANDS.iter().any(|b| b.as_slice() == *c))
    {
        return None;
    }
    Some(
        brands
            .iter()
            .any(|c| AVIF_BRANDS.iter().any(|b| b.as_slice() == *c)),
    )
}

fn is_svg(head: &[u8], extension: &str) -> bool {
    if extension == "svg" || extension == "svgz" {
        return true;
    }
    // gzip magic: a compressed SVG with some other name.
    if head.starts_with(&[0x1f, 0x8b]) {
        return false;
    }
    let prefix = String::from_utf8_lossy(&head[..head.len().min(512)]);
    let trimmed = prefix.trim_start();
    trimmed.starts_with("<svg") || (trimmed.starts_with("<?xml") && prefix.contains("<svg"))
}
