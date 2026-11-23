//! `canvas.joysticks-canvas`, which `JoysticksRenderer` paints over its layouts' images, and
//! `table.joystick-selector`.

use std::collections::HashMap;
use std::sync::Arc;

use asrs_canvas::{Canvas2d, Pixmap};
use asrs_hub::hub::HubPage;
use asrs_js::math;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::JsValue;
use asrs_model::tab_controller::{TabCommand, TabRenderer};
use asrs_model::tab_type::TabType;
use asrs_tabs::cx::TabCx;
use asrs_tabs::joysticks_renderer::{JoysticksDom, JoysticksRenderer};
use asrs_tabs::list_cx::SourceListCx;
use asrs_ui::canvas_view::{CanvasMetrics, CanvasState, canvas_view};
use asrs_ui::gpui::{
    AnyElement, App, AppContext, Context, Entity, IntoElement, ParentElement, Styled, Window, div,
    px,
};
use asrs_ui::input::style::{CONTROL_FONT, device_border};
use asrs_ui::parity::probed;
use asrs_ui::text::{self, Text, TextStyle, WhiteSpace, snap_remainder};
use asrs_ui::theme::{Appearance, FONT_FAMILY};

use super::super::field_image::{self, FieldImage};
use super::super::frame::{FrameHost, HubFrame};
use super::settings::Control;
use super::{displayed, local, lu, with_renderer};

/// The hub's `JoysticksRenderer` with its `IMAGES`: a layout's image is decoded before the first
/// render that draws it, and every `load` the renderer queued is delivered before the next one.
pub struct JoysticksTab {
    renderer: JoysticksRenderer,
    images: HashMap<JsString, Option<FieldImage>>,
}

impl Default for JoysticksTab {
    fn default() -> Self {
        JoysticksTab {
            renderer: JoysticksRenderer::new(Default::default()),
            images: HashMap::new(),
        }
    }
}

impl JoysticksTab {
    pub fn renderer(&self) -> &JoysticksRenderer {
        &self.renderer
    }

    /// The image `drawImage` names by its layout's `path`.
    fn image(&self, path: &str) -> Option<Arc<Pixmap>> {
        let (_, image) = self
            .images
            .iter()
            .find(|(p, _)| p.to_string_lossy() == path)?;
        image.as_ref().map(|image| image.pixmap.clone())
    }

    fn load_images(&mut self, paths: &[JsString]) {
        self.images.retain(|path, _| paths.contains(path));
        for path in paths {
            if !self.images.contains_key(path) {
                self.images.insert(path.clone(), field_image::load(path));
            }
        }
        self.renderer.dom().borrow_mut().images = self
            .images
            .iter()
            .filter_map(|(path, image)| image.as_ref().map(|i| (path.clone(), i.size)))
            .collect();
    }
}

impl TabRenderer<HubPage> for JoysticksTab {
    fn save_state(&self, _: &HubPage) -> JsValue {
        JsValue::Null
    }

    fn restore_state(&mut self, _: &mut HubPage, _: &JsValue) -> Result<(), JsError> {
        Ok(())
    }

    fn aspect_ratio(&self, _: &HubPage) -> Option<f64> {
        None
    }

    fn render(&mut self, cx: &mut HubPage, command: &dyn TabCommand) -> Result<(), JsError> {
        let command = command.to_js();
        self.load_images(&JoysticksRenderer::image_paths(cx.tab_assets(), &command)?);
        let renderer = &mut self.renderer;
        while renderer.deliver_image_load().is_some() {}
        renderer.render_command(cx.tab_dark(), cx.tab_assets(), &command)
    }
}

#[derive(Default)]
pub struct JoysticksView {
    canvas: Option<Entity<CanvasState>>,
    images: HashMap<String, Arc<Pixmap>>,
}

impl JoysticksView {
    /// Hands the canvas the images it does not hold yet and drops the ones no layout draws.
    fn sync_images(&mut self, images: HashMap<String, Arc<Pixmap>>, cx: &mut App) {
        let Some(canvas) = &self.canvas else {
            return;
        };
        let shown = &self.images;
        canvas.update(cx, |canvas, _| {
            for id in shown.keys().filter(|id| !images.contains_key(*id)) {
                canvas.remove_image(id);
            }
            for (id, pixmap) in &images {
                if !shown.get(id).is_some_and(|p| Arc::ptr_eq(p, pixmap)) {
                    canvas.set_image(id.clone(), pixmap.clone());
                }
            }
        });
        self.images = images;
    }
}

/// `border-spacing` and the cells' padding in the UA stylesheet.
const SPACING: f64 = 2.0;
const PADDING: f64 = 1.0;
const ROWS: usize = 2;
const COLUMNS: usize = 3;
/// `table.joystick-selector select { width: calc(100% - 30px) }`.
const SELECT_INSET: f64 = 30.0;

fn label_style() -> TextStyle {
    let font = format!("bold 16px {}", FONT_FAMILY.join(", "));
    let mut style =
        TextStyle::from_css([("font", font.as_str())]).expect("joystick selector label style");
    style.white_space = WhiteSpace::Pre;
    style
}

