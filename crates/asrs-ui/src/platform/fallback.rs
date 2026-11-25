use std::path::Path;
use std::time::Duration;

use gpui_kit::{App, Bounds, Pixels, ScrollEvent, Window};

use super::{EditCommand, MenuPath, PopupMenu, SystemSelection, WheelDelta};

pub(super) fn init() {}

pub(super) fn add_recent_document(_path: &Path) {}

pub(super) fn numpad_enter() -> bool {
    false
}

pub(super) fn scroll_button_delays() -> (Duration, Duration) {
    (Duration::from_millis(250), Duration::from_millis(50))
}

pub(super) fn set_tooltip(_window: &Window, _text: &str) {}

/// `WS_EX_TRANSPARENT` and `WS_EX_NOACTIVATE`, which a `Widget` that accepts no events and cannot be
/// activated gets.
#[cfg(windows)]
pub(super) fn ignore_input(window: &Window) {
    use std::ffi::c_void;

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_TRANSPARENT: isize = 0x20;
    const WS_EX_NOACTIVATE: isize = 0x0800_0000;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetWindowLongPtrW(window: *mut c_void, index: i32) -> isize;
        fn SetWindowLongPtrW(window: *mut c_void, index: i32, value: isize) -> isize;
    }

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = handle.hwnd.get() as *mut c_void;
    // SAFETY: the window is open, and both calls only read and write its styles.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            style | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
        );
    }
}

pub(super) fn is_key_window(_window: &Window) -> bool {
    true
}

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

pub(super) fn install_edit_commands(
    _: &mut App,
    _: impl Fn(EditCommand, &mut Window, &mut App) + 'static,
) {
}

pub(super) fn on_quit_request(
    _: &mut App,
    _: impl Fn() -> bool + 'static,
    _: impl Fn(&mut App) + 'static,
) {
}

pub(super) enum FullScreenObserver {}

pub(super) fn observe_full_screen(
    _: &Window,
    _: &App,
    _: impl Fn(bool, &mut App) + 'static,
) -> Option<FullScreenObserver> {
    None
}

pub(super) fn scrollbar_preferences() -> (bool, bool) {
    (true, false)
}

/// `RendererPreferences`' defaults, which Electron leaves as they are: its render hosts show
/// #3367d1 for the active one, this blended with white over white.
pub(super) fn selection_colors() -> SystemSelection {
    SystemSelection {
        active: 0x1967d2,
        inactive: 0xc8c8c8,
        text: Some([0xffffff, 0x323232]),
    }
}

pub(super) fn select_popup(
    _window: &Window,
    cx: &mut App,
    _menu: super::SelectMenu,
    _bounds: [f64; 4],
    on_close: impl FnOnce(Option<usize>, &mut App) + 'static,
) {
    cx.defer(move |cx| on_close(None, cx));
}

pub(super) fn set_application_menu(
    _cx: &mut App,
    _menu: &PopupMenu,
    _on_select: impl Fn(MenuPath, &mut App) + 'static,
) -> bool {
    false
}

pub(super) fn popup_window_opened(
    _window: &Window,
    _bounds: Bounds<Pixels>,
    _root: bool,
    _cx: &mut App,
) {
}

pub(super) fn close_popup_menu(_window: &Window) {}

pub(super) fn menu_takes_keys(_window: &Window) -> bool {
    false
}

pub(super) fn native_menu_label(label: &str) -> String {
    label.to_owned()
}

pub(super) fn wheel_delta(_event: &ScrollEvent) -> Option<WheelDelta> {
    None
}

pub(super) mod window {
    use gpui_kit::Window;

    #[cfg(feature = "parity")]
    pub fn titled_content_size(width: f64, height: f64) -> (f64, f64) {
        (width, height)
    }

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

        pub fn set_content_origin(&self, _x: f64, _y: f64) {
            match *self {}
        }

        pub fn center(&self, _parent: Option<&NativeWindow>) {
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

        pub fn is_visible(&self) -> bool {
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

    /// GPUI's prompts have no checkbox: its label is a last button, which answers with the
    /// default one and the checkbox checked (#402).
    pub fn message_box(
        window: Option<&mut Window>,
        cx: &mut App,
        options: MessageBoxOptions,
        on_done: impl FnOnce(MessageBoxResult, &mut App) + 'static,
    ) {
        let checkbox_checked = options.checkbox_checked;
        let Some(window) = window else {
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
        let mut buttons: Vec<&str> = options.buttons.iter().map(String::as_str).collect();
        if buttons.is_empty() {
            buttons.push("OK");
        }
        let checkbox = (!options.checkbox_label.is_empty()).then_some(buttons.len());
        if checkbox.is_some() {
            buttons.push(&options.checkbox_label);
        }
        let default_id = options.default_id.unwrap_or(0);
        let detail = (!options.detail.is_empty()).then_some(options.detail.as_str());
        let answer = window.prompt(level, &options.message, detail, &buttons, cx);
        cx.spawn(async move |cx| {
            let button = answer.await.unwrap_or(0);
            let checked = checkbox == Some(button);
            cx.update(|cx| {
                on_done(
                    MessageBoxResult {
                        response: if checked { default_id } else { button },
                        checkbox_checked: checkbox_checked || checked,
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
    use super::super::probe::{
        Frame, MenuRole, MouseAction, WheelReading, WheelSample, WindowChrome, WindowImage,
    };

    pub fn native_menu_roundtrip(_menu: &PopupMenu) -> Option<PopupMenu> {
        None
    }

    pub fn choose_open_popup_item(_window: &Window, _index: usize) -> bool {
        false
    }

    pub fn select_menu_roundtrip(
        _menu: &super::super::SelectMenu,
    ) -> Option<super::super::probe::SelectMenuReading> {
        None
    }

    pub fn read_application_menu() -> Option<(PopupMenu, Vec<String>)> {
        None
    }

    pub fn perform_application_menu_item(_path: &[usize]) -> bool {
        false
    }

    pub fn stay_in_background() {}

    pub fn menu_role(_window: &Window, _role: &str) -> Option<MenuRole> {
        None
    }

    pub fn perform_menu_role(_window: &Window, _role: &str) -> bool {
        false
    }

    pub fn perform_key_equivalent(_key: &str, _modifiers: u32) -> bool {
        false
    }

    pub fn perform_menu_role_without_window(_role: &str) -> Option<()> {
        None
    }

    pub fn validate_menu_role_without_window(_role: &str) -> Option<bool> {
        None
    }

    pub fn intercept_full_screen() -> bool {
        false
    }

    pub fn full_screen_toggles() -> usize {
        0
    }

    pub fn post_window_notification(_window: &Window, _name: &str) {}

    pub fn can_become_main(_window: &Window) -> Option<bool> {
        None
    }

    pub fn make_main(_window: &Window) {}

    pub fn window_chrome(_window: &Window) -> Option<WindowChrome> {
        None
    }

    pub fn will_resize(_window: &Window, _size: (f64, f64)) -> Option<(f64, f64)> {
        None
    }

    pub fn framed_window() -> Option<(f64, f64)> {
        None
    }

    pub fn numpad_enter_for(_key_code: u16) -> Option<bool> {
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

    pub fn tooltip_roundtrip(_texts: &[&str]) -> Vec<(Option<String>, bool)> {
        Vec::new()
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

    pub fn size_hints(_window: &Window, _number: u32) -> Option<super::super::probe::SizeHints> {
        None
    }

    pub fn post_keys(_keysyms: &[u32]) -> bool {
        false
    }
}
