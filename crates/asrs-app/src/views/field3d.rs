//! The 3D field tab (`#renderer3` and `#controller3` in `www/hub.html`, styled by
//! `www/renderers.css` and `www/hub.css`): `Field3dRenderer.ts` as the tab's renderer, its
//! canvas presented from the wgpu render thread, the model and texture loads its worker queue
//! asks for, the alert, the loading spinner, camera-controls and keyboard input, and the settings
//! block beside the source list.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use asrs_canvas::Canvas2d;
use asrs_field3d::camera_controls::{ElementRect, PointerInput, WheelInput};
use asrs_field3d::glb::{self, LoadJob, LoadedField, OptimizedMesh};
use asrs_field3d::managers::{april_tag_texture_path, decode_png_texture};
use asrs_field3d::render::{Frame, RenderThread};
use asrs_field3d::renderer::{Field3dEnv, Field3dRenderer, Field3dRendererCommand, LoadRequest};
use asrs_hub::hub::{HubModel, HubPage};
use asrs_js::math;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::JsValue;
use asrs_model::assets::built_in_3d_fields;
use asrs_model::layout_unit::{border, centred, device, percent, zoomed};
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::preferences::{Field3dMode, Preferences};
use asrs_model::tab_controller::{TabCommand, TabRenderer};
use asrs_model::tab_type::TabType;
use asrs_tabs::field3d::Field3dController;
use asrs_tabs::form::{self, OptGroup, SelectOption};
use asrs_ui::canvas_view::geometry::{client_size, device_snap, mouse_client};
use asrs_ui::canvas_view::{CanvasMetrics, CanvasState, FramePresenter, canvas_view};
use asrs_ui::focus::{self, FocusRing};
use asrs_ui::gpui::{
    AnyElement, App, AppContext, Bounds, ClickEvent, Context, Corners, Entity, Global,
    InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, RenderImage, ScrollEvent,
    StatefulInteractiveElement, Styled, Task, Window, canvas, div, point, px, rgb, size,
};
use asrs_ui::icon::{Icon, IconFit, IconSource};
use asrs_ui::input::style::CONTROL_FONT;
use asrs_ui::input::{Select, SelectEvent};
use asrs_ui::parity::probed;
use asrs_ui::platform;
use asrs_ui::scroll;
use asrs_ui::symbols::Symbol;
use asrs_ui::text::{self, Text, TextStyle, WhiteSpace, snap_remainder};
use asrs_ui::theme::{Appearance, FOCUS_RING_WIDTH, FONT_FAMILY, chains};
use asrs_ui::tooltip::{titled, untitled};

use super::frame::{FrameHost, HubFrame};
use super::select::FormSelect;
use super::settings_panel::{self, SettingsPanel};
use super::source_list::centred_offset;
use crate::main_process::app_root;

/// `div.field-3d-canvas-container`'s `height: calc(100% + 26px)`, or `+ 10px` when `.fixed`.
const CONTAINER_EXTRA: f64 = 26.0;
const CONTAINER_EXTRA_FIXED: f64 = 10.0;
const ALERT_TOP: f64 = 20.0;
const ALERT_PADDING: f64 = 10.0;
const SPINNER_SIZE: f64 = 44.0;
const SPINNER_BOTTOM: f64 = 10.0;
const SPINNER_PERIOD_MS: f64 = 6000.0;
const SPINNER_TRANSITION_MS: f64 = 100.0;
const SETTINGS_MAX_WIDTH: f64 = 200.0;
const TITLE_HEIGHT: f64 = 20.0;
const FIELD_INPUT_HEIGHT: f64 = 40.0;
const SELECT_HEIGHT: f64 = 24.0;
const MOUSE_POINTER_ID: i32 = 1;
/// How long a 3D view stays hidden before the GPU lets go of what it holds for it.
pub const RELEASE_GRACE: Duration = Duration::from_secs(10);

/// The directory holding upstream's `www`, which `TextureLoader` URLs (`../www/...`) resolve
/// against.
pub struct WwwRoot(pub PathBuf);

impl Global for WwwRoot {}

/// What `Field3dRendererImpl.render` reads from its canvas, the canvas's parent and `window`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CanvasInputs {
    pub client: [f64; 2],
    pub rect_width: f64,
    pub parent: Option<[f64; 2]>,
    pub device_pixel_ratio: f64,
    pub dark: bool,
    pub reduced_motion: bool,
}

/// The hub's `Field3dRenderer`. The view writes what the DOM would answer into `inputs`; each
/// `renderer.render` call leaves the frame it draws for the view to present.
pub struct Field3dTab {
    renderer: Field3dRenderer,
    inputs: Rc<Cell<CanvasInputs>>,
    preferences: Option<Arc<Preferences>>,
    frame: Option<Frame>,
    /// Whether the last `render` changed the DOM.
    dom_changed: bool,
}

impl Field3dTab {
    pub fn new(preferences: Option<&Preferences>, is_battery: bool) -> Self {
        Field3dTab {
            renderer: Field3dRenderer::new(preferences, is_battery, cfg!(target_os = "macos")),
            inputs: Rc::default(),
            preferences: None,
            frame: None,
            dom_changed: false,
        }
    }

    pub fn renderer(&self) -> &Field3dRenderer {
        &self.renderer
    }

    pub fn renderer_mut(&mut self) -> &mut Field3dRenderer {
        &mut self.renderer
    }

    fn shared_preferences(&mut self, page: &HubPage) -> Option<Arc<Preferences>> {
        let current = page.preferences()?;
        match &self.preferences {
            Some(p) if **p == *current => {}
            _ => self.preferences = Some(Arc::new(current.clone())),
        }
        self.preferences.clone()
    }

    fn env(&mut self, page: &HubPage) -> Field3dEnv {
        let inputs = self.inputs.get();
        let now = page.clock().now();
        Field3dEnv {
            assets: page.shared_assets().cloned(),
            preferences: self.shared_preferences(page),
            is_battery: page.is_battery(),
            client_width: inputs.client[0],
            client_height: inputs.client[1],
            rect_width: inputs.rect_width,
            parent_size: inputs.parent,
            device_pixel_ratio: inputs.device_pixel_ratio,
            is_dark: inputs.dark,
            reduced_motion: inputs.reduced_motion,
            performance_now: now,
            date_now: now,
        }
    }
}

impl TabRenderer<HubPage> for Field3dTab {
    fn save_state(&self, _: &HubPage) -> JsValue {
        self.renderer.save_state()
    }

    fn restore_state(&mut self, _: &mut HubPage, state: &JsValue) -> Result<(), JsError> {
        self.renderer.restore_state(state);
        Ok(())
    }

    fn aspect_ratio(&self, _: &HubPage) -> Option<f64> {
        self.renderer.get_aspect_ratio()
    }

    fn render(&mut self, cx: &mut HubPage, command: &dyn TabCommand) -> Result<(), JsError> {
        let command = Field3dRendererCommand::from_js(&command.to_js())?;
        let env = self.env(cx);
        let before = self.renderer.dom().clone();
        self.renderer.dom_mut().run_timers(env.performance_now);
        let result = self.renderer.render(&command, &env);
        self.dom_changed = *self.renderer.dom() != before;
        result?;
        if let Some(implementation) = self.renderer.implementation_mut()
            && !implementation.take_renders().is_empty()
        {
            self.frame = Some(implementation.build_frame());
        }
        Ok(())
    }

