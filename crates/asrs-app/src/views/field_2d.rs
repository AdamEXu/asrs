//! The 2D field tab: its renderer element (`#renderer2` in `www/hub.html`, styled by
//! `www/renderers.css`), which shows the canvas `asrs_tabs::field_2d::Field2dRenderer` paints
//! with the field image and heatmap and carries the scroll overlay its pointer, wheel and camera
//! menu listeners hang on; and the controller's `div.field-2d-settings` (`www/hub.css`): the
//! field select, the source link and the orientation and size switchers. Its source list is the
//! frame's.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use asrs_canvas::pixels::transform_bilinear;
use asrs_canvas::{Canvas2d, Pixmap, Raster, RasterState, ShadeCache};
use asrs_hub::hub::{HubModel, HubPage};
use asrs_hub::tabs::TabRef;
use asrs_js::math;
use asrs_js::string::JsString;
use asrs_model::layout_unit::{border, centred, device, percent, zoomed};
use asrs_model::scroll_sensor::ScrollSensor;
use asrs_model::tab_type::TabType;
use asrs_tabs::field_2d::{
    CanvasStyle, Field2dController, Field2dControls, Field2dRenderer, HEATMAP_IMAGE,
};
use asrs_tabs::form::{self, OptGroup, SelectOption};
use asrs_ui::canvas_view::FramePresenter;
use asrs_ui::canvas_view::geometry::{client_size, mouse_client};
use asrs_ui::focus::{self, Activate, FocusRing, LINK_OUTLINE_OFFSET};
use asrs_ui::gpui::{
    AnyElement, App, AppContext, ClickEvent, Context, Entity, InteractiveElement, IntoElement,
    KeyboardButton, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement, ScrollEvent, Styled,
    Subscription, Window, canvas, div, px,
};
use asrs_ui::icon::IconSource;
use asrs_ui::input::style::CONTROL_FONT;
use asrs_ui::input::{Segment, SegmentedControl, Select, SelectEvent, select_geometry};
use asrs_ui::parity::probed;
use asrs_ui::scroll;
use asrs_ui::symbols::Symbol;
use asrs_ui::text::{self, Text, TextStyle, WhiteSpace, snap_remainder};
use asrs_ui::theme::{Appearance, FOCUS_RING_RADIUS, FOCUS_RING_WIDTH, FONT_FAMILY};
use asrs_ui::tooltip::{titled, untitled};

use super::field_image::{self, FieldImage};
use super::frame::{FrameHost, HubFrame, overlay_range, wheel_of};
use super::scrollers::Area;
use super::select::FormSelect;
use super::settings_panel::{SettingsPanel, tab_key};
use super::source_list::{centred_offset, place_in};

/// `div.field-2d-canvas-container` and `div.field-2d-scroll` are `height: calc(100% + 26px)`
/// from the bottom of the renderer element.
const OVERHANG: f64 = 26.0;
const SETTINGS_MAX_WIDTH: f64 = 250.0;
const SETTINGS_TOP: f64 = 5.0;
const TITLE_HEIGHT: f64 = 20.0;
const FIELD_ROW_HEIGHT: f64 = 40.0;
const INPUT_MIN_HEIGHT: f64 = 40.0;
const TALL_INSET: f64 = 85.0;
const TALL_MAX_HEIGHT: f64 = 100.0;
const SPLIT_TOP: f64 = 8.0;
const SELECT_HEIGHT: f64 = 24.0;
/// `PopupInternalPaddingStart`, `PopupInternalPaddingEnd` with the arrow box, and the 1 px
/// borders around a menu list's widest option.
const SELECT_CHROME: f64 = 4.0 + 1.0 + 15.0 + 2.0;
const SOURCE_PADDING: f64 = 5.0;
const ORIENTATION_ICONS: [&str; 2] = ["rotateleft.svg", "rotateright.svg"];
const ORIENTATION_TITLES: &[&str] = &["Rotate field counter-clockwise", "Rotate field clockwise"];
const SIZE_TITLES: &[&str] = &["Large robot size", "Medium robot size", "Small robot size"];

/// The settings' rows in document order, as [`focus::section`] has them.
#[derive(Clone, Copy)]
enum Block {
    FieldTitle = 1,
    Field,
    SwitcherTitles,
    Switchers,
}
const SIZE_LABELS: [[&str; 3]; 2] = [["30", "27", "24"], ["18", "16", "14"]];

fn title_style() -> TextStyle {
    let font = format!("bold 16px {}", FONT_FAMILY.join(", "));
    TextStyle::from_css([
        ("font", font.as_str()),
        ("line-height", "20px"),
        ("white-space", "nowrap"),
    ])
    .expect("field 2d title style")
}

/// `ToPixelSnappedRect`: each edge on its own device pixel boundary.
fn pixel_snapped([x, y, w, h]: [f64; 4], scale: f64) -> [f64; 4] {
    let snap = |v: f64| (v * scale).round() / scale;
    let (left, top) = (snap(x), snap(y));
    [left, top, snap(x + w) - left, snap(y + h) - top]
}

fn source_style() -> TextStyle {
    let font = format!("14px {}", FONT_FAMILY.join(", "));
    TextStyle::from_css([("font", font.as_str()), ("white-space", "nowrap")])
        .expect("field 2d source style")
}

