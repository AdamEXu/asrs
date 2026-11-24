use std::ffi::c_void;

use gpui_kit::Window;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_SYSTEMBACKDROP_TYPE, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, GetDC, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
    MOUSE_EVENT_FLAGS, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GWL_EXSTYLE, GetCursorPos, GetMenuItemCount, GetMenuItemInfoW, GetWindowLongPtrW,
    HMENU, IsWindowVisible, MENUITEMINFOW, MFS_CHECKED, MFS_DISABLED, MFT_SEPARATOR, MIIM_BITMAP,
    MIIM_FTYPE, MIIM_ID, MIIM_STATE, MIIM_STRING, MIIM_SUBMENU, MINMAXINFO, SW_MAXIMIZE,
    SW_MINIMIZE, SW_RESTORE, SendMessageW, SetCursorPos, ShowWindow, USER_DEFAULT_SCREEN_DPI,
    WM_DPICHANGED, WM_GETMINMAXINFO, WM_SIZING, WS_EX_TOPMOST,
};
use windows::core::{PWSTR, w};

pub use super::super::fallback::probe::*;
use super::super::probe::{Frame, MouseAction, SizeHints, WheelSample, WindowImage};
use super::super::{PopupEntry, PopupItem, PopupMenu};
use super::menu::NativeMenu;
use super::window::{client_rect, window_rect};

fn handle(number: u32) -> HWND {
    HWND(number as usize as _)
}

pub fn window_number(window: &Window) -> Option<u32> {
    super::hwnd(window).map(|hwnd| hwnd.0 as usize as u32)
}

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
    super::window::native(window).map(|native| native.content_size())
}

pub fn is_visible(window: &Window) -> Option<bool> {
    super::window::native(window).map(|native| native.is_visible())
}

pub fn framed_window() -> Option<(f64, f64)> {
    let (width, height) = super::window::titled_frame();
    Some((f64::from(width), f64::from(height)))
}

pub fn size_hints(_window: &Window, number: u32) -> Option<SizeHints> {
    let hwnd = handle(number);
    let mut info = MINMAXINFO::default();
    // SAFETY: the system copies a message's structure between processes; `info` outlives the call.
    let ex_style = unsafe {
        SendMessageW(
            hwnd,
            WM_GETMINMAXINFO,
            Some(WPARAM(0)),
            Some(LPARAM(&raw mut info as isize)),
        );
        GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32
    };
    Some(SizeHints {
        min: Some((info.ptMinTrackSize.x, info.ptMinTrackSize.y)),
        max: Some((info.ptMaxTrackSize.x, info.ptMaxTrackSize.y)),
        aspect: None,
        above: ex_style & WS_EX_TOPMOST.0 != 0,
    })
}

pub fn backdrop_type(number: u32) -> Option<i32> {
    let mut value = 0i32;
    // SAFETY: the attribute's value is an `i32`.
    unsafe {
        DwmGetWindowAttribute(
            handle(number),
            DWMWA_SYSTEMBACKDROP_TYPE,
            (&raw mut value).cast::<c_void>(),
            size_of::<i32>() as u32,
        )
    }
    .ok()?;
    Some(value)
}

pub fn rects(number: u32) -> Option<[[i32; 4]; 2]> {
    let array = |r: RECT| [r.left, r.top, r.right, r.bottom];
    let hwnd = handle(number);
    Some([array(window_rect(hwnd)?), array(client_rect(hwnd)?)])
}

pub fn sizing(number: u32, edge: u32, [left, top, right, bottom]: [i32; 4]) -> [i32; 4] {
    let mut rect = RECT {
        left,
        top,
        right,
        bottom,
    };
    // SAFETY: the system copies a message's structure between processes; `rect` outlives the call.
    unsafe {
        SendMessageW(
            handle(number),
            WM_SIZING,
            Some(WPARAM(edge as usize)),
            Some(LPARAM(&raw mut rect as isize)),
        );
    }
    [rect.left, rect.top, rect.right, rect.bottom]
}

pub fn change_scale(number: u32, scale: u32) {
    let hwnd = handle(number);
    let Some(rect) = window_rect(hwnd) else {
        return;
    };
    let dpi = (USER_DEFAULT_SCREEN_DPI * scale) as usize;
    // SAFETY: `rect` outlives the call.
    unsafe {
        SendMessageW(
            hwnd,
            WM_DPICHANGED,
            Some(WPARAM(dpi | (dpi << 16))),
            Some(LPARAM(&raw const rect as isize)),
        );
    }
}

pub fn show_window(number: u32, state: &str) {
    let command = match state {
        "minimize" => SW_MINIMIZE,
        "maximize" => SW_MAXIMIZE,
        _ => SW_RESTORE,
    };
    // SAFETY: a closed window makes the call fail.
    let _ = unsafe { ShowWindow(handle(number), command) };
}

/// The screen's pixels in device px.
pub fn screen_image(x: i32, y: i32, width: i32, height: i32) -> Option<WindowImage> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    // SAFETY: the section has `width * height` 32-bit pixels, read after GDI has written them.
    let bgr = unsafe {
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        let previous = SelectObject(memory, bitmap.into());
        let copied = BitBlt(memory, 0, 0, width, height, Some(screen), x, y, SRCCOPY);
        let _ = GdiFlush();
        let pixels = std::slice::from_raw_parts(bits.cast::<u8>(), (width * height * 4) as usize);
        let pixels = pixels.to_vec();
        SelectObject(memory, previous);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        copied.ok()?;
        pixels
    };
    let (pixels, _) = bgr.as_chunks::<4>();
    Some(WindowImage {
        width: width as u32,
        height: height as u32,
        rgba: pixels
            .iter()
            .flat_map(|[b, g, r, _]| [*r, *g, *b, 255])
            .collect(),
    })
}