    fn needs_draw(&self) -> bool {
        self.frame.is_some() || self.dom_changed
    }

    fn set_3d_camera(&mut self, _: &mut HubPage, index: f64) {
        self.renderer.set_3d_camera(index);
    }

    fn set_fov(&mut self, _: &mut HubPage, fov: f64) {
        self.renderer.set_fov(fov);
    }
}

fn tab_key(tab: &Field3dTab) -> usize {
    Rc::as_ptr(&tab.inputs) as *const () as usize
}

fn field3d_tabs(hub: &HubModel) -> Vec<(usize, usize)> {
    hub.tabs()
        .tabs()
        .enumerate()
        .filter_map(|(i, tab)| {
            tab.renderer()
                .downcast_ref::<Field3dTab>()
                .map(|r| (i, tab_key(r)))
        })
        .collect()
}

fn displayed(hub: &HubModel) -> Option<(usize, &Field3dTab)> {
    hub.tabs().tabs().enumerate().find_map(|(i, tab)| {
        if tab.content_hidden() || tab.tab_type() != Some(TabType::Field3d) {
            return None;
        }
        tab.renderer().downcast_ref::<Field3dTab>().map(|r| (i, r))
    })
}

fn with_tab<R>(
    hub: &mut HubModel,
    index: usize,
    f: impl FnOnce(&mut Field3dTab, &mut HubPage) -> R,
) -> Option<R> {
    let (tabs, page) = hub.tabs_mut();
    let tab = tabs.tab_mut(index)?;
    let renderer = tab.renderer.downcast_mut::<Field3dTab>()?;
    Some(f(renderer, page))
}

fn with_controller<R>(
    hub: &mut HubModel,
    index: usize,
    f: impl FnOnce(&mut Field3dController, &JsValue, &mut HubPage) -> R,
) -> Option<R> {
    let (tabs, page) = hub.tabs_mut();
    let tab = tabs.tab_mut(index)?;
    let uuid = tab.uuid.clone();
    let controller = tab.controller.downcast_mut::<Field3dController>()?;
    Some(f(controller, &uuid, page))
}

enum Load {
    Field(u64, LoadJob<LoadedField>),
    Robot(u64, LoadJob<Vec<Vec<OptimizedMesh>>>),
}

/// The spinner's `visible` and `animating` classes over time: when each was last added or
/// removed, for its transition and keyframes.
#[derive(Clone, Copy, Debug, Default)]
struct Spinner {
    visible: bool,
    changed_at: f64,
    animating_since: Option<f64>,
}

impl Spinner {
    fn update(&mut self, visible: bool, animating: bool, now: f64) {
        if visible != self.visible {
            self.visible = visible;
            self.changed_at = now;
        }
        match (animating, self.animating_since) {
            (true, None) => self.animating_since = Some(now),
            (false, Some(_)) => self.animating_since = None,
            _ => {}
        }
    }

    /// The container's `opacity` and `scale()` under the 0.1 s `ease-in-out` transition.
    fn transition(&self, now: f64) -> (f64, f64, bool) {
        let t = math::min2(
            math::max2((now - self.changed_at) / SPINNER_TRANSITION_MS, 0.0),
            1.0,
        );
        let k = ease_in_out(t);
        let (from, to) = if self.visible { (0.0, 1.0) } else { (1.0, 0.0) };
        let opacity = from + (to - from) * k;
        (opacity, 0.3 + 0.2 * opacity, t < 1.0)
    }
}

#[derive(Default)]
struct TabView {
    presenter: FramePresenter,
    #[cfg(feature = "parity")]
    presented: Option<Frame>,
    inputs: Rc<Cell<CanvasInputs>>,
    loads: Vec<Load>,
    select: Option<Entity<Select<FormSelect>>>,
    options: Vec<(JsString, JsString, bool)>,
    spinner: Spinner,
    spinner_canvas: Option<Entity<CanvasState>>,
}

/// The displayed 3D tab's boxes in client coordinates.
#[derive(Clone, Copy, Debug)]
struct Shown {
    index: usize,
    container: [f64; 4],
    /// The canvas's border box, which is its content box unless it is `.fixed`.
    canvas: [f64; 4],
    /// The canvas's border, which only a `.fixed` one has.
    border: f64,
}

impl Shown {
    fn content(&self) -> [f64; 4] {
        let b = self.border;
        let [x, y, w, h] = self.canvas;
        [x + b, y + b, w - 2.0 * b, h - 2.0 * b]
    }

    fn element_rect(&self) -> ElementRect {
        let [x, y, width, height] = self.canvas;
        ElementRect {
            x,
            y,
            width,
            height,
        }
    }
}

struct InFlight {
    key: usize,
    #[cfg(feature = "parity")]
    frame: Frame,
}

#[derive(Default)]
pub struct Field3dViews {
    tabs: HashMap<usize, TabView>,
    shown: Option<Shown>,
    thread: Option<Result<RenderThread, String>>,
    /// Whether the thread was told to free its targets and got no frame since.
    released: bool,
    in_flight: Option<InFlight>,
    queued: Option<(usize, Frame)>,
    wake: Option<(async_channel::Sender<()>, Task<()>)>,
}

impl Field3dViews {
    /// Loads, renders or a frame waiting to be presented: what a capture waits out.
    pub fn busy(&self) -> bool {
        self.in_flight.is_some()
            || self.queued.is_some()
            || self.tabs.values().any(|t| !t.loads.is_empty())
    }
}

fn read_texture(root: Option<&PathBuf>, url: &JsString) -> Option<asrs_field3d::render::Texture> {
    let path = april_tag_texture_path(root?, url)?;
    decode_png_texture(&std::fs::read(path).ok()?)
}

fn www_root(cx: &App) -> Option<PathBuf> {
    if let Some(root) = cx.try_global::<WwwRoot>() {
        return Some(root.0.clone());
    }
    let root = app_root();
    root.join("www").is_dir().then_some(root)
}

/// `cubic-bezier(0.42, 0, 0.58, 1)` (`ease-in-out`) at `t`.
fn ease_in_out(t: f64) -> f64 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let bezier = |a: f64, b: f64, s: f64| {
        3.0 * a * s * (1.0 - s).powi(2) + 3.0 * b * s * s * (1.0 - s) + s.powi(3)
    };
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..64 {
        let mid = (lo + hi) / 2.0;
        if bezier(0.42, 0.58, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bezier(0.0, 1.0, (lo + hi) / 2.0)
}

/// `KeyboardEvent.code` for the key GPUI reports.
pub(super) fn key_code(key: &str) -> String {
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => format!("Key{}", c.to_ascii_uppercase()),
        (Some(c), None) if c.is_ascii_digit() => format!("Digit{c}"),
        _ => match key {
            "space" => "Space".into(),
            "enter" => "Enter".into(),
            "escape" => "Escape".into(),
            "tab" => "Tab".into(),
            "backspace" => "Backspace".into(),
            "up" => "ArrowUp".into(),
            "down" => "ArrowDown".into(),
            "left" => "ArrowLeft".into(),
            "right" => "ArrowRight".into(),
            "/" | "?" => "Slash".into(),
            "," | "<" => "Comma".into(),
            "." | ">" => "Period".into(),
            other => other.to_owned(),
        },
    }
}

