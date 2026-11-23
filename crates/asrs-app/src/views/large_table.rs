//! What `renderers.css` gives the Table and Console tabs alike ("Table & Console"): header
//! cells, the hand icon and boxes placed on Blink's device pixel grid.

use asrs_js::math;
use asrs_ui::button::IconButton;
use asrs_ui::gpui::{App, Div, IntoElement, ParentElement, Styled, div, px, rgb, rgba};
use asrs_ui::icon::{Icon, IconFit};
use asrs_ui::parity::probed;
use asrs_ui::symbols::Symbol;
use asrs_ui::text::{self, ELLIPSIS, Text, TextStyle, snap_remainder};
use asrs_ui::theme::{Appearance, chains};

use super::source_list::ratio_symbol;

pub(super) const BUTTON: f64 = 25.0;
const HAND_SIZE: f64 = 100.0;
const HAND_OPACITY: f64 = 0.15;
const HAND_DELAY_MS: f64 = 1000.0;
const HAND_FADE_MS: f64 = 1000.0;

pub(super) struct HeaderPalette {
    pub bottom: u32,
    pub right: u32,
    pub background: u32,
}

impl HeaderPalette {
    pub fn of(appearance: Appearance) -> HeaderPalette {
        appearance.pick(
            HeaderPalette {
                bottom: 0x222222,
                right: 0xeeeeee,
                background: 0xffffff,
            },
            HeaderPalette {
                bottom: 0xeeeeee,
                right: 0x333333,
                background: 0x222222,
            },
        )
    }
}

/// Positions in the renderer element for client coordinates, on the device pixel grid.
#[derive(Clone, Copy)]
pub(super) struct Grid {
    pub origin: [f64; 2],
    pub scale: f64,
}

impl Grid {
    pub fn snap(&self, v: f64) -> f64 {
        math::floor(v * self.scale + 0.5) / self.scale
    }

    /// Blink's pixel-snapped rect of a box.
    pub fn place(&self, [x, y, w, h]: [f64; 4]) -> Div {
        let (left, top) = (self.snap(x), self.snap(y));
        div()
            .absolute()
            .left(px((left - self.origin[0]) as f32))
            .top(px((top - self.origin[1]) as f32))
            .w(px((self.snap(x + w) - left) as f32))
            .h(px((self.snap(y + h) - top) as f32))
    }

    pub fn remainder(&self, v: f64) -> f32 {
        snap_remainder(v as f32, self.scale as f32)
    }

    /// Text whose box starts at client (`x`, `y`).
    pub fn text(&self, text: Text, [x, y]: [f64; 2]) -> Div {
        div()
            .absolute()
            .left(px((self.snap(x) - self.origin[0]) as f32))
            .top(px((self.snap(y) - self.origin[1]) as f32))
            .child(text.subpixel(self.remainder(x), self.remainder(y)))
    }

    /// One of the header's buttons at client (`x`, `y`).
    pub fn button(&self, button: IconButton, [x, y]: [f64; 2]) -> Div {
        div()
            .absolute()
            .left(px((self.snap(x) - self.origin[0]) as f32))
            .top(px((self.snap(y) - self.origin[1]) as f32))
            .child(
                button
                    .size(BUTTON as f32, BUTTON as f32)
                    .subpixel(x - self.snap(x), y - self.snap(y)),
            )
    }
}

pub(super) fn symbol(name: &str) -> Symbol {
    Symbol::named(name).unwrap_or_else(|| panic!("bundled symbol {name}"))
}

pub(super) fn mean(a: u32, b: u32) -> u32 {
    let channel = |shift: u32| (((a >> shift & 0xff) + (b >> shift & 0xff)).div_ceil(2)) << shift;
    channel(16) | channel(8) | channel(0)
}

/// A `th`: its background, and its `border` px borders meeting in a mitre at the bottom right corner,
/// where the pixels on the diagonal take half of each.
pub(super) fn header_cell(
    grid: Grid,
    cell: [f64; 4],
    right: bool,
    border: f64,
    palette: &HeaderPalette,
) -> Div {
    let [x, y, w, h] = cell;
    let (right_edge, bottom_edge) = (grid.snap(x + w), grid.snap(y + h));
    let mut out = div()
        .absolute()
        .inset_0()
        .child(grid.place(cell).bg(rgb(palette.background)))
        .child(
            grid.place([x, bottom_edge - border, right_edge - x, border])
                .bg(rgb(palette.bottom)),
        );
    if !right {
        return out;
    }
    let left = right_edge - border;
    out = out.child(
        grid.place([left, y, border, bottom_edge - border - y])
            .bg(rgb(palette.right)),
    );
    let pixel = 1.0 / grid.scale;
    let half = mean(palette.bottom, palette.right);
    for step in 0..(border * grid.scale).round() as usize {
        let at = step as f64 * pixel;
        let top = bottom_edge - border + at;
        out = out
            .child(grid.place([left + at, top, pixel, pixel]).bg(rgb(half)))
            .child(
                grid.place([left + at + pixel, top, border - at - pixel, pixel])
                    .bg(rgb(palette.right)),
            );
    }
    out
}

