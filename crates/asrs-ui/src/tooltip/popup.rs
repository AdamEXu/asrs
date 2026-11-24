//! `TooltipViewAura` in a window of its own, which can reach past the one it is for.

use gpui_kit::popup::{PopupAnchor, PopupConstraintAdjustment, PopupGravity, PopupOptions};
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Bounds, Context, IntoElement, ParentElement, Pixels, Point,
    Render, Styled, Window, WindowBounds, WindowKind, WindowOptions, div, point, px, rgb, size,
};

use super::aura;
use crate::input::style::system_font;
use crate::platform;
use crate::text::{self, Direction, Text, TextStyle, WhiteSpace};

use crate::theme::Appearance;

const PADDING: [f32; 4] = [4.0, 8.0, 5.0, 8.0];
const FONT_SIZE: f32 = 12.0;

fn style(direction: Direction) -> TextStyle {
    let mut style = TextStyle::new(system_font(FONT_SIZE));
    style.white_space = WhiteSpace::Pre;
    style.direction = direction;
    style
}

/// The text inside the marks `GetWrappedTooltipText` puts around a right-to-left title.
fn unwrapped(text: &str) -> (Direction, &str) {
    match text.strip_prefix('\u{202b}') {
        Some(text) => (
            Direction::Rtl,
            text.strip_suffix('\u{202c}').unwrap_or(text),
        ),
        None => (Direction::Ltr, text),
    }
}

/// `gfx::RenderText` with `WRAP_LONG_WORDS`: lines end at newlines, at the last space that leaves
/// them no wider than `room`, and inside a word that is wider by itself.
fn wrap(text: &str, room: f32, mut measure: impl FnMut(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_inclusive(' ') {
            if measure(format!("{line}{word}").trim_end_matches(' ')) <= room {
                line.push_str(word);
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            for c in word.chars() {
                line.push(c);
                if measure(line.trim_end_matches(' ')) > room && line.chars().count() > 1 {
                    line.pop();
                    lines.push(std::mem::replace(&mut line, c.to_string()));
                }
            }
        }
        lines.push(line);
    }
    lines
}

/// `kColorTooltipBackground` and `kColorTooltipForeground`.
fn colors(appearance: Appearance) -> [u32; 2] {
    appearance.pick([0xffffff, 0x3d3e40], [0x292a2d, 0xcfd1d4])
}

struct View {
    lines: Vec<String>,
    direction: Direction,
}

impl Render for View {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let [background, foreground] = colors(Appearance::of(window, cx));
        let [top, right, bottom, left] = PADDING.map(|p| px(p - 1.0));
        let lines = self.lines.iter().map(|line| {
            let line = if line.is_empty() { " " } else { line };
            Text::new(line.to_owned(), style(self.direction)).color(foreground)
        });
        div()
            .size_full()
            .border_1()
            .border_color(rgb(foreground))
            .bg(rgb(background))
            .pt(top)
            .pr(right)
            .pb(bottom)
            .pl(left)
            .flex()
            .flex_col()
            .children(lines)
    }
}

pub(super) struct Popup(AnyWindowHandle);

impl Popup {
    pub(super) fn open(
        text: &str,
        pointer: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Popup> {
        let scale = window.scale_factor();
        let (origin, display) = platform::popup_space(window, pointer, cx)?;
        let room = f32::from(aura::max_width(display));
        let (direction, text) = unwrapped(text);
        let style = style(direction);
        let lines = wrap(text, room, |line| {
            text::measure(cx, &style, line, scale).0 as f32
        });
        let mut content = (0.0f32, 0.0f32);
        for line in &lines {
            let line = if line.is_empty() { " " } else { line };
            let (width, height) = text::measure(cx, &style, line, scale);
            content = (
                content.0.max(width.ceil() as f32),
                content.1 + height as f32,
            );
        }
        let [top, right, bottom, left] = PADDING;
        let size = size(px(content.0 + left + right), px(content.1 + top + bottom));
        let (kind, bounds) = match origin {
            Some(origin) => (
                WindowKind::PopUp,
                aura::place(origin + pointer, size, display),
            ),
            None => (
                WindowKind::AnchoredPopup(PopupOptions {
                    parent: window.window_handle(),
                    anchor_rect: Bounds::new(
                        pointer,
                        gpui_kit::size(px(aura::CURSOR_OFFSET[0]), px(aura::CURSOR_OFFSET[1])),
                    ),
                    anchor: PopupAnchor::BottomRight,
                    gravity: PopupGravity::BottomRight,
                    constraint_adjustment: PopupConstraintAdjustment::SLIDE_X
                        | PopupConstraintAdjustment::SLIDE_Y
                        | PopupConstraintAdjustment::FLIP_Y,
                    offset: point(px(0.0), px(0.0)),
                    grab: false,
                }),
                Bounds::new(Point::default(), size),
            ),
        };
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            focus: false,
            show: true,
            kind,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            ..WindowOptions::default()
        };
        let (handle, _) = crate::open_window(options, cx, move |window, cx| {
            platform::ignore_input(window);
            if origin.is_some() {
                platform::popup_window_opened(window, bounds, false, cx);
            }
            cx.new(|_| View { lines, direction })
        })
        .ok()?;
        Some(Popup(handle))
    }

    pub(super) fn close(self, cx: &mut App) {
        self.0
            .update(cx, |_, window, _| window.remove_window())
            .ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_to_left_titles_lose_their_marks() {
        assert_eq!(unwrapped("\u{202b}/A\u{202c}"), (Direction::Rtl, "/A"));
        assert_eq!(unwrapped("/A"), (Direction::Ltr, "/A"));
    }

    fn lines(text: &str) -> Vec<String> {
        wrap(text, 10.0, |line| line.chars().count() as f32)
    }

    #[test]
    fn lines_break_at_newlines_spaces_and_inside_long_words() {
        assert_eq!(lines("a: 1\nb: 2"), ["a: 1", "b: 2"]);
        assert_eq!(lines("one two three"), ["one two ", "three"]);
        assert_eq!(lines("abcdefghijklmn op"), ["abcdefghij", "klmn op"]);
        assert_eq!(lines("a\n\nb"), ["a", "", "b"]);
    }
}
