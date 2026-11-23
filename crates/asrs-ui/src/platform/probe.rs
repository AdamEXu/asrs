//! Read-back and input-injection helpers for platform checks and manual debugging. Nothing in the
//! app proper calls these.

use gpui_kit::{ScrollDelta, Window};

use super::{PopupMenu, WheelDelta, imp};

/// A window rectangle in screen points. On macOS the origin is the bottom-left of the primary
/// screen and y grows upwards.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl WindowImage {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }
}

/// One native scroll-wheel event, by its CGEvent fields (axis 1 is y, axis 2 is x).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WheelSample {
    pub continuous: bool,
    pub delta: [i32; 2],
    pub point_delta: [i64; 2],
    pub fixed_delta: [f64; 2],
    pub scroll_phase: i64,
    pub momentum_phase: i64,
}

/// GPUI's `ScrollDelta` in comparable form.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GpuiDelta {
    Pixels([f32; 2]),
    Lines([f32; 2]),
}

impl GpuiDelta {
    pub fn of(delta: &ScrollDelta) -> Self {
        match delta {
            ScrollDelta::Pixels(p) => Self::Pixels([f32::from(p.x), f32::from(p.y)]),
            ScrollDelta::Lines(l) => Self::Lines([l.x, l.y]),
        }
    }
}

/// What each side makes of one native wheel event: Chromium's `WebMouseWheelEvent` deltas and
/// the delta GPUI reports.
#[derive(Clone, Debug, PartialEq)]
pub struct WheelReading {
    pub chromium: WheelDelta,
    pub gpui: GpuiDelta,
}

/// A two-finger trackpad flick with momentum (phases began, changed, ended, then momentum
/// begin, continue, end), followed by notched wheel ticks of 1, 3, -1 and -5 lines. Synthetic
/// CGEvents carry integer point deltas only, so GPUI's `scrollingDeltaY` equals the integer here;
/// on real trackpads it is fractional while Chromium still reads the integer.
pub fn wheel_gesture_samples() -> Vec<WheelSample> {
    let trackpad = |point: i64, fixed: f64, scroll_phase: i64, momentum_phase: i64| WheelSample {
        continuous: true,
        delta: [0, point.signum() as i32],
        point_delta: [0, point],
        fixed_delta: [0.0, fixed],
        scroll_phase,
        momentum_phase,
    };
    let notch = |lines: i32| WheelSample {
        delta: [0, lines],
        fixed_delta: [0.0, f64::from(lines)],
        ..WheelSample::default()
    };
    vec![
        trackpad(0, 0.0, 1, 0),
        trackpad(1, 0.578125, 2, 0),
        trackpad(3, 2.71875, 2, 0),
        trackpad(7, 6.890625, 2, 0),
        trackpad(12, 12.3125, 2, 0),
        trackpad(9, 8.5, 2, 0),
        trackpad(-2, -1.5, 2, 0),
        trackpad(0, 0.0, 4, 0),
        trackpad(11, 10.6171875, 0, 1),
        trackpad(6, 6.25, 0, 2),
        trackpad(2, 1.8984375, 0, 2),
        trackpad(1, 0.5, 0, 2),
        trackpad(0, 0.0, 0, 3),
        notch(1),
        notch(3),
        notch(-1),
        notch(-5),
    ]
}

/// Builds the native menu for `menu` and reads the tree back from the OS.
pub fn native_menu_roundtrip(menu: &PopupMenu) -> Option<PopupMenu> {
    imp::probe::native_menu_roundtrip(menu)
}

/// A `<select>` popup's native menu read back from the OS.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectMenuReading {
    pub items: Vec<SelectMenuItemReading>,
    /// The tag of the item the popup cell positions over the control.
    pub selected_tag: Option<isize>,
    pub pulls_down: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelectMenuItemReading {
    pub separator: bool,
    /// The plain title, which AppKit's type-to-select matches.
    pub title: String,
    /// The string the menu draws.
    pub attributed_title: Option<String>,
    pub font_size: Option<f64>,
    pub enabled: bool,
    pub tag: isize,
}

/// Chooses item `index` of the popup open over `window`, as a click on it would, and closes the
/// menu. Returns whether a popup was open.
pub fn choose_open_popup_item(window: &Window, index: usize) -> bool {
    imp::probe::choose_open_popup_item(window, index)
}

/// Builds the native popup for a `<select>` and reads its menu and cell back from the OS,
/// without running it.
pub fn select_menu_roundtrip(menu: &super::SelectMenu) -> Option<SelectMenuReading> {
    imp::probe::select_menu_roundtrip(menu)
}

/// The application's main menu read back from the OS, with each item's role as its native
/// wiring shows it, and the titles of the items the OS added on its own. `works_when_hidden`
/// is read only for items with a key equivalent, the only ones it affects, and is true for the
/// rest.
pub fn read_application_menu() -> Option<(PopupMenu, Vec<String>)> {
    imp::probe::read_application_menu()
}

/// Chooses the main menu entry at `path`, as a click on it would.
pub fn perform_application_menu_item(path: &[usize]) -> bool {
    imp::probe::perform_application_menu_item(path)
}

