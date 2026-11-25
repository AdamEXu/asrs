//! `div.title-bar-menu` of `www/hub.html`, which Windows and Linux show over the sidebar: its
//! table's columns as Blink's automatic table layout makes them, and the buttons.

use asrs_ui::focus::{Activate, FocusRing};
use asrs_ui::gpui::{
    AnyElement, App, Div, InteractiveElement, IntoElement, MouseButton, ParentElement,
    ScrollHandle, Stateful, StatefulInteractiveElement, Styled, Window, div, px, rgb, rgba,
};
use asrs_ui::input::style::CONTROL_FONT;
use asrs_ui::parity::probed;
use asrs_ui::text::{self, FontSpec, Text, TextStyle, WhiteSpace, snap_remainder};
use asrs_ui::theme::{Appearance, BUTTON_RADIUS, FOCUS_RING_RADIUS};
use asrs_ui::tooltip::titled;

pub const LABELS: [&str; 6] = ["App", "File", "View", "Tabs", "Window", "Help"];
const PADDING: f64 = 4.0;
const BUTTON_HEIGHT: f64 = 28.0;
const BUTTON_PADDING: f64 = 3.0;
const BUTTON_PARITY_ID: &str = "div.title-bar-menu button";

fn label_style() -> TextStyle {
    let mut style = TextStyle::new(FontSpec {
        size: 12.0,
        ..CONTROL_FONT
    });
    style.white_space = WhiteSpace::Pre;
    style
}

