//! `div.points-background-container`: the `div.points-background` `PointsRenderer` fills with
//! point SVGs, and `div.points-settings`.

use std::collections::HashMap;

use asrs_canvas::Pixmap;
use asrs_canvas::path::Affine;
use asrs_js::math;
use asrs_model::tab_type::TabType;
use asrs_tabs::dom_tree::Element;
use asrs_tabs::points_renderer::PointsRenderer;
use asrs_ui::canvas_view::geometry::client_rect_coord;
use asrs_ui::gpui::{AnyElement, Context, IntoElement, ParentElement, Styled, Window, canvas, div};
use asrs_ui::input::style::device_border;
use asrs_ui::parity::probed;
use asrs_ui::svg::{Backend, Document, InlineElement};
use asrs_ui::theme::Appearance;

use super::super::frame::{FrameHost, HubFrame};
use super::boxes::{CentredBox, Layer, opaque};
use super::inline_svg::{self, SvgInputs};
use super::settings::{Block, Control, Panel};
use super::{displayed, local, lu, overhung, with_renderer};

const POINT_SIZE: f64 = 20.0;
/// `div.points-background svg { stroke-width: 5px }`.
const STROKE_WIDTH: f32 = 5.0;

/// The canvas and background colours `renderers.css` shares between Swerve and Points: the fill
/// and the 1 px border.
pub(super) fn surface(dark: bool) -> ([u8; 4], [u8; 4]) {
    if dark {
        (opaque(0x292929), opaque(0x999999))
    } else {
        (opaque(0xf4f4f4), opaque(0x555555))
    }
}

fn background_box(inputs: &SvgInputs) -> CentredBox {
    let scale = f64::from(inputs.scale);
    let border = f64::from(device_border(1.0, inputs.scale));
    let style = inputs.tree.style();
    let layout = ["width", "height"]
        .map(|name| lu(inline_svg::number(style.get(name)).unwrap_or(0.0) * scale) + 2.0 * border);
    CentredBox::new(inputs.container, layout, scale)
}

/// The `s` of the point's `translate(-50%, -50%) scale(s, s)`.
fn point_scale(svg: &Element) -> Option<f64> {
    let transform = svg.style().get("transform");
    let arguments = transform.split_once("scale(")?.1;
    arguments.split([',', ')']).next()?.trim().parse().ok()
}

/// A point's SVG under its transform at `zoom * scale` px per user unit, its corner at the
/// fractional device offset `fraction`.
fn sprite(svg: &Element, scale: f64, zoom: f64, fraction: [f64; 2], text: u32) -> Option<Pixmap> {
    let elements: Vec<InlineElement> = svg
        .children()
        .iter()
        .filter_map(|e| inline_svg::painted(e, svg, STROKE_WIDTH, text))
        .collect();
    let unit = zoom * scale;
    let side = POINT_SIZE * unit;
    let [width, height] = fraction.map(|f| math::ceil(f + side) as u32);
    let mut pixmap = Pixmap::new(width, height)?;
    let ctm = Affine {
        a: unit,
        b: 0.0,
        c: 0.0,
        d: unit,
        e: fraction[0],
        f: fraction[1],
    };
    let clip = [
        fraction[0],
        fraction[1],
        fraction[0] + side,
        fraction[1] + side,
    ]
    .map(|v| v as f32);
    Document::inline(&elements).paint(&ctm, clip, Backend::HOST, &mut pixmap);
    Some(pixmap)
}

fn paint(inputs: &SvgInputs) -> Option<Layer> {
    let zoom = f64::from(inputs.scale);
    let border = f64::from(device_border(1.0, inputs.scale));
    let frame = background_box(inputs);
    let mut layer = Layer::enclosing(frame.edges(), POINT_SIZE * zoom / 2.0 + 1.0)?;
    let (fill, stroke) = surface(inputs.dark);
    layer.fill_rect(frame.edges(), fill);
    layer.border(frame.edges(), border, stroke);

    type SpriteKey = ([u64; 3], Element);
    let mut sprites: HashMap<[u64; 3], Vec<(SpriteKey, Option<Pixmap>)>> = HashMap::new();
    for svg in inputs.tree.children() {
        let scale = point_scale(svg);
        let shift = if scale.is_some() {
            POINT_SIZE / 2.0 * zoom
        } else {
            0.0
        };
        let scale = scale.unwrap_or(1.0);
        let mut corner = [0.0; 2];
        for (axis, name) in ["left", "top"].into_iter().enumerate() {
            let offset = lu(inline_svg::number(svg.style().get(name)).unwrap_or(0.0) * zoom);
            let rounded = math::floor(frame.fraction[axis] + border + offset + 0.5);
            corner[axis] = frame.origin[axis] + rounded - shift * scale;
        }
        let whole = corner.map(math::floor);
        let fraction = [corner[0] - whole[0], corner[1] - whole[1]];
        let bits = [fraction[0], fraction[1], scale].map(f64::to_bits);
        let template = unplaced(svg);
        let candidates = sprites.entry(bits).or_default();
        let key = (bits, template);
        let found = candidates.iter().position(|(k, _)| *k == key);
        let found = found.unwrap_or_else(|| {
            let pixmap = sprite(svg, scale, zoom, fraction, inputs.text);
            candidates.push((key, pixmap));
            candidates.len() - 1
        });
        if let Some(pixmap) = &candidates[found].1 {
            layer.over(pixmap, whole[0] as i32, whole[1] as i32);
        }
    }
    Some(layer)
}

/// The point without its position, which every point of its set shares.
fn unplaced(svg: &Element) -> Element {
    let mut out = svg.clone();
    for name in ["left", "top"] {
        let _ = out.set_style(name, "");
    }
    out
}

impl<H: FrameHost> HubFrame<H> {
    pub(super) fn points_element(
        &mut self,
        rect: [f64; 4],
        appearance: Appearance,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let (index, _) = displayed(self.host().hub(cx), TabType::Points)?;
        let container = overhung(rect);
        let size = [container[2], container[3]].map(|v| client_rect_coord(v as f32, scale));
        let tree = self.host_mut().input(cx, |hub| {
            with_renderer(hub, index, |renderer: &mut PointsRenderer| {
                renderer.container_size = size;
                renderer.background().clone()
            })
        })?;
        let inputs = SvgInputs {
            tree,
            container,
            scale,
            text: appearance.theme().text,
            dark: appearance == Appearance::Dark,
        };
        let background = background_box(&inputs).client_rect(f64::from(scale));
        self.canvas_tabs.points.update(inputs, paint, window, cx);
        let entity = cx.entity().downgrade();
        let layer = canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                let _ = entity.update(cx, |frame, _| frame.canvas_tabs.points.paint(window));
            },
        )
        .absolute()
        .size_full();
        let root = div()
            .absolute()
            .left_0()
            .top_0()
            .size_full()
            .child(probed(
                "div.points-background-container",
                local(container, rect).child(layer),
            ))
            .child(probed("div.points-background", local(background, rect)));
        Some(root.into_any_element())
    }

    pub(super) fn points_settings(
        &mut self,
        controls: [f64; 4],
        appearance: Appearance,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let (_, key) = displayed(self.host().hub(cx), TabType::Points)?;
        let panel = Panel {
            probe: "div.points-settings",
            blocks: [
                (
                    "Dimensions",
                    Block::Halves([Control::Width, Control::Height]),
                ),
                ("Orientation", Block::Select(Control::Orientation)),
                ("Origin", Block::Select(Control::Origin)),
            ],
        };
        Some(self.settings_panel(key, &panel, controls, appearance, scale, window, cx))
    }
}
