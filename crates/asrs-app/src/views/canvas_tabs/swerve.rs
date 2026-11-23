//! `div.swerve-canvas-container`: the canvas `SwerveRenderer` paints, with its border and
//! background, and `div.swerve-settings`.

use std::collections::HashMap;
use std::sync::Arc;

use asrs_canvas::pixels::transform_bilinear;
use asrs_canvas::{Canvas2d, Pixmap, RasterState, ShadeCache};
use asrs_model::tab_type::TabType;
use asrs_tabs::swerve_renderer::SwerveRenderer;
use asrs_ui::canvas_view::geometry::client_size;
use asrs_ui::canvas_view::{FramePresenter, rasterize};
use asrs_ui::gpui::{AnyElement, Context, IntoElement, ParentElement, Styled, Window, canvas, div};
use asrs_ui::input::style::device_border;
use asrs_ui::parity::probed;
use asrs_ui::theme::Appearance;

use super::super::frame::{FrameHost, HubFrame};
use super::boxes::{CentredBox, Layer};
use super::points::surface;
use super::settings::{Block, Control, Panel};
use super::{displayed, local, lu, overhung, with_renderer};

#[derive(Default)]
pub struct SwerveView {
    raster: RasterState,
    shades: ShadeCache,
    frame: Option<Canvas2d>,
    bitmap: Option<Arc<Pixmap>>,
    presenter: FramePresenter,
    presented: Option<([u64; 5], bool)>,
    origin: [i32; 2],
}

impl SwerveView {
    pub fn take_presenter(&mut self) -> FramePresenter {
        std::mem::take(&mut self.presenter)
    }
}

/// The canvas element: its border box's background and border with the bitmap inside them.
fn composite(
    frame: &CentredBox,
    border: f64,
    bitmap: Option<&Pixmap>,
    dark: bool,
) -> Option<Layer> {
    let mut layer = Layer::enclosing(frame.edges(), 0.0)?;
    let (fill, stroke) = surface(dark);
    layer.fill_rect(frame.edges(), fill);
    layer.border(frame.edges(), border, stroke);
    if let Some(bitmap) = bitmap {
        let x = frame.origin[0] + border - f64::from(layer.x);
        let y = frame.origin[1] + border - f64::from(layer.y);
        let inverse = [1.0, 0.0, 0.0, 1.0, -x, -y];
        let (width, height) = (layer.pixmap.width(), layer.pixmap.height());
        if let Some(placed) = transform_bilinear(bitmap, width, height, inverse) {
            layer.over(&placed, layer.x, layer.y);
        }
    }
    Some(layer)
}

impl<H: FrameHost> HubFrame<H> {
    pub(super) fn swerve_element(
        &mut self,
        rect: [f64; 4],
        appearance: Appearance,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let (index, key) = displayed(self.host().hub(cx), TabType::Swerve)?;
        let container = overhung(rect);
        let dpr = f64::from(scale);
        let dark = appearance == Appearance::Dark;
        let client = [container[2], container[3]].map(|v| f64::from(client_size(v as f32, scale)));
        let (painted, size) = self.host_mut().input(cx, |hub| {
            with_renderer(hub, index, |renderer: &mut SwerveRenderer| {
                let mut dom = renderer.dom().borrow_mut();
                dom.client_size = client;
                dom.device_pixel_ratio = dpr;
                (dom.painted.take(), dom.canvas_size)
            })
        })?;
        let view = self.canvas_tabs.swerve.entry(key).or_default();
        if let Some(frame) = painted
            && view.frame.as_ref() != Some(&frame)
        {
            let (raster, shades) = (&mut view.raster, &mut view.shades);
            view.bitmap = rasterize(&frame, &HashMap::new(), raster, shades, cx)
                .map(|(pixmap, _)| Arc::new(pixmap));
            view.frame = Some(frame);
            view.presented = None;
        }
        let border = f64::from(device_border(1.0, scale));
        let side = lu(size? * dpr) + 2.0 * border;
        let frame = CentredBox::new(container, [side, side], dpr);
        let edges = frame.edges().map(f64::to_bits);
        let wanted = (
            [edges[0], edges[1], edges[2], edges[3], dpr.to_bits()],
            dark,
        );
        if view.presented != Some(wanted) {
            view.presented = Some(wanted);
            match composite(&frame, border, view.bitmap.as_deref(), dark) {
                Some(layer) => {
                    view.origin = [layer.x, layer.y];
                    view.presenter.present(layer.pixmap, window, cx);
                }
                None => view.presenter.clear(window, cx),
            }
        }

        let client_rect = frame.client_rect(dpr);
        let dump = view.bitmap.clone();
        let entity = cx.entity().downgrade();
        let layer = canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                let _ = entity.update(cx, |frame, _| {
                    if let Some(view) = frame.canvas_tabs.swerve.get(&key) {
                        let origin = view.origin.map(|v| v as f32 / scale);
                        view.presenter.paint(origin, window);
                    }
                });
                #[cfg(feature = "parity")]
                if let Some(bitmap) = &dump {
                    asrs_ui::parity::record_canvas(cx, || asrs_ui::parity::CanvasDump {
                        rect: client_rect,
                        width: bitmap.width(),
                        height: bitmap.height(),
                        rgba: asrs_canvas::pixels::to_straight_rgba(bitmap),
                    });
                }
                #[cfg(not(feature = "parity"))]
                let _ = &dump;
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
                "div.swerve-canvas-container",
                local(container, rect).child(layer),
            ))
            .child(probed("canvas.swerve-canvas", local(client_rect, rect)));
        Some(root.into_any_element())
    }

    pub(super) fn swerve_settings(
        &mut self,
        controls: [f64; 4],
        appearance: Appearance,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let (_, key) = displayed(self.host().hub(cx), TabType::Swerve)?;
        let panel = Panel {
            probe: "div.swerve-settings",
            blocks: [
                ("Max Speed (m/s)", Block::Number(Control::MaxSpeed)),
                (
                    "Frame Size (m)",
                    Block::Halves([Control::SizeX, Control::SizeY]),
                ),
                ("Orientation", Block::Rotate),
            ],
        };
        Some(self.settings_panel(key, &panel, controls, appearance, scale, window, cx))
    }
}