/// Where `div.field-2d-settings` puts its parts inside `div.controls-content`, in client
/// coordinates of its unscrolled content. `select_width` is the field select's intrinsic width.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SettingsLayout {
    /// The border box, `border` of it on the left the `border-left`.
    panel: [f64; 4],
    border: f64,
    content_height: f64,
    titles: [[f64; 4]; 3],
    select: [f64; 4],
    source: Option<[f64; 2]>,
    orientation: [f64; 4],
    size: [f64; 4],
}

impl SettingsLayout {
    /// `bar` is what a scrollbar takes of the panel where it overflows, in CSS px.
    fn new(
        controls: [f64; 4],
        bar: f64,
        select_width: f64,
        source_width: Option<f64>,
        scale: f64,
    ) -> SettingsLayout {
        let [x, y, w, h] = controls.map(|v| device(v, scale));
        let length = |v: f64| zoomed(v, scale);
        let css = |rect: [f64; 4]| rect.map(|v| v / scale);
        let edge = border(1.0, scale);
        let title_height = length(TITLE_HEIGHT);
        let row_height = length(FIELD_ROW_HEIGHT);
        let width = math::min2(percent(w, 40.0), length(SETTINGS_MAX_WIDTH));
        let panel = [x + w - width - edge, y, width + edge, h];
        let tall = math::max2(
            math::min2(
                math::max2(h - length(TALL_INSET), 0.0),
                length(TALL_MAX_HEIGHT),
            ),
            length(INPUT_MIN_HEIGHT),
        );
        let content_height = length(SETTINGS_TOP) + 2.0 * title_height + row_height + tall;
        let content_width = if content_height > h {
            width - length(bar)
        } else {
            width
        };
        let content_x = panel[0] + edge;
        let half = percent(content_width, 50.0);
        let title_top = y + length(SETTINGS_TOP);
        let field_top = title_top + title_height;
        let second_title = field_top + row_height;
        let tall_top = second_title + title_height;
        let select_width = device(select_width, scale);
        let source = source_width.map(|w| device(w, scale) + 2.0 * length(SOURCE_PADDING));
        let line_width = select_width + source.unwrap_or(0.0);
        let select_x = content_x + centred(content_width, line_width);
        let select_height = length(SELECT_HEIGHT);
        let select_y = field_top + percent(row_height, 50.0) - select_height / 2.0;
        let split_width = percent(half, 80.0);
        let split_height = percent(tall, 65.0);
        let split = |left: f64| {
            css([
                left + percent(half, 50.0) - split_width / 2.0,
                tall_top + length(SPLIT_TOP),
                split_width,
                split_height,
            ])
        };
        SettingsLayout {
            panel: css(panel),
            border: edge / scale,
            content_height: content_height / scale,
            titles: [
                css([content_x, title_top, content_width, title_height]),
                css([content_x, second_title, half, title_height]),
                css([content_x + half, second_title, half, title_height]),
            ],
            select: css([select_x, select_y, select_width, select_height]),
            source: source.map(|_| [(select_x + select_width) / scale, select_y / scale]),
            orientation: split(content_x),
            size: split(content_x + half),
        }
    }
}

/// The field select's options: an FRC optgroup and an FTC optgroup, each option's text set
/// through `innerText` and its value the field's id, then `value` assigned.
fn field_select(controls: &Field2dControls) -> form::Select {
    let group = |options: &[(JsString, JsString)]| -> Vec<form::SelectItem> {
        options
            .iter()
            .map(|(value, label)| form::SelectItem::Option {
                option: SelectOption::from_inner_text(label).with_value(value),
                grouped: true,
            })
            .collect()
    };
    let mut items = vec![form::SelectItem::Group(OptGroup::new("FRC Fields"))];
    items.extend(group(&controls.frc));
    items.push(form::SelectItem::Group(OptGroup::new("FTC Fields")));
    items.extend(group(&controls.ftc));
    let mut select = form::Select::with_items(items);
    select.set_value(&controls.field);
    select
}

/// The canvas's `getBoundingClientRect()` for `style` centred in `container`.
fn canvas_rect(container: [f64; 4], style: &CanvasStyle) -> [f64; 4] {
    let (sin, cos) = quarter_sin_cos(style.rotation);
    let (sin, cos) = (sin.abs(), cos.abs());
    let hx = (cos * style.width + sin * style.height) / 2.0;
    let hy = (sin * style.width + cos * style.height) / 2.0;
    let cx = container[0] + container[2] / 2.0;
    let cy = container[1] + container[3] / 2.0;
    [cx - hx, cy - hy, 2.0 * hx, 2.0 * hy]
}

/// `sin` and `cos` of `angle`, exact for multiples of a quarter turn.
fn quarter_sin_cos(angle: f64) -> (f64, f64) {
    let quarters = angle / (std::f64::consts::PI / 2.0);
    if quarters == quarters.round() {
        match (quarters.round() as i64).rem_euclid(4) {
            0 => (0.0, 1.0),
            1 => (1.0, 0.0),
            2 => (0.0, -1.0),
            _ => (-1.0, 0.0),
        }
    } else {
        angle.sin_cos()
    }
}

