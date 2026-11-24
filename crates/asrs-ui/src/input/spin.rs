//! `::-webkit-inner-spin-button`: what Blink's `SpinButtonElement` does with the pointer and how
//! Chromium's native theme paints it. Its arrows are the scrollbar's.

use gpui_kit::{App, Bounds, Pixels, Window};
use tiny_skia::{FillRule, Paint, PathBuilder, Transform};

use crate::input::native::PointerState;
use crate::input::skia;

/// `GetVerticalScrollbarButtonSize().width()`.
pub const WIDTH: f32 = 15.0;

/// The colours at rest, hovered and pressed: `NativeThemeAura`'s on macOS, `NativeThemeFluent`'s
/// elsewhere.
const BACKGROUND: [u32; 3] = if cfg!(target_os = "macos") {
    [0xf1f1f1ff, 0xd2d2d2ff, 0x787878ff]
} else {
    [0xfcfcfcff; 3]
};
const ARROW: [u32; 3] = if cfg!(target_os = "macos") {
    [0x505050ff, 0x505050ff, 0xffffffff]
} else {
    [0x8b8b8bff, 0x8b8b8bff, 0x636363ff]
};

/// `NativeThemeBase::GetArrowRect`: the left, top and side of the arrow's square in a button of
/// `w` x `h` device px.
fn aura_square(w: i32, h: i32) -> [i32; 3] {
    let inset = (w.max(h) + 3) / 4;
    let side = w.min(h).min(w.max(h) - inset * 2);
    [(w - side + 1) / 2, (h - side + 1) / 2, side]
}

/// `NativeThemeFluent::GetArrowRect` without its icon font.
fn fluent_square(w: i32, h: i32, up: bool, pressed: bool) -> [i32; 3] {
    const BUTTON: f32 = 18.0;
    let scale = w.max(h) as f32 / BUTTON;
    let side = (if pressed { 7.0 } else { 9.0 } * scale).ceil() as i32;
    let side = side + (w.min(h) - side) % 2;
    let offset = scale.round() as i32;
    let offset = if up { -offset } else { offset };
    let (width, height) = (side.min(w), side.min(h));
    [
        (w - width).div_euclid(2),
        (h - height).div_euclid(2) + offset,
        width.min(height),
    ]
}

/// `PathForArrow` in the square `[left, top, side]` of a button at (0, `y`).
fn arrow([left, top, side]: [i32; 3], y: i32, up: bool) -> [[f64; 2]; 3] {
    let height = side / 2 + 1;
    let [left, top, side] = [left, top + y, side].map(f64::from);
    let base = top + side - f64::from(height / 2) + 1.0;
    let points = [
        [left, base],
        [left + side, base],
        [left + side / 2.0, base - f64::from(height)],
    ];
    if up {
        return points;
    }
    let mirror = 2.0 * top + side;
    points.map(|[x, y]| [x, mirror - y])
}

/// `PaintInnerSpinButton` with the pointer doing `pointer` to the half that is `up`.
pub fn paint_spin_button(
    bounds: Bounds<Pixels>,
    up: bool,
    pointer: PointerState,
    window: &mut Window,
    cx: &mut App,
) {
    let (w, h) = skia::device_size(bounds, window.scale_factor());
    let half = h as i32 / 2;
    let draw = || {
        let parts = [(true, 0), (false, half)].map(|(part, y)| {
            let state = if part == up {
                pointer
            } else {
                PointerState::Normal
            };
            let square = if cfg!(target_os = "macos") {
                aura_square(w as i32, half)
            } else {
                fluent_square(w as i32, half, part, state == PointerState::Pressed)
            };
            (arrow(square, y, part), y, state as usize)
        });
        let mut body = String::new();
        for (points, y, state) in &parts {
            let fill = skia::paint_attrs("fill", BACKGROUND[*state]);
            body += &format!("<rect x=\"0\" y=\"{y}\" width=\"{w}\" height=\"{half}\" {fill}/>");
            if cfg!(target_os = "macos") {
                let [[ax, ay], [bx, by], [tx, ty]] = points;
                let fill = skia::paint_attrs("fill", ARROW[*state]);
                body += &format!("<path d=\"M{ax} {ay} L{bx} {by} L{tx} {ty} Z\" {fill}/>");
            }
        }
        let mut pixmap = skia::svg(w, h, 1.0, &body)?;
        // Graphite antialiases every path; CPU Skia leaves these alone, as their paint asks.
        if !cfg!(target_os = "macos") {
            for (points, _, state) in &parts {
                let mut path = PathBuilder::new();
                let [a, b, c] = points.map(|p| p.map(|v| v as f32));
                path.move_to(a[0], a[1]);
                path.line_to(b[0], b[1]);
                path.line_to(c[0], c[1]);
                path.close();
                let [r, g, b, a] = ARROW[*state].to_be_bytes();
                let mut paint = Paint::default();
                paint.set_color_rgba8(r, g, b, a);
                paint.anti_alias = false;
                pixmap.fill_path(
                    &path.finish()?,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }
        Some(pixmap)
    };
    let key = ("spin-button", w, h, up, pointer);
    skia::paint(key, bounds, draw, window, cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrows_sit_where_chromium_draws_them() {
        let aura = aura_square(15, 7);
        assert_eq!(arrow(aura, 0, true), [[4.0, 6.0], [11.0, 6.0], [7.5, 2.0]]);
        assert_eq!(
            arrow(aura, 7, false),
            [[4.0, 8.0], [11.0, 8.0], [7.5, 12.0]]
        );
        for pressed in [false, true] {
            assert_eq!(fluent_square(15, 7, true, pressed), [4, -1, 7]);
            assert_eq!(fluent_square(15, 7, false, pressed), [4, 1, 7]);
        }
        assert_eq!(fluent_square(30, 15, true, false), [7, -2, 15]);
        assert_eq!(fluent_square(30, 15, false, true), [8, 3, 13]);
    }
}
