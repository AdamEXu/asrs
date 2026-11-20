use gpui_kit::{App, ScrollEvent, Window};

use super::{MenuPath, PopupMenu, WheelDelta};

pub(super) fn init() {}

pub(super) fn install_sidebar_vibrancy(_window: &Window) -> bool {
    false
}

pub(super) fn keep_visible_when_inactive(_window: &Window) -> bool {
    false
}

pub(super) fn set_aspect_ratio(_window: &Window, _aspect_ratio: f64) {}

pub(super) fn tag_srgb_colorspace(_window: &Window) -> bool {
    false
}

pub(super) fn popup_menu(
    _window: &Window,
    cx: &mut App,
    _menu: PopupMenu,
    _x: i32,
    _y: i32,
    on_close: impl FnOnce(Option<MenuPath>, &mut App) + 'static,
) {
    cx.defer(move |cx| on_close(None, cx));
}

pub(super) fn close_popup_menu(_window: &Window) {}

pub(super) fn native_menu_label(label: &str) -> String {
    label.to_owned()
}

pub(super) fn wheel_delta(_event: &ScrollEvent) -> Option<WheelDelta> {
    None
}

pub(super) mod probe {
    use gpui_kit::Window;

    use super::super::PopupMenu;
    use super::super::probe::{Frame, MouseAction, WheelReading, WheelSample, WindowImage};

    pub fn native_menu_roundtrip(_menu: &PopupMenu) -> Option<PopupMenu> {
        None
    }

    pub fn read_wheel_sample(_sample: &WheelSample) -> Option<WheelReading> {
        None
    }

    pub fn post_wheel(_window: &Window, _sample: &WheelSample) -> bool {
        false
    }

    pub fn post_wheel_at(_x: f64, _y: f64, _sample: &WheelSample) -> bool {
        false
    }

    pub fn post_mouse(_action: MouseAction, _x: f64, _y: f64) -> bool {
        false
    }

    pub fn cursor_location() -> (f64, f64) {
        (0.0, 0.0)
    }

    pub fn warp_cursor(_x: f64, _y: f64) {}

    pub fn window_frame(_window: &Window) -> Option<Frame> {
        None
    }

    pub fn content_size(_window: &Window) -> Option<(f64, f64)> {
        None
    }

    pub fn window_number(_window: &Window) -> Option<u32> {
        None
    }

    pub fn menu_window_frames() -> Vec<Frame> {
        Vec::new()
    }

    pub fn hides_on_deactivate(_window: &Window) -> Option<bool> {
        None
    }

    pub fn has_sidebar_vibrancy(_window: &Window) -> bool {
        false
    }

    pub fn untag_colorspace(_window: &Window) -> bool {
        false
    }

    pub fn screen_height() -> f64 {
        0.0
    }

    pub fn is_tagged_srgb(_window: &Window) -> Option<bool> {
        None
    }

    pub fn proposed_resize(_window: &Window, _width: f64, _height: f64) -> Option<(f64, f64)> {
        None
    }

    pub fn capture_window(_window_number: u32) -> Option<WindowImage> {
        None
    }

    pub fn can_post_events() -> bool {
        false
    }

    pub fn can_capture_screen() -> bool {
        false
    }
}
