//! Window messages GPUI leaves to the system: size limits and the aspect ratio of a live resize.

use std::cell::RefCell;
use std::collections::HashMap;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    MINMAXINFO, WM_GETMINMAXINFO, WM_NCDESTROY, WM_SIZING, WMSZ_BOTTOMLEFT, WMSZ_LEFT, WMSZ_RIGHT,
    WMSZ_TOP, WMSZ_TOPLEFT, WMSZ_TOPRIGHT,
};

use super::window::{frame, scale};

const ID: usize = 0x6173_7273;

type Size = (f64, f64);

#[derive(Clone, Copy, Default)]
struct Constraints {
    /// Content sizes in DIP.
    min: Option<Size>,
    max: Option<Size>,
    aspect_ratio: Option<f64>,
}

thread_local! {
    static WINDOWS: RefCell<HashMap<isize, Constraints>> = RefCell::default();
}

fn update(hwnd: HWND, f: impl FnOnce(&mut Constraints)) {
    WINDOWS.with_borrow_mut(|windows| f(windows.entry(hwnd.0 as isize).or_default()));
    // SAFETY: windows belong to this thread; installing the same procedure twice updates it.
    let _ = unsafe { SetWindowSubclass(hwnd, Some(procedure), ID, 0) };
}

pub fn set_limits(hwnd: HWND, min: Option<Size>, max: Option<Size>) {
    update(hwnd, |constraints| {
        constraints.min = min;
        constraints.max = max;
    });
}

/// `HWNDMessageHandler::SetAspectRatio`, less the size it gives the window at once: upstream sets
/// the content's size right after.
pub fn set_aspect_ratio(hwnd: HWND, aspect_ratio: f64) {
    let ratio = (aspect_ratio > 0.0).then_some(aspect_ratio);
    update(hwnd, |constraints| constraints.aspect_ratio = ratio);
}

fn constraints(hwnd: HWND) -> Constraints {
    WINDOWS.with_borrow(|windows| windows.get(&(hwnd.0 as isize)).copied().unwrap_or_default())
}

/// A content size in DIP as a window size in device px.
fn track_size(hwnd: HWND, (width, height): Size) -> Option<(i32, i32)> {
    let frame = frame(hwnd)?;
    let device = |v: f64| (v * scale(hwnd)).round() as i32;
    Some((
        device(width).saturating_add(frame.width),
        device(height).saturating_add(frame.height),
    ))
}

/// `gfx::SizeRectToAspectRatio` for the edge being dragged.
fn size_to_aspect_ratio(hwnd: HWND, edge: u32, rect: RECT) -> RECT {
    let c = constraints(hwnd);
    let Some(ratio) = c.aspect_ratio else {
        return rect;
    };
    let min = c.min.and_then(|min| track_size(hwnd, min));
    let max = c.max.and_then(|max| track_size(hwnd, max));
    fit(
        rect,
        edge,
        ratio,
        min.unwrap_or((0, 0)),
        max.unwrap_or((i32::MAX, i32::MAX)),
    )
}

fn fit(rect: RECT, edge: u32, ratio: f64, min: (i32, i32), max: (i32, i32)) -> RECT {
    let round = |v: f64| v.round() as i32;
    let (mut width, mut height) = (rect.right - rect.left, rect.bottom - rect.top);
    if matches!(
        edge,
        WMSZ_LEFT | WMSZ_RIGHT | WMSZ_TOPLEFT | WMSZ_BOTTOMLEFT
    ) {
        height = round(f64::from(width) / ratio);
        if height < min.1 || height > max.1 {
            height = height.clamp(min.1, max.1);
            width = round(f64::from(height) * ratio);
        }
    } else {
        width = round(f64::from(height) * ratio);
        if width < min.0 || width > max.0 {
            width = width.clamp(min.0, max.0);
            height = round(f64::from(width) / ratio);
        }
    }
    width = width.min(max.0).max(min.0);
    height = height.min(max.1).max(min.1);
    let mut sized = RECT {
        left: rect.left,
        top: rect.top,
        right: rect.left + width,
        bottom: rect.top + height,
    };
    if matches!(edge, WMSZ_LEFT | WMSZ_TOPLEFT | WMSZ_BOTTOMLEFT) {
        sized.left = rect.right - width;
        sized.right = rect.right;
    }
    if matches!(edge, WMSZ_LEFT | WMSZ_TOP | WMSZ_TOPLEFT | WMSZ_TOPRIGHT) {
        sized.top = rect.bottom - height;
        sized.bottom = rect.bottom;
    }
    sized
}

/// SAFETY: called by the system with a live window of this thread and the message's own
/// parameters.
unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // SAFETY: as above; the structures a message's `lparam` points to outlive it.
    unsafe {
        match message {
            WM_GETMINMAXINFO => {
                let result = DefSubclassProc(hwnd, message, wparam, lparam);
                let info = &mut *(lparam.0 as *mut MINMAXINFO);
                let c = constraints(hwnd);
                if let Some((width, height)) = c.min.and_then(|min| track_size(hwnd, min)) {
                    info.ptMinTrackSize.x = width;
                    info.ptMinTrackSize.y = height;
                }
                if let Some((width, height)) = c.max.and_then(|max| track_size(hwnd, max)) {
                    info.ptMaxTrackSize.x = info.ptMaxTrackSize.x.min(width);
                    info.ptMaxTrackSize.y = info.ptMaxTrackSize.y.min(height);
                }
                result
            }
            WM_SIZING if constraints(hwnd).aspect_ratio.is_some() => {
                let rect = &mut *(lparam.0 as *mut RECT);
                *rect = size_to_aspect_ratio(hwnd, wparam.0 as u32, *rect);
                LRESULT(1)
            }
            WM_NCDESTROY => {
                WINDOWS.with_borrow_mut(|windows| windows.remove(&(hwnd.0 as isize)));
                let _ = RemoveWindowSubclass(hwnd, Some(procedure), ID);
                DefSubclassProc(hwnd, message, wparam, lparam)
            }
            _ => DefSubclassProc(hwnd, message, wparam, lparam),
        }
    }
}
