//! `views::corewm::TooltipController` and `TooltipStateManager` for the one window Electron gives
//! a page, with `TooltipAura`'s size and place. Windows and Linux draw a page's tooltips with it.

use std::time::Duration;

use gpui_kit::{Bounds, Pixels, Point, Size, point, px};

pub const SHOW_DELAY: Duration = Duration::from_millis(500);
pub const CURSOR_OFFSET: [f32; 2] = [10.0, 15.0];
pub const MAX_WIDTH: f32 = 800.0;
/// `kMaxTooltipLength`, in UTF-16 units.
pub const MAX_LENGTH: usize = if cfg!(windows) { 1024 } else { 2048 };

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Command {
    /// Hides the tooltip and stops the timer.
    Hide,
    /// Shows the tooltip after [`SHOW_DELAY`], where the pointer is by then.
    Start,
}

#[derive(Default)]
pub(super) struct Controller {
    /// The text the page set on the window.
    window: String,
    observed: bool,
    pointer: Point<Pixels>,
    pressed: bool,
    /// The window's text when a button was pressed over it.
    at_press: Option<String>,
    /// The text the state manager was last asked to show.
    text: String,
    /// Whether that tooltip is shown or about to be.
    active: bool,
    visible: bool,
}

impl Controller {
    pub(super) fn mouse_moved(&mut self, pointer: Point<Pixels>, pressed: bool) -> Vec<Command> {
        let mut out = Vec::new();
        self.pressed = pressed;
        if cfg!(windows) && pointer == self.pointer && self.observed {
            return out;
        }
        self.pointer = pointer;
        self.observe(!pressed, &mut out);
        if self.visible || (self.observed && self.text != self.window) {
            self.update(&mut out);
        }
        out
    }

    pub(super) fn mouse_exited(&mut self) -> Vec<Command> {
        let mut out = Vec::new();
        self.observe(false, &mut out);
        out
    }

    pub(super) fn mouse_pressed(&mut self) -> Vec<Command> {
        self.pressed = true;
        self.at_press = Some(self.window.clone());
        let mut out = Vec::new();
        self.hide(&mut out);
        out
    }

    pub(super) fn mouse_released(&mut self) {
        self.pressed = false;
    }

    pub(super) fn wheel(&mut self) -> Vec<Command> {
        let mut out = Vec::new();
        if self.visible {
            self.hide(&mut out);
        }
        out
    }

    /// A key press, or another window becoming the active one.
    pub(super) fn reset(&mut self) -> Vec<Command> {
        let mut out = Vec::new();
        self.hide(&mut out);
        self.observe(false, &mut out);
        out
    }

    /// The page set the window's text.
    pub(super) fn set_text(&mut self, text: &str) -> Vec<Command> {
        let mut out = Vec::new();
        text.clone_into(&mut self.window);
        if self.observed && (self.active || self.text != self.window) {
            self.update(&mut out);
        }
        if self.at_press.as_ref().is_some_and(|at| *at != self.window) {
            self.at_press = None;
        }
        out
    }

    /// The show timer fired: what to show, and where the pointer is.
    pub(super) fn show_now(&mut self) -> Option<(String, Point<Pixels>)> {
        let text = shown_text(&self.text);
        self.visible = self.active && !text.is_empty();
        self.visible.then_some((text, self.pointer))
    }

    fn observe(&mut self, observed: bool, out: &mut Vec<Command>) {
        if self.observed && !observed && self.active {
            self.hide(out);
        }
        self.observed = observed;
    }

    fn hide(&mut self, out: &mut Vec<Command>) {
        self.active = false;
        self.visible = false;
        out.push(Command::Hide);
    }

    fn update(&mut self, out: &mut Vec<Command>) {
        if self.pressed || !self.observed {
            self.hide(out);
            return;
        }
        if !self.window.is_empty() && self.at_press.as_ref() == Some(&self.window) {
            self.hide(out);
            return;
        }
        self.at_press = None;
        if self.visible && self.text == self.window {
            return;
        }
        self.hide(out);
        self.text.clone_from(&self.window);
        self.active = true;
        if !shown_text(&self.text).is_empty() {
            out.push(Command::Start);
        }
    }
}

/// `gfx::TruncateString` at a word and `base::TrimWhitespace`, with tabs as spaces.
fn shown_text(text: &str) -> String {
    let units = text.encode_utf16().count();
    let text = if units > MAX_LENGTH {
        truncate(text)
    } else {
        text.to_owned()
    };
    text.trim().replace('\t', "        ")
}

