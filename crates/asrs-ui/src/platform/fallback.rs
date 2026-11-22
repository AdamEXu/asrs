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

pub(super) mod window {
    use gpui_kit::Window;

    pub enum NativeWindow {}

    pub fn native(_window: &Window) -> Option<NativeWindow> {
        None
    }

    pub fn bounds(_window: &Window) -> Option<(f64, f64, f64, f64)> {
        None
    }

    impl NativeWindow {
        pub fn hidden_inset_titlebar(&self) {
            match *self {}
        }

        pub fn content_size_for_frame(&self, _width: f64, _height: f64) -> (f64, f64) {
            match *self {}
        }

        pub fn content_size(&self) -> (f64, f64) {
            match *self {}
        }

        pub fn set_content_size(&self, _width: f64, _height: f64) {
            match *self {}
        }

        pub fn set_content_size_limits(&self, _min: Option<(f64, f64)>, _max: Option<(f64, f64)>) {
            match *self {}
        }

        pub fn set_fullscreenable(&self, _fullscreenable: bool) {
            match *self {}
        }

        pub fn begin_sheet(&self, _sheet: &NativeWindow) {
            match *self {}
        }

        pub fn end_sheet(&self) {
            match *self {}
        }

        pub fn show(&self, _focus: bool) {
            match *self {}
        }
    }
}

pub(super) mod dialog {
    use std::path::PathBuf;

    use gpui_kit::{App, PathPromptOptions, PromptLevel, Window};

    use super::super::dialog::{
        MessageBoxOptions, MessageBoxResult, MessageBoxType, OpenDialogOptions, SaveDialogOptions,
    };

    pub fn message_box(
        window: Option<&mut Window>,
        cx: &mut App,
        options: MessageBoxOptions,
        on_done: impl FnOnce(MessageBoxResult, &mut App) + 'static,
    ) {
        let checkbox_checked = options.checkbox_checked;
        let (Some(window), false) = (window, options.buttons.is_empty()) else {
            cx.defer(move |cx| {
                on_done(
                    MessageBoxResult {
                        response: 0,
                        checkbox_checked,
                    },
                    cx,
                )
            });
            return;
        };
        let level = match options.kind {
            MessageBoxType::Warning => PromptLevel::Warning,
            MessageBoxType::Error => PromptLevel::Critical,
            _ => PromptLevel::Info,
        };
        let buttons: Vec<&str> = options.buttons.iter().map(String::as_str).collect();
        let detail = (!options.detail.is_empty()).then_some(options.detail.as_str());
        let answer = window.prompt(level, &options.message, detail, &buttons, cx);
        cx.spawn(async move |cx| {
            let response = answer.await.unwrap_or(0);
            cx.update(|cx| {
                on_done(
                    MessageBoxResult {
                        response,
                        checkbox_checked,
                    },
                    cx,
                )
            })
        })
        .detach();
    }

    pub fn open_dialog(
        _window: Option<&mut Window>,
        cx: &mut App,
        options: OpenDialogOptions,
        on_done: impl FnOnce(Vec<PathBuf>, &mut App) + 'static,
    ) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: options.open_file,
            directories: options.open_directory,
            multiple: options.multi_selections,
            prompt: (!options.button_label.is_empty()).then(|| options.button_label.into()),
        });
        cx.spawn(async move |cx| {
            let paths = paths.await.ok().and_then(Result::ok).flatten();
            cx.update(|cx| on_done(paths.unwrap_or_default(), cx))
        })
        .detach();
    }

    pub fn save_dialog(
        _window: Option<&mut Window>,
        cx: &mut App,
        options: SaveDialogOptions,
        on_done: impl FnOnce(Option<PathBuf>, &mut App) + 'static,
    ) {
        let default = options.default_path.unwrap_or_default();
        let (directory, name) = if default.is_dir() {
            (default, None)
        } else {
            (
                default.parent().map(PathBuf::from).unwrap_or_default(),
                default
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned()),
            )
        };
        let path = cx.prompt_for_new_path(&directory, name.as_deref());
        cx.spawn(async move |cx| {
            let path = path.await.ok().and_then(Result::ok).flatten();
            cx.update(|cx| on_done(path, cx))
        })
        .detach();
    }
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

    pub fn is_visible(_window: &Window) -> Option<bool> {
        None
    }

    pub fn app_is_active() -> Option<bool> {
        None
    }

    pub fn is_sheet_of(_window: &Window, _parent: &Window) -> bool {
        false
    }

    pub fn hides_on_deactivate(_window: &Window) -> Option<bool> {
        None
    }

    pub fn click_sheet_button(_parent: &Window, _title: &str) -> bool {
        false
    }

    pub fn sheet_panel_state(_parent: &Window) -> Option<super::super::probe::PanelState> {
        None
    }

    pub fn cancel_sheet_panel(_parent: &Window) -> bool {
        false
    }

    pub fn has_attached_sheet(_window: &Window) -> bool {
        false
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
