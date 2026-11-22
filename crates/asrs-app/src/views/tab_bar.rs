//! The tab bar (`div.tab-bar` with its `div.tab` titles, shadows, drag highlight and the
//! `div.tab-bar-scroll` overlay) as `www/hub.css` draws it. Everything it shows comes from
//! `asrs_hub::tabs::Tabs`; the geometry Tabs reads (title rects, the bar's scroll extent) is
//! laid out here and answered through [`TabBarGeometry`].

use asrs_hub::tabs::TitleElement;
use asrs_js::math;
use asrs_model::scroll_sensor::assign_scroll_offset;
use asrs_model::source_list::Rect;
use asrs_ui::gpui::{
    AnyElement, App, IntoElement, ParentElement, Styled, div, linear_color_stop, linear_gradient,
    px, rgb, rgba,
};
use asrs_ui::text::{self, Text, TextStyle};
use asrs_ui::theme::FONT_FAMILY;

/// `div.tab`: `margin-top: 10px`, `height: 30px` plus a 1 px border.
const TAB_TOP: f64 = 10.0;
const TAB_HEIGHT: f64 = 32.0;
/// `padding-left` and `padding-right` plus the border.
const TAB_INSET: f64 = 11.0;

pub fn tab_style() -> TextStyle {
    let font = format!("16px {}", FONT_FAMILY.join(", "));
    TextStyle::from_css([
        ("font", font.as_str()),
        ("line-height", "30px"),
        ("white-space", "nowrap"),
    ])
    .expect("tab style")
}

/// The tab bar's layout and scroll position: what `Tabs` reads through `TabsHost`.
#[derive(Clone, Debug, Default)]
pub struct TabBarGeometry {
    /// `div.tab-bar`'s client rect, `[x, y, width, height]`.
    pub bar: [f64; 4],
    /// Each `div.tab`'s border-box width, in bar order.
    pub widths: Vec<f64>,
    pub add_button: [f64; 4],
    pub device_pixel_ratio: f64,
    pub scroll_left: f64,
}

impl TabBarGeometry {
    /// Lays the titles out for `texts` (each title's `innerText`) in a bar at `bar`.
    pub fn layout(&mut self, cx: &mut App, bar: [f64; 4], texts: &[String], scale: f32) {
        let style = tab_style();
        self.bar = bar;
        self.device_pixel_ratio = f64::from(scale);
        self.widths = texts
            .iter()
            .map(|t| text::measure(cx, &style, t, scale).0 + 2.0 * TAB_INSET)
            .collect();
        self.scroll_left = assign_scroll_offset(
            self.scroll_left,
            self.scroll_width() - self.client_width(),
            self.device_pixel_ratio,
        );
    }

    pub fn client_width(&self) -> f64 {
        math::round(self.bar[2])
    }

    pub fn scroll_width(&self) -> f64 {
        let content: f64 = self.widths.iter().sum();
        math::max2(self.client_width(), math::round(content))
    }

    fn left_of(&self, index: usize) -> f64 {
        self.widths[..index.min(self.widths.len())].iter().sum()
    }

    pub fn title_rect(&self, index: usize) -> Rect {
        Rect {
            x: self.bar[0] + self.left_of(index) - self.scroll_left,
            y: self.bar[1] + TAB_TOP,
            width: self.widths.get(index).copied().unwrap_or(0.0),
            height: TAB_HEIGHT,
        }
    }

