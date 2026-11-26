//! The parts of Electron's `NativeWindowViews` on X11 that GPUI's window options do not cover.

use gpui_kit::Window;

use super::x11::X11Window;

pub struct NativeWindow(X11Window);

#[cfg(feature = "parity")]
pub fn titled_content_size(width: f64, height: f64, _scale: f64) -> (f64, f64) {
    (width, height)
}

pub fn native(window: &Window) -> Option<NativeWindow> {
    X11Window::of(window).map(NativeWindow)
}

/// `DesktopWindowTreeHostLinux`'s bounds are the content's: the window manager draws the frame.
pub fn bounds(window: &Window) -> Option<(f64, f64, f64, f64)> {
    let window = X11Window::of(window)?;
    let origin = window.origin().ok()?;
    let (width, height) = window.size().ok()?;
    let f = |v: gpui_kit::Pixels| f64::from(f32::from(v));
    Some((f(origin.x), f(origin.y), width, height))
}

impl NativeWindow {
    pub fn content_size_for_frame(&self, width: f64, height: f64) -> (f64, f64) {
        (width, height)
    }

    pub fn content_size(&self) -> (f64, f64) {
        self.0.size().unwrap_or((0.0, 0.0))
    }

    pub fn set_content_size(&self, width: f64, height: f64) {
        self.0.resize(width, height).ok();
    }

    pub fn set_content_origin(&self, x: f64, y: f64) {
        self.0.place(x, y).ok();
    }

    pub fn center(&self, _parent: Option<&NativeWindow>) {}

    pub fn set_content_size_limits(&self, min: Option<(f64, f64)>, max: Option<(f64, f64)>) {
        self.0.set_size_limits(min, max).ok();
    }

    pub fn set_fullscreenable(&self, _fullscreenable: bool) {}

    pub fn begin_sheet(&self, sheet: &NativeWindow) {
        sheet.0.set_transient_for(&self.0).ok();
    }

    pub fn end_sheet(&self) {}

    pub fn is_visible(&self) -> bool {
        self.0.is_mapped().unwrap_or(true)
    }

    pub fn show(&self, _focus: bool) {}
}
