//! `MechanismRenderer.ts`: draws a `MechanismState` as SVG lines with round end caps.

use asrs_js::number;
use asrs_js::ops;
use asrs_js::string::JsError;
use asrs_js::value::JsValue;
use asrs_model::tab_controller::{TabCommand, TabRenderer};

use crate::dom_tree::{Element, Namespace};

/// `[renderWidth, renderHeight]`: the largest box with the aspect ratio of `dimensions` that
/// fits the container's `[clientWidth, clientHeight]`, with upstream's comparison.
pub fn render_size(client_size: [f64; 2], dimensions: [f64; 2]) -> [f64; 2] {
    let [client_width, client_height] = client_size;
    if client_width / client_height > dimensions[0] / dimensions[1] {
        [
            client_height * (dimensions[0] / dimensions[1]),
            client_height,
        ]
    } else {
        [client_width, client_width * (dimensions[1] / dimensions[0])]
    }
}

/// A mechanism point in SVG coordinates (y down).
pub fn to_svg(point: [f64; 2], dimensions: [f64; 2], render_size: [f64; 2]) -> [f64; 2] {
    let [width, height] = render_size;
    [
        (point[0] / dimensions[0]) * width,
        height - (point[1] / dimensions[1]) * height,
    ]
}

/// `[value[0], value[1]]` after `ToNumber`, as upstream's arithmetic reads a translation or the
/// dimensions (`null` is 0, a missing element NaN); a `null` or `undefined` value throws.
pub fn read_number_pair(value: &JsValue) -> Result<[f64; 2], JsError> {
    Ok([
        ops::value_to_number(&ops::index(value, 0)?)?,
        ops::value_to_number(&ops::index(value, 1)?)?,
    ])
}

/// What a `style` assignment hands `setProperty`: `null` becomes the empty string,
/// everything else its `ToString`.
pub fn style_string(value: &JsValue) -> String {
    match value {
        JsValue::Null => String::new(),
        v => ops::to_string(v).to_string_lossy(),
    }
}

/// The elements `<expression>.forEach` visits, or what it throws when `value` is not an array.
pub(crate) fn for_each_items<'a>(
    value: &'a JsValue,
    expression: &str,
) -> Result<&'a [JsValue], JsError> {
    match value {
        JsValue::Array(items) => Ok(items),
        JsValue::Null | JsValue::Undefined => {
            Err(ops::get(value, "forEach")
                .expect_err("reading a property of null or undefined throws"))
        }
        _ => Err(JsError::type_error(format!(
            "{expression}.forEach is not a function"
        ))),
    }
}

pub struct MechanismRenderer {
    svg: Element,
    aspect_ratio: f64,
    /// `CONTAINER.clientWidth` and `clientHeight`, kept current by the host.
    pub client_size: [f64; 2],
}

impl Default for MechanismRenderer {
    fn default() -> Self {
        MechanismRenderer::new()
    }
}

impl MechanismRenderer {
    /// `<svg class="mechanism-svg">` with no children, aspect ratio 1.
    pub fn new() -> MechanismRenderer {
        MechanismRenderer {
            svg: Element::with_attributes(Namespace::Svg, "svg", &[("class", "mechanism-svg")]),
            aspect_ratio: 1.0,
            client_size: [0.0, 0.0],
        }
    }

    /// The `svg.mechanism-svg` element and everything under it.
    pub fn svg(&self) -> &Element {
        &self.svg
    }

    pub fn svg_mut(&mut self) -> &mut Element {
        &mut self.svg
    }

    /// `getAspectRatio()`.
    pub fn aspect_ratio(&self) -> f64 {
        self.aspect_ratio
    }

    /// `render(command)` against a container of `client_size`. A throw leaves the tree as far
    /// as upstream got before it.
    pub fn render_command(
        &mut self,
        client_size: [f64; 2],
        command: &JsValue,
    ) -> Result<(), JsError> {
        let svg = &mut self.svg;
        let hidden = matches!(command, JsValue::Null);
        svg.set_style("display", if hidden { "none" } else { "initial" })?;
        if hidden {
            return Ok(());
        }

        let dimensions = read_number_pair(&ops::get(command, "dimensions")?)?;
        let size = render_size(client_size, dimensions);
        svg.set_attribute("width", &number::to_string(size[0]));
        svg.set_attribute("height", &number::to_string(size[1]));
        let background = style_string(&ops::get(command, "backgroundColor")?);
        svg.set_style("background-color", &background)?;
        self.aspect_ratio = dimensions[0] / dimensions[1];

        svg.clear_children();

        let lines = ops::get(command, "lines")?;
        for line_data in for_each_items(&lines, "command.lines")? {
            let start = to_svg(
                read_number_pair(&ops::get(line_data, "start")?)?,
                dimensions,
                size,
            );
            let end = to_svg(
                read_number_pair(&ops::get(line_data, "end")?)?,
                dimensions,
                size,
            );

            let line = svg.append_child(Element::new(Namespace::Svg, "line"));
            line.set_attribute("x1", &number::to_string(start[0]));
            line.set_attribute("y1", &number::to_string(start[1]));
            line.set_attribute("x2", &number::to_string(end[0]));
            line.set_attribute("y2", &number::to_string(end[1]));
            let color = style_string(&ops::get(line_data, "color")?);
            line.set_style("stroke", &color)?;
            let weight = ops::get(line_data, "weight")?;
            ops::get(&weight, "toString")?;
            line.set_style("stroke-width", &ops::to_string(&weight).to_string_lossy())?;

            for coordinates in [start, end] {
                let circle = svg.append_child(Element::new(Namespace::Svg, "circle"));
                circle.set_attribute("cx", &number::to_string(coordinates[0]));
                circle.set_attribute("cy", &number::to_string(coordinates[1]));
                let radius = ops::value_to_number(&ops::get(line_data, "weight")?)? / 2.0;
                circle.set_attribute("r", &number::to_string(radius));
                let color = style_string(&ops::get(line_data, "color")?);
                circle.set_style("fill", &color)?;
            }
        }
        Ok(())
    }
}

impl<Cx: ?Sized> TabRenderer<Cx> for MechanismRenderer {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        JsValue::Null
    }

    fn restore_state(&mut self, _cx: &mut Cx, _state: &JsValue) -> Result<(), JsError> {
        Ok(())
    }

    fn aspect_ratio(&self, _cx: &Cx) -> Option<f64> {
        Some(self.aspect_ratio())
    }

    fn render(&mut self, _cx: &mut Cx, command: &dyn TabCommand) -> Result<(), JsError> {
        let client_size = self.client_size;
        self.render_command(client_size, &command.to_js())
    }
}
