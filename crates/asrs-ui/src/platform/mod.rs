#![allow(unsafe_code)]

#[cfg(not(target_os = "macos"))]
mod fallback;
#[cfg(target_os = "macos")]
mod mac;

#[cfg(not(target_os = "macos"))]
use fallback as imp;
#[cfg(target_os = "macos")]
use mac as imp;

pub mod dialog;
pub mod probe;

use gpui_kit::{App, ScrollDelta, ScrollEvent, Window};

/// Chromium's pixel size of one notched wheel tick on macOS (`kScrollbarPixelsPerCocoaTick`).
pub const PIXELS_PER_WHEEL_TICK: f32 = 40.0;

/// Side length in pixels and points of a menu colour swatch, as upstream builds it with
/// `nativeImage.createFromBitmap`.
pub const SWATCH_SIZE: u32 = 15;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PopupMenu {
    pub items: Vec<PopupItem>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PopupItem {
    Separator,
    Entry(PopupEntry),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PopupEntry {
    pub label: String,
    pub enabled: bool,
    pub visible: bool,
    pub checked: bool,
    pub swatch: Option<[u8; 3]>,
    pub submenu: Option<PopupMenu>,
}

impl PopupMenu {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn entry(mut self, entry: PopupEntry) -> Self {
        self.items.push(PopupItem::Entry(entry));
        self
    }

    pub fn separator(mut self) -> Self {
        self.items.push(PopupItem::Separator);
        self
    }

    pub fn has_visible_items(&self) -> bool {
        self.items.iter().any(|item| match item {
            PopupItem::Separator => true,
            PopupItem::Entry(entry) => entry.visible,
        })
    }
}

impl PopupEntry {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            enabled: true,
            visible: true,
            checked: false,
            swatch: None,
            submenu: None,
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn swatch(mut self, rgb: [u8; 3]) -> Self {
        self.swatch = Some(rgb);
        self
    }

    pub fn submenu(mut self, submenu: PopupMenu) -> Self {
        self.submenu = Some(submenu);
        self
    }
}

/// Index path of a chosen entry: one index per menu level, counting separators.
pub type MenuPath = Vec<usize>;

/// The wheel delta Chromium puts in `WebMouseWheelEvent` (`delta_x`, `delta_y`) for the same
/// native event. Positive y is GPUI's positive y; DOM `deltaY` is its negation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WheelDelta {
    pub x: f32,
    pub y: f32,
}

/// Installs the process-wide hooks (the raw wheel monitor on macOS). Idempotent.
pub fn init(_cx: &mut App) {
    imp::init();
}

/// Electron's `vibrancy: "sidebar"`. The window must be opened with a transparent background.
pub fn install_sidebar_vibrancy(window: &Window) -> bool {
    imp::install_sidebar_vibrancy(window)
}

/// Makes an always-on-top window behave like Electron's `alwaysOnTop: true`, which stays visible
/// while the app is inactive.
pub fn keep_visible_when_inactive(window: &Window) -> bool {
    imp::keep_visible_when_inactive(window)
}

/// Electron's `BrowserWindow.setAspectRatio(aspect_ratio)` with no extra size; zero or less
/// removes the lock.
pub fn set_aspect_ratio(window: &Window, aspect_ratio: f64) {
    imp::set_aspect_ratio(window, aspect_ratio);
}

/// Electron's `titleBarStyle: "hiddenInset"`: an empty toolbar under the transparent title bar,
/// which moves the traffic lights in from the window edge. The window must be opened with a
/// transparent title bar and no `traffic_light_position`.
pub fn hidden_inset_titlebar(window: &Window) -> bool {
    imp::window::hidden_inset_titlebar(window)
}

/// The content size a window frame of `width` by `height` leaves for this window's style, which
/// is how Electron reads frame sizes for windows without `useContentSize`. `None` where the
/// platform has no such conversion.
pub fn content_size_for_frame(window: &Window, width: f64, height: f64) -> Option<(f64, f64)> {
    imp::window::content_size_for_frame(window, width, height)
}

/// Minimum and maximum content sizes, enforced during live resize.
pub fn set_content_size_limits(window: &Window, min: Option<(f64, f64)>, max: Option<(f64, f64)>) {
    imp::window::set_content_size_limits(window, min, max);
}

/// Electron's `fullscreenable` option.
pub fn set_fullscreenable(window: &Window, fullscreenable: bool) {
    imp::window::set_fullscreenable(window, fullscreenable);
}

/// Electron's placement of a window opened without `x` and `y`.
pub fn center_window(window: &Window) {
    imp::window::center(window);
}

/// Tags the window's drawable as sRGB so the compositor converts it to the display's colour
/// space, as Chromium's output is.
pub fn tag_srgb_colorspace(window: &Window) -> bool {
    imp::tag_srgb_colorspace(window)
}

/// Electron's `menu.popup({ window, x, y })`: `x` and `y` are window content coordinates and
/// either being -1 means the mouse location. The menu opens on a later turn of the event loop,
/// and `on_close` gets the chosen entry, if any, once it closes.
pub fn popup_menu(
    window: &Window,
    cx: &mut App,
    menu: PopupMenu,
    x: i32,
    y: i32,
    on_close: impl FnOnce(Option<MenuPath>, &mut App) + 'static,
) {
    imp::popup_menu(window, cx, menu, x, y, on_close);
}

/// Electron's `menu.closePopup(window)`.
pub fn close_popup_menu(window: &Window) {
    imp::close_popup_menu(window);
}

/// The label a native menu shows for `label` (Chromium's `FixUpWindowsStyleLabel` on macOS).
pub fn native_menu_label(label: &str) -> String {
    imp::native_menu_label(label)
}

/// Chromium's wheel delta for a GPUI scroll event. On macOS this is exact for any event the
/// raw wheel hook saw; otherwise it falls back to GPUI's values.
pub fn wheel_delta(event: &ScrollEvent) -> WheelDelta {
    imp::wheel_delta(event).unwrap_or_else(|| fallback_wheel_delta(&event.delta))
}

fn fallback_wheel_delta(delta: &ScrollDelta) -> WheelDelta {
    match delta {
        ScrollDelta::Lines(lines) => WheelDelta {
            x: lines.x * PIXELS_PER_WHEEL_TICK,
            y: lines.y * PIXELS_PER_WHEEL_TICK,
        },
        ScrollDelta::Pixels(pixels) => WheelDelta {
            x: f32::from(pixels.x).round(),
            y: f32::from(pixels.y).round(),
        },
    }
}