/// `MouseEvent.buttons` with `button` held.
fn buttons_of(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Right => 2,
        MouseButton::Middle => 4,
        _ => 0,
    }
}

/// `MouseEvent.button`.
fn button_number(button: MouseButton) -> f64 {
    match button {
        MouseButton::Left => 0.0,
        MouseButton::Middle => 1.0,
        MouseButton::Right => 2.0,
        _ => 3.0,
    }
}

#[cfg(feature = "parity")]
fn mode_name(mode: Field3dMode) -> &'static str {
    match mode {
        Field3dMode::Cinematic => "cinematic",
        Field3dMode::Standard => "standard",
        Field3dMode::LowPower => "low-power",
    }
}

/// The share of a `"N%"` inline size; `None` for an unset or unparsable value.
fn share(value: &str) -> Option<f64> {
    let share: f64 = value.strip_suffix('%')?.parse().ok()?;
    share.is_finite().then_some(share)
}

/// `canvas.field-3d-canvas.fixed`'s border box in `container`, at `left: 50%; top: 50%;
/// transform: translate(-50%, -50%)`.
fn fixed_canvas(container: [f64; 4], size: [&str; 2], scale: f64) -> [f64; 4] {
    let container = container.map(|v| device(v, scale));
    let edge = border(1.0, scale);
    let mut out = [0.0; 4];
    for i in 0..2 {
        let space = container[i + 2];
        let length = share(size[i]).map_or(0.0, |share| percent(space, share)) + 2.0 * edge;
        out[i] = (container[i] + percent(space, 50.0) - length / 2.0) / scale;
        out[i + 2] = length / scale;
    }
    out
}

/// `div.field-3d-alert`'s border box, from its container's corner, and its lines' lefts and tops
/// from its own, around lines of these sizes.
fn alert_layout(container_width: f64, lines: &[[f64; 2]], scale: f64) -> ([f64; 4], Vec<[f64; 2]>) {
    let lines: Vec<[f64; 2]> = lines.iter().map(|l| l.map(|v| device(v, scale))).collect();
    let edge = border(1.0, scale);
    let padding = zoomed(ALERT_PADDING, scale);
    let inner = lines.iter().map(|l| l[0]).fold(0.0, math::max2);
    let height: f64 = lines.iter().map(|l| l[1]).sum();
    let width = inner + 2.0 * (padding + edge);
    let left = percent(device(container_width, scale), 50.0) - width / 2.0;
    let mut top = edge + padding;
    let placed = lines
        .iter()
        .map(|[w, h]| {
            let at = [edge + padding + centred(inner, *w), top].map(|v| v / scale);
            top += h;
            at
        })
        .collect();
    let height = height + 2.0 * (padding + edge);
    let rect = [left, zoomed(ALERT_TOP, scale), width, height].map(|v| v / scale);
    (rect, placed)
}

/// Where `div.field-3d-settings` puts its parts inside `div.controls-content`, in client
/// coordinates of its unscrolled content.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SettingsLayout {
    /// The border box, `border` of it on the left the `border-left`.
    panel: [f64; 4],
    border: f64,
    content_height: f64,
    titles: [[f64; 4]; 2],
    select: [f64; 4],
    button: [f64; 4],
    image: [f64; 4],
}

impl SettingsLayout {
    /// `bar` is what a scrollbar takes of the panel where it overflows, in CSS px.
    fn new(controls: [f64; 4], bar: f64, select_width: f64, scale: f64) -> SettingsLayout {
        let [x, y, w, h] = controls.map(|v| device(v, scale));
        let length = |v: f64| zoomed(v, scale);
        let css = |rect: [f64; 4]| rect.map(|v| v / scale);
        let edge = border(1.0, scale);
        let outer = math::min2(percent(w, 40.0), length(SETTINGS_MAX_WIDTH));
        let panel = [x + w - outer - edge, y, outer + edge, h];
        let left = panel[0] + edge;
        let title = length(TITLE_HEIGHT);
        let row = length(FIELD_INPUT_HEIGHT);
        let tall = math::min2(math::max2(h - length(85.0), row), length(100.0));
        let content_height = length(5.0) + 2.0 * title + row + tall;
        let width = if content_height > h {
            outer - length(bar)
        } else {
            outer
        };
        let top = y + length(5.0);
        let select = [device(select_width, scale), length(SELECT_HEIGHT)];
        let split = percent(width, 80.0);
        let [button_width, button_height] = [percent(split, 60.0), percent(tall, 65.0)];
        let button = [
            left + percent(width, 50.0) - split / 2.0 + centred(split, button_width),
            top + 2.0 * title + row + length(8.0),
            button_width,
            button_height,
        ];
        let inner = [button_width, button_height].map(|v| math::max2(v - 2.0 * edge, 0.0));
        let image = inner.map(|v| percent(v, 60.0));
        SettingsLayout {
            panel: css(panel),
            border: edge / scale,
            content_height: content_height / scale,
            titles: [0.0, 1.0].map(|i| css([left, top + i * (title + row), width, title])),
            select: css([
                left + centred(width, select[0]),
                top + title + percent(row, 50.0) - select[1] / 2.0,
                select[0],
                select[1],
            ]),
            button: css(button),
            image: css([
                button[0] + edge + centred(inner[0], image[0]),
                button[1] + edge + percent(inner[1], 50.0) - image[1] / 2.0,
                image[0],
                image[1],
            ]),
        }
    }
}

/// Paints `image` one device pixel per bitmap pixel at the canvas content box's snapped origin.
fn paint_frame(image: &Arc<RenderImage>, origin: [f32; 2], window: &mut Window) {
    let dpr = window.scale_factor();
    let device = image.size(0);
    let to_px = |v: i32| px(v as f32 / dpr);
    let bounds = Bounds {
        origin: point(
            to_px(device_snap(origin[0], dpr)),
            to_px(device_snap(origin[1], dpr)),
        ),
        size: size(to_px(device.width.0), to_px(device.height.0)),
    };
    let _ = window.paint_image(bounds, bounds, Corners::default(), image.clone(), 0, false);
}

#[cfg(feature = "parity")]
fn gl_dump(frame: &Frame, rect: [f64; 4], mode: Field3dMode) -> asrs_ui::parity::GlDump {
    use std::cell::RefCell;

    use asrs_field3d::render::{Renderer, edge_mask, swap_red_blue};
    thread_local! {
        // Leaked: dropping a wgpu device in a thread-local destructor at exit panics.
        static RENDERER: RefCell<Option<&'static mut Renderer>> = const { RefCell::new(None) };
    }
    RENDERER.with(|cell| {
        let mut cell = cell.borrow_mut();
        let renderer = cell.get_or_insert_with(|| {
            Box::leak(Box::new(
                Renderer::new().expect("a wgpu device for the 3D dump"),
            ))
        });
        let mut rgba = Vec::new();
        renderer
            .render(frame, &mut rgba)
            .expect("a rendered 3D dump");
        swap_red_blue(&mut rgba);
        let edges = edge_mask(&renderer.render_aux(frame));
        asrs_ui::parity::GlDump {
            rect,
            width: frame.options.width,
            height: frame.options.height,
            rgba,
            edges,
            mode: mode_name(mode),
        }
    })
}