    pub fn overlay_rect(&self) -> Rect {
        let [x, y, width, height] = self.bar;
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    pub fn add_button_rect(&self) -> Rect {
        let [x, y, width, height] = self.add_button;
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// `TAB_BAR.scrollLeft = value`.
    pub fn assign_scroll_left(&mut self, value: f64) {
        self.scroll_left = assign_scroll_offset(
            value,
            self.scroll_width() - self.client_width(),
            self.device_pixel_ratio,
        );
    }
}

fn tab_colors(element: &TitleElement, dark: bool) -> (u32, u32) {
    let has = |c: &str| element.classes.contains(&c);
    match (dark, has("tab-selected"), has("tab-hovered")) {
        (false, true, _) => (0xaaaaaa, 0x222222),
        (false, false, true) => (0xdddddd, 0xaaaaaa),
        (false, false, false) => (0xeeeeee, 0xaaaaaa),
        (true, true, _) => (0x888888, 0xeeeeee),
        (true, false, true) => (0x555555, 0x666666),
        (true, false, false) => (0x444444, 0x666666),
    }
}

/// One `div.tab`, `width` wide, at the origin of its parent.
pub fn tab(
    element: &TitleElement,
    width: f64,
    first: bool,
    last: bool,
    dark: bool,
    text_color: u32,
) -> asrs_ui::gpui::Div {
    let (background, border) = tab_colors(element, dark);
    let mut tab = div()
        .absolute()
        .w(px(width as f32))
        .h(px(TAB_HEIGHT as f32))
        .bg(rgb(background))
        .border_1()
        .border_color(rgb(border));
    if first {
        tab = tab.rounded_l(px(10.0));
    }
    if last {
        tab = tab.rounded_r(px(10.0));
    }
    let label = element.inner_text().to_string_lossy();
    tab.child(
        Text::new(label, tab_style())
            .color(text_color)
            .absolute()
            .left(px((TAB_INSET - 1.0) as f32))
            .top(px(0.0)),
    )
}

/// The titles, clipped to the bar and scrolled by its `scrollLeft`, in bar coordinates.
pub fn titles(
    geometry: &TabBarGeometry,
    elements: &[TitleElement],
    dark: bool,
    text_color: u32,
) -> AnyElement {
    let mut row = div()
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .w(px(geometry.bar[2] as f32))
        .h(px(geometry.bar[3] as f32))
        .overflow_hidden();
    let count = elements.len();
    for (i, element) in elements.iter().enumerate() {
        let x = geometry.left_of(i) - geometry.scroll_left;
        let width = geometry.widths.get(i).copied().unwrap_or(0.0);
        row = row.child(
            tab(element, width, i == 0, i + 1 == count, dark, text_color)
                .left(px(x as f32))
                .top(px(TAB_TOP as f32)),
        );
    }
    row.into_any_element()
}

/// `div.tab-bar-shadow-left` or `-right` at `rect` (viewer coordinates relative to `origin`)
/// with its `style.opacity`.
pub fn shadow(
    rect: [f64; 4],
    origin: (f64, f64),
    left: bool,
    opacity: Option<&str>,
    dark: bool,
) -> AnyElement {
    let shown = opacity == Some("1");
    let solid: u32 = if dark { 0x222222ff } else { 0xffffffff };
    let mid: u32 = if dark { 0x222222bf } else { 0xffffffbf };
    let clear: u32 = if dark { 0x22222200 } else { 0xffffff00 };
    let half = rect[2] / 2.0;
    let angle = if left { 90.0 } else { 270.0 };
    let (near, far) = if left { (0.0, half) } else { (half, 0.0) };
    let base = div()
        .absolute()
        .left(px((rect[0] - origin.0) as f32))
        .top(px((rect[1] - origin.1) as f32))
        .w(px(rect[2] as f32))
        .h(px(rect[3] as f32))
        .opacity(if shown { 1.0 } else { 0.0 });
    let piece = |x: f64, from: u32, to: u32| {
        div()
            .absolute()
            .left(px(x as f32))
            .top(px(0.0))
            .w(px(half as f32))
            .h(px(rect[3] as f32))
            .bg(linear_gradient(
                angle,
                linear_color_stop(rgba(from), 0.0),
                linear_color_stop(rgba(to), 1.0),
            ))
    };
    base.child(piece(near, solid, mid))
        .child(piece(far, mid, clear))
        .into_any_element()
}

/// `div.tab-bar-drag-highlight` with `style.left = left`, in viewer coordinates relative to
/// `origin` (`translateX(-50%)` of its 15 px width).
pub fn drag_highlight(left: f64, viewer: (f64, f64), origin: (f64, f64)) -> AnyElement {
    div()
        .absolute()
        .left(px((viewer.0 + left - 7.5 - origin.0) as f32))
        .top(px((viewer.1 + 10.0 - origin.1) as f32))
        .w(px(15.0))
        .h(px(32.0))
        .bg(rgb(0x90ee90))
        .opacity(0.75)
        .into_any_element()
}
