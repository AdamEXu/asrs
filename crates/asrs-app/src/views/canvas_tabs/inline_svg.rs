//! Inline `<svg>` elements the Mechanism and Points renderers leave in their DOM, painted as
//! Chromium rasterizes the DOM on this platform.

use asrs_canvas::color;
use asrs_canvas::pixels::to_unorm;
use asrs_tabs::dom_tree::Element;
use asrs_ui::canvas_view::FramePresenter;
use asrs_ui::gpui::{App, Window};
use asrs_ui::svg::{ElementShape, InlineElement};

use super::boxes::Layer;

/// What a layer was painted from: the element tree, its container and the window's scale and
/// text colour.
#[derive(Clone, PartialEq)]
pub struct SvgInputs {
    pub tree: Element,
    pub container: [f64; 4],
    pub scale: f32,
    pub text: u32,
    pub dark: bool,
}

#[derive(Default)]
pub struct SvgView {
    presenter: FramePresenter,
    inputs: Option<SvgInputs>,
    origin: Option<[i32; 2]>,
}

impl SvgView {
    /// Shows what `paint` draws for `inputs`, unless they equal the last ones.
    pub fn update(
        &mut self,
        inputs: SvgInputs,
        paint: impl FnOnce(&SvgInputs) -> Option<Layer>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.inputs.as_ref() == Some(&inputs) {
            return;
        }
        match paint(&inputs) {
            Some(layer) => {
                self.origin = Some([layer.x, layer.y]);
                self.presenter.present(layer.pixmap, window, cx);
            }
            None => {
                self.origin = None;
                self.presenter.clear(window, cx);
            }
        }
        self.inputs = Some(inputs);
    }

    pub fn paint(&self, window: &mut Window) {
        if let Some([x, y]) = self.origin {
            let scale = window.scale_factor();
            self.presenter
                .paint([x as f32 / scale, y as f32 / scale], window);
        }
    }
}

/// A number attribute or inline length as Blink reads it, `None` where its parser rejects the
/// value and the property keeps its initial value.
pub fn number(value: &str) -> Option<f64> {
    let value = value.strip_suffix("px").unwrap_or(value);
    value.parse().ok().filter(|v: &f64| v.is_finite())
}

/// The used value of `color` where `currentcolor` is `text`.
fn resolve(value: &str, inherited: Option<[u8; 4]>, text: u32) -> Option<[u8; 4]> {
    match value {
        "" | "inherit" | "initial" | "unset" | "revert" | "revert-layer" => inherited,
        "none" => None,
        "currentcolor" => Some(super::boxes::opaque(text)),
        _ => color::parse(value).map(|c| {
            let [r, g, b] = c.to_u8();
            [r, g, b, to_unorm(c.a)]
        }),
    }
}

pub fn background_color(element: &Element, text: u32) -> Option<[u8; 4]> {
    resolve(element.style().get("background-color"), None, text)
}

/// `shape`'s paint: its own inline style over its `<svg>`'s.
pub fn painted(
    element: &Element,
    svg: &Element,
    stroke_width: f32,
    text: u32,
) -> Option<InlineElement> {
    let read = |name: &str| {
        element
            .attribute(name)
            .and_then(number)
            .map_or(0.0, |v| v as f32)
    };
    let shape = match element.tag() {
        "line" => ElementShape::Line([read("x1"), read("y1"), read("x2"), read("y2")]),
        "circle" => ElementShape::Circle {
            center: [read("cx"), read("cy")],
            radius: read("r"),
        },
        _ => return None,
    };
    let paint = |name: &str, initial: Option<[u8; 4]>| {
        let inherited = resolve(svg.style().get(name), initial, text);
        resolve(element.style().get(name), inherited, text)
    };
    Some(InlineElement {
        shape,
        fill: paint("fill", Some([0, 0, 0, 255])),
        stroke: paint("stroke", None),
        stroke_width: number(element.style().get("stroke-width"))
            .map_or(stroke_width, |v| v as f32),
    })
}