/// The six faces of `div.spinner-cubes` (44 px, `transform-style: preserve-3d`) as their
/// `transform-origin` and transform list, applied right to left.
#[derive(Clone, Copy)]
enum Op {
    TranslateX(f64),
    TranslateY(f64),
    TranslateZ(f64),
    RotateX(f64),
    RotateY(f64),
}

const FACES: [([f64; 2], [Op; 2]); 6] = [
    ([22.0, 22.0], [Op::TranslateZ(-22.0), Op::RotateY(180.0)]),
    ([44.0, 0.0], [Op::RotateY(-270.0), Op::TranslateX(22.0)]),
    ([0.0, 22.0], [Op::RotateY(270.0), Op::TranslateX(-22.0)]),
    ([22.0, 0.0], [Op::RotateX(90.0), Op::TranslateY(-22.0)]),
    ([22.0, 44.0], [Op::RotateX(-90.0), Op::TranslateY(22.0)]),
    ([22.0, 22.0], [Op::TranslateZ(22.0), Op::TranslateZ(0.0)]),
];

type V3 = [f64; 3];
type Quad = [[f64; 2]; 4];

fn apply(op: Op, p: V3) -> V3 {
    let [x, y, z] = p;
    match op {
        Op::TranslateX(d) => [x + d, y, z],
        Op::TranslateY(d) => [x, y + d, z],
        Op::TranslateZ(d) => [x, y, z + d],
        Op::RotateX(deg) => {
            let (s, c) = deg.to_radians().sin_cos();
            [x, c * y - s * z, s * y + c * z]
        }
        Op::RotateY(deg) => {
            let (s, c) = deg.to_radians().sin_cos();
            [c * x + s * z, y, -s * x + c * z]
        }
    }
}

fn rotate_z(deg: f64, p: V3) -> V3 {
    let (s, c) = deg.to_radians().sin_cos();
    [c * p[0] - s * p[1], s * p[0] + c * p[1], p[2]]
}

/// `rotate(z) rotateX(x) rotateY(y)` of `@keyframes spinner-cubes` at `ms` into the animation,
/// each 1.5 s segment eased with `ease-in-out`.
fn spinner_angles(ms: f64) -> [f64; 3] {
    const KEYS: [[f64; 3]; 5] = [
        [0.0, -35.0, 45.0],
        [180.0, -215.0, -135.0],
        [0.0, -35.0, 45.0],
        [180.0, 145.0, 225.0],
        [0.0, -35.0, 45.0],
    ];
    let phase = (ms.rem_euclid(SPINNER_PERIOD_MS)) / SPINNER_PERIOD_MS * 4.0;
    let i = (phase.floor() as usize).min(3);
    let k = ease_in_out(phase - i as f64);
    let (a, b) = (KEYS[i], KEYS[i + 1]);
    [0, 1, 2].map(|j| a[j] + (b[j] - a[j]) * k)
}

/// The cube flattened onto the page (no perspective): each face's outer and inner (border)
/// quads in the 44 px box, back to front.
fn spinner_faces(angles: Option<[f64; 3]>) -> Vec<(Quad, Quad)> {
    let cube = |p: V3| -> V3 {
        let Some([z, x, y]) = angles else {
            return p;
        };
        let centred = [p[0] - 22.0, p[1] - 22.0, p[2]];
        let r = rotate_z(z, apply(Op::RotateX(x), apply(Op::RotateY(y), centred)));
        [r[0] + 22.0, r[1] + 22.0, r[2]]
    };
    let mut faces: Vec<(f64, Quad, Quad)> = FACES
        .iter()
        .map(|(origin, ops)| {
            let place = |x: f64, y: f64| {
                let mut p = [x - origin[0], y - origin[1], 0.0];
                for op in ops.iter().rev() {
                    p = apply(*op, p);
                }
                cube([p[0] + origin[0], p[1] + origin[1], p[2]])
            };
            let quad = |inset: f64| {
                [
                    place(inset, inset),
                    place(SPINNER_SIZE - inset, inset),
                    place(SPINNER_SIZE - inset, SPINNER_SIZE - inset),
                    place(inset, SPINNER_SIZE - inset),
                ]
            };
            let (outer, inner) = (quad(0.0), quad(2.0));
            let depth = outer.iter().map(|p| p[2]).sum::<f64>() / 4.0;
            let flat = |q: [V3; 4]| q.map(|p| [p[0], p[1]]);
            (depth, flat(outer), flat(inner))
        })
        .collect();
    faces.sort_by(|a, b| a.0.total_cmp(&b.0));
    faces.into_iter().map(|(_, o, i)| (o, i)).collect()
}

fn spinner_canvas(metrics: &CanvasMetrics, angles: Option<[f64; 3]>, scale: f64) -> Canvas2d {
    let (w, h) = metrics.client_bitmap_size();
    let mut c = Canvas2d::new(w, h);
    let s = metrics.device_pixel_ratio * scale;
    c.scale(s, s);
    for (outer, inner) in spinner_faces(angles) {
        c.begin_path();
        for (i, p) in outer.iter().enumerate() {
            if i == 0 {
                c.move_to(p[0], p[1]);
            } else {
                c.line_to(p[0], p[1]);
            }
        }
        c.close_path();
        c.set_fill_style("rgba(0, 170, 255, 0.2)");
        c.fill();
        c.begin_path();
        for (i, p) in outer.iter().enumerate() {
            if i == 0 {
                c.move_to(p[0], p[1]);
            } else {
                c.line_to(p[0], p[1]);
            }
        }
        c.close_path();
        for (i, p) in inner.iter().rev().enumerate() {
            if i == 0 {
                c.move_to(p[0], p[1]);
            } else {
                c.line_to(p[0], p[1]);
            }
        }
        c.close_path();
        c.set_fill_style("#00aaff");
        c.fill();
    }
    c
}

fn title_style() -> TextStyle {
    let font = format!("bold 16px {}", FONT_FAMILY.join(", "));
    TextStyle::from_css([
        ("font", font.as_str()),
        ("line-height", "20px"),
        ("white-space", "nowrap"),
    ])
    .expect("setting title style")
}

fn alert_style() -> TextStyle {
    let font = format!("16px {}", FONT_FAMILY.join(", "));
    TextStyle::from_css([("font", font.as_str()), ("white-space", "nowrap")])
        .expect("3D alert style")
}

