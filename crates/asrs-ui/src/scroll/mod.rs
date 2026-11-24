//! What Chromium adds to a scroller: its scrolling keys and its scrollbars.

mod bars;
mod keys;

pub use bars::{Extent, Request, Scrollbars, Style, scrollbars, set_style, style};
pub use keys::{Step, page_step, step};