fn truncate(text: &str) -> String {
    let mut units = 0;
    let kept: String = text
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units < MAX_LENGTH
        })
        .collect();
    let end = kept
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(kept.len(), |(at, _)| at);
    format!("{}\u{2026}", kept[..end].trim_end())
}

/// `TooltipAura::GetMaxWidth`.
pub fn max_width(display: Bounds<Pixels>) -> Pixels {
    let half = ((f32::from(display.size.width) + 1.0) / 2.0).floor();
    px(MAX_WIDTH.min(half))
}

/// `TooltipAura::GetTooltipBounds` for a cursor at `cursor`, all in screen coordinates.
pub fn place(cursor: Point<Pixels>, size: Size<Pixels>, display: Bounds<Pixels>) -> Bounds<Pixels> {
    let offset = point(px(CURSOR_OFFSET[0]), px(CURSOR_OFFSET[1]));
    let mut bounds = Bounds::new(cursor + offset, size);
    if bounds.left() < display.left() {
        bounds.origin.x += bounds.left() - display.left();
    }
    if bounds.right() > display.right() {
        bounds.origin.x -= bounds.right() - display.right();
    }
    if bounds.bottom() > display.bottom() {
        bounds.origin.y = cursor.y - size.height;
    }
    let fit =
        |at: Pixels, length: Pixels, low: Pixels, high: Pixels| (at.min(high - length)).max(low);
    bounds.origin.x = fit(bounds.origin.x, size.width, display.left(), display.right());
    bounds.origin.y = fit(
        bounds.origin.y,
        size.height,
        display.top(),
        display.bottom(),
    );
    bounds
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::size;

    fn at(x: f32) -> Point<Pixels> {
        point(px(x), px(5.0))
    }

    fn over(text: &str) -> Controller {
        let mut c = Controller::default();
        c.mouse_moved(at(1.0), false);
        assert_eq!(c.set_text(text), [Command::Hide, Command::Start]);
        c
    }

    #[test]
    fn shows_where_the_pointer_is_when_the_delay_ends() {
        let mut c = over("Play");
        assert!(c.mouse_moved(at(2.0), false).is_empty());
        assert!(c.set_text("Play").contains(&Command::Start));
        assert_eq!(c.show_now(), Some(("Play".to_owned(), at(2.0))));
        assert!(c.mouse_moved(at(3.0), false).is_empty());
        assert!(c.set_text("Play").is_empty());
        assert!(c.visible);
    }

    #[test]
    fn a_press_hides_it_until_the_text_changes() {
        let mut c = over("Play");
        c.show_now();
        assert_eq!(c.mouse_pressed(), [Command::Hide]);
        c.mouse_released();
        c.mouse_moved(at(2.0), false);
        assert!(!c.set_text("Play").contains(&Command::Start));
        c.set_text("Close tab");
        assert!(c.show_now().is_some());
    }

    #[test]
    fn keys_wheel_and_leaving_hide_it() {
        for hide in [
            Controller::reset as fn(&mut Controller) -> Vec<Command>,
            Controller::wheel,
            Controller::mouse_exited,
        ] {
            let mut c = over("Play");
            c.show_now();
            assert!(hide(&mut c).contains(&Command::Hide));
            assert!(!c.visible);
        }
    }

    #[test]
    fn whitespace_shows_nothing_and_newlines_stay() {
        let mut c = over("a\nb");
        assert_eq!(c.show_now().map(|s| s.0).as_deref(), Some("a\nb"));
        assert!(!c.set_text(" \n").contains(&Command::Start));
        assert_eq!(shown_text("a\tb "), "a        b");
    }

    #[test]
    fn stays_on_the_display_and_flips_above_the_cursor() {
        let display = Bounds::new(point(px(0.0), px(0.0)), size(px(1000.0), px(800.0)));
        let tip = size(px(200.0), px(30.0));
        let place = |x: f32, y: f32| place(point(px(x), px(y)), tip, display).origin;
        assert_eq!(place(100.0, 100.0), point(px(110.0), px(115.0)));
        assert_eq!(place(900.0, 100.0), point(px(800.0), px(115.0)));
        assert_eq!(place(100.0, 780.0), point(px(110.0), px(750.0)));
        assert_eq!(max_width(display), px(500.0));
    }
}