/// One cell of the selector, in client coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Cell {
    rect: [f64; 4],
    label: [f64; 2],
    select: [f64; 4],
}

/// What the selector's layout reads of its fonts and options, in CSS px.
#[derive(Clone, Copy, Debug)]
struct SelectorMetrics {
    /// Each label's width with the space that separates it from its select.
    labels: [f64; ROWS * COLUMNS],
    /// The widest option's intrinsic select width.
    select_width: f64,
    select_height: f64,
    /// Above and below the line's baseline.
    label_extent: [f64; 2],
    select_extent: [f64; 2],
}

/// `table.joystick-selector` over `rect`: automatic table layout, whose columns share the width
/// in proportion to their max-content widths, and equal rows.
fn selector_layout(rect: [f64; 4], m: &SelectorMetrics) -> [Cell; ROWS * COLUMNS] {
    let [x, y, width, height] = rect;
    let cells_width = width - SPACING * (COLUMNS as f64 + 1.0);
    let row_height = lu((height - SPACING * (ROWS as f64 + 1.0)) / ROWS as f64);
    let max: [f64; COLUMNS] = std::array::from_fn(|column| {
        let label = math::max2(m.labels[column], m.labels[COLUMNS + column]);
        label + m.select_width + 2.0 * PADDING
    });
    let total: f64 = max.iter().sum();
    let mut widths = max.map(|w| lu(cells_width * w / total));
    widths[COLUMNS - 1] = cells_width - widths[..COLUMNS - 1].iter().sum::<f64>();
    let above = math::max2(m.label_extent[0], m.select_extent[0]);
    let below = math::max2(m.label_extent[1], m.select_extent[1]);
    std::array::from_fn(|i| {
        let (row, column) = (i / COLUMNS, i % COLUMNS);
        let before: f64 = widths[..column].iter().sum();
        let cell_x = x + SPACING * (column as f64 + 1.0) + before;
        let cell_y = y + SPACING * (row as f64 + 1.0) + row_height * row as f64;
        let content = widths[column] - 2.0 * PADDING;
        let select_width = math::max2(content - SELECT_INSET, 0.0);
        let label_x =
            cell_x + PADDING + lu(math::max2(content - m.labels[i] - select_width, 0.0) / 2.0);
        let top = cell_y + row_height / 2.0 - (above + below) / 2.0;
        Cell {
            rect: [cell_x, cell_y, widths[column], row_height],
            label: [label_x, top + above - m.label_extent[0]],
            select: [
                label_x + m.labels[i],
                top + above - m.select_extent[0],
                select_width,
                m.select_height,
            ],
        }
    })
}

impl<H: FrameHost> HubFrame<H> {
    /// The displayed joysticks canvas.
    pub fn joysticks_canvas(&self, cx: &App) -> Option<Entity<CanvasState>> {
        let (_, key) = displayed(self.host().hub(cx), TabType::Joysticks)?;
        self.canvas_tabs.joysticks.get(&key)?.canvas.clone()
    }

    pub(super) fn joysticks_element(&mut self, cx: &mut Context<'_, Self>) -> Option<AnyElement> {
        let (_, key) = displayed(self.host().hub(cx), TabType::Joysticks)?;
        let view = self.canvas_tabs.joysticks.entry(key).or_default();
        let canvas = view
            .canvas
            .get_or_insert_with(|| cx.new(CanvasState::new))
            .clone();
        let entity = cx.entity().downgrade();
        let painter = move |metrics: &CanvasMetrics, cx: &mut App| {
            entity
                .update(cx, |frame, cx| frame.paint_joysticks(metrics, cx))
                .ok()
                .flatten()
        };
        Some(
            div()
                .absolute()
                .left_0()
                .top_0()
                .size_full()
                .child(probed(
                    "canvas.joysticks-canvas",
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .size_full()
                        .child(canvas_view(canvas, painter).size_full()),
                ))
                .into_any_element(),
        )
    }

    fn paint_joysticks(&mut self, metrics: &CanvasMetrics, cx: &mut App) -> Option<Canvas2d> {
        let (index, key) = displayed(self.host().hub(cx), TabType::Joysticks)?;
        let (frame, images) = self.host_mut().input(cx, |hub| {
            with_renderer(hub, index, |tab: &mut JoysticksTab| {
                let frame = {
                    let mut dom = tab.renderer().dom().borrow_mut();
                    let JoysticksDom {
                        client_size,
                        device_pixel_ratio,
                        painted,
                        ..
                    } = &mut *dom;
                    *client_size = [metrics.client_width, metrics.client_height].map(f64::from);
                    *device_pixel_ratio = metrics.device_pixel_ratio;
                    painted.take()?
                };
                let images: HashMap<String, Arc<Pixmap>> = tab
                    .images
                    .keys()
                    .map(|path| path.to_string_lossy())
                    .filter_map(|id| tab.image(&id).map(|pixmap| (id, pixmap)))
                    .collect();
                Some((frame, images))
            })
            .flatten()
        })?;
        let view = self.canvas_tabs.joysticks.get_mut(&key)?;
        view.sync_images(images, cx);
        Some(frame)
    }