/// What the compositor shows of the canvas bitmap inside the container (`overflow: hidden`): the
/// bitmap stretched over its CSS box, centred and rotated by `translate(-50%, -50%)
/// rotate(...)`, resampled onto the container's device pixels.
fn composite(
    bitmap: &Pixmap,
    style: &CanvasStyle,
    container: [f64; 4],
    scale: f64,
) -> Option<Pixmap> {
    let x0 = math::round(container[0] * scale);
    let y0 = math::round(container[1] * scale);
    let width = math::round((container[0] + container[2]) * scale) - x0;
    let height = math::round((container[1] + container[3]) * scale) - y0;
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let cx = (container[0] + container[2] / 2.0) * scale - x0;
    let cy = (container[1] + container[3] / 2.0) * scale - y0;
    let (bw, bh) = (f64::from(bitmap.width()), f64::from(bitmap.height()));
    let [w, h] = [style.width * scale, style.height * scale];
    let kx = math::round(w) / bw;
    let ky = math::round(h) / bh;
    let (sin, cos) = quarter_sin_cos(style.rotation);
    let inverse = [
        cos / kx,
        -sin / ky,
        sin / kx,
        cos / ky,
        (w / 2.0 - (cos * cx + sin * cy)) / kx,
        (h / 2.0 - (-sin * cx + cos * cy)) / ky,
    ];
    transform_bilinear(bitmap, width as u32, height as u32, inverse)
}

struct SelectView {
    select: Entity<Select<FormSelect>>,
    synced: Option<form::Select>,
    _subscription: Subscription,
}

#[derive(Default)]
struct TabView {
    sensor: ScrollSensor,
    raster: RasterState,
    shades: ShadeCache,
    bitmap: Option<Arc<Pixmap>>,
    style: Option<CanvasStyle>,
    presenter: FramePresenter,
    presented: Option<(CanvasStyle, [u64; 4], u64)>,
    select: Option<SelectView>,
}

/// The displayed field's boxes in client coordinates: the canvas container, which is also
/// the scroll overlay's box.
#[derive(Clone, Copy, Debug)]
struct Shown {
    key: usize,
    index: usize,
    container: [f64; 4],
    device_pixel_ratio: f64,
}

impl Shown {
    fn overlay_client_size(&self) -> [f64; 2] {
        let dpr = self.device_pixel_ratio as f32;
        [
            f64::from(client_size(self.container[2] as f32, dpr)),
            f64::from(client_size(self.container[3] as f32, dpr)),
        ]
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        let [ox, oy, w, h] = self.container;
        w > 0.0 && h > 0.0 && x >= ox && x < ox + w && y >= oy && y < oy + h
    }
}

#[derive(Default)]
pub struct Field2dViews {
    tabs: HashMap<usize, TabView>,
    images: HashMap<JsString, Option<Rc<FieldImage>>>,
    shown: Option<Shown>,
    /// The timeline's box, which sits above the overlay.
    covered: Option<[f64; 4]>,
    hovered: bool,
    pointer: Option<[f64; 2]>,
    pressed: bool,
}

impl Field2dViews {
    fn over_overlay(&self, x: f64, y: f64) -> bool {
        let covered = self
            .covered
            .is_some_and(|[cx, cy, w, h]| x >= cx && x < cx + w && y >= cy && y < cy + h);
        !covered && self.shown.is_some_and(|s| s.contains(x, y))
    }
}

fn renderer_key(renderer: &Field2dRenderer) -> usize {
    Rc::as_ptr(renderer.dom()) as *const () as usize
}

/// The displayed tab when it is a 2D field, with its index and its renderer.
fn displayed(hub: &HubModel) -> Option<(usize, &Field2dRenderer, TabRef<'_, HubPage>)> {
    hub.tabs().tabs().enumerate().find_map(|(i, tab)| {
        if tab.content_hidden() || tab.tab_type() != Some(TabType::Field2d) {
            return None;
        }
        let renderer = tab.renderer().downcast_ref::<Field2dRenderer>()?;
        Some((i, renderer, tab))
    })
}

/// The image path of the field a tab's select names, or in a satellite, which has none, of the
/// one its last command names.
fn field_path(hub: &HubModel, tab: &TabRef<'_, HubPage>) -> Option<JsString> {
    let renderer = tab.renderer().downcast_ref::<Field2dRenderer>()?;
    let field = match tab.controller().downcast_ref::<Field2dController>() {
        Some(controller) => &controller.controls.field,
        None => renderer.field()?,
    };
    hub.page()
        .assets()?
        .field2ds
        .iter()
        .find(|f| f.id == *field)
        .map(|f| f.path.clone())
}

fn with_renderer<R>(
    hub: &mut HubModel,
    index: usize,
    f: impl FnOnce(&mut Field2dRenderer, &mut HubPage) -> R,
) -> Option<R> {
    let (tabs, page) = hub.tabs_mut();
    let renderer = tabs.tab_mut(index)?.renderer;
    Some(f(renderer.downcast_mut::<Field2dRenderer>()?, page))
}

fn with_tab<R>(
    hub: &mut HubModel,
    index: usize,
    f: impl FnOnce(&mut Field2dRenderer, &mut Field2dController, &mut HubPage) -> R,
) -> Option<R> {
    let (tabs, page) = hub.tabs_mut();
    let tab = tabs.tab_mut(index)?;
    let renderer = tab.renderer.downcast_mut::<Field2dRenderer>()?;
    let controller = tab.controller.downcast_mut::<Field2dController>()?;
    Some(f(renderer, controller, page))
}

