//! `MechanismRenderer.ts`: draws a `MechanismState` as SVG lines with round end caps.

#![allow(clippy::todo, dead_code)]

use asrs_js::string::JsError;
use asrs_js::value::JsValue;
use asrs_model::tab_controller::{TabCommand, TabRenderer};

use crate::dom_tree::Element;

/// `[renderWidth, renderHeight]`: the largest box with the aspect ratio of `dimensions` that
/// fits the container's `[clientWidth, clientHeight]`, with upstream's comparison.
pub fn render_size(client_size: [f64; 2], dimensions: [f64; 2]) -> [f64; 2] {
    let _ = (client_size, dimensions);
    todo!("R08: render_size")
}

/// A mechanism point in SVG coordinates (y down).
pub fn to_svg(point: [f64; 2], dimensions: [f64; 2], render_size: [f64; 2]) -> [f64; 2] {
    let _ = (point, dimensions, render_size);
    todo!("R08: to_svg")
}

/// `[value[0], value[1]]` after `ToNumber`, as upstream's arithmetic reads a translation or the
/// dimensions (`null` is 0, a missing element NaN); a `null` or `undefined` value throws.
pub fn read_number_pair(value: &JsValue) -> Result<[f64; 2], JsError> {
    let _ = value;
    todo!("R08: read_number_pair")
}

/// What a `style` assignment hands `setProperty`: `null` becomes the empty string,
/// everything else its `ToString`.
pub fn style_string(value: &JsValue) -> String {
    let _ = value;
    todo!("R08: style_string")
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
        todo!("R08: MechanismRenderer::new")
    }

    /// The `svg.mechanism-svg` element and everything under it.
    pub fn svg(&self) -> &Element {
        todo!("R08: MechanismRenderer::svg")
    }

    pub fn svg_mut(&mut self) -> &mut Element {
        todo!("R08: MechanismRenderer::svg_mut")
    }

    /// `getAspectRatio()`.
    pub fn aspect_ratio(&self) -> f64 {
        todo!("R08: MechanismRenderer::aspect_ratio")
    }

    /// `render(command)` against a container of `client_size`. A throw leaves the tree as far
    /// as upstream got before it.
    pub fn render_command(
        &mut self,
        client_size: [f64; 2],
        command: &JsValue,
    ) -> Result<(), JsError> {
        let _ = (client_size, command);
        todo!("R08: MechanismRenderer::render_command")
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
