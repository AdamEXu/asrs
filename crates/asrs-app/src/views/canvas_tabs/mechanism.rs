//! `div.mechanism-svg-container`: the `svg.mechanism-svg` `MechanismRenderer` fills.

use asrs_canvas::path::Affine;
use asrs_model::tab_type::TabType;
use asrs_tabs::mechanism_renderer::MechanismRenderer;
use asrs_ui::canvas_view::geometry::client_size;
use asrs_ui::gpui::{AnyElement, Context, IntoElement, ParentElement, Styled, Window, canvas, div};
use asrs_ui::parity::probed;
use asrs_ui::svg::{Backend, Document, InlineElement};
use asrs_ui::theme::Appearance;

use super::super::frame::{FrameHost, HubFrame};
use super::boxes::{CentredBox, Layer};
use super::inline_svg::{self, SvgInputs};
use super::{displayed, local, lu, overhung, with_renderer};

/// The `width` and `height` of an `<svg>` whose attributes give no valid length.
const DEFAULT_SIZE: [f64; 2] = [300.0, 150.0];

/// The `<svg>`'s box in device px, or `None` while it is not displayed.
fn svg_box(inputs: &SvgInputs) -> Option<CentredBox> {
    let svg = &inputs.tree;
    if svg.style().get("display") == "none" {
        return None;
    }
    let scale = f64::from(inputs.scale);
    let size = |name: &str, default: f64| {
        let value = svg.attribute(name).and_then(inline_svg::number);
        lu(value.filter(|v| *v >= 0.0).unwrap_or(default) * scale)
    };
    let layout = [
        size("width", DEFAULT_SIZE[0]),
        size("height", DEFAULT_SIZE[1]),
    ];
    Some(CentredBox::new(inputs.container, layout, scale))
}

fn paint(inputs: &SvgInputs) -> Option<Layer> {
    let frame = svg_box(inputs)?;
    let mut layer = Layer::enclosing(frame.edges(), 0.0)?;
    if let Some(color) = inline_svg::background_color(&inputs.tree, inputs.text) {
        layer.fill_rect(frame.edges(), color);
    }
    let elements: Vec<InlineElement> = inputs
        .tree
        .children()
        .iter()
        .filter_map(|e| inline_svg::painted(e, &inputs.tree, 1.0, inputs.text))
        .collect();
    let zoom = f64::from(inputs.scale);
    let fit = |axis: usize| zoom * frame.size[axis] / frame.layout[axis];
    let ctm = Affine {
        a: fit(0),
        b: 0.0,
        c: 0.0,
        d: fit(1),
        e: frame.origin[0] - f64::from(layer.x),
        f: frame.origin[1] - f64::from(layer.y),
    };
    let clip = layer.local(frame.edges());
    Document::inline(&elements).paint(&ctm, clip, Backend::HOST, &mut layer.pixmap);
    Some(layer)
}

impl<H: FrameHost> HubFrame<H> {
    pub(super) fn mechanism_element(
        &mut self,
        rect: [f64; 4],
        appearance: Appearance,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let (index, _) = displayed(self.host().hub(cx), TabType::Mechanism)?;
        let container = overhung(rect);
        let client = [container[2], container[3]].map(|v| f64::from(client_size(v as f32, scale)));
        let tree = self.host_mut().input(cx, |hub| {
            with_renderer(hub, index, |renderer: &mut MechanismRenderer| {
                renderer.client_size = client;
                renderer.svg().clone()
            })
        })?;
        let inputs = SvgInputs {
            tree,
            container,
            scale,
            text: appearance.theme().text,
            dark: appearance == Appearance::Dark,
        };
        let shown = svg_box(&inputs);
        self.canvas_tabs.mechanism.update(inputs, paint, window, cx);
        let entity = cx.entity().downgrade();
        let layer = canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                let _ = entity.update(cx, |frame, _| frame.canvas_tabs.mechanism.paint(window));
            },
        )
        .absolute()
        .size_full();
        let mut root = div().absolute().left_0().top_0().size_full().child(probed(
            "div.mechanism-svg-container",
            local(container, rect).child(layer),
        ));
        if let Some(frame) = shown {
            root = root.child(probed(
                "svg.mechanism-svg",
                local(frame.client_rect(f64::from(scale)), rect),
            ));
        }
        Some(root.into_any_element())
    }
}
