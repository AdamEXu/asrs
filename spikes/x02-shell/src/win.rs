use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMSBT_TRANSIENTWINDOW, DWMWA_EXTENDED_FRAME_BOUNDS, DWMWA_SYSTEMBACKDROP_TYPE,
    DwmGetWindowAttribute, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, ClientToScreen, CreateCompatibleBitmap,
    CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC,
    SRCCOPY, SelectObject,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetClientRect, GetWindowLongPtrW, GetWindowRect, GetWindowTextW,
    HWND_TOPMOST, IsWindowVisible, SPI_GETWHEELSCROLLCHARS, SPI_GETWHEELSCROLLLINES,
    SWP_NOMOVE, SWP_NOSIZE, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetCursorPos, SetWindowPos,
    SystemParametersInfoW, WS_EX_TOPMOST,
};

pub fn hwnd(window: &impl raw_window_handle::HasWindowHandle) -> Option<HWND> {
    match window.window_handle().ok()?.as_raw() {
        raw_window_handle::RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as *mut _)),
        _ => None,
    }
}

pub fn is_topmost(hwnd: HWND) -> bool {
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    style & WS_EX_TOPMOST.0 != 0
}

pub fn set_topmost(hwnd: HWND) -> bool {
    unsafe { SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE).is_ok() }
}

pub fn set_acrylic(hwnd: HWND) -> String {
    let value = DWMSBT_TRANSIENTWINDOW;
    let hr = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &value as *const _ as *const _,
            std::mem::size_of_val(&value) as u32,
        )
    };
    format!("{hr:?}")
}

pub fn backdrop(hwnd: HWND) -> i32 {
    let mut value = -1i32;
    let _ = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &mut value as *mut _ as *mut _,
            4,
        )
    };
    value
}

pub fn client_rect(hwnd: HWND) -> [i32; 4] {
    let mut rect = RECT::default();
    let mut origin = POINT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rect);
        let _ = ClientToScreen(hwnd, &mut origin);
    }
    [origin.x, origin.y, rect.right, rect.bottom]
}

pub fn window_rect(hwnd: HWND) -> [i32; 4] {
    let mut rect = RECT::default();
    let mut frame = RECT::default();
    unsafe {
        let _ = GetWindowRect(hwnd, &mut rect);
        let _ = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut frame as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        );
    }
    [frame.left, frame.top, frame.right - frame.left, frame.bottom - frame.top]
}

pub fn capture_screen(x: i32, y: i32, w: i32, h: i32) -> image::RgbaImage {
    let mut bgra = vec![0u8; (w * h * 4) as usize];
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bitmap.into());
        let _ = BitBlt(mem, 0, 0, w, h, Some(screen), x, y, SRCCOPY);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        GetDIBits(
            mem,
            bitmap,
            0,
            h as u32,
            Some(bgra.as_mut_ptr() as *mut _),
            &mut info,
            DIB_RGB_COLORS,
        );
        SelectObject(mem, old);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
    }
    for p in bgra.chunks_exact_mut(4) {
        p.swap(0, 2);
        p[3] = 255;
    }
    image::RgbaImage::from_raw(w as u32, h as u32, bgra).expect("size")
}

pub fn wheel_settings() -> (u32, u32) {
    let mut lines = 0u32;
    let mut chars = 0u32;
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETWHEELSCROLLLINES,
            0,
            Some(&mut lines as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        let _ = SystemParametersInfoW(
            SPI_GETWHEELSCROLLCHARS,
            0,
            Some(&mut chars as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    (lines, chars)
}

/// Deltas in WHEEL_DELTA units of 1/120 notch; `h` before a value sends a horizontal event.
pub fn send_wheel(x: i32, y: i32, deltas: &[String]) {
    unsafe {
        let _ = SetCursorPos(x, y);
    }
    std::thread::sleep(Duration::from_millis(300));
    for d in deltas {
        let (flags, value) = match d.strip_prefix('h') {
            Some(v) => (MOUSEEVENTF_HWHEEL, v.parse::<i32>().unwrap_or(0)),
            None => (MOUSEEVENTF_WHEEL, d.parse::<i32>().unwrap_or(0)),
        };
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    mouseData: value as u32,
                    dwFlags: flags,
                    ..Default::default()
                },
            },
        };
        unsafe {
            SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
        }
        std::thread::sleep(Duration::from_millis(400));
    }
}

pub fn windows_matching(needle: &str) -> Vec<String> {
    struct State {
        needle: String,
        out: Vec<String>,
    }
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
        let state = unsafe { &mut *(lparam.0 as *mut State) };
        let mut title = [0u16; 256];
        let n = unsafe { GetWindowTextW(hwnd, &mut title) } as usize;
        let title = String::from_utf16_lossy(&title[..n]);
        if unsafe { IsWindowVisible(hwnd) }.as_bool() && title.contains(&state.needle) {
            state.out.push(format!(
                "{title:?}: topmost={} backdrop={} frame={:?} client={:?}",
                is_topmost(hwnd),
                backdrop(hwnd),
                window_rect(hwnd),
                client_rect(hwnd)
            ));
        }
        true.into()
    }
    let mut state = State {
        needle: needle.to_owned(),
        out: Vec::new(),
    };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut state as *mut State as isize));
    }
    state.out
}