fn mouse_button(button: MouseButton) -> f64 {
    match button {
        MouseButton::Left => 0.0,
        MouseButton::Middle => 1.0,
        MouseButton::Right => 2.0,
        MouseButton::Navigate(_) => 3.0,
    }
}

/// A frame `render` painted, with the canvas style it set and the heatmap canvas it drew from.
struct Painted {
    frame: Canvas2d,
    style: Option<CanvasStyle>,
    heatmap: Option<Arc<Pixmap>>,
}

/// What a settings control does to its controller.
#[derive(Clone, Copy, Debug, PartialEq)]
enum SettingsInput {
    Source,
    Orientation(usize),
    Size(usize),
}

fn tab_index(hub: &HubModel, key: usize) -> Option<usize> {
    hub.tabs().tabs().position(|t| {
        t.renderer()
            .downcast_ref::<Field2dRenderer>()
            .is_some_and(|r| renderer_key(r) == key)
    })
}

/// The painted frame drawn through the field image and the heatmap canvas.
fn rasterize(
    frame: &Canvas2d,
    images: &[(String, Arc<Pixmap>)],
    state: &mut RasterState,
    shades: &mut ShadeCache,
) -> Option<Pixmap> {
    let Some(mut raster) = Raster::new(frame.width, frame.height) else {
        *state = RasterState::default();
        *shades = ShadeCache::default();
        return None;
    };
    raster = raster
        .with_state(*state)
        .with_shades(std::mem::take(shades));
    for (id, pixmap) in images {
        if id == HEATMAP_IMAGE {
            raster.add_image(id, pixmap.clone());
        } else {
            raster.add_image_element(id, pixmap.clone());
        }
    }
    raster.run(&frame.calls);
    *state = raster.state();
    *shades = raster.take_shades();
    Some(raster.into_pixmap())
}