/// Offers the main menu a key-down event with the given characters and `NSEventModifierFlags`.
/// Returns whether an entry took it.
pub fn perform_key_equivalent(key: &str, modifiers: u32) -> bool {
    imp::probe::perform_key_equivalent(key, modifiers)
}

/// How much taller a titled, resizable window's frame is than its content.
pub fn framed_title_bar() -> Option<f64> {
    imp::probe::framed_title_bar()
}

/// Takes a key-down event with this virtual key code off the application's queue, and what
/// [`super::numpad_enter`] says while it is the current event.
pub fn numpad_enter_for(key_code: u16) -> Option<bool> {
    imp::probe::numpad_enter_for(key_code)
}

pub fn read_wheel_sample(sample: &WheelSample) -> Option<WheelReading> {
    imp::probe::read_wheel_sample(sample)
}

/// Posts a native wheel event at the centre of `window`'s content. See [`post_wheel_at`].
pub fn post_wheel(window: &Window, sample: &WheelSample) -> bool {
    imp::probe::post_wheel(window, sample)
}

/// Moves the cursor to a point in global top-left screen coordinates and posts a native wheel
/// event there through the HID event tap, so the window server routes it to whatever window is
/// under the cursor, exactly like a device event.
pub fn post_wheel_at(x: f64, y: f64, sample: &WheelSample) -> bool {
    imp::probe::post_wheel_at(x, y, sample)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Down,
    Drag,
    Up,
}

/// A left-button event at a point in global top-left screen coordinates, posted through the HID
/// event tap like a real mouse.
pub fn post_mouse(action: MouseAction, x: f64, y: f64) -> bool {
    imp::probe::post_mouse(action, x, y)
}

/// The cursor position in global top-left screen coordinates.
pub fn cursor_location() -> (f64, f64) {
    imp::probe::cursor_location()
}

pub fn warp_cursor(x: f64, y: f64) {
    imp::probe::warp_cursor(x, y);
}

pub fn window_frame(window: &Window) -> Option<Frame> {
    imp::probe::window_frame(window)
}

pub fn content_size(window: &Window) -> Option<(f64, f64)> {
    imp::probe::content_size(window)
}

pub fn window_number(window: &Window) -> Option<u32> {
    imp::probe::window_number(window)
}

/// Frames of the visible popup menu windows of this process.
pub fn menu_window_frames() -> Vec<Frame> {
    imp::probe::menu_window_frames()
}

pub fn hides_on_deactivate(window: &Window) -> Option<bool> {
    imp::probe::hides_on_deactivate(window)
}

/// Whether any part of the window is on screen: ordered in and not fully covered.
pub fn is_visible(window: &Window) -> Option<bool> {
    imp::probe::is_visible(window)
}

pub fn app_is_active() -> Option<bool> {
    imp::probe::app_is_active()
}

/// Whether `window` is attached as a sheet to `parent`.
pub fn is_sheet_of(window: &Window, parent: &Window) -> bool {
    imp::probe::is_sheet_of(window, parent)
}

/// Clicks the button titled `title` in the sheet attached to `parent`, as a user would.
pub fn click_sheet_button(parent: &Window, title: &str) -> bool {
    imp::probe::click_sheet_button(parent, title)
}

/// The configuration of the open or save panel attached to `parent` as a sheet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PanelState {
    pub open: bool,
    pub title: String,
    pub message: String,
    pub prompt: String,
    pub allowed_file_types: Vec<String>,
    pub directory: Option<String>,
    pub name: String,
    pub can_create_directories: bool,
    pub can_choose_files: bool,
    pub can_choose_directories: bool,
    pub allows_multiple_selection: bool,
}

pub fn sheet_panel_state(parent: &Window) -> Option<PanelState> {
    imp::probe::sheet_panel_state(parent)
}

/// Cancels the open or save panel attached to `parent`.
pub fn cancel_sheet_panel(parent: &Window) -> bool {
    imp::probe::cancel_sheet_panel(parent)
}

pub fn has_attached_sheet(window: &Window) -> bool {
    imp::probe::has_attached_sheet(window)
}

pub fn has_sidebar_vibrancy(window: &Window) -> bool {
    imp::probe::has_sidebar_vibrancy(window)
}

/// Undoes [`super::tag_srgb_colorspace`], leaving the drawable untagged as GPUI makes it.
pub fn untag_colorspace(window: &Window) -> bool {
    imp::probe::untag_colorspace(window)
}

/// Height of the primary screen in points, for converting between AppKit's bottom-left and
/// global top-left screen coordinates.
pub fn screen_height() -> f64 {
    imp::probe::screen_height()
}

pub fn is_tagged_srgb(window: &Window) -> Option<bool> {
    imp::probe::is_tagged_srgb(window)
}

/// The frame size the window's delegate answers for a proposed live-resize frame size.
pub fn proposed_resize(window: &Window, width: f64, height: f64) -> Option<(f64, f64)> {
    imp::probe::proposed_resize(window, width, height)
}

/// The window's composited pixels, as the window server holds them, including the backdrop of
/// translucent regions.
pub fn capture_window(window_number: u32) -> Option<WindowImage> {
    imp::probe::capture_window(window_number)
}

pub fn can_post_events() -> bool {
    imp::probe::can_post_events()
}

pub fn can_capture_screen() -> bool {
    imp::probe::can_capture_screen()
}