/// `img` of `viewinar.svg` without its intrinsic size, so it scales to its box.
fn xr_icon() -> Option<IconSource> {
    let svg = std::str::from_utf8(Symbol::named("viewinar.svg")?.svg()).ok()?;
    Some(IconSource::Svg(
        svg.replacen(r#" height="24px""#, "", 1)
            .replacen(r#" width="24px""#, "", 1)
            .into(),
    ))
}

fn local(rect: [f64; 4], origin: [f64; 4]) -> asrs_ui::gpui::Div {
    div()
        .absolute()
        .left(px((rect[0] - origin[0]) as f32))
        .top(px((rect[1] - origin[1]) as f32))
        .w(px(rect[2] as f32))
        .h(px(rect[3] as f32))
}

impl<H: FrameHost> HubFrame<H> {
    fn field3d(&mut self) -> &mut Field3dViews {
        &mut self.field3d
    }

    pub fn field3d_busy(&self) -> bool {
        self.field3d.busy()
    }

    /// Whether the 3D render thread holds render targets and models; `None` without one.
    pub fn field3d_holds(&self) -> Option<(bool, bool)> {
        match &self.field3d.thread {
            Some(Ok(thread)) => Some((thread.holds_targets(), thread.holds_models())),
            _ => None,
        }
    }

    fn wake_sender(&mut self, cx: &mut Context<'_, Self>) -> async_channel::Sender<()> {
        if let Some((sender, _)) = &self.field3d.wake {
            return sender.clone();
        }
        let (sender, receiver) = async_channel::unbounded::<()>();
        let pump = cx.spawn(async move |this, cx| {
            while receiver.recv().await.is_ok() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        self.field3d.wake = Some((sender.clone(), pump));
        sender
    }

    /// One frame of the work upstream's workers, texture loader and WebGL context do between
    /// renders: starts and answers loads, then hands the newest frame to the render thread and
    /// presents what it finished.
    pub(super) fn field3d_frame(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let tabs = field3d_tabs(self.host().hub(cx));
        let live: Vec<usize> = tabs.iter().map(|t| t.1).collect();
        self.field3d.tabs.retain(|key, view| {
            let keep = live.contains(key);
            if !keep {
                view.presenter.release(cx);
            }
            keep
        });
        if tabs.is_empty() {
            self.field3d_close(cx);
            return;
        }
        let hidden = displayed(self.host().hub(cx)).is_none();
        let now = self.host().frame_time(cx);
        let root = www_root(cx);
        let waker = self.wake_sender(cx);
        for (index, key) in tabs {
            let (requests, textures, frame, inputs, dom) = self
                .host_mut()
                .input(cx, |hub| {
                    with_tab(hub, index, |tab, _| {
                        tab.renderer.dom_mut().run_timers(now);
                        (
                            tab.renderer.take_load_requests(),
                            tab.renderer.take_texture_requests(),
                            tab.frame.take(),
                            tab.inputs.clone(),
                            tab.renderer.dom().clone(),
                        )
                    })
                })
                .expect("a 3D tab at its index");
            let view = self.field3d.tabs.entry(key).or_default();
            view.inputs = inputs;
            view.spinner
                .update(dom.spinner_visible, dom.spinner_animating, now);
            for request in requests {
                let fetch: Arc<glb::Fetch<'static>> = Arc::new(glb::read_file_url);
                let wake = waker.clone();
                let notify = move || {
                    let _ = wake.try_send(());
                };
                view.loads.push(match request {
                    LoadRequest::Field {
                        ticket,
                        config,
                        options,
                    } => Load::Field(
                        ticket,
                        glb::spawn_load_field(config, options, fetch, notify),
                    ),
                    LoadRequest::Robot { ticket, request } => Load::Robot(
                        ticket,
                        glb::spawn_load_robot(
                            request.config,
                            request.is_ftc,
                            request.options,
                            fetch,
                            notify,
                        ),
                    ),
                });
            }
            let mut finished = Vec::new();
            view.loads.retain(|load| {
                let (ticket, done) = match load {
                    Load::Field(ticket, job) => (*ticket, job.try_take().map(|r| r.map(Ok))),
                    Load::Robot(ticket, job) => (*ticket, job.try_take().map(|r| r.map(Err))),
                };
                match done {
                    None => true,
                    Some(Ok(result)) => {
                        finished.push((ticket, result));
                        false
                    }
                    Some(Err(error)) => {
                        eprintln!("asrs: 3D model load {ticket} failed: {error:?}");
                        false
                    }
                }
            });
            let textures: Vec<_> = textures
                .into_iter()
                .filter_map(|t| read_texture(root.as_ref(), &t.url).map(|tex| (t.ticket, tex)))
                .collect();
            if !finished.is_empty() || !textures.is_empty() {
                self.host_mut().input(cx, |hub| {
                    with_tab(hub, index, |tab, _| {
                        for (ticket, result) in finished {
                            match result {
                                Ok(field) => tab.renderer.field_loaded(ticket, &field),
                                Err(robot) => tab.renderer.robot_loaded(ticket, &robot),
                            }
                        }
                        for (ticket, texture) in textures {
                            tab.renderer.texture_loaded(ticket, texture);
                        }
                    });
                });
            }
            if let Some(frame) = frame {
                self.field3d.queued = Some((key, frame));
            }
        }
        self.field3d_present(hidden, waker, window, cx);
    }

    /// With the last 3D tab gone, so are the render thread and its device, off the UI thread.
    fn field3d_close(&mut self, cx: &mut Context<'_, Self>) {
        let views = self.field3d();
        views.queued = None;
        views.in_flight = None;
        views.wake = None;
        views.released = false;
        if let Some(Ok(thread)) = views.thread.take() {
            cx.background_spawn(async move { drop(thread) }).detach();
        }
    }

    fn field3d_present(
        &mut self,
        hidden: bool,
        waker: async_channel::Sender<()>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let views = self.field3d();
        if views.thread.is_none() && views.queued.is_some() {
            views.thread = Some(
                RenderThread::spawn(move || {
                    let _ = waker.try_send(());
                })
                .map_err(|e| {
                    eprintln!("asrs: no 3D renderer: {e}");
                    e.to_string()
                }),
            );
        }
        if let Some(Ok(thread)) = &views.thread
            && let Some(failure) = thread.failure()
        {
            views.thread = Some(Err(failure));
            views.in_flight = None;
        }
        let Some(Ok(thread)) = &views.thread else {
            views.queued = None;
            return;
        };
        if let Some(rendered) = thread.take()
            && let Some(flight) = views.in_flight.take()
            && let Some(view) = views.tabs.get_mut(&flight.key)
        {
            view.presenter
                .present_bgra(rendered.width, rendered.height, rendered.bgra, window, cx);
            #[cfg(feature = "parity")]
            {
                view.presented = Some(flight.frame);
            }
        }
        if views.in_flight.is_none()
            && let Some((key, frame)) = views.queued.take()
        {
            views.in_flight = Some(InFlight {
                key,
                #[cfg(feature = "parity")]
                frame: frame.clone(),
            });
            views.released = false;
            thread.submit(frame);
        }
        if hidden && !views.released && views.in_flight.is_none() {
            views.released = true;
            thread.release_after(RELEASE_GRACE);
        } else if !hidden && views.released {
            views.released = false;
            thread.cancel_release();
        }
    }

    /// The displayed 3D tab's renderer element over `rect` (`div.renderer-content`).
    pub(super) fn field3d_element(
        &mut self,
        rect: [f64; 4],
        appearance: Appearance,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let hub = self.host().hub(cx);
        let Some((index, tab)) = displayed(hub) else {
            self.field3d.shown = None;
            return None;
        };
        let key = tab_key(tab);
        let dom = tab.renderer.dom().clone();
        let mode = tab
            .renderer
            .implementation()
            .map_or(Field3dMode::Standard, |i| i.mode());
        let dark = appearance == Appearance::Dark;
        let now = self.host().frame_time(cx);

        let extra = if hub.page().is_satellite() {
            0.0
        } else if dom.container_fixed {
            CONTAINER_EXTRA_FIXED
        } else {
            CONTAINER_EXTRA
        };
        let container = [rect[0], rect[1] - extra, rect[2], rect[3] + extra];
        let canvas_box = if dom.canvas_fixed {
            let size = [&*dom.canvas_width, &*dom.canvas_height];
            fixed_canvas(container, size, f64::from(scale))
        } else {
            container
        };
        let shown = Shown {
            index,
            container,
            canvas: canvas_box,
            border: if dom.canvas_fixed {
                border(1.0, f64::from(scale)) / f64::from(scale)
            } else {
                0.0
            },
        };
        self.field3d.shown = Some(shown);
        let content = shown.content();
        let dpr = scale;
        let client = [
            f64::from(client_size(content[2] as f32, dpr)),
            f64::from(client_size(content[3] as f32, dpr)),
        ];
        let parent = [
            f64::from(client_size(container[2] as f32, dpr)),
            f64::from(client_size(container[3] as f32, dpr)),
        ];
        let view = self.field3d.tabs.entry(key).or_default();
        view.inputs.set(CanvasInputs {
            client,
            rect_width: shown.canvas[2],
            parent: Some(parent),
            device_pixel_ratio: f64::from(dpr),
            dark,
            reduced_motion: platform::motion::prefers_reduced_motion(cx),
        });
        let image = view.presenter.image().cloned();

        let entity = cx.entity().downgrade();
        let origin = [content[0] as f32, content[1] as f32];
        let painter = canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                if let Some(image) = &image {
                    paint_frame(image, origin, window);
                }
                #[cfg(feature = "parity")]
                {
                    let presented = entity.upgrade().and_then(|frame| {
                        frame
                            .read(cx)
                            .field3d
                            .tabs
                            .get(&key)
                            .and_then(|v| v.presented.clone())
                    });
                    if let Some(presented) = presented {
                        asrs_ui::parity::record_gl(cx, || gl_dump(&presented, shown.canvas, mode));
                    }
                }
                #[cfg(not(feature = "parity"))]
                let _ = (&entity, cx, mode);
            },
        )
        .size_full();

        let mut canvas_el = local(shown.canvas, container)
            .id("field-3d-canvas")
            .on_any_mouse_down(cx.listener(|frame, event: &MouseDownEvent, _, cx| {
                frame.field3d_mouse_down(event, cx)
            }))
            .on_scroll(
                cx.listener(|frame, event: &ScrollEvent, _, cx| frame.field3d_wheel(event, cx)),
            );
        for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
            canvas_el = canvas_el.on_mouse_up(
                button,
                cx.listener(|frame, event: &MouseUpEvent, _, cx| {
                    frame.field3d_canvas_up(event, cx)
                }),
            );
        }
        if dom.canvas_fixed {
            canvas_el = canvas_el
                .border(px(shown.border as f32))
                .border_color(rgb(if dark { 0x999999 } else { 0x555555 }));
        }
        let mut container_el = local(container, rect)
            .overflow_hidden()
            .child(probed("canvas.field-3d-canvas", canvas_el.child(painter)));
        if !dom.alert_hidden {
            container_el =
                container_el.child(self.field3d_alert(&dom.alert_html, container, dark, scale, cx));
        }
        let mut root = div()
            .absolute()
            .left_0()
            .top_0()
            .size_full()
            .child(probed("div.field-3d-canvas-container", container_el));

        let view = self.field3d.tabs.get_mut(&key).expect("inserted above");
        let (opacity, spinner_scale, transitioning) = view.spinner.transition(now);
        if opacity > 0.0 {
            let angles = view
                .spinner
                .animating_since
                .map(|since| spinner_angles(now - since));
            let state = view
                .spinner_canvas
                .get_or_insert_with(|| cx.new(CanvasState::new))
                .clone();
            let side = SPINNER_SIZE * spinner_scale;
            let centre = [rect[2] / 2.0, rect[3] - SPINNER_BOTTOM - SPINNER_SIZE / 2.0];
            let painter = move |metrics: &CanvasMetrics, _: &mut App| {
                Some(spinner_canvas(metrics, angles, spinner_scale))
            };
            let spinner = div()
                .absolute()
                .left(px((centre[0] - side / 2.0) as f32))
                .top(px((centre[1] - side / 2.0) as f32))
                .w(px(side as f32))
                .h(px(side as f32))
                .opacity(opacity as f32)
                .child(canvas_view(state, painter).size_full());
            root = root.child(probed("div.spinner-cubes-container", spinner));
            if transitioning || angles.is_some() {
                window.request_animation_frame();
            }
        }
        Some(root.into_any_element())
    }

    fn field3d_alert(
        &self,
        html: &str,
        container: [f64; 4],
        dark: bool,
        scale: f32,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let style = alert_style();
        let lines: Vec<(String, f64, f64)> = html
            .split("</br>")
            .map(|line| {
                let (w, h) = text::measure(cx, &style, line, scale);
                (line.to_owned(), w, h)
            })
            .collect();
        let sizes: Vec<[f64; 2]> = lines.iter().map(|l| [l.1, l.2]).collect();
        let zoom = f64::from(scale);
        let ([left, top, w, h], places) = alert_layout(container[2], &sizes, zoom);
        let edge = border(1.0, zoom) / zoom;
        let mut alert = div()
            .absolute()
            .left(px(left as f32))
            .top(px(top as f32))
            .w(px(w as f32))
            .h(px(h as f32))
            .bg(rgb(if dark { 0x222222 } else { 0xffffff }))
            .border(px(edge as f32))
            .border_color(rgb(if dark { 0x999999 } else { 0x555555 }));
        for ((line, _, _), place) in lines.into_iter().zip(places) {
            let [x, y] = place.map(|v| v - edge);
            alert =
                alert.child(
                    div().absolute().left(px(x as f32)).top(px(y as f32)).child(
                        Text::new(line, style).color(if dark { 0xffffff } else { 0x000000 }),
                    ),
                );
        }
        probed("div.field-3d-alert", alert).into_any_element()
    }

    fn field3d_mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<'_, Self>) {
        let Some(shown) = self.field3d.shown else {
            return;
        };
        let x = mouse_client(f32::from(event.position.x));
        let y = mouse_client(f32::from(event.position.y));
        let pointer = PointerInput {
            pointer_id: MOUSE_POINTER_ID,
            buttons: buttons_of(event.button),
            client_x: x,
            client_y: y,
        };
        let button = button_number(event.button);
        self.host_mut().input(cx, |hub| {
            with_tab(hub, shown.index, |tab, _| {
                if let Some(implementation) = tab.renderer.implementation_mut() {
                    implementation
                        .controls_mut()
                        .pointer_down(pointer, shown.element_rect());
                }
                tab.renderer.mouse_down(button, x, y);
            });
        });
        cx.notify();
    }

    fn field3d_canvas_up(&mut self, event: &MouseUpEvent, cx: &mut Context<'_, Self>) {
        let Some(shown) = self.field3d.shown else {
            return;
        };
        let x = mouse_client(f32::from(event.position.x));
        let y = mouse_client(f32::from(event.position.y));
        let ask = self.host_mut().input(cx, |hub| {
            let (tabs, page) = hub.tabs_mut();
            let assets = page.shared_assets().cloned();
            let tab = tabs.tab_mut(shown.index)?;
            let renderer = tab.renderer.downcast_mut::<Field3dTab>()?;
            renderer.renderer.mouse_up(x, y, assets.as_deref())
        });
        if let Some(data) = ask {
            self.host_mut()
                .send_main_message(NamedMessage::new(HubToMain::Ask3dCamera, data), cx);
        }
        cx.notify();
    }

    /// camera-controls' `pointermove` and `pointerup` listeners on the document.
    pub(super) fn field3d_window_move(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(shown) = self.field3d.shown else {
            return;
        };
        let pointer = PointerInput {
            pointer_id: MOUSE_POINTER_ID,
            buttons: event.pressed_button.map_or(0, buttons_of),
            client_x: mouse_client(f32::from(event.position.x)),
            client_y: mouse_client(f32::from(event.position.y)),
        };
        self.host_mut().input(cx, |hub| {
            with_tab(hub, shown.index, |tab, _| {
                if let Some(implementation) = tab.renderer.implementation_mut()
                    && implementation.controls().is_listening_to_document()
                {
                    implementation.controls_mut().pointer_move(pointer);
                }
            });
        });
    }

    pub(super) fn field3d_window_up(&mut self, event: &MouseUpEvent, cx: &mut Context<'_, Self>) {
        let Some(shown) = self.field3d.shown else {
            return;
        };
        let pointer = PointerInput {
            pointer_id: MOUSE_POINTER_ID,
            buttons: 0,
            client_x: mouse_client(f32::from(event.position.x)),
            client_y: mouse_client(f32::from(event.position.y)),
        };
        self.host_mut().input(cx, |hub| {
            with_tab(hub, shown.index, |tab, _| {
                if let Some(implementation) = tab.renderer.implementation_mut() {
                    implementation.controls_mut().pointer_up(pointer);
                }
            });
        });
    }

    /// camera-controls' `wheel` listener: `WheelEvent.deltaY` is Chromium's wheel delta negated.
    fn field3d_wheel(&mut self, event: &ScrollEvent, cx: &mut Context<'_, Self>) {
        let Some(shown) = self.field3d.shown else {
            return;
        };
        let delta_y = -f64::from(platform::wheel_delta(event).y);
        self.host_mut().input(cx, |hub| {
            with_tab(hub, shown.index, |tab, _| {
                if let Some(implementation) = tab.renderer.implementation_mut() {
                    implementation.controls_mut().wheel(WheelInput {
                        delta_y,
                        delta_mode: 0,
                    });
                }
            });
        });
        cx.notify();
    }

    /// Every 3D renderer's window `keydown` and `keyup` listeners. A hidden tab's canvas
    /// container has no height, so it ignores key downs.
    pub(super) fn field3d_key(
        &mut self,
        key: &str,
        down: bool,
        command_modifier: bool,
        editable: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let code = key_code(key);
        let shown = self.field3d.shown;
        let tabs = field3d_tabs(self.host().hub(cx));
        self.host_mut().input(cx, |hub| {
            for (index, _) in tabs {
                let height = match shown {
                    Some(s) if s.index == index => s.container[3],
                    _ => 0.0,
                };
                with_tab(hub, index, |tab, _| {
                    if down {
                        tab.renderer
                            .key_down(&code, command_modifier, editable, height);
                    } else {
                        tab.renderer.key_up(&code, editable);
                    }
                });
            }
        });
    }

    pub(super) fn field3d_key_down(
        &mut self,
        event: &KeyDownEvent,
        editable: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let m = event.keystroke.modifiers;
        let command = if cfg!(target_os = "macos") {
            m.platform
        } else {
            m.control
        };
        self.field3d_key(&event.keystroke.key, true, command, editable, cx);
    }

    pub(super) fn field3d_key_up(
        &mut self,
        event: &KeyUpEvent,
        editable: bool,
        cx: &mut Context<'_, Self>,
    ) {
        self.field3d_key(&event.keystroke.key, false, false, editable, cx);
    }

    /// `div.field-3d-settings` over the right of `controls` (`div.controls-content`).
    pub(super) fn field3d_settings(
        &mut self,
        controls: [f64; 4],
        appearance: Appearance,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let hub = self.host().hub(cx);
        let tab = hub
            .tabs()
            .tabs()
            .enumerate()
            .find(|(_, t)| !t.content_hidden() && t.tab_type() == Some(TabType::Field3d));
        let (index, tab) = tab?;
        let renderer = tab.renderer().downcast_ref::<Field3dTab>()?;
        let key = tab_key(renderer);
        let panel_key = settings_panel::tab_key(&tab);
        let controller = tab.controller().downcast_ref::<Field3dController>()?;
        let page = hub.page();
        let names: Vec<(JsString, JsString, bool)> = page
            .assets()
            .map(|a| {
                a.field3ds
                    .iter()
                    .chain(built_in_3d_fields())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
            .into_iter()
            .map(|f| (f.id.clone(), f.name.clone(), f.is_ftc))
            .collect();
        let (frc, ftc) = controller.field_options();
        let label = |id: &JsString| {
            names
                .iter()
                .find(|(i, _, _)| i == id)
                .map_or_else(JsString::new, |n| n.1.clone())
        };
        let options: Vec<(JsString, JsString, bool)> = frc
            .iter()
            .map(|id| (id.clone(), label(id), false))
            .chain(ftc.iter().map(|id| (id.clone(), label(id), true)))
            .collect();
        let value = controller.field_value();
        let dark = appearance == Appearance::Dark;
        let theme = appearance.theme();

        let select_w = {
            let mut style = TextStyle::new(CONTROL_FONT);
            style.white_space = WhiteSpace::Pre;
            let widest = options
                .iter()
                .map(|o| {
                    let indented = format!("    {}", o.1.to_string_lossy());
                    text::measure(cx, &style, &indented, scale).0
                })
                .fold(0.0, math::max2);
            let device = f64::from(scale);
            math::ceil(widest * device) / device + 4.0 + 1.0 + 15.0 + 2.0
        };
        let layout = SettingsLayout::new(
            controls,
            scroll::style(cx).reserved(),
            select_w,
            f64::from(scale),
        );
        let settings = layout.panel;
        let content = [settings[0] + layout.border, settings[1]];
        let [select_x, select_y, ..] = layout.select;

        let view = self.field3d.tabs.entry(key).or_default();
        if view.select.is_none() {
            let select = cx.new(|cx| Select::new(FormSelect::default(), window, cx));
            cx.subscribe(&select, move |frame, select, event: &SelectEvent, cx| {
                if *event != SelectEvent::Change {
                    return;
                }
                let value = select.read(cx).model().select().value();
                let result = frame.host_mut().input(cx, |hub| {
                    with_controller(hub, index, |controller, _, page| {
                        controller.set_field_value(&value);
                        controller.field_select_change(page)
                    })
                });
                if let Some(Err(e)) = result {
                    eprintln!("asrs: {e:?}");
                }
                cx.notify();
            })
            .detach();
            view.select = Some(select);
        }
        let select = view.select.clone().expect("created above");
        if view.options != options {
            view.options = options.clone();
            let mut model = form::Select::new();
            model.append_group(OptGroup::new("FRC Fields"));
            model.append_group(OptGroup::new("FTC Fields"));
            for (id, name, ftc) in &options {
                let option = SelectOption::from_inner_text(name).with_value(id);
                model.append_to_group(usize::from(*ftc), option);
            }
            select.update(cx, |s, cx| {
                s.update_model(|m| *m.select_mut() = model, cx);
            });
        }
        select.update(cx, |s, cx| {
            if s.model().select().value() != value {
                s.update_model(|m| m.select_mut().set_value(&value), cx);
            }
            s.set_size(select_w as f32, SELECT_HEIGHT as f32);
            s.set_subpixel(
                snap_remainder(select_x as f32, scale),
                snap_remainder(select_y as f32, scale),
            );
        });

        let mut title = |text: &'static str, i: usize, window: &mut Window| {
            let place = focus::place((text, key), window, cx);
            let (w, _) = text::measure(cx, &title_style(), text, scale);
            let [left, top, width, _] = layout.titles[i];
            let x = centred_offset(width, w, f64::from(scale)).unwrap_or(0.0);
            let (abs_x, abs_y) = ((left + x) as f32, top as f32);
            div()
                .absolute()
                .left(px(x as f32))
                .top(px((top - content[1]) as f32))
                .track_focus(&place)
                .child(
                    Text::new(text.to_owned(), title_style())
                        .color(theme.text)
                        .subpixel(snap_remainder(abs_x, scale), snap_remainder(abs_y, scale)),
                )
        };
        let field_title = title("Field", 0, window);
        let xr_title = title("XR", 1, window);

        let button = layout.button;
        let side = math::min2(layout.image[2], layout.image[3]);
        let [img_x, img_y] = [0, 1].map(|i| {
            let [at, size] = [layout.image[i], layout.image[i + 2]];
            at - button[i] - layout.border + (size - side) / 2.0
        });
        let (bg, edge) = if dark {
            (0x444444, 0x666666)
        } else {
            (0xeeeeee, 0xaaaaaa)
        };
        let focus = FocusRing::of(("xr-button-focus", key), None, window, cx);
        let radius = px(10.0);
        let click = move |frame: &mut HubFrame<H>, cx: &mut Context<'_, HubFrame<H>>| {
            frame.host_mut().input(cx, |hub| {
                with_controller(hub, index, |controller, uuid, page| {
                    controller.xr_click(page, uuid)
                })
            });
            cx.notify();
        };
        let mut xr = div()
            .id("xr-button")
            .track_focus(&focus.handle)
            .absolute()
            .left(px((button[0] - content[0]) as f32))
            .top(px((button[1] - content[1]) as f32))
            .w(px(button[2] as f32))
            .h(px(button[3] as f32))
            .bg(rgb(bg))
            .border(px(layout.border as f32))
            .border_color(rgb(edge))
            .rounded(radius)
            .on_click(cx.listener(move |frame, event: &ClickEvent, _, cx| {
                // GPUI clicks a focused element on key up, `setupKeyboardControls` on `keydown`.
                if !matches!(event, ClickEvent::Keyboard(_)) {
                    click(frame, cx);
                }
            }))
            .on_key_down(cx.listener(move |frame, event: &KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    click(frame, cx);
                }
            }));
        if let Some(icon) = xr_icon() {
            xr = xr.child(untitled(
                div()
                    .absolute()
                    .left(px(img_x as f32))
                    .top(px(img_y as f32))
                    .w(px(side as f32))
                    .h(px(side as f32))
                    .child(Icon::new(icon, chains::XR_BUTTON).fit(IconFit::Natural)),
            ));
        }

        xr = xr.children(focus.visible.then(|| {
            focus::outline(appearance)
                .inset(px(-FOCUS_RING_WIDTH - 1.0))
                .rounded(radius + px(FOCUS_RING_WIDTH))
        }));

        let select_el = div()
            .absolute()
            .left(px((select_x - content[0]) as f32))
            .top(px((select_y - content[1]) as f32))
            .w(px(select_w as f32))
            .h(px(SELECT_HEIGHT as f32))
            .child(select);
        let content = div()
            .child(field_title)
            .child(probed(
                "select.field-select",
                titled("Select field model", select_el),
            ))
            .child(xr_title)
            .child(probed("div.xr-button", titled("Start XR streaming", xr)));
        let panel = SettingsPanel {
            key: panel_key,
            probe: "div.field-3d-settings",
            controls,
            rect: settings,
            border: layout.border,
            content,
            content_height: layout.content_height,
        };
        Some(self.settings_scroller(panel, appearance, scale, window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::control_layout_vectors;

    #[test]
    fn key_codes() {
        assert_eq!(key_code("w"), "KeyW");
        assert_eq!(key_code("7"), "Digit7");
        assert_eq!(key_code("space"), "Space");
        assert_eq!(key_code("up"), "ArrowUp");
    }

    #[test]
    fn spinner_keyframes_repeat() {
        assert_eq!(spinner_angles(0.0), [0.0, -35.0, 45.0]);
        assert_eq!(spinner_angles(1500.0), [180.0, -215.0, -135.0]);
        assert_eq!(spinner_angles(6000.0), [0.0, -35.0, 45.0]);
    }

    #[test]
    fn unrotated_cube_shows_its_front_face_last() {
        let faces = spinner_faces(None);
        assert_eq!(faces.len(), 6);
        let (front, _) = faces[5];
        for p in front.iter().flatten() {
            assert!(p.abs() < 1e-9 || (p - 44.0).abs() < 1e-9, "{front:?}");
        }
    }

    #[test]
    fn canvas_sizes_are_percentages() {
        assert_eq!(share("75%"), Some(75.0));
        assert_eq!(share(""), None);
        assert_eq!(share("NaN%"), None);
    }

    #[test]
    fn controls_lie_where_blink_puts_them() {
        control_layout_vectors::each(3.0, 1549, |blink| {
            let scale = blink.scale;
            let select = blink.one("div.setting-blocks select", "field-select").rect;
            let layout = SettingsLayout::new(blink.controls, 0.0, select[2], scale);
            let panel = blink.all("div.setting-blocks")[0].rect;
            blink.check("panel", &layout.panel, &panel);
            let blocks = [0, 2].map(|i| blink.all("div.setting-blocks > div")[i].rect);
            for (title, block) in layout.titles.iter().zip(blocks) {
                blink.check("title", title, &block);
            }
            blink.check("select", &layout.select, &select);
            let button = blink.all("div.setting-blocks div.xr-button")[0].rect;
            blink.check("button", &layout.button, &button);
            let image = blink.all("div.setting-blocks div.xr-button img")[0].rect;
            blink.check("image", &layout.image, &image);
            let [x, y, w, h] = blink.renderer;
            let alert = blink.all("div.field-3d-alert")[0].clone();
            let sizes: Vec<_> = alert.lines.iter().map(|l| [l[2], l[3]]).collect();
            // In half a narrow container the alert wraps, which the port's does not.
            if sizes.len() == 2 {
                let ([left, top, width, height], lines) = alert_layout(w, &sizes, scale);
                let at = [x + left, y - CONTAINER_EXTRA_FIXED + top, width, height];
                blink.check("alert", &at, &alert.rect);
                for (ours, line) in lines.iter().zip(&alert.lines) {
                    let ours = [at[0] + ours[0], at[1] + ours[1]];
                    blink.check("alert text", &ours, &line[..2]);
                }
            }
            let canvas = blink.all("canvas.field-3d-canvas")[0].rect;
            let container = [x, y - CONTAINER_EXTRA_FIXED, w, h + CONTAINER_EXTRA_FIXED];
            let style = blink.canvas_style.clone().expect("a fixed canvas");
            let ours = fixed_canvas(container, [&style[0], &style[1]], scale);
            blink.check("canvas", &ours, &canvas);
        });
    }
}