/// The widths `DistributeInlineSizeToComputedInlineSizeAuto` gives the columns whose cells'
/// max-content widths are `content`, in a table of `available` width, all in `LayoutUnit`s (1/64
/// device px): the excess in proportion to the content, its rounding on the last column. Cells
/// cannot shrink, since a button's label does not break.
fn column_widths(content: &[i64], available: i64) -> Vec<i64> {
    let total: i64 = content.iter().sum();
    let excess = available - total;
    if total == 0 || excess <= 0 {
        return content.to_vec();
    }
    let mut widths: Vec<i64> = content.iter().map(|&w| w + excess * w / total).collect();
    let used: i64 = widths.iter().sum();
    if let Some(last) = widths.last_mut() {
        *last += available - used;
    }
    widths
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuLayout {
    /// The buttons' boxes from the menu's border box, unscrolled: `[x, y, width, height]`.
    pub buttons: Vec<[f64; 4]>,
    /// The labels' boxes from the buttons'.
    labels: Vec<[f64; 4]>,
}

impl MenuLayout {
    pub fn compute(width: f64, scale: f32, cx: &mut App) -> MenuLayout {
        let style = label_style();
        let zoom = f64::from(scale);
        let lu = |css: f64| (css * zoom * 64.0) as i64;
        let css = |units: i64| units as f64 / 64.0 / zoom;
        let sizes: Vec<(f64, f64)> = LABELS
            .iter()
            .map(|label| text::measure(cx, &style, label, scale))
            .collect();
        let content: Vec<i64> = sizes
            .iter()
            .map(|(w, _)| lu(*w) + 2 * lu(BUTTON_PADDING))
            .collect();
        let mut x = lu(PADDING);
        let mut layout = MenuLayout::default();
        let columns = column_widths(&content, lu(width) - 2 * lu(PADDING));
        for (column, (w, h)) in columns.iter().zip(&sizes) {
            layout
                .buttons
                .push([css(x), PADDING, css(*column), BUTTON_HEIGHT]);
            layout.labels.push([
                css((column - lu(*w)) / 2),
                css((lu(BUTTON_HEIGHT) - lu(*h)) / 2),
                *w,
                *h,
            ]);
            x += column;
        }
        layout
    }

    /// `scrollWidth`.
    fn scroll_width(&self) -> f64 {
        self.buttons
            .last()
            .map_or(0.0, |[x, _, w, _]| x + w + PADDING)
    }

    /// The buttons' `getBoundingClientRect()`s, for the menu at `rect`.
    pub fn client_rects(&self, rect: [f64; 4], scroll: &ScrollHandle) -> Vec<[f64; 4]> {
        let scrolled = f64::from(f32::from(scroll.offset().x));
        self.buttons
            .iter()
            .map(|[x, y, w, h]| [rect[0] + x + scrolled, rect[1] + y, *w, *h])
            .collect()
    }
}

/// The menu's contents, in a box of the size of `div.title-bar-menu` at `rect`. `on_click` and
/// `on_leave` get a button's index.
pub fn title_bar_menu(
    layout: &MenuLayout,
    rect: [f64; 4],
    scroll: &ScrollHandle,
    on_click: impl Fn(usize, &mut Window, &mut App) + Clone + 'static,
    on_leave: impl Fn(usize, &mut App) + Clone + 'static,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let mut content = div()
        .relative()
        .flex_none()
        .w(px(layout.scroll_width() as f32))
        .h(px(rect[3] as f32));
    let buttons = layout.buttons.iter().zip(&layout.labels).zip(LABELS);
    for (index, ((button, label), text)) in buttons.enumerate() {
        content = content.child(menu_button(
            index,
            (rect[0], *button),
            (text, *label),
            on_click.clone(),
            on_leave.clone(),
            window,
            cx,
        ));
    }
    div()
        .id("title-bar-menu")
        .flex()
        .size_full()
        .overflow_x_scroll()
        .restrict_scroll_to_axis()
        .track_scroll(scroll)
        .child(content)
}

fn menu_button(
    index: usize,
    (menu_x, [x, y, w, h]): (f64, [f64; 4]),
    (text, label): (&'static str, [f64; 4]),
    on_click: impl Fn(usize, &mut Window, &mut App) + 'static,
    on_leave: impl Fn(usize, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let scale = window.scale_factor();
    let appearance = Appearance::of(window, cx);
    let id = ("title-bar-menu-button", index);
    let focus = FocusRing::of(id, None, window, cx);
    let (color, hover, active) = appearance.pick(
        (0x000000, 0xcccccc88, 0xcccccc),
        (0xeeeeee, 0x44444488, 0x444444),
    );
    let pressed = focus.handle.clone();
    let label_x = menu_x + x + label[0];
    let snap = |v: f64| (v * f64::from(scale)).round() / f64::from(scale);
    // GPUI snaps the button first, and the label from there.
    let label_left = snap(label_x) - snap(menu_x + x);
    let button = div()
        .id(id)
        .size_full()
        .relative()
        .rounded(px(if focus.visible {
            FOCUS_RING_RADIUS
        } else {
            BUTTON_RADIUS
        }))
        .track_focus(&focus.handle)
        .hover(move |s| s.bg(rgba(hover)))
        .active(move |s| s.bg(rgb(active)))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            pressed.focus(window, cx);
            cx.stop_propagation();
        })
        .on_activate(move |_, window, cx| on_click(index, window, cx))
        .on_hover(move |hovered, _, cx| {
            if !hovered {
                on_leave(index, cx);
            }
        })
        .child(
            Text::new(text, label_style())
                .color(color)
                .subpixel(snap_remainder(label_x as f32, scale), 0.0)
                .absolute()
                .left(px(label_left as f32))
                .top(px(label[1] as f32)),
        )
        .children(focus.outline(appearance));
    probed(
        BUTTON_PARITY_ID,
        div()
            .absolute()
            .left(px(x as f32))
            .top(px(y as f32))
            .w(px(w as f32))
            .h(px(h as f32))
            .child(titled(format!("{text} menu"), button)),
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::column_widths;

    /// `td` boxes of Electron 39's `div.title-bar-menu` on Linux (Liberation Sans), in
    /// `LayoutUnit`s: the scale, the cells' natural widths, then per `--side-bar-width` the
    /// table's width and its columns.
    #[test]
    fn columns_are_chromiums() {
        let one = [1751, 1622, 2035, 2007, 3116, 1964];
        let two = [3501, 3243, 4070, 4013, 6231, 3927];
        for (content, table, want) in MEASURED {
            let natural = if *content == 1 { &one } else { &two };
            assert_eq!(column_widths(natural, *table), want, "{content}x {table}");
        }
    }

    const MEASURED: &[(u32, i64, [i64; 6])] = &[
        (1, 0, [1751, 1622, 2035, 2007, 3116, 1964]),
        (1, 12288, [1751, 1622, 2035, 2007, 3116, 1964]),
        (1, 12928, [1811, 1678, 2105, 2076, 3223, 2035]),
        (1, 13568, [1901, 1761, 2209, 2179, 3383, 2135]),
        (1, 14208, [1991, 1844, 2313, 2282, 3543, 2235]),
        (1, 14656, [2053, 1902, 2386, 2354, 3654, 2307]),
        (1, 14848, [2080, 1927, 2418, 2384, 3702, 2337]),
        (1, 15072, [2112, 1956, 2454, 2420, 3758, 2372]),
        (1, 15488, [2170, 2010, 2522, 2487, 3862, 2437]),
        (2, 0, [3501, 3243, 4070, 4013, 6231, 3927]),
        (2, 24576, [3501, 3243, 4070, 4013, 6231, 3927]),
        (2, 25856, [3623, 3356, 4211, 4152, 6448, 4066]),
        (2, 27136, [3802, 3522, 4420, 4358, 6767, 4267]),
        (2, 28416, [3981, 3688, 4628, 4564, 7086, 4469]),
        (2, 29312, [4107, 3804, 4774, 4707, 7310, 4610]),
        (2, 29696, [4161, 3854, 4837, 4769, 7405, 4670]),
        (2, 30144, [4223, 3912, 4910, 4841, 7517, 4741]),
        (2, 30976, [4340, 4020, 5045, 4975, 7725, 4871]),
    ];
}