fn send(inputs: &[INPUT]) -> bool {
    // SAFETY: no preconditions.
    let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    sent as usize == inputs.len()
}

fn mouse(flags: MOUSE_EVENT_FLAGS, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dwFlags: flags,
                mouseData: data as u32,
                ..Default::default()
            },
        },
    }
}

pub fn warp_cursor(x: f64, y: f64) {
    // SAFETY: no preconditions.
    unsafe { SetCursorPos(x as i32, y as i32) }.ok();
}

pub fn cursor_location() -> (f64, f64) {
    let mut point = POINT::default();
    // SAFETY: `point` outlives the call.
    unsafe { GetCursorPos(&mut point) }.ok();
    (f64::from(point.x), f64::from(point.y))
}

pub fn post_mouse(action: MouseAction, x: f64, y: f64) -> bool {
    warp_cursor(x, y);
    let flags = match action {
        MouseAction::Down => MOUSEEVENTF_LEFTDOWN,
        MouseAction::Up => MOUSEEVENTF_LEFTUP,
        MouseAction::Drag => MOUSEEVENTF_MOVE,
    };
    send(&[mouse(flags, 0)])
}

/// `sample.delta` is in wheel units here, 120 a notch.
pub fn post_wheel_at(x: f64, y: f64, sample: &WheelSample) -> bool {
    warp_cursor(x, y);
    let [dy, dx] = sample.delta;
    let axes = [(dy, MOUSEEVENTF_WHEEL), (dx, MOUSEEVENTF_HWHEEL)];
    axes.into_iter()
        .filter(|(delta, _)| *delta != 0)
        .all(|(delta, flags)| send(&[mouse(flags, delta)]))
}

/// Presses the keys of virtual key codes in turn, then releases them.
pub fn post_keys(keys: &[u32]) -> bool {
    let key = |code: u32, flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(code as u16),
                dwFlags: flags,
                ..Default::default()
            },
        },
    };
    let down = keys.iter().map(|code| key(*code, KEYBD_EVENT_FLAGS(0)));
    let up = keys.iter().rev().map(|code| key(*code, KEYEVENTF_KEYUP));
    send(&down.chain(up).collect::<Vec<_>>())
}

/// The open native menu's window, the class of which is `#32768`.
pub fn menu_window_frames() -> Vec<Frame> {
    // SAFETY: no preconditions.
    let found = unsafe { FindWindowW(w!("#32768"), None) };
    // SAFETY: a closed window is not visible.
    let shown = found
        .ok()
        .filter(|hwnd| unsafe { IsWindowVisible(*hwnd).as_bool() });
    shown
        .and_then(window_rect)
        .map(|r| Frame {
            x: f64::from(r.left),
            y: f64::from(r.top),
            width: f64::from(r.right - r.left),
            height: f64::from(r.bottom - r.top),
        })
        .into_iter()
        .collect()
}

pub fn native_menu_roundtrip(menu: &PopupMenu) -> Option<PopupMenu> {
    let native = NativeMenu::build(menu)?;
    read(native.handle)
}

fn read(menu: HMENU) -> Option<PopupMenu> {
    // SAFETY: the menu is open.
    let count = unsafe { GetMenuItemCount(Some(menu)) };
    let mut out = PopupMenu::new();
    for index in 0..u32::try_from(count).ok()? {
        let mut text = [0u16; 256];
        let mut info = MENUITEMINFOW {
            cbSize: size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_FTYPE | MIIM_STATE | MIIM_STRING | MIIM_SUBMENU | MIIM_BITMAP | MIIM_ID,
            dwTypeData: PWSTR(text.as_mut_ptr()),
            cch: text.len() as u32,
            ..Default::default()
        };
        // SAFETY: `info`'s text buffer is as long as `cch` says.
        unsafe { GetMenuItemInfoW(menu, index, true, &mut info) }.ok()?;
        if info.fType.contains(MFT_SEPARATOR) {
            out.items.push(PopupItem::Separator);
            continue;
        }
        let text = String::from_utf16_lossy(&text[..info.cch as usize]);
        let (label, shortcut) = match text.split_once('\t') {
            Some((label, shortcut)) => (label, Some(shortcut)),
            None => (text.as_str(), None),
        };
        let mut entry = PopupEntry::new(label)
            .enabled(!info.fState.contains(MFS_DISABLED))
            .checked(info.fState.contains(MFS_CHECKED));
        if let Some(shortcut) = shortcut {
            entry = entry.shortcut(shortcut);
        }
        if !info.hbmpItem.is_invalid() {
            entry = entry.swatch(read_swatch(info.hbmpItem)?);
        }
        if !info.hSubMenu.is_invalid() {
            entry = entry.submenu(read(info.hSubMenu)?);
        }
        out.items.push(PopupItem::Entry(entry));
    }
    Some(out)
}

fn read_swatch(bitmap: windows::Win32::Graphics::Gdi::HBITMAP) -> Option<[u8; 3]> {
    use windows::Win32::Graphics::Gdi::{DIBSECTION, GetObjectW};
    let mut section = DIBSECTION::default();
    // SAFETY: the bitmap is a DIB section whose bits live as long as it does.
    let pixel = unsafe {
        let size = size_of::<DIBSECTION>() as i32;
        if GetObjectW(bitmap.into(), size, Some((&raw mut section).cast())) != size {
            return None;
        }
        *section.dsBm.bmBits.cast::<u32>()
    };
    let [_, r, g, b] = pixel.to_be_bytes();
    Some([r, g, b])
}
