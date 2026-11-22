//! `PointsRenderer.ts`: clones a plus, cross or circle SVG template per point into the
//! `div.points-background` box.

use asrs_js::number;
use asrs_js::ops;
use asrs_js::string::JsError;
use asrs_js::value::JsValue;
use asrs_model::tab_controller::{TabCommand, TabRenderer};

use asrs_log::colors::GRAPH_COLORS;

use crate::dom_tree::{Element, Namespace};
use crate::list_cx::SourceListCx;
use crate::mechanism_renderer::for_each_items;

/// The background box for a container of `[width, height]` (its bounding rect) and the
/// command's `dimensions`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointsLayout {
    /// `getAspectRatio()` after the render.
    pub aspect_ratio: f64,
    pub final_width: f64,
    pub final_height: f64,
}

pub fn layout(container: [f64; 2], dimensions: [f64; 2]) -> PointsLayout {
    let [container_width, container_height] = container;
    let target_width = if dimensions[0] < 1.0 {
        1.0
    } else {
        dimensions[0]
    };
    let target_height = if dimensions[1] < 1.0 {
        1.0
    } else {
        dimensions[1]
    };
    let aspect_ratio = target_width / target_height;
    let (final_width, final_height) =
        if target_width / target_height < container_width / container_height {
            (
                container_height * (target_width / target_height),
                container_height,
            )
        } else {
            (
                container_width,
                container_width * (target_height / target_width),
            )
        };
    PointsLayout {
        aspect_ratio,
        final_width,
        final_height,
    }
}

impl PointsLayout {
    /// The `width` and `height` in px the background gets: `Math.ceil(final + 1)`.
    pub fn background_size(&self) -> [f64; 2] {
        [
            (self.final_width + 1.0).ceil(),
            (self.final_height + 1.0).ceil(),
        ]
    }

    /// The point's `left` and `top` in px, or `None` when it lies outside `dimensions`.
    pub fn position(&self, point: [f64; 2], dimensions: [f64; 2]) -> Option<[f64; 2]> {
        if point[0] < 0.0 || point[0] > dimensions[0] || point[1] < 0.0 || point[1] > dimensions[1]
        {
            return None;
        }
        Some([
            (point[0] / dimensions[0]) * self.final_width,
            (point[1] / dimensions[1]) * self.final_height,
        ])
    }
}

/// The colour of the `index`th point of a set: black or white below a group size of 1, else
/// the graph colour of its group. A group size that is not a number throws as upstream's
/// `GraphColors[NaN].key` does.
pub fn point_color(index: usize, group_size: f64, dark: bool) -> Result<&'static str, JsError> {
    if group_size < 1.0 {
        return Ok(if dark { "white" } else { "black" });
    }
    let slot = (index as f64 / group_size).floor() % GRAPH_COLORS.len() as f64;
    if slot.is_nan() {
        return Err(ops::get(&JsValue::Undefined, "key")
            .expect_err("reading a property of undefined throws"));
    }
    Ok(GRAPH_COLORS[slot as usize].key)
}

/// A fresh copy of the template `<svg>` for `shape` (`plus`, `cross` or `circle`) from
/// `hub.html`.
pub fn point_template(shape: &str) -> Option<Element> {
    let children: &[(&str, &[(&str, &str)])] = match shape {
        "plus" => &[
            (
                "line",
                &[("x1", "10"), ("y1", "0"), ("x2", "10"), ("y2", "20")],
            ),
            (
                "line",
                &[("x1", "0"), ("y1", "10"), ("x2", "20"), ("y2", "10")],
            ),
        ],
        "cross" => &[
            (
                "line",
                &[("x1", "2"), ("y1", "2"), ("x2", "18"), ("y2", "18")],
            ),
            (
                "line",
                &[("x1", "18"), ("y1", "2"), ("x2", "2"), ("y2", "18")],
            ),
        ],
        "circle" => &[("circle", &[("cx", "10"), ("cy", "10"), ("r", "7")])],
        _ => return None,
    };
    let mut svg =
        Element::with_attributes(Namespace::Svg, "svg", &[("height", "20"), ("width", "20")]);
    for (tag, attributes) in children {
        svg.append_child(Element::with_attributes(Namespace::Svg, tag, attributes));
    }
    Some(svg)
}

