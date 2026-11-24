//! The parts of Electron's `NativeWindowViewsWin` that GPUI's window options do not cover.

use gpui_kit::{Bounds, Pixels, Window};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowRect, IsIconic, IsWindowVisible, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SetWindowPos, USER_DEFAULT_SCREEN_DPI,
};

use super::subclass;

#[derive(Clone, Copy)]
pub struct NativeWindow(HWND);

#[cfg(feature = "parity")]
pub fn titled_content_size(width: f64, height: f64) -> (f64, f64) {
    let (w, h) = titled_frame();
    (width - f64::from(w), height - f64::from(h))
}

/// How much larger a titled, resizable window's bounds are than its content at scale 1.
pub fn titled_frame() -> (i32, i32) {
    use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;
    use windows::Win32::UI::WindowsAndMessaging::{WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW};
    let mut rect = RECT::default();
    // SAFETY: `rect` outlives the call.
    let _ = unsafe {
        AdjustWindowRectExForDpi(
            &mut rect,
            WS_OVERLAPPEDWINDOW,
            false,
            WINDOW_EX_STYLE(0),
            USER_DEFAULT_SCREEN_DPI,
        )
    };
    (rect.right - rect.left, rect.bottom - rect.top)
}

pub fn native(window: &Window) -> Option<NativeWindow> {
    super::hwnd(window).map(NativeWindow)
}

pub fn bounds(window: &Window) -> Option<(f64, f64, f64, f64)> {
    let native = native(window)?;
    let rect = native.bounds()?;
    let f = |v: i32| f64::from(v) / native.scale();
    Some((
        f(rect.left),
        f(rect.top),
        f(rect.right - rect.left),
        f(rect.bottom - rect.top),
    ))
}

/// A box of the window's content on the screen.
pub fn screen_bounds(window: &Window, bounds: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
    let native = native(window)?;
    let client = native.client()?;
    let origin = super::logical(client, native.scale() as f32).origin;
    Some(Bounds::new(origin + bounds.origin, bounds.size))
}

/// The work area of the window's monitor.
pub fn monitor_bounds(window: &Window) -> Option<Bounds<Pixels>> {
    let native = native(window)?;
    Some(super::logical(work_area(native.0)?, native.scale() as f32))
}

/// A window's frame in device px.
pub struct Frame {
    pub width: i32,
    pub height: i32,
}

pub fn window_rect(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    // SAFETY: `rect` outlives the call; a closed window makes it fail.
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    Some(rect)
}

pub fn client_rect(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    let mut origin = POINT::default();
    // SAFETY: the pointers outlive the calls; a closed window makes them fail.
    unsafe {
        GetClientRect(hwnd, &mut rect).ok()?;
        ClientToScreen(hwnd, &mut origin).ok().ok()?;
    }
    Some(RECT {
        left: origin.x,
        top: origin.y,
        right: origin.x + rect.right,
        bottom: origin.y + rect.bottom,
    })
}

pub fn frame(hwnd: HWND) -> Option<Frame> {
    let (window, client) = (window_rect(hwnd)?, client_rect(hwnd)?);
    Some(Frame {
        width: (window.right - window.left) - (client.right - client.left),
        height: (window.bottom - window.top) - (client.bottom - client.top),
    })
}

fn work_area(hwnd: HWND) -> Option<RECT> {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` outlives the call.
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(monitor, &mut info).ok().ok()?;
    }
    Some(info.rcWork)
}

/// Electron's frameless windows have no title bar, and their bounds are the content's.
pub fn frameless(hwnd: HWND) -> bool {
    matches!((window_rect(hwnd), client_rect(hwnd)), (Some(w), Some(c)) if w.top == c.top)
}

pub fn scale(hwnd: HWND) -> f64 {
    // SAFETY: a closed window gives 0.
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    f64::from(dpi.max(1)) / f64::from(USER_DEFAULT_SCREEN_DPI)
}

impl NativeWindow {
    fn scale(&self) -> f64 {
        scale(self.0)
    }

    fn client(&self) -> Option<RECT> {
        client_rect(self.0)
    }

    /// `getBounds()` in device px.
    fn bounds(&self) -> Option<RECT> {
        if frameless(self.0) {
            self.client()
        } else {
            window_rect(self.0)
        }
    }

    pub fn hidden_inset_titlebar(&self) {}

    pub fn content_size_for_frame(&self, width: f64, height: f64) -> (f64, f64) {
        match frame(self.0) {
            Some(frame) if !frameless(self.0) => (
                width - f64::from(frame.width) / self.scale(),
                height - f64::from(frame.height) / self.scale(),
            ),
            _ => (width, height),
        }
    }

    pub fn content_size(&self) -> (f64, f64) {
        self.client().map_or((0.0, 0.0), |client| {
            (
                f64::from(client.right - client.left) / self.scale(),
                f64::from(client.bottom - client.top) / self.scale(),
            )
        })
    }

    pub fn set_content_size(&self, width: f64, height: f64) {
        let Some(frame) = frame(self.0) else {
            return;
        };
        let device = |v: f64| (v * self.scale()).round() as i32;
        let flags = SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE;
        let (width, height) = (device(width) + frame.width, device(height) + frame.height);
        // SAFETY: a closed window makes the call fail.
        unsafe { SetWindowPos(self.0, None, 0, 0, width, height, flags) }.ok();
    }

    pub fn set_content_origin(&self, x: f64, y: f64) {
        let device = |v: f64| (v * self.scale()).round() as i32;
        if let Some(bounds) = self.bounds() {
            self.move_bounds(bounds, device(x), device(y));
        }
    }

    fn move_bounds(&self, bounds: RECT, x: i32, y: i32) {
        let Some(window) = window_rect(self.0) else {
            return;
        };
        let flags = SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE;
        let x = x - (bounds.left - window.left);
        let y = y - (bounds.top - window.top);
        // SAFETY: a closed window makes the call fail.
        unsafe { SetWindowPos(self.0, None, x, y, 0, 0, flags) }.ok();
    }

    /// `CenterAndSizeWindow`, over the parent's bounds or on the work area.
    pub fn center(&self, parent: Option<&NativeWindow>) {
        let over = parent.and_then(NativeWindow::bounds);
        let (Some(bounds), Some(over)) = (self.bounds(), over.or_else(|| work_area(self.0))) else {
            return;
        };
        let start = |low: i32, high: i32, size: i32| low + ((high - low - size) / 2).max(0);
        let x = start(over.left, over.right, bounds.right - bounds.left);
        let y = start(over.top, over.bottom, bounds.bottom - bounds.top);
        self.move_bounds(bounds, x, y);
    }

    pub fn set_content_size_limits(&self, min: Option<(f64, f64)>, max: Option<(f64, f64)>) {
        subclass::set_limits(self.0, min, max);
    }

    pub fn set_fullscreenable(&self, _fullscreenable: bool) {}

    pub fn begin_sheet(&self, _sheet: &NativeWindow) {}

    pub fn end_sheet(&self) {}

    pub fn is_visible(&self) -> bool {
        // SAFETY: no preconditions.
        unsafe { IsWindowVisible(self.0).as_bool() && !IsIconic(self.0).as_bool() }
    }

    pub fn show(&self, _focus: bool) {}
}
