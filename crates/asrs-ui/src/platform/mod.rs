#![allow(unsafe_code)]

#[cfg(not(target_os = "macos"))]
#[cfg_attr(target_os = "linux", allow(dead_code))]
mod fallback;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod mac;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
use fallback as imp;
#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(target_os = "macos")]
use mac as imp;

pub mod dialog;
#[cfg(not(target_os = "linux"))]
pub mod parent;
pub mod probe;

use std::path::Path;

use gpui_kit::{App, Bounds, Pixels, ScrollDelta, ScrollEvent, Window};

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
    /// A macOS key equivalent and `NSEventModifierFlags` mask.
    pub key_equivalent: Option<(String, u32)>,
    /// The accelerator as a drawn menu shows it.
    pub shortcut: Option<String>,
    pub works_when_hidden: bool,
    /// The item's Electron role, lowercased. On macOS some roles become AppKit selectors or
    /// system menus, as `ElectronMenuController` makes them.
    pub role: Option<String>,
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
            key_equivalent: None,
            shortcut: None,
            works_when_hidden: true,
            role: None,
        }
    }

    pub fn key_equivalent(mut self, key: impl Into<String>, modifiers: u32) -> Self {
        self.key_equivalent = Some((key.into(), modifiers));
        self
    }

    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn works_when_hidden(mut self, works: bool) -> Self {
        self.works_when_hidden = works;
        self
    }

    pub fn role(mut self, role: impl Into<String>) -> Self {
        self.role = Some(role.into());
        self
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

/// Electron's `app.addRecentDocument(path)`. Only macOS has it so far (#279).
pub fn add_recent_document(cx: &mut App, path: &Path) {
    #[cfg(feature = "parity")]
    if cx.has_global::<dialog::Script>() {
        let script = cx.global_mut::<dialog::Script>();
        script.recent_documents.push(path.to_owned());
        return;
    }
    let _ = cx;
    imp::add_recent_document(path);
}

/// Installs the process-wide hooks (the raw wheel monitor on macOS). Idempotent.
pub fn init(_cx: &mut App) {
    imp::init();
}

/// Whether the key event being handled is the numpad's Enter, which GPUI also calls `enter`.
/// Off macOS nothing tells the two apart (#262).
pub fn numpad_enter(cx: &App) -> bool {
    #[cfg(feature = "parity")]
    if let Some(scripted) = cx.try_global::<ScriptedNumpadEnter>() {
        return scripted.0;
    }
    let _ = cx;
    imp::numpad_enter()
}

/// What [`numpad_enter`] says where a test dispatches the keystrokes.
#[cfg(feature = "parity")]
pub struct ScriptedNumpadEnter(pub bool);

#[cfg(feature = "parity")]
impl gpui_kit::Global for ScriptedNumpadEnter {}

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

/// A window's native handle for the work AppKit answers with synchronous resize and move
/// notifications. GPUI handles those by borrowing the app, so these calls belong in a
/// foreground task outside any window update. `None` on Windows and Wayland.
pub fn native_window(window: &Window) -> Option<NativeWindow> {
    imp::window::native(window).map(NativeWindow)
}

/// Electron's `getBounds()`: the window frame (on X11 its content) as `(x, y, width, height)` in
/// top-left coordinates of the primary screen. `None` on Windows and Wayland.
pub fn window_bounds(window: &Window) -> Option<(f64, f64, f64, f64)> {
    imp::window::bounds(window)
}

pub struct NativeWindow(imp::window::NativeWindow);

impl NativeWindow {
    /// Electron's `titleBarStyle: "hiddenInset"`: an empty toolbar under the transparent title
    /// bar, which moves the traffic lights in from the window edge. The window must be opened
    /// with a transparent title bar and no `traffic_light_position`.
    pub fn hidden_inset_titlebar(&self) {
        self.0.hidden_inset_titlebar();
    }

    /// The content size a frame of `width` by `height` leaves for this window's style, which is
    /// how Electron reads the sizes of windows without `useContentSize`.
    pub fn content_size_for_frame(&self, width: f64, height: f64) -> (f64, f64) {
        self.0.content_size_for_frame(width, height)
    }

    pub fn content_size(&self) -> (f64, f64) {
        self.0.content_size()
    }

    /// Resizes the content, keeping the frame's top-left corner in place.
    pub fn set_content_size(&self, width: f64, height: f64) {
        self.0.set_content_size(width, height);
    }

    /// Puts the content's top-left corner at a screen position where the window manager places
    /// windows (X11). AppKit opens them where they ask.
    pub fn set_content_origin(&self, x: f64, y: f64) {
        self.0.set_content_origin(x, y);
    }

    /// Minimum and maximum content sizes, enforced during live resize.
    pub fn set_content_size_limits(&self, min: Option<(f64, f64)>, max: Option<(f64, f64)>) {
        self.0.set_content_size_limits(min, max);
    }

    /// Electron's `fullscreenable` option.
    pub fn set_fullscreenable(&self, fullscreenable: bool) {
        self.0.set_fullscreenable(fullscreenable);
    }

    /// Attaches `sheet` to this window as a sheet, as Electron does for `modal: true` with a
    /// `parent`.
    pub fn begin_sheet(&self, sheet: &NativeWindow) {
        self.0.begin_sheet(&sheet.0);
    }

    /// Detaches this window from the window it is a sheet on, if any.
    pub fn end_sheet(&self) {
        self.0.end_sheet();
    }

    /// Electron's `isVisible()`: on screen and not minimized. Being covered by other windows does
    /// not count, whatever its documentation says.
    pub fn is_visible(&self) -> bool {
        self.0.is_visible()
    }

    /// Shows a window opened hidden, making it key when `focus` is set.
    pub fn show(&self, focus: bool) {
        self.0.show(focus);
    }
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

/// Electron's `Menu.setApplicationMenu(menu)`: each top-level entry is a submenu of the menu bar.
/// `on_select` gets the path of each chosen entry. Returns false where the platform has no
/// native menu bar.
pub fn set_application_menu(
    cx: &mut App,
    menu: &PopupMenu,
    on_select: impl Fn(MenuPath, &mut App) + 'static,
) -> bool {
    imp::set_application_menu(cx, menu, on_select)
}

/// What a `<select>`'s native popup lists, as Blink's `ExternalPopupMenu::GetPopupMenuInfo`
/// sends it to the browser.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectMenu {
    pub items: Vec<SelectMenuItem>,
    /// `int(computed font size)` of the select, in CSS px.
    pub font_size: f64,
    pub selected: Option<usize>,
    pub right_aligned: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelectMenuItem {
    pub label: String,
    pub kind: SelectMenuItemKind,
    pub enabled: bool,
    /// The item's `title`; empty sets no tool tip.
    pub tool_tip: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectMenuItemKind {
    Option,
    Group,
    Separator,
}

/// Chromium's popup for a `<select>`: on macOS an `NSPopUpButtonCell` menu with the selected
/// item over the control, as `WebMenuRunner` shows it, on Linux a drawn list below it. `bounds` is the select's border box in
/// window content coordinates `[x, y, width, height]`. The menu opens on a later turn of the
/// event loop, and `on_close` gets the chosen item's index in `menu.items`, if any.
pub fn select_popup(
    window: &Window,
    cx: &mut App,
    menu: SelectMenu,
    bounds: [f64; 4],
    on_close: impl FnOnce(Option<usize>, &mut App) + 'static,
) {
    imp::select_popup(window, cx, menu, bounds, on_close);
}

/// Finishes a drawn menu's `PopUp` window, which belongs at `bounds` on screen. The outermost menu
/// (`root`) takes the pointer where the platform needs that to dismiss it.
pub(crate) fn popup_window_opened(
    window: &Window,
    bounds: Bounds<Pixels>,
    root: bool,
    cx: &mut App,
) {
    imp::popup_window_opened(window, bounds, root, cx);
}

/// Closes the drawn menu open for `window`, if any.
#[cfg(target_os = "linux")]
pub(crate) fn dismiss_popup(window: &Window) -> bool {
    imp::dismiss_popup(window)
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
