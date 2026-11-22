//! The hub's macOS title bar (`div.title-bar` under `body.fancy-title-bar-mac` in `hub.css`):
//! a 38 px drag strip across the window top, under a transparent native title bar, with the
//! window title drawn beside the sidebar.

use asrs_ui::gpui::{
    InteractiveElement, IntoElement, MouseButton, ParentElement, SharedString, Styled, div, px,
};
use asrs_ui::text::{Text, TextStyle};
use asrs_ui::theme::FONT_FAMILY;

pub const TITLE_BAR_HEIGHT: f32 = 38.0;

/// The variables `div.title-bar-text`'s position reads: `--side-bar-width`,
/// `--show-side-bar`, `--show-lock-buttons`, `--show-update-button` and
/// `--show-feedback-button`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TitleBarLayout {
    pub side_bar_width: f64,
    pub show_side_bar: bool,
    pub show_lock_buttons: bool,
    pub show_update_button: bool,
    pub show_feedback_button: bool,
}

impl TitleBarLayout {
    /// The title text box's CSS `left` and `right`.
    pub fn text_insets(&self) -> (f64, f64) {
        let on = |flag: bool| if flag { 1.0 } else { 0.0 };
        let left = self.side_bar_width + 20.0 + (1.0 - on(self.show_side_bar)) * 60.0;
        let right = 154.0
            + on(self.show_lock_buttons) * 28.0
            + on(self.show_update_button) * 38.0
            + on(self.show_feedback_button) * 38.0;
        (left, right)
    }
}

pub fn title_style() -> TextStyle {
    let font = format!("600 16px {}", FONT_FAMILY.join(", "));
    TextStyle::from_css([
        ("font", font.as_str()),
        ("line-height", "38px"),
        ("white-space", "nowrap"),
        ("text-overflow", "ellipsis"),
    ])
    .expect("title bar text style")
}

/// The title bar. Upstream shows it only on macOS outside full screen.
pub fn title_bar(
    title: impl Into<SharedString>,
    layout: &TitleBarLayout,
    text_color: u32,
) -> impl IntoElement {
    let (left, right) = layout.text_insets();
    div()
        .id("title-bar")
        .absolute()
        .left_0()
        .right_0()
        .top_0()
        .h(px(TITLE_BAR_HEIGHT))
        .on_mouse_down(MouseButton::Left, |event, window, _| {
            if event.click_count == 2 {
                window.titlebar_double_click();
            } else {
                window.start_window_move();
            }
        })
        .child(
            Text::new(title, title_style())
                .color(text_color)
                .probe("title-bar-text")
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(left as f32))
                .right(px(right as f32)),
        )
}
