//! Windows: `alwaysOnTop`, the acrylic backdrop, native popup menus and the wheel mapping over
//! GPUI's window handles. Select lists are drawn (`crate::menu`), as Chromium draws its own, and
//! leave the activation with the window they are for.

use std::cell::{Cell, RefCell};
use std::os::windows::io::AsRawHandle;
use std::rc::Rc;

use gpui_kit::{
    AnyWindowHandle, App, AsyncApp, Bounds, Pixels, ScrollDelta, ScrollEvent, Subscription,
    WeakEntity, Window, WindowId, point, px, size,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Wdk::System::SystemServices::RtlGetVersion;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWM_SYSTEMBACKDROP_TYPE, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
};
use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    EndMenu, GWL_EXSTYLE, GetWindowLongPtrW, HWND_TOPMOST, MA_NOACTIVATE, PostMessageW,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowLongPtrW, SetWindowPos, WM_CANCELMODE,
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_MOUSEACTIVATE, WM_NCDESTROY, WM_NCLBUTTONDBLCLK,
    WM_NCLBUTTONDOWN, WS_EX_NOACTIVATE,
};
use windows::core::BOOL;

pub(super) use super::fallback::*;
use super::{MenuPath, PopupEntry, PopupMenu, SelectMenu, SelectMenuItemKind, WheelDelta};
use crate::menu::{CloseHandler, ListStyle, MenuView, Placement};

pub(super) mod dialog;
mod menu;
#[cfg(feature = "parity")]
pub(crate) mod offscreen;
pub(super) mod probe;
mod subclass;
pub(super) mod window;

/// The DOM delta of one line: Chromium's 100 px a notch over GPUI's 3 lines.
const PIXELS_PER_LINE: f32 = 100.0 / 3.0;

/// Windows 11 22H2, the first with `DWMWA_SYSTEMBACKDROP_TYPE`, as `main.ts` asks for acrylic.
const ACRYLIC_BUILD: u32 = 22621;

const LIST_SUBCLASS: usize = 0x6c69_7374;

struct OpenList {
    id: u64,
    parent: WindowId,
    parent_hwnd: isize,
    view: WeakEntity<MenuView>,
    window: AnyWindowHandle,
    cx: AsyncApp,
    pressed: async_channel::Sender<()>,
    _keys: Subscription,
    _parent_closed: Subscription,
}

thread_local! {
    /// The window whose native menu is open.
    static MENU_OWNER: Cell<Option<isize>> = const { Cell::new(None) };
    static LIST: RefCell<Option<OpenList>> = const { RefCell::new(None) };
    static NEXT_LIST: Cell<u64> = const { Cell::new(0) };
}

pub(super) fn init() {}

pub(super) fn attach_console() {
    if std::io::stdout().as_raw_handle().is_null() {
        // SAFETY: no pointers; without a console to attach to it fails and changes nothing.
        let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }
}

fn version() -> OSVERSIONINFOW {
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is sized as its first field says.
    let _ = unsafe { RtlGetVersion(&mut info) };
    info
}

/// `os.release()`.
pub(super) fn os_release() -> String {
    let info = version();
    format!(
        "{}.{}.{}",
        info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber
    )
}

pub(super) fn has_acrylic() -> bool {
    version().dwBuildNumber >= ACRYLIC_BUILD
}

fn hwnd(window: &Window) -> Option<HWND> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as _)),
        _ => None,
    }
}

/// `Window` has a method of this name in two traits.
fn handle_of(window: &Window) -> AnyWindowHandle {
    Window::window_handle(window)
}

/// Electron's `backgroundMaterial: "acrylic"`: the DWM backdrop, with the frame extended over the
/// client area so that it shows through the transparent pixels.
pub(super) fn install_sidebar_vibrancy(window: &Window) -> bool {
    let Some(hwnd) = hwnd(window).filter(|_| has_acrylic()) else {
        return false;
    };
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    let backdrop = DWMSBT_TRANSIENTWINDOW;
    // SAFETY: the window is open, and the attribute's value is a DWM_SYSTEMBACKDROP_TYPE.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            (&raw const backdrop).cast(),
            size_of::<DWM_SYSTEMBACKDROP_TYPE>() as u32,
        )
        .and_then(|()| DwmExtendFrameIntoClientArea(hwnd, &margins))
        .is_ok()
    }
}

pub(super) fn set_dark_frame(window: &Window, dark: bool) {
    let Some(hwnd) = hwnd(window) else {
        return;
    };
    let dark = BOOL::from(dark);
    // SAFETY: the window is open, and the attribute's value is a BOOL.
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const dark).cast(),
            size_of::<BOOL>() as u32,
        )
    };
}

