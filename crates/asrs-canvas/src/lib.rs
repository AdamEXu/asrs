#![allow(clippy::disallowed_methods)]

pub mod color;
pub mod display;
pub mod hairline;
pub mod path;
pub mod pixels;
pub mod raster;
pub mod scan;
pub mod text;
pub mod trace;

pub use display::{Call, Canvas2d, Gradient, ImageData, ImageDataSource, Property, Style};
pub use raster::{Raster, TextRaster};
pub use tiny_skia::Pixmap;