impl<H: FrameHost> HubFrame<H> {
    /// The displayed field's renderer element over `rect` (`div.renderer-content`), if the
    /// displayed tab is a 2D field. `covered` is the timeline's box.
    pub(super) fn field_2d_element(
        &mut self,
        rect: [f64; 4],
        covered: Option<[f64; 4]>,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let hub = self.host().hub(cx);
        let mut live = HashSet::new();
        let mut paths = HashSet::new();
        for tab in hub.tabs().tabs() {
            if let Some(renderer) = tab.renderer().downcast_ref::<Field2dRenderer>() {
                live.insert(renderer_key(renderer));
            }
            paths.extend(field_path(hub, &tab));
        }
        let shown = displayed(hub)
            .map(|(index, renderer, tab)| (index, renderer_key(renderer), field_path(hub, &tab)));
        let satellite = hub.page().is_satellite();
        let overhang = if satellite { 0.0 } else { OVERHANG };
        let now = self.host().frame_time(cx);
        let views = &mut self.field_2d;
        views.covered = covered;
        let mut dropped = Vec::new();
        views.tabs.retain(|key, tab| {
            let keep = live.contains(key);
            if !keep {
                dropped.push(std::mem::take(&mut tab.presenter));
            }
            keep
        });
        for mut presenter in dropped {
            presenter.release(cx);
        }
        views.images.retain(|path, _| paths.contains(path));
        let Some((index, key, path)) = shown else {
            views.shown = None;
            views.hovered = false;
            return None;
        };
        if let Some(path) = &path {
            views
                .images
                .entry(path.clone())
                .or_insert_with(|| field_image::load_shared(path));
        }
        let container = [rect[0], rect[1] - overhang, rect[2], rect[3] + overhang];
        let shown = Shown {
            key,
            index,
            container,
            device_pixel_ratio: f64::from(scale),
        };
        if views.shown.is_some_and(|s| s.key != key) {
            views.hovered = false;
        }
        views.shown = Some(shown);
        let left_under_pointer = views.hovered
            && !views.pressed
            && views
                .pointer
                .is_some_and(|[x, y]| !views.over_overlay(x, y));
        if left_under_pointer {
            views.hovered = false;
        }
        let images: Vec<(JsString, [f64; 2])> = views
            .images
            .iter()
            .filter_map(|(path, image)| image.as_ref().map(|i| (path.clone(), i.size)))
            .collect();
        let field_image = path
            .as_ref()
            .and_then(|p| views.images.get(p).cloned().flatten());
        let tab = views.tabs.entry(key).or_default();
        let client = shown.overlay_client_size();
        tab.sensor.set_layout(client, shown.device_pixel_ratio);
        tab.sensor.animate(now);
        let scroll = tab
            .sensor
            .scroll_step(now, client[0] != 0.0 || client[1] != 0.0);
        tab.sensor.periodic(now);
        if tab.sensor.animating() {
            window.request_animation_frame();
        }
        let painted = self
            .host_mut()
            .input(cx, |hub| {
                with_renderer(hub, index, |renderer, page| {
                    {
                        let mut dom = renderer.dom().borrow_mut();
                        dom.container = container;
                        dom.scroll_container = container;
                        dom.device_pixel_ratio = shown.device_pixel_ratio;
                        dom.satellite = satellite;
                        dom.images = images.into_iter().collect();
                    }
                    if left_under_pointer {
                        renderer.mouse(page, "leave", 0.0, false, 0.0, 0.0);
                    }
                    if let Some(d) = scroll {
                        renderer.wheel(&*page, d.dx, d.dy);
                    }
                    renderer.flush_image_loads();
                    renderer.take_frame_requests();
                    let frame = renderer.dom().borrow_mut().painted.take()?;
                    let style = renderer.dom().borrow().canvas_style;
                    Some(Painted {
                        frame,
                        style,
                        heatmap: renderer.heatmap().pixmap().cloned(),
                    })
                })
            })
            .flatten();
        let tab = self.field_2d.tabs.get_mut(&key)?;
        if let Some(painted) = painted {
            let mut images = Vec::new();
            if let (Some(path), Some(image)) = (&path, &field_image) {
                images.push((path.to_string_lossy(), image.pixmap.clone()));
            }
            if let Some(heatmap) = painted.heatmap {
                images.push((HEATMAP_IMAGE.to_owned(), heatmap));
            }
            let (raster, shades) = (&mut tab.raster, &mut tab.shades);
            tab.bitmap = rasterize(&painted.frame, &images, raster, shades).map(Arc::new);
            tab.style = painted.style;
            tab.presented = None;
        }
        let style = tab.style;
        let device = container.map(|v| (v * shown.device_pixel_ratio).to_bits());
        let wanted = style.map(|s| (s, device, shown.device_pixel_ratio.to_bits()));
        if tab.presented != wanted {
            tab.presented = wanted;
            match (&tab.bitmap, style) {
                (Some(bitmap), Some(style)) => {
                    match composite(bitmap, &style, container, shown.device_pixel_ratio) {
                        Some(pixmap) => tab.presenter.present(pixmap, window, cx),
                        None => tab.presenter.clear(window, cx),
                    }
                }
                _ => tab.presenter.clear(window, cx),
            }
        }

        let dump = tab
            .bitmap
            .clone()
            .zip(style)
            .map(|(b, s)| (b, canvas_rect(container, &s)));
        let entity = cx.entity().downgrade();
        let origin = [container[0] as f32, container[1] as f32];
        let layer = canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                let _ = entity.update(cx, |frame, _| {
                    if let Some(tab) = frame.field_2d.tabs.get(&key) {
                        tab.presenter.paint(origin, window);
                    }
                });
                #[cfg(feature = "parity")]
                if let Some((bitmap, rect)) = &dump {
                    asrs_ui::parity::record_canvas(cx, || asrs_ui::parity::CanvasDump {
                        rect: *rect,
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
        let local = |r: [f64; 4]| {
            div()
                .absolute()
                .left(px((r[0] - rect[0]) as f32))
                .top(px((r[1] - rect[1]) as f32))
                .w(px(r[2] as f32))
                .h(px(r[3] as f32))
        };
        let mut root = div()
            .absolute()
            .left_0()
            .top_0()
            .size_full()
            .child(local(container).child(layer));
        if let Some(style) = style {
            root = root.child(probed(
                "canvas.field-2d-canvas",
                local(canvas_rect(container, &style)),
            ));
        }
        root =
            root.child(probed(
                "div.field-2d-scroll",
                local(container)
                    .id("field-2d-scroll")
                    .capture_any_mouse_down(
                        cx.listener(move |frame, _, _, _| frame.press_area(&Area::Field2d(key))),
                    )
                    .on_any_mouse_down(cx.listener(|frame, event: &MouseDownEvent, _, cx| {
                        frame.field_2d_mouse_down(event, cx)
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|frame, event: &MouseUpEvent, _, cx| {
                            frame.field_2d_mouse_up(event, cx)
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Middle,
                        cx.listener(|frame, event: &MouseUpEvent, _, cx| {
                            frame.field_2d_mouse_up(event, cx)
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Right,
                        cx.listener(|frame, event: &MouseUpEvent, _, cx| {
                            frame.field_2d_mouse_up(event, cx)
                        }),
                    )
                    .on_scroll(cx.listener(|frame, event: &ScrollEvent, _, cx| {
                        frame.field_2d_wheel(event, cx)
                    })),
            ));
        Some(root.into_any_element())
    }

    /// `div.field-2d-settings` in `controls` (`div.controls-content`), if the displayed tab is
    /// a 2D field.
    pub(super) fn field_2d_controls(
        &mut self,
        controls: [f64; 4],
        appearance: Appearance,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let hub = self.host().hub(cx);
        let (_, renderer, tab) = displayed(hub)?;
        let panel_key = tab_key(&tab);
        let controller = tab.controller().downcast_ref::<Field2dController>()?;
        let key = renderer_key(renderer);
        let state = controller.controls.clone();
        let source_shown = !state.source_hidden;
        let wanted = field_select(&state);
        let theme = appearance.theme();

        let mut select_style = TextStyle::new(CONTROL_FONT);
        select_style.white_space = WhiteSpace::Pre;
        let options_width = wanted
            .items()
            .iter()
            .enumerate()
            .filter(|(_, item)| matches!(item, form::SelectItem::Option { .. }))
            .map(|(i, _)| {
                let label = wanted.item_text(i).to_string_lossy();
                text::measure(cx, &select_style, &label, scale).0
            })
            .fold(0.0, math::max2);
        let zoom = f64::from(scale);
        let select_width = math::ceil(options_width * zoom) / zoom + SELECT_CHROME;
        let source_width =
            source_shown.then(|| text::measure(cx, &source_style(), "Source", scale).0);
        let layout = SettingsLayout::new(
            controls,
            scroll::style(cx).reserved(),
            select_width,
            source_width,
            f64::from(scale),
        );
        let content = [layout.panel[0] + layout.border, layout.panel[1]];

        let entity = cx.entity().downgrade();
        let tab = self.field_2d.tabs.entry(key).or_default();
        let view = tab.select.get_or_insert_with(|| {
            let select = cx.new(|cx| Select::new(FormSelect::new(wanted.clone()), window, cx));
            let subscription = cx.subscribe(
                &select,
                move |frame: &mut Self, select, event: &SelectEvent, cx| {
                    if *event == SelectEvent::Change {
                        let value = select.read(cx).model().select().value();
                        frame.field_2d_select(key, value, cx);
                    }
                },
            );
            SelectView {
                select,
                synced: None,
                _subscription: subscription,
            }
        });
        let changed = view.synced.as_ref().is_none_or(|s| {
            s.items() != wanted.items() || s.selected_index() != wanted.selected_index()
        });
        let select = view.select.clone();
        if changed {
            view.synced = Some(wanted.clone());
            select.update(cx, |s, cx| {
                s.update_model(|m| *m.select_mut() = wanted, cx);
            });
        }
        let [sx, sy, sw, sh] = layout.select;
        select.update(cx, |s, _| {
            s.set_size(sw as f32, sh as f32);
            s.set_subpixel(
                snap_remainder(sx as f32, scale),
                snap_remainder(sy as f32, scale),
            );
        });

        let local = |r: [f64; 4]| {
            div()
                .absolute()
                .left(px((r[0] - content[0]) as f32))
                .top(px((r[1] - content[1]) as f32))
                .w(px(r[2] as f32))
                .h(px(r[3] as f32))
        };
        let place = |part: &'static str, window: &mut Window, cx: &mut App| {
            focus::place((part, key), window, cx)
        };
        let mut panel = div();
        let titles = [("Field", 0), ("Orientation", 1), ("Size", 2)];
        for (label, i) in titles {
            let r = layout.titles[i];
            let (w, _) = text::measure(cx, &title_style(), label, scale);
            let Some(offset) = centred_offset(r[2], w, f64::from(scale)) else {
                continue;
            };
            let (x, dx) = place_in(r[0], offset, f64::from(scale));
            let section = if i == 0 {
                Block::FieldTitle
            } else {
                Block::SwitcherTitles
            };
            let title = focus::section(local(r), section as isize, &place(label, window, cx));
            panel = panel.child(
                title.overflow_hidden().child(
                    div().absolute().left(px(x)).top_0().child(
                        Text::new(label, title_style())
                            .color(theme.text)
                            .subpixel(dx as f32, 0.0),
                    ),
                ),
            );
        }
        panel = panel.child(probed(
            "select.field-select",
            titled(
                "Select field image",
                local(layout.select)
                    .tab_group()
                    .tab_index(Block::Field as isize)
                    .child(select),
            ),
        ));
        if let (Some([lx, ly]), Some(width)) = (layout.source, source_width) {
            let (_, h) = text::measure(cx, &source_style(), "Source", scale);
            let entity = entity.clone();
            let focus = FocusRing::of(("field-source-focus", key), None, window, cx);
            let (ascent, descent) = text::font_metrics(cx, &source_style(), scale);
            let content = f64::from((ascent + descent) / scale);
            // On the select's baseline.
            let top = ly + f64::from(select_baseline(scale, cx) - ascent / scale);
            let r = [lx + SOURCE_PADDING, top - (h - content) / 2.0, width, h];
            let padded = [
                r[0] - SOURCE_PADDING,
                top - SOURCE_PADDING,
                r[2] + 2.0 * SOURCE_PADDING,
                content + 2.0 * SOURCE_PADDING,
            ];
            panel = panel.child(titled("Open field image source URL", local(padded)));
            panel = panel.child(untitled(
                local(padded)
                    .tab_group()
                    .tab_index(Block::Field as isize)
                    .id("field-source")
                    .track_focus(&focus.handle)
                    .on_activate(move |event, _, cx| {
                        // Space clicks a button and scrolls from a link.
                        if matches!(event, ClickEvent::Keyboard(e) if e.button == KeyboardButton::Space)
                        {
                            return;
                        }
                        let _ = entity.update(cx, |frame, cx| {
                            frame.field_2d_settings(key, SettingsInput::Source, cx)
                        });
                    })
                    .child(
                        div()
                            .absolute()
                            .left(px((r[0] - padded[0]) as f32))
                            .top(px((r[1] - padded[1]) as f32))
                            .child(
                                Text::new("Source", source_style())
                                    .color(theme.link)
                                    .underline()
                                    .subpixel(
                                        snap_remainder(r[0] as f32, scale),
                                        snap_remainder(r[1] as f32, scale),
                                    ),
                            ),
                    ),
            ));
            if focus.visible {
                let around = SOURCE_PADDING + LINK_OUTLINE_OFFSET;
                let outlined = pixel_snapped(
                    [
                        r[0] - around,
                        top - around,
                        width + 2.0 * around,
                        content + 2.0 * around,
                    ],
                    f64::from(scale),
                );
                let radius = px(FOCUS_RING_RADIUS + LINK_OUTLINE_OFFSET as f32 + FOCUS_RING_WIDTH);
                panel =
                    panel.child(local(outlined).child(focus::outline(appearance).rounded(radius)));
            }
        }
        let orientation = ORIENTATION_ICONS
            .iter()
            .filter_map(|name| Symbol::named(name).map(|s| Segment::Icon(IconSource::from(s))))
            .collect();
        let labels = SIZE_LABELS[usize::from(state.is_ftc)];
        let size = labels.iter().map(|l| Segment::Badge((*l).into())).collect();
        let orientation_input: fn(usize) -> SettingsInput = SettingsInput::Orientation;
        let size_input: fn(usize) -> SettingsInput = SettingsInput::Size;
        let switchers = [
            (
                "f2d-orientation",
                orientation,
                ORIENTATION_TITLES,
                None,
                layout.orientation,
                orientation_input,
            ),
            (
                "f2d-size",
                size,
                SIZE_TITLES,
                Some(state.size),
                layout.size,
                size_input,
            ),
        ];
        for (id, segments, titles, selected, r, input) in switchers {
            let snapped = |v: f64| math::round(v * f64::from(scale)) / f64::from(scale);
            let (x, y) = (snapped(r[0]), snapped(r[1]));
            let entity = entity.clone();
            panel = panel.child(
                local([x, y, r[2], r[3]])
                    .tab_group()
                    .tab_index(Block::Switchers as isize)
                    .child(
                        SegmentedControl::new((id, key), segments)
                            .titles(titles)
                            .subpixel((r[0] - x) as f32)
                            .selected(selected)
                            .size(r[2] as f32, r[3] as f32)
                            .appearance(appearance)
                            .on_select(move |segment, _, cx| {
                                let input = input(*segment);
                                let _ = entity.update(cx, |frame, cx| {
                                    frame.field_2d_settings(key, input, cx)
                                });
                            }),
                    ),
            );
        }
        let panel = SettingsPanel {
            key: panel_key,
            probe: "div.field-2d-settings",
            controls,
            rect: layout.panel,
            border: layout.border,
            content: panel,
            content_height: layout.content_height,
        };
        Some(self.settings_scroller(panel, appearance, scale, window, cx))
    }

    fn field_2d_select(&mut self, key: usize, value: JsString, cx: &mut Context<'_, Self>) {
        self.host_mut().input(cx, |hub| {
            let index = tab_index(hub, key)?;
            with_tab(hub, index, |_, controller, page| {
                controller.select_field(page, value)
            })
        });
        cx.notify();
    }

    fn field_2d_settings(&mut self, key: usize, input: SettingsInput, cx: &mut Context<'_, Self>) {
        self.host_mut().input(cx, |hub| {
            let index = tab_index(hub, key)?;
            with_tab(hub, index, |_, controller, page| match input {
                SettingsInput::Source => controller.open_source(page),
                SettingsInput::Orientation(i) => controller.click_orientation(i),
                SettingsInput::Size(i) => controller.click_size(i),
            })
        });
        cx.notify();
    }

    /// The pointer left the window.
    pub(super) fn field_2d_mouse_exit(&mut self, cx: &mut Context<'_, Self>) {
        let views = &mut self.field_2d;
        let hovered = std::mem::take(&mut views.hovered);
        let pointer = views.pointer.take();
        let (Some(shown), Some([x, y])) = (views.shown.filter(|_| hovered), pointer) else {
            return;
        };
        self.host_mut().input(cx, |hub| {
            with_renderer(hub, shown.index, |renderer, page| {
                renderer.mouse(page, "leave", 0.0, false, x, y);
            })
        });
        cx.notify();
    }

    /// The overlay's `mousemove` listeners when the pointer is over it, and its `mouseleave`
    /// when it left.
    pub(super) fn field_2d_mouse_move(
        &mut self,
        x: f64,
        y: f64,
        shift: bool,
        pressed: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let views = &mut self.field_2d;
        views.pointer = Some([x, y]);
        views.pressed = pressed;
        let Some(shown) = views.shown else {
            return;
        };
        let inside = views.over_overlay(x, y);
        let was_inside = std::mem::replace(&mut views.hovered, inside);
        if !inside && !was_inside {
            return;
        }
        self.host_mut().input(cx, |hub| {
            with_renderer(hub, shown.index, |renderer, page| {
                if inside {
                    renderer.mouse(page, "move", 0.0, shift, x, y);
                } else {
                    renderer.mouse(page, "leave", 0.0, shift, x, y);
                }
            });
        });
        cx.notify();
    }

    fn field_2d_mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<'_, Self>) {
        let x = mouse_client(f32::from(event.position.x));
        let y = mouse_client(f32::from(event.position.y));
        let Some(shown) = self
            .field_2d
            .shown
            .filter(|_| self.field_2d.over_overlay(x, y))
        else {
            return;
        };
        let button = mouse_button(event.button);
        let shift = event.modifiers.shift;
        self.host_mut().input(cx, |hub| {
            with_renderer(hub, shown.index, |renderer, page| {
                renderer.mouse(page, "down", button, shift, x, y);
            });
        });
        cx.notify();
    }

    fn field_2d_mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<'_, Self>) {
        let x = mouse_client(f32::from(event.position.x));
        let y = mouse_client(f32::from(event.position.y));
        let Some(shown) = self
            .field_2d
            .shown
            .filter(|_| self.field_2d.over_overlay(x, y))
        else {
            return;
        };
        let button = mouse_button(event.button);
        let shift = event.modifiers.shift;
        self.host_mut().input(cx, |hub| {
            with_renderer(hub, shown.index, |renderer, page| {
                renderer.mouse(page, "up", button, shift, x, y);
            });
        });
        cx.notify();
    }

    pub(super) fn field_2d_smooth(&mut self, key: usize, by: [f64; 2], cx: &mut Context<'_, Self>) {
        let now = self.host().frame_time(cx);
        if let Some(tab) = self.field_2d.tabs.get_mut(&key) {
            tab.sensor.scroll_by(by, now);
        }
    }

    pub(super) fn field_2d_range(&self, key: usize) -> Option<[[f64; 2]; 2]> {
        let shown = self.field_2d.shown.filter(|s| s.key == key)?;
        Some(overlay_range(shown.overlay_client_size()))
    }

    fn field_2d_wheel(&mut self, event: &ScrollEvent, cx: &mut Context<'_, Self>) {
        let x = mouse_client(f32::from(event.position.x));
        let y = mouse_client(f32::from(event.position.y));
        let Some(shown) = self
            .field_2d
            .shown
            .filter(|_| self.field_2d.over_overlay(x, y))
        else {
            return;
        };
        let now = self.host().frame_time(cx);
        let client = shown.overlay_client_size();
        if let Some(tab) = self.field_2d.tabs.get_mut(&shown.key) {
            tab.sensor.set_layout(client, shown.device_pixel_ratio);
            tab.sensor.wheel(wheel_of(event), now);
        }
        cx.notify();
    }
}

/// The field select's baseline below its top, in CSS px.
fn select_baseline(scale: f32, cx: &mut App) -> f32 {
    let (ascent, descent) = text::font_metrics(cx, &TextStyle::new(CONTROL_FONT), scale);
    let height = SELECT_HEIGHT as f32 * scale;
    let snapped = (0, height.round() as i32);
    let [_, top, _, _] = select_geometry(0.0, height, snapped, ascent + descent, scale).text;
    (top + ascent) / scale
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::control_layout_vectors;

    #[test]
    fn settings_boxes_match_blink() {
        let layout = SettingsLayout::new([300.0, 450.0, 800.0, 200.0], 0.0, 99.0, None, 1.0);
        assert_eq!(layout.panel, [849.0, 450.0, 251.0, 200.0]);
        assert_eq!(
            layout.titles,
            [
                [850.0, 455.0, 250.0, 20.0],
                [850.0, 515.0, 125.0, 20.0],
                [975.0, 515.0, 125.0, 20.0]
            ]
        );
        assert_eq!(layout.select, [925.5, 483.0, 99.0, 24.0]);
        assert_eq!(layout.orientation, [862.5, 543.0, 100.0, 65.0]);
        assert_eq!(layout.size, [987.5, 543.0, 100.0, 65.0]);
    }

    /// The focused link of `focus-field-2d` in Electron on Linux, at 1x.
    #[test]
    fn the_ring_of_the_source_link_has_snapped_edges() {
        let box_ = [994.453125, 480.5, 60.09375, 28.0];
        assert_eq!(pixel_snapped(box_, 1.0), [994.0, 481.0, 61.0, 28.0]);
    }

    #[test]
    fn source_link_has_a_style() {
        source_style();
    }

    #[test]
    fn quarter_turns_are_exact() {
        assert_eq!(quarter_sin_cos(0.0), (0.0, 1.0));
        assert_eq!(quarter_sin_cos(-std::f64::consts::PI / 2.0), (-1.0, 0.0));
        assert_eq!(quarter_sin_cos(std::f64::consts::PI), (0.0, -1.0));
    }

    #[test]
    fn controls_lie_where_blink_puts_them() {
        control_layout_vectors::each(2.0, 1440, |blink| {
            let scale = blink.scale;
            let select = blink.one("div.setting-blocks select", "field-select").rect;
            let source = blink.all("div.setting-blocks a").first().map(|a| a.rect);
            let padding = 2.0 * zoomed(SOURCE_PADDING, scale) / scale;
            let source_width = source.map(|a| a[2] - padding);
            let layout = SettingsLayout::new(blink.controls, 0.0, select[2], source_width, scale);
            let panel = blink.all("div.setting-blocks")[0].rect;
            blink.check("panel", &layout.panel, &panel);
            let field = blink.all("div.setting-blocks > div")[0].rect;
            let halves: Vec<_> = blink.all("div.setting-blocks div.half")[..2]
                .iter()
                .map(|half| (half.rect, half.lines[0]))
                .collect();
            blink.check("title", &layout.titles[0], &field);
            for (title, (half, line)) in layout.titles[1..].iter().zip(halves) {
                blink.check("title", title, &half);
                let offset = centred_offset(title[2], line[2], scale).expect("fits");
                blink.check("title text", &[title[0] + offset], &line[..1]);
            }
            blink.check("select", &layout.select, &select);
            if let (Some([x, _]), Some(a)) = (layout.source, source) {
                blink.check("source", &[x], &a[..1]);
            }
            for (ours, class) in [(layout.orientation, "orientation"), (layout.size, "size")] {
                let split = blink.one("div.setting-blocks div.split", class).rect;
                blink.check(class, &ours, &split);
            }
        });
    }
}