    pub(super) fn joystick_selector(
        &mut self,
        controls: [f64; 4],
        appearance: Appearance,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let (index, key) = displayed(self.host().hub(cx), TabType::Joysticks)?;
        let style = label_style();
        let label = |i: usize| format!("{i}: ");
        let mut labels = [0.0; ROWS * COLUMNS];
        for (i, width) in labels.iter_mut().enumerate() {
            *width = text::measure(cx, &style, &label(i), scale).0;
        }
        let mut options = TextStyle::new(CONTROL_FONT);
        options.white_space = WhiteSpace::Pre;
        let option_labels: Vec<String> = self.host_mut().input(cx, |hub| {
            Control::Joystick(0)
                .select(hub, index, |select| {
                    (0..select.items().len())
                        .map(|i| select.item_text(i).to_string_lossy())
                        .collect()
                })
                .unwrap_or_default()
        });
        let widest = option_labels
            .iter()
            .map(|l| text::measure(cx, &options, l, scale).0)
            .fold(0.0, math::max2);
        let zoom = f64::from(scale);
        let (label_ascent, label_descent) = text::font_metrics(cx, &style, scale);
        let (ascent, descent) = text::font_metrics(cx, &options, scale);
        let chrome = device_border(1.0, scale) + scale.trunc();
        let device = |v: f32| f64::from(v) / zoom;
        let metrics = SelectorMetrics {
            labels,
            select_width: math::ceil(widest * zoom) / zoom + SELECT_CHROME,
            select_height: device(ascent + descent + 2.0 * chrome),
            label_extent: [device(label_ascent), device(label_descent)],
            select_extent: [device(ascent + chrome), device(descent + chrome)],
        };
        let cells = selector_layout(controls, &metrics);
        let mut table = div().absolute().left_0().top_0().size_full();
        for (i, cell) in cells.into_iter().enumerate() {
            let [x, y] = cell.label;
            table = table
                .child(probed(
                    "table.joystick-selector td",
                    local(cell.rect, controls),
                ))
                .child(
                    div()
                        .absolute()
                        .left(px((x - controls[0]) as f32))
                        .top(px((y - controls[1]) as f32))
                        .child(
                            Text::new(label(i), style.clone())
                                .color(appearance.theme().text)
                                .subpixel(
                                    snap_remainder(x as f32, scale),
                                    snap_remainder(y as f32, scale),
                                ),
                        ),
                );
            let control = Control::Joystick(i);
            if let Some(select) = self.select_control(key, control, cell.select, scale, window, cx)
            {
                table = table.child(probed(
                    "table.joystick-selector select",
                    local(cell.select, controls).child(select),
                ));
            }
        }
        Some(probed("table.joystick-selector", table).into_any_element())
    }
}

/// `PopupInternalPaddingStart`, `PopupInternalPaddingEnd` with the arrow box, and the 1 px
/// borders around a menu list's widest option.
const SELECT_CHROME: f64 = 4.0 + 1.0 + 15.0 + 2.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_boxes_match_blink() {
        let metrics = SelectorMetrics {
            labels: [19.703125, 16.9375, 19.046875, 19.453125, 19.75, 19.375],
            select_width: 126.0,
            select_height: 19.0,
            label_extent: [15.0, 4.0],
            select_extent: [14.0, 5.0],
        };
        let cells = selector_layout([300.0, 565.0, 800.0, 85.0], &metrics);
        let want = [
            (
                [302.0, 567.0, 264.171875, 39.5],
                [308.140625, 576.75],
                [327.84375, 577.75, 232.171875, 19.0],
            ),
            (
                [568.171875, 567.0, 264.25, 39.5],
                [575.703125, 576.75],
                [592.640625, 577.75, 232.25, 19.0],
            ),
            (
                [834.421875, 567.0, 263.578125, 39.5],
                [840.890625, 576.75],
                [859.9375, 577.75, 231.578125, 19.0],
            ),
            (
                [302.0, 608.5, 264.171875, 39.5],
                [308.265625, 618.25],
                [327.71875, 619.25, 232.171875, 19.0],
            ),
            (
                [568.171875, 608.5, 264.25, 39.5],
                [574.296875, 618.25],
                [594.046875, 619.25, 232.25, 19.0],
            ),
            (
                [834.421875, 608.5, 263.578125, 39.5],
                [840.734375, 618.25],
                [860.109375, 619.25, 231.578125, 19.0],
            ),
        ];
        for (i, (cell, (rect, label, select))) in cells.iter().zip(want).enumerate() {
            let close = |a: &[f64], b: &[f64]| a.iter().zip(b).all(|(a, b)| (a - b).abs() <= 0.05);
            assert!(close(&cell.rect, &rect), "{i}: {cell:?}");
            assert!(close(&cell.label, &label), "{i}: {cell:?}");
            assert!(close(&cell.select, &select), "{i}: {cell:?}");
        }
    }
}