/// `cubic-bezier(0.42, 0, 1, 1)` (`ease-in`) at `t`.
fn ease_in(t: f64) -> f64 {
    if t <= 0.0 {
        return 0.0;
    }
    let (x1, x2) = (0.42, 1.0);
    let (y1, y2) = (0.0, 1.0);
    let bezier = |a: f64, b: f64, s: f64| {
        let r = 1.0 - s;
        3.0 * r * r * s * a + 3.0 * r * s * s * b + s * s * s
    };
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..64 {
        let mid = (lo + hi) / 2.0;
        if bezier(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bezier(y1, y2, (lo + hi) / 2.0)
}

/// `img.large-table-hand-icon`'s opacity. `transition: opacity 1s ease-in 1s` runs only when the
/// icon was on display with opacity 0 before.
#[derive(Default)]
pub(super) struct HandFade {
    shown: Option<bool>,
    since: Option<f64>,
}

impl HandFade {
    /// The opacity at `now` (ms), and whether it is still changing.
    pub fn opacity(&mut self, show: bool, now: f64) -> (f64, bool) {
        let was = self.shown.replace(show);
        match (was, show) {
            (Some(false), true) => self.since = Some(now),
            (_, false) => self.since = None,
            _ => {}
        }
        if !show {
            return (0.0, false);
        }
        let Some(since) = self.since else {
            return (HAND_OPACITY, false);
        };
        let t = (now - since - HAND_DELAY_MS) / HAND_FADE_MS;
        if t >= 1.0 {
            self.since = None;
            return (HAND_OPACITY, false);
        }
        (HAND_OPACITY * ease_in(t), true)
    }
}

pub(super) fn hand_icon(root: [f64; 4], opacity: f64, dark: bool, grid: Grid) -> impl IntoElement {
    let [x, y, w, h] = root;
    let half = HAND_SIZE / 2.0;
    let chain = if dark { chains::INVERT } else { "" };
    let at = [x + w / 2.0 - half, y + h / 2.0 - half];
    probed(
        "img.large-table-hand-icon",
        grid.place([at[0], at[1], HAND_SIZE, HAND_SIZE])
            .opacity(opacity as f32)
            .child(Icon::new(ratio_symbol("trackpadinput.svg"), chain).fit(IconFit::Natural)),
    )
}

/// `text-decoration: line-through` on a key whose line box starts at client (`x`, `y`), cut to
/// `width` if any: Blink decorates the text and not its ellipsis, rounds the line to whole device rows
/// and leaves its ends antialiased.
pub(super) fn line_through(
    key: &str,
    style: &TextStyle,
    [x, y]: [f64; 2],
    width: Option<f64>,
    color: u32,
    grid: Grid,
    cx: &mut App,
) -> Vec<Div> {
    let scale = grid.scale;
    let layout = text::layout(cx, style, key, scale as f32, width.map(|w| w as f32));
    let Some(line) = layout.lines.first() else {
        return Vec::new();
    };
    let device = |lu: i32| f64::from(lu) / 64.0;
    let ascent = device(layout.ascent);
    let top = y * scale + device(line.baseline) - ascent;
    let row = math::floor(top + 2.0 * ascent / 3.0 + 0.5);
    let thickness = math::max2(math::floor(14.0 * scale / 10.0), 1.0);
    let mut out = Vec::new();
    let mut span = |from: f64, to: f64, alpha: f64| {
        if to > from {
            let alpha = (alpha * 255.0).round() as u32;
            out.push(
                grid.place([from, row, to - from, thickness].map(|v| v / scale))
                    .bg(rgba(color << 8 | alpha)),
            );
        }
    };
    for f in &line.fragments {
        if line.cut_at.is_some() && f.text == ELLIPSIS {
            continue;
        }
        let from = x * scale + device(f.x);
        let to = from + device(f.width);
        let (inner_from, inner_to) = (math::ceil(from), math::floor(to));
        span(math::floor(from), inner_from, inner_from - from);
        span(inner_from, inner_to, 1.0);
        span(inner_to, math::ceil(to), to - inner_to);
    }
    out
}

pub(super) fn plain(text: impl Into<String>, style: TextStyle, color: u32) -> Text {
    Text::new(text.into(), style).color(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hand_fades_in_only_after_it_was_gone() {
        let mut hand = HandFade::default();
        assert_eq!(hand.opacity(true, 0.0), (HAND_OPACITY, false));
        assert_eq!(hand.opacity(false, 10.0), (0.0, false));
        assert_eq!(hand.opacity(true, 20.0), (0.0, true));
        assert_eq!(hand.opacity(true, 1020.0), (0.0, true));
        let (mid, _) = hand.opacity(true, 1520.0);
        assert!((0.3..0.33).contains(&(mid / HAND_OPACITY)));
        assert_eq!(hand.opacity(true, 2020.0), (HAND_OPACITY, false));
    }

    #[test]
    fn corner_pixels_take_half_of_each_border() {
        assert_eq!(mean(0x222222, 0xeeeeee), 0x888888);
        assert_eq!(mean(0xeeeeee, 0x333333), 0x919191);
    }
}
