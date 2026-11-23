use gpui_kit::Window;

pub use super::super::fallback::probe::*;
use super::super::probe::Frame;
use super::x11::X11Window;

/// The content's box: the window manager draws the frame.
pub fn window_frame(window: &Window) -> Option<Frame> {
    let (x, y, width, height) = super::window::bounds(window)?;
    Some(Frame {
        x,
        y,
        width,
        height,
    })
}

pub fn content_size(window: &Window) -> Option<(f64, f64)> {
    X11Window::of(window)?.size().ok()
}

pub fn is_visible(window: &Window) -> Option<bool> {
    X11Window::of(window)?.is_mapped().ok()
}

pub fn size_hints(window: &Window) -> Option<super::super::probe::SizeHints> {
    let window = X11Window::of(window)?;
    let [min, max] = window.size_limits().ok()?;
    Some(super::super::probe::SizeHints {
        min,
        max,
        aspect: window.aspect_ratio().ok()?,
        above: window.is_above().ok()?,
    })
}
