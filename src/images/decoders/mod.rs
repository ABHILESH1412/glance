//! One backend per format family. Each returns a [`LoadedImage`] so the rest of
//! the app never learns which library did the work.

pub mod heif;
pub mod raster;
pub mod raw;
pub mod svg;