/// Chromium's `alwaysOnTop`, `WS_EX_TOPMOST`.
pub(super) fn keep_visible_when_inactive(window: &Window) -> bool {
    let Some(hwnd) = hwnd(window) else {
        return false;
    };
    let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
    // SAFETY: the window is open.
    unsafe { SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, flags) }.is_ok()
}

pub(super) fn set_aspect_ratio(window: &Window, aspect_ratio: f64) {
    if let Some(hwnd) = hwnd(window) {
        subclass::set_aspect_ratio(hwnd, aspect_ratio);
    }
}

/// GPUI's lines already have the system's lines a notch in them, as Chromium's deltas do.
pub(super) fn wheel_delta(event: &ScrollEvent) -> Option<WheelDelta> {
    match event.delta {
        ScrollDelta::Lines(lines) => Some(WheelDelta {
            x: lines.x * PIXELS_PER_LINE,
            y: lines.y * PIXELS_PER_LINE,
        }),
        ScrollDelta::Pixels(_) => None,
    }
}

/// A menu's `&` marks mnemonics, which Win32 underlines itself; a drawn menu shows none.
pub(super) fn native_menu_label(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut chars = label.chars();
    while let Some(c) = chars.next() {
        match c {
            '&' => out.extend(chars.next()),
            c => out.push(c),
        }
    }
    out
}

/// `TrackPopupMenuEx` runs its own message loop, through which GPUI draws and runs tasks, so it
/// is entered with the app not borrowed.
pub(super) fn popup_menu(
    window: &Window,
    cx: &mut App,
    menu: PopupMenu,
    x: i32,
    y: i32,
    on_close: impl FnOnce(Option<MenuPath>, &mut App) + 'static,
) {
    let Some(hwnd) = hwnd(window) else {
        cx.defer(move |cx| on_close(None, cx));
        return;
    };
    let at = (x != -1 && y != -1).then(|| {
        let scale = window.scale_factor();
        let device = |v: i32| (v as f32 * scale).round() as i32;
        (device(x), device(y))
    });
    let owner = hwnd.0 as isize;
    cx.spawn(async move |cx| {
        let chosen = menu::NativeMenu::build(&menu).and_then(|native| {
            MENU_OWNER.set(Some(owner));
            let chosen = native.track(HWND(owner as _), at);
            MENU_OWNER.set(None);
            chosen
        });
        cx.update(|cx| on_close(chosen, cx));
    })
    .detach();
}

pub(super) fn menu_takes_keys(window: &Window) -> bool {
    let id = handle_of(window).window_id();
    LIST.with_borrow(|list| list.as_ref().is_some_and(|list| list.parent == id))
}

pub(super) fn close_popup_menu(window: &Window) {
    if menu_takes_keys(window) {
        close_open_list();
    }
    let Some(hwnd) = hwnd(window) else {
        return;
    };
    if MENU_OWNER.get() == Some(hwnd.0 as isize) {
        // SAFETY: ends the menu of this thread; the message is for a menu about to open its loop.
        unsafe {
            if EndMenu().is_err() {
                PostMessageW(Some(hwnd), WM_CANCELMODE, WPARAM(0), LPARAM(0)).ok();
            }
        }
    }
}

