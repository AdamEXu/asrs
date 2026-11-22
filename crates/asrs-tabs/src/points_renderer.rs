//! `PointsRenderer.ts`: clones a plus, cross or circle SVG template per point into the
//! `div.points-background` box.

#![allow(clippy::todo, dead_code)]

use asrs_js::string::JsError;
use asrs_js::value::JsValue;
use asrs_model::tab_controller::{TabCommand, TabRenderer};

use crate::dom_tree::Element;
use crate::list_cx::SourceListCx;

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
    let _ = (container, dimensions);
    todo!("R08: points layout")
}

impl PointsLayout {
    /// The `width` and `height` in px the background gets: `Math.ceil(final + 1)`.
    pub fn background_size(&self) -> [f64; 2] {
        todo!("R08: PointsLayout::background_size")
    }

    /// The point's `left` and `top` in px, or `None` when it lies outside `dimensions`.
    pub fn position(&self, point: [f64; 2], dimensions: [f64; 2]) -> Option<[f64; 2]> {
        let _ = (point, dimensions);
        todo!("R08: PointsLayout::position")
    }
}

/// The colour of the `index`th point of a set: black or white below a group size of 1, else
/// the graph colour of its group. A group size that is not a number throws as upstream's
/// `GraphColors[NaN].key` does.
pub fn point_color(index: usize, group_size: f64, dark: bool) -> Result<&'static str, JsError> {
    let _ = (index, group_size, dark);
    todo!("R08: point_color")
}

/// A fresh copy of the template `<svg>` for `shape` (`plus`, `cross` or `circle`) from
/// `hub.html`.
pub fn point_template(shape: &str) -> Option<Element> {
    let _ = shape;
    todo!("R08: point_template")
}

/// The `transform` a point of `size` gets, as upstream writes it, or `None` for an unknown
/// size.
pub fn size_transform(size: &str) -> Option<&'static str> {
    let _ = size;
    todo!("R08: size_transform")
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
        todo!("R08: PointsRenderer::new")
    }

    /// The `div.points-background` element and the points under it.
    pub fn background(&self) -> &Element {
        todo!("R08: PointsRenderer::background")
    }

    pub fn background_mut(&mut self) -> &mut Element {
        todo!("R08: PointsRenderer::background_mut")
    }

    /// `getAspectRatio()`.
    pub fn aspect_ratio(&self) -> f64 {
        todo!("R08: PointsRenderer::aspect_ratio")
    }

    /// `render(command)` against a container of `container_size`, in the colour scheme
    /// `dark`. A throw leaves the tree as far as upstream got before it.
    pub fn render_command(
        &mut self,
        container_size: [f64; 2],
        dark: bool,
        command: &JsValue,
    ) -> Result<(), JsError> {
        let _ = (container_size, dark, command);
        todo!("R08: PointsRenderer::render_command")
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
