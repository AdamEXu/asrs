//! `::-webkit-inner-spin-button`: what Blink's `SpinButtonElement` does with the pointer and how
//! Chromium's native theme paints it. Its arrows are the scrollbar's.

use gpui_kit::{App, Bounds, Pixels, Window};
use tiny_skia::{FillRule, Paint, PathBuilder, Transform};

use crate::input::native::PointerState;
use crate::input::skia;

/// `GetVerticalScrollbarButtonSize().width()`.
pub const WIDTH: f32 = 15.0;

const BACKGROUND: [u32; 3] = [0xf1f1f1ff, 0xd2d2d2ff, 0x787878ff];
const ARROW: [u32; 3] = [0x505050ff, 0x505050ff, 0xffffffff];

/// `GetArrowRect` and `PathForArrow` for an up or down button of `w` x `h` device px at (0, `y`).
fn arrow(w: i32, h: i32, y: i32, up: bool) -> [[f64; 2]; 3] {
    let inset = (w.max(h) + 3) / 4;
    let side = w.min(h).min(w.max(h) - inset * 2);
    let left = f64::from((w - side + 1) / 2);
    let top = f64::from(y + (h - side + 1) / 2);
    let side_f = f64::from(side);
    let height = f64::from(side / 2 + 1);
    let base = top + side_f - f64::from((side / 2 + 1) / 2) + 1.0;
    let points = [
        [left, base],
        [left + side_f, base],
        [left + side_f / 2.0, base - height],
    ];
    if up {
        return points;
    }
    let mirror = 2.0 * top + side_f;
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
            (arrow(w as i32, half, y, part), y, state as usize)
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
        assert_eq!(arrow(15, 7, 0, true), [[4.0, 6.0], [11.0, 6.0], [7.5, 2.0]]);
        assert_eq!(
            arrow(15, 7, 7, false),
            [[4.0, 8.0], [11.0, 8.0], [7.5, 12.0]]
        );
    }
}