pub(super) fn select_popup(
    window: &Window,
    cx: &mut App,
    menu: SelectMenu,
    [x, y, width, height]: [f64; 4],
    on_close: impl FnOnce(Option<usize>, &mut App) + 'static,
) {
    let px = |v: f64| px(v as f32);
    let anchor = Bounds::new(point(px(x), px(y)), size(px(width), px(height)));
    let list = ListStyle {
        min_width: width as f32,
        selected: menu.selected,
        groups: menu
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.kind == SelectMenuItemKind::Group)
            .map(|(i, _)| i)
            .collect(),
    };
    let mut popup = PopupMenu::new();
    for (i, item) in menu.items.into_iter().enumerate() {
        popup = match item.kind {
            SelectMenuItemKind::Separator => popup.separator(),
            kind => popup.entry(
                // The label is text, in which `&` marks nothing.
                PopupEntry::new(item.label.replace('&', "&&"))
                    .enabled(item.enabled && kind == SelectMenuItemKind::Option)
                    .checked(menu.selected == Some(i)),
            ),
        };
    }
    let parent = handle_of(window);
    let parent_hwnd = hwnd(window);
    cx.defer(move |cx| {
        let opened = parent.update(cx, |_, window, _| {
            let bounds = window::screen_bounds(window, anchor);
            let monitor = window::monitor_bounds(window);
            (bounds.unwrap_or(anchor), monitor, window.scale_factor())
        });
        let (Ok((anchor, monitor, scale)), Some(parent_hwnd)) = (opened, parent_hwnd) else {
            return on_close(None, cx);
        };
        if let Some(open) = LIST.take() {
            open.window
                .update(cx, |_, window, _| window.remove_window())
                .ok();
        }
        let id = NEXT_LIST.replace(NEXT_LIST.get() + 1);
        let on_close = Rc::new(Cell::new(Some(on_close)));
        let close = on_close.clone();
        let handler: CloseHandler = Rc::new(move |path, _, cx| {
            LIST.with_borrow_mut(|list| list.take_if(|list| list.id == id));
            if let Some(on_close) = close.take() {
                let chosen = path.and_then(|path| path.first().copied());
                cx.defer(move |cx| on_close(chosen, cx));
            }
        });
        let placement = Placement::Screen(anchor, monitor);
        let Some((handle, view)) = MenuView::open(popup, placement, Some(list), scale, cx, handler)
        else {
            if let Some(on_close) = on_close.take() {
                on_close(None, cx);
            }
            return;
        };
        parent
            .update(cx, |_, window, cx| {
                view.update(cx, |view, cx| view.dismiss_with(window, cx));
            })
            .ok();
        let (pressed, presses) = async_channel::unbounded();
        cx.spawn(async move |_| {
            if presses.recv().await.is_ok() {
                close_open_list();
            }
        })
        .detach();
        // SAFETY: the window belongs to this thread; installing the procedure again updates it.
        let _ = unsafe { SetWindowSubclass(parent_hwnd, Some(parent_procedure), LIST_SUBCLASS, 0) };
        let keys = crate::menu::intercept_keys(&view, cx);
        let parent_closed = cx.on_window_closed(move |_, closed| {
            if closed == parent.window_id() {
                close_open_list();
            }
        });
        LIST.set(Some(OpenList {
            id,
            parent: parent.window_id(),
            parent_hwnd: parent_hwnd.0 as isize,
            view: view.downgrade(),
            window: handle,
            cx: cx.to_async(),
            pressed,
            _keys: keys,
            _parent_closed: parent_closed,
        }));
    });
}

fn close_open_list() {
    if let Some(open) = LIST.take() {
        let OpenList {
            view, window, cx, ..
        } = open;
        cx.spawn(async move |cx| {
            window
                .update(cx, |_, window, cx| {
                    view.update(cx, |view, cx| view.cancel(window, cx))
                })
                .ok();
        })
        .detach();
    }
}

/// Blink's `WebViewImpl::HandleMouseDown`: a left press in the page closes its open list, and the
/// page still gets it.
///
/// SAFETY: called by the system with a live window of this thread and the message's own
/// parameters.
unsafe extern "system" fn parent_procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if matches!(
        message,
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK
    ) {
        LIST.with(|list| {
            if let Ok(list) = list.try_borrow()
                && let Some(list) = list.as_ref()
                && list.parent_hwnd == hwnd.0 as isize
            {
                let _ = list.pressed.try_send(());
            }
        });
    }
    // SAFETY: as above.
    unsafe {
        if message == WM_NCDESTROY {
            let _ = RemoveWindowSubclass(hwnd, Some(parent_procedure), LIST_SUBCLASS);
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}

/// GPUI answers `WM_MOUSEACTIVATE` with `MA_ACTIVATE`, which a press on a `WS_EX_NOACTIVATE`
/// window would otherwise follow.
///
/// SAFETY: as [`parent_procedure`].
unsafe extern "system" fn list_procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // SAFETY: as above.
    unsafe {
        match message {
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_NCDESTROY => {
                let _ = RemoveWindowSubclass(hwnd, Some(list_procedure), LIST_SUBCLASS);
                DefSubclassProc(hwnd, message, wparam, lparam)
            }
            _ => DefSubclassProc(hwnd, message, wparam, lparam),
        }
    }
}

/// GPUI takes half of the frame below a window's content for a frame above it, and opens a window
/// that is not on the primary monitor somewhere on that. A list, like Chromium's, cannot be
/// activated.
pub(super) fn popup_window_opened(
    window: &Window,
    bounds: Bounds<Pixels>,
    root: bool,
    cx: &mut App,
) {
    if let Some(hwnd) = hwnd(window).filter(|_| root) {
        // SAFETY: the window belongs to this thread and is open.
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_NOACTIVATE.0 as isize);
            let _ = SetWindowSubclass(hwnd, Some(list_procedure), LIST_SUBCLASS, 0);
        }
    }
    let Some(native) = window::native(window) else {
        return;
    };
    cx.spawn(async move |_| {
        let px = |v: Pixels| f64::from(f32::from(v));
        native.set_content_origin(px(bounds.origin.x), px(bounds.origin.y));
        native.set_content_size(px(bounds.size.width), px(bounds.size.height));
    })
    .detach();
}

fn logical(rect: RECT, scale: f32) -> Bounds<Pixels> {
    let px = |v: i32| px(v as f32 / scale);
    Bounds::new(
        point(px(rect.left), px(rect.top)),
        size(px(rect.right - rect.left), px(rect.bottom - rect.top)),
    )
}