/// The `transform` a point of `size` gets, as upstream writes it, or `None` for an unknown
/// size.
pub fn size_transform(size: &str) -> Option<&'static str> {
    Some(match size {
        "large" => "translate(-50%,-50%) scale(1, 1)",
        "medium" => "translate(-50%,-50%) scale(0.5, 0.5)",
        "small" => "translate(-50%,-50%) scale(0.25, 0.25)",
        _ => return None,
    })
}

/// `value[key]` when it is a string, for upstream's `switch` on it.
fn string_member(value: &JsValue, key: &str) -> Result<Option<String>, JsError> {
    Ok(match &ops::get(value, key)? {
        JsValue::String(s) => Some(s.to_string_lossy()),
        _ => None,
    })
}

pub struct PointsRenderer {
    background: Element,
    aspect_ratio: f64,
    /// `CONTAINER.getBoundingClientRect()` width and height, kept current by the host.
    pub container_size: [f64; 2],
}

impl Default for PointsRenderer {
    fn default() -> Self {
        PointsRenderer::new()
    }
}

impl PointsRenderer {
    /// `<div class="points-background">` with no children, aspect ratio 1.
    pub fn new() -> PointsRenderer {
        PointsRenderer {
            background: Element::with_attributes(
                Namespace::Html,
                "div",
                &[("class", "points-background")],
            ),
            aspect_ratio: 1.0,
            container_size: [0.0, 0.0],
        }
    }

    /// The `div.points-background` element and the points under it.
    pub fn background(&self) -> &Element {
        &self.background
    }

    pub fn background_mut(&mut self) -> &mut Element {
        &mut self.background
    }

    /// `getAspectRatio()`.
    pub fn aspect_ratio(&self) -> f64 {
        self.aspect_ratio
    }

    /// `render(command)` against a container of `container_size`, in the colour scheme
    /// `dark`. A throw leaves the tree as far as upstream got before it.
    pub fn render_command(
        &mut self,
        container_size: [f64; 2],
        dark: bool,
        command: &JsValue,
    ) -> Result<(), JsError> {
        let number_at = |value: &JsValue, i: usize| ops::value_to_number(&ops::index(value, i)?);
        let dimensions_value = ops::get(command, "dimensions")?;
        let dimensions = [
            number_at(&dimensions_value, 0)?,
            number_at(&dimensions_value, 1)?,
        ];
        let layout = layout(container_size, dimensions);
        self.aspect_ratio = layout.aspect_ratio;

        let [width, height] = layout.background_size();
        let background = &mut self.background;
        background.set_style("width", &format!("{}px", number::to_string(width)));
        background.set_style("height", &format!("{}px", number::to_string(height)));

        background.clear_children();

        let sets = ops::get(command, "sets")?;
        for set in for_each_items(&sets, "command.sets")? {
            let points = ops::get(set, "points")?;
            for (index, position_source) in
                for_each_items(&points, "set.points")?.iter().enumerate()
            {
                let point = [
                    number_at(position_source, 0)?,
                    number_at(position_source, 1)?,
                ];
                let Some(position) = layout.position(point, dimensions) else {
                    continue;
                };

                let Some(mut point) = string_member(set, "shape")?.and_then(|s| point_template(&s))
                else {
                    continue;
                };
                if let Some(transform) =
                    string_member(set, "size")?.and_then(|s| size_transform(&s))
                {
                    point.set_style("transform", transform);
                }

                let group_size = ops::value_to_number(&ops::get(set, "groupSize")?)?;
                let color = point_color(index, group_size, dark)?;
                point.set_style("fill", color);
                point.set_style("stroke", color);

                point.set_style("left", &format!("{}px", number::to_string(position[0])));
                point.set_style("top", &format!("{}px", number::to_string(position[1])));
                background.append_child(point);
            }
        }
        Ok(())
    }
}

impl<Cx: SourceListCx + ?Sized> TabRenderer<Cx> for PointsRenderer {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        JsValue::Null
    }

    fn restore_state(&mut self, _cx: &mut Cx, _state: &JsValue) -> Result<(), JsError> {
        Ok(())
    }

    fn aspect_ratio(&self, _cx: &Cx) -> Option<f64> {
        Some(self.aspect_ratio())
    }

    fn render(&mut self, cx: &mut Cx, command: &dyn TabCommand) -> Result<(), JsError> {
        let container_size = self.container_size;
        let dark = cx.tab_dark();
        self.render_command(container_size, dark, &command.to_js())
    }
}
