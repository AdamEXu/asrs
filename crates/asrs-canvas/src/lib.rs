#![allow(clippy::disallowed_methods)]

pub mod aaa;
pub mod blit;
pub mod clip;
pub mod color;
pub mod color_profile;
pub mod dash;
pub mod display;
pub mod geometry;
pub mod gradient;
pub mod hairline;
pub mod heatmap;
mod mip;
pub mod msaa;
pub mod path;
pub mod pixels;
pub mod raster;
pub mod scan;
pub mod skpath;
pub mod stroke;
pub mod text;
pub mod trace;

pub use display::{Call, Canvas2d, Gradient, ImageData, ImageDataSource, Property, Style};
pub use raster::{Raster, RasterState, ShadeCache, TextRaster};
pub use tiny_skia::Pixmap;
