use gpui_kit::Window;

pub use super::super::fallback::probe::*;
use super::super::probe::{Frame, MouseAction, SizeHints, WheelSample};
use super::x11::{self, X11Window};

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

pub fn window_number(window: &Window) -> Option<u32> {
    X11Window::of(window).map(|window| window.id())
}

pub fn size_hints(window: &Window, number: u32) -> Option<SizeHints> {
    let window = X11Window::of(window)?.with_id(number);
    let [min, max] = window.size_limits().ok()?;
    Some(SizeHints {
        min,
        max,
        aspect: window.aspect_ratio().ok()?,
        above: window.is_above().ok()?,
    })
}

pub fn post_mouse(action: MouseAction, x: f64, y: f64) -> bool {
    let button = match action {
        MouseAction::Down => Some(true),
        MouseAction::Up => Some(false),
        MouseAction::Drag => None,
    };
    x11::fake_pointer(Some((x, y)), button.map(|down| (1, down))).is_ok()
}

/// Presses a core wheel button once for each line of `sample.delta`: the notches of a wheel
/// without scroll valuators.
pub fn post_wheel_at(x: f64, y: f64, sample: &WheelSample) -> bool {
    let [dy, dx] = sample.delta;
    let presses = [(dy, 4, 5), (dx, 6, 7)];
    let mut ok = x11::fake_pointer(Some((x, y)), None).is_ok();
    for (lines, positive, negative) in presses {
        let button = if lines > 0 { positive } else { negative };
        for _ in 0..lines.unsigned_abs() {
            for down in [true, false] {
                ok &= x11::fake_pointer(None, Some((button, down))).is_ok();
            }
        }
    }
    ok
}

pub fn post_keys(keysyms: &[u32]) -> bool {
    x11::fake_keys(keysyms).is_ok()
}

pub fn split_monitors(x: Option<u16>) -> bool {
    x11::split_monitors(x).is_ok()
}
