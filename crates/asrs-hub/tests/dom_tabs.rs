#![allow(clippy::disallowed_methods)]

//! Replays `parity/specs/dom/tabs/<case>.entry.json` against `asrs_hub::tabs::Tabs` built
//! through the production `asrs_hub::factory::HubTabFactory`, so every tab runs its real
//! controller and, where ported, its real renderer; the oracle's `field3dStub.ts` stands in for
//! `Field3dRenderer` on both sides. Emitted records are compared with the committed
//! `dom/tabs/<case>` vector, line for line; `NOT_COMPARED` lists the records whose payload is
//! not.
//!
//! The table fixtures (`TABLE_FIXTURES`) ran on Blink with real geometry: the tab's layout reads
//! and scroll writes are answered from and checked against the recorded ones, which the port's
//! `TableLayout` must also predict. Events the harness does not model fail the run.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;
#[path = "../../asrs-tabs/tests/support/table_geometry.rs"]
mod table_geometry;

use std::cell::{Cell, Ref, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use asrs_canvas::Canvas2d;
use asrs_hub::factory::{HubTabFactory, RendererHook};
use asrs_hub::hub::assets_alert;
use asrs_hub::tabs::{Tabs, TabsHost};
use asrs_js::ops;
use asrs_js::string::{JsError, JsString};
use asrs_js::time::{Clock, Rng};
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::log::Log;
use asrs_log::log::LoggableType;
use asrs_model::assets::AdvantageScopeAssets;
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::preferences::Preferences;
use asrs_model::selection::Selection;
use asrs_model::selection_impl::{KeyTarget, SelectionImpl};
use asrs_model::source_list::{ListInput, ListPointer, Rect, SourceList};
use asrs_model::tab_controller::{TabCommand, TabController, TabRenderer, keyboard_clicks};
use asrs_model::tab_type::TabType;
use asrs_parity::cve1::Value;
use asrs_parity::geometry::{GeometryRecord, ScriptedReads};
use asrs_parity::json::{self, Json};
use asrs_parity::tally::{Fields, Tally};
use asrs_parity::utf16::Utf16String;
use asrs_parity::vectors::{self, Record};
use asrs_tabs::console::controller::{
    ConsoleController, ConsoleDom, ConsoleGeometry, ConsoleRenderer,
};
use asrs_tabs::console::view::ConsoleRow;
use asrs_tabs::cx::TabCx;
use asrs_tabs::field_2d::{Field2dController, Field2dDom, Field2dRenderer};
use asrs_tabs::field3d::Field3dController;
use asrs_tabs::joysticks::JoysticksController;
use asrs_tabs::joysticks_renderer::JoysticksRenderer;
use asrs_tabs::line_graph::{LineGraphController, ListId};
use asrs_tabs::list_cx::{ListParts, SourceListCx, js_error, with_env};
use asrs_tabs::mechanism::MechanismController;
use asrs_tabs::mechanism_renderer::MechanismRenderer;
use asrs_tabs::points::{Dimension, PointsController, PointsSelect};
use asrs_tabs::points_renderer::PointsRenderer;
use asrs_tabs::statistics::{StatisticsController, StatisticsCx};
use asrs_tabs::swerve::{SwerveController, SwerveInput};
use asrs_tabs::swerve_renderer::SwerveRenderer;
use asrs_tabs::table::{TableController, TableDom, TableRenderer};
use asrs_tabs::video::{VideoController, VideoCx};
use table_geometry::SharedTable;

fn err(message: impl Into<String>) -> String {
    message.into()
}

fn js(s: &str) -> JsString {
    JsString::from(s)
}

fn cve_str(s: &str) -> Value {
    js_json::from_js(&JsValue::String(js(s)))
}

fn to_js(json: &Json) -> Result<JsValue, String> {
    js_json::to_js(&Value::from_json_lenient(json))
}

fn truthy(v: &Value) -> bool {
    !matches!(v, Value::Null | Value::Undefined | Value::Bool(false))
        && v.as_f64().is_none_or(|n| n != 0.0 && !n.is_nan())
        && v.as_str().is_none_or(|s| !s.is_empty())
}

struct DomRng(u32);

impl Rng for DomRng {
    fn next_f64(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        (t ^ (t >> 14)) as f64 / 4294967296.0
    }
}

struct ScriptedClock(Rc<Cell<f64>>);

impl Clock for ScriptedClock {
    fn now(&self) -> f64 {
        self.0.get()
    }
}

/// `el.matches(selector)` with descendant combinators, `.class` and `tag` compounds plus
/// `:nth-of-type(...)`, on the small element trees the tab renderers own.
struct ElDesc {
    tag: &'static str,
    classes: &'static [&'static str],
    hidden: bool,
    /// Position among same-tag siblings, for `:nth-of-type`.
    pos: usize,
    /// Position among all siblings, for `:nth-child`, where the tree knows it.
    child_pos: Option<usize>,
    parent: Option<Box<ElDesc>>,
}

fn el(
    tag: &'static str,
    classes: &'static [&'static str],
    hidden: bool,
    pos: usize,
    parent: ElDesc,
) -> ElDesc {
    ElDesc {
        tag,
        classes,
        hidden,
        pos,
        child_pos: None,
        parent: Some(Box::new(parent)),
    }
}

fn matches_compound(el: &ElDesc, compound: &str) -> bool {
    let (compound, nth, pos) = if let Some((head, arg)) = compound.split_once(":nth-of-type(") {
        (head, Some(arg.trim_end_matches(')')), Some(el.pos))
    } else if let Some((head, arg)) = compound.split_once(":nth-child(") {
        (head, Some(arg.trim_end_matches(')')), el.child_pos)
    } else {
        (compound, None, None)
    };
    let mut parts = compound.split('.');
    let tag = parts.next().unwrap_or("");
    if !tag.is_empty() && tag != "*" && tag != el.tag {
        return false;
    }
    for class in parts {
        if !el.classes.iter().any(|c| c == &class) {
            return false;
        }
    }
    if let Some(arg) = nth {
        let Some(pos) = pos else {
            return false;
        };
        let arg = arg.trim();
        let pos = pos as f64;
        let ok = match arg {
            "odd" => pos % 2.0 == 1.0,
            "even" => pos % 2.0 == 0.0,
            _ if arg.contains('n') => {
                let mut it = arg.splitn(2, 'n');
                let a_raw = it.next().unwrap_or("").trim().trim_end_matches('+');
                let a = match a_raw {
                    "" | "+" => 1.0,
                    "-" => -1.0,
                    _ => a_raw.parse::<f64>().unwrap_or(f64::NAN),
                };
                let b = it
                    .next()
                    .unwrap_or("0")
                    .trim()
                    .parse::<f64>()
                    .unwrap_or(0.0);
                let k = (pos - b) / a;
                a != 0.0 && k >= 0.0 && k.fract() == 0.0
            }
            _ => pos == arg.parse::<f64>().unwrap_or(f64::NAN),
        };
        if !ok {
            return false;
        }
    }
    true
}

fn matches_selector(el: &ElDesc, selector: &str) -> bool {
    let compounds: Vec<&str> = selector.split_whitespace().collect();
    let Some(last) = compounds.last() else {
        return false;
    };
    if !matches_compound(el, last) {
        return false;
    }
    let mut cursor = el.parent.as_deref();
    for compound in compounds[..compounds.len() - 1].iter().rev() {
        let mut found = false;
        while let Some(parent) = cursor {
            if matches_compound(parent, compound) {
                found = true;
                cursor = parent.parent.as_deref();
                break;
            }
            cursor = parent.parent.as_deref();
        }
        if !found {
            return false;
        }
    }
    true
}

fn is_hidden(el: &ElDesc) -> bool {
    let mut cursor = Some(el);
    while let Some(e) = cursor {
        if e.hidden {
            return true;
        }
        cursor = e.parent.as_deref();
    }
    false
}

/// `env.ts::rectOf`: fixture rules first, then the defaults, first match wins. A rule without
/// `rect` resolves to the parent's rect.
struct Layout {
    rules: Vec<(String, Option<[f64; 4]>)>,
}

impl Layout {
    fn rect_of(&self, el: &ElDesc) -> Rect {
        if is_hidden(el) {
            return Rect::default();
        }
        for (selector, rect) in &self.rules {
            if !matches_selector(el, selector) {
                continue;
            }
            if let Some(rect) = rect {
                return Rect {
                    x: rect[0],
                    y: rect[1],
                    width: rect[2],
                    height: rect[3],
                };
            }
            return el
                .parent
                .as_deref()
                .map(|p| self.rect_of(p))
                .unwrap_or_default();
        }
        Rect::default()
    }
}

fn root_el(hidden: bool, pos: usize) -> ElDesc {
    let renderer_content = ElDesc {
        tag: "div",
        classes: &["renderer-content"],
        hidden: false,
        pos: 1,
        child_pos: None,
        parent: None,
    };
    el("div", &[], hidden, pos, renderer_content)
}

fn console_container_el(hidden: bool, pos: usize) -> ElDesc {
    el(
        "div",
        &["console-table-container"],
        false,
        2,
        root_el(hidden, pos),
    )
}

const CONSOLE_CONTAINER: &str = "div.console-table-container:2";
const CONSOLE_BODY: &str = "div.console-table-container:2>table.console-table:0>tbody:0";

/// Answers a Console tab's layout reads from the recording.
#[derive(Clone)]
struct RecordedConsole(Rc<RefCell<ScriptedReads>>);

impl ConsoleGeometry for RecordedConsole {
    fn container_rect(&mut self) -> Rect {
        let [x, y, width, height] = self.0.borrow_mut().read_rect(CONSOLE_CONTAINER);
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn scroll_top(&mut self, _: &[ConsoleRow]) -> f64 {
        self.0.borrow_mut().read_f64("scrollTop", CONSOLE_CONTAINER)
    }

    fn set_scroll_top(&mut self, _: &[ConsoleRow], value: f64) {
        let mut reads = self.0.borrow_mut();
        reads.write_f64("scrollTop", CONSOLE_CONTAINER, value);
    }

    fn scroll_height(&mut self, _: &[ConsoleRow]) -> f64 {
        let mut reads = self.0.borrow_mut();
        reads.read_f64("scrollHeight", CONSOLE_CONTAINER)
    }

    fn container_client_height(&mut self) -> f64 {
        let mut reads = self.0.borrow_mut();
        reads.read_f64("clientHeight", CONSOLE_CONTAINER)
    }

    fn lay_out(&mut self, _: &[ConsoleRow]) {}

    fn row_client_height(&mut self, index: usize) -> f64 {
        let mut reads = self.0.borrow_mut();
        reads.read_f64("clientHeight", &format!("{CONSOLE_BODY}>tr:{}", index + 1))
    }
}

/// `div.controls-content > #controller7 > .video-source > button`: the three source
/// buttons of the selected (visible) Video tab. `button_pos` is 1-based among the
/// same-tag siblings, matching `sourceSection.children[n]`.
fn video_source_button_el(button_pos: usize) -> ElDesc {
    let controls = ElDesc {
        tag: "div",
        classes: &["controls-content"],
        hidden: false,
        pos: 1,
        child_pos: None,
        parent: None,
    };
    let controller_root = el("div", &[], false, 1, controls);
    let source = el("div", &["video-source"], false, 1, controller_root);
    el("button", &[], false, button_pos, source)
}

fn field_2d_container_el(hidden: bool, pos: usize) -> ElDesc {
    el(
        "div",
        &["field-2d-canvas-container"],
        false,
        1,
        root_el(hidden, pos),
    )
}

fn field_2d_scroll_el(hidden: bool, pos: usize) -> ElDesc {
    el("div", &["field-2d-scroll"], false, 2, root_el(hidden, pos))
}

#[derive(Clone)]
enum DomKind {
    Table(Rc<RefCell<TableDom>>),
    Console(Rc<RefCell<ConsoleDom>>),
    Field2d(Rc<RefCell<Field2dDom>>),
    Other,
}

/// Wraps every renderer the production factory builds, recording the last command it drew.
struct Recorder {
    inner: Box<dyn TabRenderer<Host>>,
    slot: Rc<RefCell<Option<JsValue>>>,
    drawing: Rc<RefCell<Option<Drawing>>>,
}

impl TabRenderer<Host> for Recorder {
    fn save_state(&self, cx: &Host) -> JsValue {
        self.inner.save_state(cx)
    }

    fn restore_state(&mut self, cx: &mut Host, state: &JsValue) -> Result<(), JsError> {
        self.inner.restore_state(cx, state)
    }

    fn aspect_ratio(&self, cx: &Host) -> Option<f64> {
        self.inner.aspect_ratio(cx)
    }

    fn render(&mut self, cx: &mut Host, command: &dyn TabCommand) -> Result<(), JsError> {
        *self.slot.borrow_mut() = Some(command.to_js());
        let result = self.inner.render(cx, command);
        if let Some((class, canvas)) = take_painted(&mut *self.inner) {
            let mut drawing = self.drawing.borrow_mut();
            let frames = drawing.as_ref().map_or(0, |d| d.frames) + 1;
            *drawing = Some(Drawing {
                class,
                frames,
                canvas,
                drawn_since_capture: true,
            });
        }
        result
    }

    fn renderer_uuid(&self) -> Option<&JsString> {
        self.inner.renderer_uuid()
    }

    fn timestamp_range(&self, cx: &Host) -> Option<[f64; 2]> {
        self.inner.timestamp_range(cx)
    }
}

fn inner_ref<T: 'static>(renderer: &dyn TabRenderer<Host>) -> Option<&T> {
    renderer
        .downcast_ref::<Recorder>()?
        .inner
        .downcast_ref::<T>()
}

fn inner_mut<T: 'static>(renderer: &mut dyn TabRenderer<Host>) -> Option<&mut T> {
    renderer
        .downcast_mut::<Recorder>()?
        .inner
        .downcast_mut::<T>()
}

/// The DOM mirror a tab's renderer draws into, found through the renderer so a restored
/// tab's new UUID does not lose it.
fn dom_of(renderer: &dyn TabRenderer<Host>) -> DomKind {
    if let Some(r) = inner_ref::<TableRenderer>(renderer) {
        DomKind::Table(r.dom().clone())
    } else if let Some(r) = inner_ref::<ConsoleRenderer>(renderer) {
        DomKind::Console(r.dom().clone())
    } else if let Some(r) = inner_ref::<Field2dRenderer>(renderer) {
        DomKind::Field2d(r.dom().clone())
    } else {
        DomKind::Other
    }
}

/// The oracle swaps `Field3dRenderer` for `oracle/src/dom/field3dStub.ts`, so the harness swaps
/// the production placeholder for the same stub.
struct Hook;

impl RendererHook<Host> for Hook {
    fn renderer(
        &mut self,
        _cx: &mut Host,
        kind: Option<TabType>,
        renderer: Box<dyn TabRenderer<Host>>,
    ) -> Box<dyn TabRenderer<Host>> {
        let inner: Box<dyn TabRenderer<Host>> = match kind {
            Some(TabType::Field3d) => Box::new(Field3dStubRenderer {
                state: JsValue::Null,
            }),
            Some(TabType::Mechanism) if renderer.downcast_ref::<MechanismRenderer>().is_none() => {
                Box::new(MissingPainter("MechanismRenderer"))
            }
            Some(TabType::Points) if renderer.downcast_ref::<PointsRenderer>().is_none() => {
                Box::new(MissingPainter("PointsRenderer"))
            }
            Some(TabType::Swerve) if renderer.downcast_ref::<SwerveRenderer>().is_none() => {
                Box::new(MissingPainter("SwerveRenderer"))
            }
            Some(TabType::Joysticks) if renderer.downcast_ref::<JoysticksRenderer>().is_none() => {
                Box::new(MissingPainter("JoysticksRenderer"))
            }
            _ => renderer,
        };
        Box::new(Recorder {
            inner,
            slot: Rc::new(RefCell::new(None)),
            drawing: Rc::new(RefCell::new(None)),
        })
    }
}

/// Stands in for a painter the production factory does not build yet; every render fails,
/// so the case shows an exception upstream never recorded.
struct MissingPainter(&'static str);

impl<Cx: ?Sized> TabRenderer<Cx> for MissingPainter {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        JsValue::Null
    }

    fn restore_state(&mut self, _cx: &mut Cx, _state: &JsValue) -> Result<(), JsError> {
        Ok(())
    }

    fn aspect_ratio(&self, _cx: &Cx) -> Option<f64> {
        None
    }

    fn render(&mut self, _cx: &mut Cx, _command: &dyn TabCommand) -> Result<(), JsError> {
        Err(JsError::new(
            "Unmodeled",
            format!("the production factory builds no {}", self.0),
        ))
    }
}

/// `oracle/src/dom/field3dStub.ts`: keeps whatever `restoreState` was given (`null` before)
/// and draws nothing.
struct Field3dStubRenderer {
    state: JsValue,
}

impl<Cx: TabCx + ?Sized> TabRenderer<Cx> for Field3dStubRenderer {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        self.state.clone()
    }

    fn restore_state(&mut self, _cx: &mut Cx, state: &JsValue) -> Result<(), JsError> {
        self.state = state.clone();
        Ok(())
    }

    fn aspect_ratio(&self, _cx: &Cx) -> Option<f64> {
        None
    }

    fn render(&mut self, _cx: &mut Cx, _command: &dyn TabCommand) -> Result<(), JsError> {
        Ok(())
    }
}

struct Host {
    log: Rc<RefCell<Log>>,
    selection: SelectionImpl,
    preferences: Option<Preferences>,
    rng: DomRng,
    inner_height: f64,
    renderer_top: f64,
    /// `sendMainMessage` calls in order; `open-xr` is outside `HubToMain`.
    messages: Vec<NamedMessage<&'static str>>,
    /// `window.assets`: the raw `set-assets` payload for `JSON.stringify` compare, and
    /// the parsed view controllers read.
    assets_raw: JsValue,
    assets: Option<AdvantageScopeAssets>,
    /// `window.typeMemory`.
    type_memory: JsValue,
    dark: bool,
    /// `new Date().getTime()` — the same scripted clock the selection reads.
    clock: Rc<Cell<f64>>,
}

impl VideoCx for Host {
    fn now(&self) -> f64 {
        self.clock.get()
    }
}

impl StatisticsCx for Host {
    fn now(&self) -> f64 {
        self.clock.get()
    }
}

impl TabCx for Host {
    fn tab_log(&self) -> Ref<'_, Log> {
        self.log.borrow()
    }

    fn tab_selection(&mut self) -> &mut dyn Selection {
        &mut self.selection
    }

    fn tab_preferences(&self) -> Option<&Preferences> {
        self.preferences.as_ref()
    }

    fn tab_preferences_mut(&mut self) -> Option<&mut Preferences> {
        self.preferences.as_mut()
    }

    fn tab_assets(&self) -> Option<&AdvantageScopeAssets> {
        self.assets.as_ref()
    }

    fn tab_rng(&mut self) -> &mut dyn Rng {
        &mut self.rng
    }

    fn send_main_message(&mut self, message: NamedMessage<HubToMain>) {
        self.messages
            .push(NamedMessage::new(message.name.name(), message.data));
    }

    fn send_open_xr(&mut self, uuid: JsValue) {
        self.messages.push(NamedMessage::new("open-xr", uuid));
    }
}

impl SourceListCx for Host {
    fn tab_dark(&self) -> bool {
        self.dark
    }

    fn list_parts(&mut self) -> ListParts<'_> {
        ListParts {
            log: self.log.borrow(),
            type_memory: &mut self.type_memory,
            selection: &mut self.selection,
            dark: self.dark,
            dom: asrs_model::source_list::Undisplayed::dom(),
        }
    }
}

/// `window.assets = message.data`, parsed into the typed assets controllers read. An entry
/// the config parsers drop fails the run.
fn assets_from_js(data: &JsValue) -> Result<Option<AdvantageScopeAssets>, String> {
    let JsValue::Object(o) = data else {
        return match data {
            JsValue::Null => Ok(None),
            _ => Err(format!("set-assets with {data:?} is not modeled")),
        };
    };
    let list = |key: &str| match o.get(key) {
        Some(JsValue::Array(items)) => Ok(items.len()),
        _ => Err(format!("set-assets without a {key} array is not modeled")),
    };
    list("loadFailures")?;
    let assets = AdvantageScopeAssets::from_js(data);
    for (key, parsed) in [
        ("field2ds", assets.field2ds.len()),
        ("field3ds", assets.field3ds.len()),
        ("robots", assets.robots.len()),
        ("joysticks", assets.joysticks.len()),
    ] {
        if list(key)? != parsed {
            return Err(format!("set-assets {key} has an entry that does not parse"));
        }
    }
    Ok(Some(assets))
}

impl TabsHost for Host {
    fn inner_height(&self) -> f64 {
        self.inner_height
    }

    fn renderer_top(&self, _show_timeline: bool) -> f64 {
        self.renderer_top
    }
}

enum Emit {
    Record {
        values: Vec<(&'static str, Value)>,
    },
    /// One `canvas` capture: what the port's painters drew since the last one.
    Canvases(Vec<Drawing>),
}

/// A `RecordingContext`: the last frame on a canvas, the number of frames so far and whether
/// the next capture records it.
#[derive(Clone)]
struct Drawing {
    class: &'static str,
    frames: usize,
    canvas: Canvas2d,
    drawn_since_capture: bool,
}

const COMPARED_CANVASES: [&str; 3] = ["swerve-canvas", "joysticks-canvas", "field-2d-canvas"];

fn take_painted(renderer: &mut dyn TabRenderer<Host>) -> Option<(&'static str, Canvas2d)> {
    let (class, painted) = if let Some(r) = renderer.downcast_mut::<SwerveRenderer>() {
        ("swerve-canvas", r.dom().borrow_mut().painted.take())
    } else if let Some(r) = renderer.downcast_mut::<JoysticksRenderer>() {
        ("joysticks-canvas", r.dom().borrow_mut().painted.take())
    } else {
        let r = renderer.downcast_mut::<Field2dRenderer>()?;
        ("field-2d-canvas", r.dom().borrow_mut().painted.take())
    };
    painted.map(|canvas| (class, canvas))
}

fn pair_value(pair: Option<[f64; 2]>) -> Value {
    pair.map_or(Value::Null, |[a, b]| {
        Value::Array(vec![Value::number(a), Value::number(b)])
    })
}

fn selection_value(sel: &mut SelectionImpl) -> Value {
    let nullable = |t: Option<f64>| t.map_or(Value::Null, Value::number);
    let controls = sel.controls();
    let range = sel.get_timeline_range();
    Value::object([
        ("mode", Value::number(sel.get_mode().number())),
        ("hoveredTime", nullable(sel.get_hovered_time())),
        ("selectedTime", nullable(sel.get_selected_time())),
        ("renderTime", nullable(sel.get_render_time())),
        ("currentLiveTime", nullable(sel.get_current_live_time())),
        ("grabZoomRange", pair_value(sel.get_grab_zoom_range())),
        (
            "timelineRange",
            Value::Array(vec![Value::number(range[0]), Value::number(range[1])]),
        ),
        (
            "timelineIsMaxZoom",
            Value::Bool(sel.get_timeline_is_max_zoom()),
        ),
        (
            "buttons",
            Value::object([
                ("play", Value::Bool(controls.play_hidden)),
                ("pause", Value::Bool(controls.pause_hidden)),
                ("lock", Value::Bool(controls.lock_hidden)),
                ("unlock", Value::Bool(controls.unlock_hidden)),
                (
                    "showLockButtons",
                    Value::string(match controls.show_lock_buttons {
                        None => "",
                        Some(true) => "1",
                        Some(false) => "0",
                    }),
                ),
            ]),
        ),
    ])
}

struct Runner {
    layout: Layout,
    host: Host,
    tabs: Tabs<Host>,
    emitted: Vec<Emit>,
    clock: Rc<Cell<f64>>,
    now: f64,
    start: f64,
    last_frame: f64,
    frame_period: f64,
    show_pending: bool,
    frame_loop_dead: bool,
    active_drag: Option<JsValue>,
    /// Tabs (by controller address) in `.controls-content` order. `addTab` splices the tab in
    /// after the selected one but appends its controls, and `shiftSelected` moves only the
    /// tab, so a selector's match index follows creation order, not `tabList`.
    controls_order: Vec<usize>,
    /// The Table tab of a real-geometry fixture.
    table: Option<SharedTable>,
    sidebar_width: f64,
    sidebar_expanded: Vec<JsString>,
    ignored: Vec<String>,
    device_pixel_ratio: f64,
    images: HashMap<JsString, [f64; 2]>,
    /// The console containers' layout accesses of a real-geometry fixture.
    console_reads: Option<RecordedConsole>,
}

impl Runner {
    fn emit_main(&mut self, name: &str, data: Value) {
        if self.ignored.iter().any(|i| i == name) {
            return;
        }
        self.emitted.push(Emit::Record {
            values: vec![("main", cve_str(name)), ("data", data)],
        });
    }

    fn exception(&mut self, where_: String, error: impl Into<String>) {
        self.emitted.push(Emit::Record {
            values: vec![
                ("exception", cve_str(&where_)),
                ("error", cve_str(&error.into())),
            ],
        });
    }

    fn tab_dom(&self, index: usize) -> DomKind {
        self.tabs
            .tab(index)
            .map_or(DomKind::Other, |t| dom_of(t.renderer()))
    }

    /// The DOM state each renderer's shared `dom` mirrors: hidden flags, resolved rects.
    fn sync_dom(&mut self) {
        for i in 0..self.tabs.len() {
            let Some(tab) = self.tabs.tab(i) else {
                continue;
            };
            let hidden = tab.content_hidden();
            if tab.uuid().as_str().is_none() {
                continue;
            }
            let pos = i + 1;
            match self.tab_dom(i) {
                DomKind::Table(dom) => {
                    let mut dom = dom.borrow_mut();
                    dom.hidden = hidden;
                    if let (None, Some(table)) = (&dom.geometry, &self.table) {
                        dom.geometry = Some(Box::new(table.clone()));
                    }
                }
                DomKind::Console(dom) => {
                    let mut dom = dom.borrow_mut();
                    dom.hidden = hidden;
                    if let Some(reads) = self.console_reads.clone() {
                        let index = JsValue::Number(i as f64);
                        if let Ok(r) = self.tabs.renderer_mut(&index)
                            && let Some(r) = inner_mut::<ConsoleRenderer>(r)
                        {
                            r.set_geometry(Box::new(reads.clone()));
                        }
                        if let Ok(c) = self.tabs.controller_mut(&index)
                            && let Some(c) = c.downcast_mut::<ConsoleController>()
                        {
                            c.set_geometry(Box::new(reads));
                        }
                        continue;
                    }
                    dom.container = self.layout.rect_of(&console_container_el(hidden, pos));
                    dom.scroll_height = asrs_js::math::round(dom.container.height);
                }
                DomKind::Field2d(dom) => {
                    let mut dom = dom.borrow_mut();
                    let container = self.layout.rect_of(&field_2d_container_el(hidden, pos));
                    let scroll = self.layout.rect_of(&field_2d_scroll_el(hidden, pos));
                    dom.container = [container.x, container.y, container.width, container.height];
                    dom.scroll_container = [scroll.x, scroll.y, scroll.width, scroll.height];
                    dom.device_pixel_ratio = self.device_pixel_ratio;
                    dom.images = self.images.clone();
                }
                DomKind::Other => {}
            }
            let mechanism = self.layout.rect_of(&el(
                "div",
                &["mechanism-svg-container"],
                false,
                1,
                root_el(hidden, pos),
            ));
            let points = self.layout.rect_of(&el(
                "div",
                &["points-background-container"],
                false,
                1,
                root_el(hidden, pos),
            ));
            let swerve = self.layout.rect_of(&el(
                "div",
                &["swerve-canvas-container"],
                false,
                1,
                root_el(hidden, pos),
            ));
            let joysticks = self.layout.rect_of(&el(
                "canvas",
                &["joysticks-canvas"],
                false,
                1,
                root_el(hidden, pos),
            ));
            let index = JsValue::Number(i as f64);
            if let Ok(renderer) = self.tabs.renderer_mut(&index) {
                if let Some(r) = inner_mut::<SwerveRenderer>(renderer) {
                    let mut dom = r.dom().borrow_mut();
                    dom.client_size = [
                        asrs_js::math::round(swerve.width),
                        asrs_js::math::round(swerve.height),
                    ];
                    dom.device_pixel_ratio = self.device_pixel_ratio;
                } else if let Some(r) = inner_mut::<JoysticksRenderer>(renderer) {
                    let mut dom = r.dom().borrow_mut();
                    dom.client_size = [
                        asrs_js::math::round(joysticks.width),
                        asrs_js::math::round(joysticks.height),
                    ];
                    dom.device_pixel_ratio = self.device_pixel_ratio;
                    dom.images = self.images.clone();
                } else if let Some(r) = inner_mut::<MechanismRenderer>(renderer) {
                    r.client_size = [
                        asrs_js::math::round(mechanism.width),
                        asrs_js::math::round(mechanism.height),
                    ];
                } else if let Some(r) = inner_mut::<PointsRenderer>(renderer) {
                    r.container_size = [points.width, points.height];
                }
            }
        }
    }

    fn renderer_top(&self) -> f64 {
        let tab_list = ElDesc {
            tag: "div",
            classes: &["tab-list"],
            hidden: false,
            pos: 1,
            child_pos: None,
            parent: None,
        };
        self.layout.rect_of(&tab_list).top()
            + self
                .tabs
                .selected_index()
                .and_then(|i| self.tabs.tab(i))
                .and_then(|t| t.controls_height().as_f64())
                .unwrap_or(0.0)
    }

    /// One animation frame: `periodic` plus the first frame's `show` message.
    fn frame(&mut self) {
        if self.frame_loop_dead {
            return;
        }
        self.host.renderer_top = self.renderer_top();
        self.sync_dom();
        for i in 0..self.tabs.len() {
            let index = JsValue::Number(i as f64);
            let Ok(renderer) = self.tabs.renderer_mut(&index) else {
                continue;
            };
            if let Some(r) = inner_mut::<Field2dRenderer>(renderer) {
                r.flush_image_loads();
            } else if let Some(r) = inner_mut::<JoysticksRenderer>(renderer) {
                while r.deliver_image_load().is_some() {}
            }
        }
        let now = self.host.clock.get();
        let out = self.tabs.periodic(&mut self.host, now);
        if let Some(e) = out.error {
            self.frame_loop_dead = true;
            self.exception("requestAnimationFrame".to_owned(), e.to_string());
        }
        for update in out.satellite_updates {
            let message = update.to_message();
            self.emit_main(message.name.name(), js_json::from_js(&message.data));
        }
        self.flush_messages();
        if self.show_pending {
            self.show_pending = false;
            self.emit_main("show", Value::Undefined);
        }
    }

    fn flush_messages(&mut self) {
        let messages = self.tabs.take_messages();
        let from_tabs = std::mem::take(&mut self.host.messages);
        for message in messages {
            self.emit_main(message.name.name(), js_json::from_js(&message.data));
        }
        for message in from_tabs {
            self.emit_main(message.name, js_json::from_js(&message.data));
        }
    }

    fn advance(&mut self, ms: f64) {
        let end = self.now + ms;
        loop {
            let frame_at = self.last_frame + self.frame_period;
            if frame_at <= end {
                self.now = frame_at;
                self.clock.set(frame_at);
                self.last_frame = frame_at;
                self.frame();
            } else {
                self.now = end;
                self.clock.set(end);
                break;
            }
        }
    }

    /// The `drag-update` window listener: the capture's `{"drag"}` record is written first,
    /// then every controller's handler runs in registration order.
    fn dispatch_drag(&mut self, detail: &JsValue) {
        let get = |key: &str| ops::get(detail, key).unwrap_or(JsValue::Undefined);
        self.emitted.push(Emit::Record {
            values: vec![
                ("drag", js_json::from_js(&get("data"))),
                ("end", js_json::from_js(&get("end"))),
                ("x", js_json::from_js(&get("x"))),
                ("y", js_json::from_js(&get("y"))),
            ],
        });
        for i in 0..self.tabs.len() {
            let index = JsValue::Number(i as f64);
            let kind = self.tabs.tab(i).and_then(|t| t.tab_type());
            let result = match kind {
                Some(TabType::Table) => self
                    .tabs
                    .controller_mut(&index)
                    .ok()
                    .and_then(|c| c.downcast_mut::<TableController>())
                    .map(|c| c.handle_drag(&mut self.host, detail)),
                Some(TabType::Console) => self
                    .tabs
                    .controller_mut(&index)
                    .ok()
                    .and_then(|c| c.downcast_mut::<ConsoleController>())
                    .map(|c| c.handle_drag(&mut self.host, detail)),
                _ => None,
            };
            if let Some(Err(e)) = result {
                self.exception(
                    "event drag-update".to_owned(),
                    format!("{}: {}", e.name, e.message),
                );
                return;
            }
        }
    }

    /// A mouse event on the selected tab's renderer root or table body.
    fn root_event(&mut self, etype: &str, x: f64, y: f64) {
        let Some(i) = self.tabs.selected_index() else {
            return;
        };
        let index = JsValue::Number(i as f64);
        let kind = self.tabs.tab(i).and_then(|t| t.tab_type());
        match (kind, etype) {
            (Some(TabType::Table), "mousedown") => {
                if let Some(c) = self
                    .tabs
                    .controller_mut(&index)
                    .ok()
                    .and_then(|c| c.downcast_mut::<TableController>())
                {
                    c.root_mouse_down(x, y);
                }
            }
            (Some(TabType::Table), "mouseup") => {
                if let Some(c) = self
                    .tabs
                    .controller_mut(&index)
                    .ok()
                    .and_then(|c| c.downcast_mut::<TableController>())
                {
                    c.root_mouse_up();
                }
            }
            (Some(TabType::Table), "mousemove") => {
                if let Some(c) = self
                    .tabs
                    .controller_mut(&index)
                    .ok()
                    .and_then(|c| c.downcast_mut::<TableController>())
                    && let Some(drag) = c.root_mouse_move(x, y)
                {
                    self.drag_start_table(drag);
                }
            }
            _ => {}
        }
    }

    /// `startDrag({tableIndex})`. The `mousemove` that started it then bubbles to the hub's
    /// window listener, which sends the first `drag-update`.
    fn drag_start_table(&mut self, drag: asrs_tabs::table::ColumnDrag) {
        let mut data = JsObject::new();
        data.insert("tableIndex", JsValue::Number(drag.table_index as f64));
        self.active_drag = Some(JsValue::Object(data));
    }
}

fn load_json(path: &Path) -> Result<Json, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn run_log_step(log: &Rc<RefCell<Log>>, calls: &[Value]) -> Result<(), String> {
    for call in calls {
        let parts = call.as_array().ok_or_else(|| err("log call is an array"))?;
        let method = parts
            .first()
            .and_then(Value::as_str)
            .map(|s| s.to_string_lossy())
            .ok_or_else(|| err("log method is a string"))?;
        let args: Vec<JsValue> = parts[1..]
            .iter()
            .map(js_json::to_js)
            .collect::<Result<_, _>>()?;
        let key = |i: usize| -> Result<JsString, String> {
            match args.get(i) {
                Some(JsValue::String(s)) => Ok(s.clone()),
                _ => Err(err("log key is a string")),
            }
        };
        let ts = |i: usize| -> Result<f64, String> {
            args.get(i)
                .and_then(JsValue::as_f64)
                .ok_or_else(|| err("timestamp is a number"))
        };
        let mut log = log.borrow_mut();
        match method.as_str() {
            "putBoolean" => log
                .put_boolean(&key(0)?, ts(1)?, truthy(&js_json::from_js(&args[2])))
                .unwrap(),
            "putNumber" => log
                .put_number(&key(0)?, ts(1)?, args[2].as_f64().unwrap_or(f64::NAN))
                .unwrap(),
            "putString" => {
                let v = match &args[2] {
                    JsValue::String(s) => s.clone(),
                    _ => return Err(err("putString value is a string")),
                };
                log.put_string(&key(0)?, ts(1)?, v).unwrap();
            }
            "putBooleanArray" => {
                let JsValue::Array(items) = &args[2] else {
                    return Err(err("putBooleanArray value is an array"));
                };
                let items = items.iter().map(|v| truthy(&js_json::from_js(v))).collect();
                log.put_boolean_array(&key(0)?, ts(1)?, items).unwrap();
            }
            "putNumberArray" => {
                let JsValue::Array(items) = &args[2] else {
                    return Err(err("putNumberArray value is an array"));
                };
                let items = items
                    .iter()
                    .map(|v| v.as_f64().unwrap_or(f64::NAN))
                    .collect();
                log.put_number_array(&key(0)?, ts(1)?, items).unwrap();
            }
            "putStringArray" => {
                let JsValue::Array(items) = &args[2] else {
                    return Err(err("putStringArray value is an array"));
                };
                let items = items
                    .iter()
                    .map(|v| match v {
                        JsValue::String(s) => s.clone(),
                        _ => JsString::new(),
                    })
                    .collect();
                log.put_string_array(&key(0)?, ts(1)?, items).unwrap();
            }
            "putUnknownStruct" => {
                log.put_unknown_struct(&key(0)?, ts(1)?, &args[2], false)
                    .map_err(|e| format!("{e:?}"))?;
            }
            "putRaw" => {
                let v = match &args[2] {
                    JsValue::Bytes(b) => b.clone(),
                    _ => return Err(err("putRaw value is bytes")),
                };
                log.put_raw(&key(0)?, ts(1)?, v)
                    .map_err(|e| format!("{e:?}"))?;
            }
            "putJSON" => {
                let v = match &args[2] {
                    JsValue::String(s) => s.clone(),
                    _ => return Err(err("putJSON value is a string")),
                };
                log.put_json(&key(0)?, ts(1)?, v)
                    .map_err(|e| format!("putJSON: {e:?}"))?;
            }
            "putPose" => {
                let pose = &args[2];
                let t = ops::get(pose, "translation").map_err(|e| e.message)?;
                let x = ops::index(&t, 0)
                    .map_err(|e| e.message)?
                    .as_f64()
                    .unwrap_or(f64::NAN);
                let y = ops::index(&t, 1)
                    .map_err(|e| e.message)?
                    .as_f64()
                    .unwrap_or(f64::NAN);
                let r = ops::get(pose, "rotation")
                    .map_err(|e| e.message)?
                    .as_f64()
                    .unwrap_or(f64::NAN);
                log.put_pose(&key(0)?, ts(1)?, [x, y], r).unwrap();
            }
            "createBlankField" => {
                let ty = match args[1].as_f64() {
                    Some(0.0) => LoggableType::Raw,
                    Some(1.0) => LoggableType::Boolean,
                    Some(2.0) => LoggableType::Number,
                    Some(3.0) => LoggableType::String,
                    Some(4.0) => LoggableType::BooleanArray,
                    Some(5.0) => LoggableType::NumberArray,
                    Some(6.0) => LoggableType::StringArray,
                    Some(7.0) => LoggableType::Empty,
                    _ => return Err(err("bad LoggableType")),
                };
                log.create_blank_field(&key(0)?, ty);
            }
            "setStructuredType" => {
                let ty = match &args[1] {
                    JsValue::Null | JsValue::Undefined => None,
                    JsValue::String(s) => Some(s.clone()),
                    _ => return Err(err("structured type is a string")),
                };
                log.set_structured_type(&key(0)?, ty);
            }
            "setWpilibType" => {
                let ty = match &args[1] {
                    JsValue::String(s) => s.clone(),
                    _ => return Err(err("wpilib type is a string")),
                };
                log.set_wpilib_type(&key(0)?, ty);
            }
            "setGeneratedParent" => log.set_generated_parent(&key(0)?),
            "deleteField" => log.delete_field(&key(0)?),
            "clearBeforeTime" => log.clear_before_time(ts(0)?, truthy(&js_json::from_js(&args[1]))),
            "updateRangeWithTimestamp" => log.update_range_with_timestamp(ts(0)?),
            other => return Err(err(format!("unsupported log method {other}"))),
        }
    }
    Ok(())
}

/// `window.eval` is restricted to the expressions the fixtures use.
/// `/*video:view*/`: per Video tab, in tab order, what `updateButtons`, the slider, the
/// progress bar and the source animations show, plus the controller's private fields.
fn video_view(r: &Runner) -> JsValue {
    let bool_ = JsValue::Bool;
    let pair = |hidden: bool, disabled: bool| JsValue::Array(vec![bool_(hidden), bool_(disabled)]);
    let tabs = r
        .tabs
        .tabs()
        .filter_map(|tab| {
            let video = tab.controller().downcast_ref::<VideoController>()?;
            let b = video.buttons();
            let slider = video.slider();
            let (play_frame, play_real) = video.play_start();
            Some(ops::object([
                ("uuid", tab.uuid().clone()),
                (
                    "buttons",
                    JsValue::Array(vec![
                        pair(b.lock_hidden, b.lock_disabled),
                        pair(b.unlock_hidden, b.unlock_disabled),
                        pair(false, b.controls_disabled),
                        pair(false, b.controls_disabled),
                        pair(b.play_hidden, b.controls_disabled),
                        pair(b.pause_hidden, b.controls_disabled),
                        pair(false, b.controls_disabled),
                        pair(false, b.controls_disabled),
                    ]),
                ),
                (
                    "slider",
                    JsValue::Array(vec![
                        JsValue::String(slider.value().clone()),
                        JsValue::String(slider.max().clone()),
                        bool_(b.controls_disabled),
                    ]),
                ),
                ("progress", JsValue::String(video.progress_width())),
                (
                    "animating",
                    JsValue::Array(vec![
                        bool_(video.youtube_animating()),
                        bool_(video.tba_animating()),
                    ]),
                ),
                (
                    "data",
                    JsValue::Array(video.data().into_iter().cloned().collect()),
                ),
                ("locked", bool_(video.is_locked())),
                ("lockedStartLog", JsValue::Number(video.locked_start_log())),
                ("playing", bool_(video.is_playing())),
                (
                    "play",
                    JsValue::Array(vec![
                        JsValue::Number(play_frame),
                        JsValue::Number(play_real),
                    ]),
                ),
            ]))
        })
        .collect();
    JsValue::Array(tabs)
}

/// Blink's serialization of an inline length assigned as `n.toString() + "px"`.
fn css_px(n: f64) -> String {
    let text = asrs_js::number::to_precision(n, Some(6.0)).unwrap();
    let text = match text.contains('.') {
        true => text.trim_end_matches('0').trim_end_matches('.').to_owned(),
        false => text,
    };
    format!("{text}px")
}

/// Reads and assignments of `TABLE_CONTAINER`'s scroll position and of the `bottom` inset that
/// sets its height, which are fixture input and leave no geometry records.
fn eval_table_container(r: &mut Runner, body: &str) -> Option<JsValue> {
    const CONTAINER: &str =
        "window.tabs.tabList[window.tabs.selectedTab].renderer.TABLE_CONTAINER.";
    let mut table = r.table.as_ref()?.0.borrow_mut();
    let axis = |name: &str| match name {
        "scrollLeft" => Some(0),
        "scrollTop" => Some(1),
        _ => None,
    };
    if let Some(statements) = body.strip_prefix("{ ").and_then(|b| b.strip_suffix(" }")) {
        for statement in statements.split_terminator(';') {
            let (name, value) = statement
                .trim()
                .strip_prefix(CONTAINER)?
                .split_once(" = ")?;
            if name == "style.bottom" {
                let bottom: f64 = value.trim_matches('"').strip_suffix("px")?.parse().ok()?;
                table.layout.container.height = TABLE_CONTAINER.height - bottom;
                continue;
            }
            let axis = axis(name)?;
            let mut to = table.layout.scroll();
            to[axis] = value.parse().ok()?;
            table.layout.scroll_to(to);
        }
        return Some(JsValue::Undefined);
    }
    let name = body.strip_prefix(CONTAINER)?;
    let scroll = table.layout.scroll();
    Some(JsValue::Number(scroll[axis(name)?]))
}

fn run_eval(r: &mut Runner, expr: &str) -> Result<JsValue, String> {
    if let Some(body) = expr
        .strip_prefix("window.__oracleQuiet(() => ")
        .and_then(|e| e.strip_suffix(')'))
    {
        return eval_table_container(r, body)
            .ok_or_else(|| err(format!("unsupported eval {expr}")));
    }
    if expr.starts_with("/*table:highlight*/") {
        let dom = r
            .selected_table_dom()
            .ok_or_else(|| err("table:highlight needs a selected Table tab"))?;
        let dom = dom.borrow();
        let left = dom.drag_highlight_left.map_or(String::new(), css_px);
        return Ok(JsValue::Array(vec![
            JsValue::Bool(dom.drag_highlight_hidden),
            JsValue::string(&left),
        ]));
    }
    // The scroll is an input; the position it leaves comes back from the recorded reads.
    if expr.starts_with("/*console:scroll*/") {
        if r.console_reads.is_none() {
            return Err(err("console:scroll needs real geometry"));
        }
        return Ok(JsValue::Undefined);
    }
    if expr.starts_with("/*r07:mirror*/") {
        return Ok(JsValue::Undefined);
    }
    if expr.starts_with("/*video:view*/") {
        return Ok(video_view(r));
    }
    if let Some(inner) = expr
        .strip_prefix("[...")
        .and_then(|e| e.strip_suffix(']'))
        .filter(|e| e.starts_with("window.tabs.tabList["))
    {
        return run_eval(r, inner);
    }
    if let Some(container) = expr
        .strip_prefix("document.querySelector('div.renderer-content > :not([hidden]) div.")
        .and_then(|e| e.strip_suffix("').scrollTop"))
    {
        let dom = r
            .tabs
            .selected_index()
            .map_or(DomKind::Other, |i| r.tab_dom(i));
        let scroll_top = match (container, dom) {
            ("console-table-container", DomKind::Console(dom)) => match &mut r.console_reads {
                Some(reads) => reads.scroll_top(&[]),
                None => dom.borrow().scroll_top,
            },
            _ => return Err(err(format!("unsupported eval {expr}"))),
        };
        return Ok(JsValue::Number(scroll_top));
    }
    if let Some(rest) = expr.strip_prefix("window.tabs.tabList[")
        && let Some(close) = rest.find(']')
    {
        let index = &rest[..close];
        let suffix = &rest[close + 1..];
        let index = if index == "window.tabs.selectedTab" {
            r.tabs.selected().clone()
        } else {
            JsValue::Number(index.parse::<f64>().map_err(|_| err("bad tab index"))?)
        };
        return eval_tab_suffix(r, suffix, &index);
    }
    if let Some(index) = expr
        .strip_prefix("window.tabs.setSelected(")
        .and_then(|e| e.strip_suffix(')'))
    {
        let index = index.parse::<f64>().map_err(|_| err("bad tab index"))?;
        r.tabs
            .set_selected(&r.host, &JsValue::Number(index))
            .map_err(|e| err(e.to_string()))?;
        return Ok(JsValue::Undefined);
    }
    match expr {
        "window.tabs.saveState()" => Ok(r.tabs.save_state(&r.host)),
        "[...window.tabs.getActiveFields()]" => {
            let fields = r.tabs.active_fields(&r.host, &r.host.log.borrow());
            Ok(JsValue::Array(
                fields.into_iter().map(JsValue::String).collect(),
            ))
        }
        // `updateElements` sets the var every pass; it is `""` only before the first.
        "document.documentElement.style.getPropertyValue(\"--show-timeline\")" => {
            Ok(JsValue::String(js(match r.tabs.show_timeline() {
                Some(true) => "1",
                Some(false) => "0",
                None => "",
            })))
        }
        "window.tabs.isValidUUID(window.tabs.tabList[window.tabs.selectedTab].controller.UUID)" => {
            let uuid = r
                .tabs
                .selected_index()
                .and_then(|i| r.tabs.tab(i))
                .map_or(JsValue::Undefined, |t| t.uuid().clone());
            Ok(JsValue::Bool(r.tabs.is_valid_uuid(&uuid)))
        }
        _ => eval_selection_expr(&mut r.host.selection, expr)
            .ok_or_else(|| err(format!("unsupported eval {expr}"))),
    }
}

/// A harness-side gap reported as an exception the vector can never contain.
fn unmodeled(what: String) -> asrs_hub::tabs::TabsError {
    asrs_hub::tabs::TabsError::Thrown(JsError::new("Unmodeled", what))
}

fn tab_source_list(controller: &mut dyn TabController<Host>) -> Option<&mut SourceList> {
    let any = controller.as_any_mut();
    if any.is::<SwerveController>() {
        return any
            .downcast_mut::<SwerveController>()
            .map(SwerveController::source_list_mut);
    }
    if any.is::<MechanismController>() {
        return any
            .downcast_mut::<MechanismController>()
            .map(MechanismController::source_list_mut);
    }
    if any.is::<Field2dController>() {
        return any
            .downcast_mut::<Field2dController>()
            .map(Field2dController::source_list_mut);
    }
    if any.is::<Field3dController>() {
        return any
            .downcast_mut::<Field3dController>()
            .map(Field3dController::source_list_mut);
    }
    if any.is::<StatisticsController>() {
        return any
            .downcast_mut::<StatisticsController>()
            .map(StatisticsController::source_list_mut);
    }
    any.downcast_mut::<PointsController>()
        .map(PointsController::source_list_mut)
}

/// The classes `www/hub.html` gives an `<input>`, and gives nothing else.
const INPUT_CLASSES: [&str; 13] = [
    "console-table-jump-input",
    "data-table-jump-input",
    "dimensions-height",
    "dimensions-width",
    "filter",
    "max-speed",
    "range-max",
    "range-min",
    "side-bar-search",
    "size-x",
    "size-y",
    "step-size",
    "video-timeline-slider",
];

/// What the keydown guards in `SelectionImpl` and `VideoController` ask of `event.target`: its
/// tag as the selector's last compound spells it, or as `www/hub.html` gives its class. A class
/// this does not know fails.
fn key_target_of(target: &str) -> Result<KeyTarget, String> {
    let element = target.rsplit(' ').next().expect("rsplit yields a part");
    let mut parts = element.split(['.', '#', '[', ':']);
    let tag = parts.next().expect("split yields a part");
    if !tag.is_empty() {
        return match tag {
            "input" => Ok(KeyTarget::Input),
            "textarea" => Ok(KeyTarget::TextArea),
            "window" | "document" | "body" | "div" | "select" | "button" => Ok(KeyTarget::Other),
            _ => Err(err(format!("keydown on a {tag} is not modeled"))),
        };
    }
    for class in parts {
        if INPUT_CLASSES.contains(&class) {
            return Ok(KeyTarget::Input);
        }
        if class == "xr-button" {
            return Ok(KeyTarget::Other);
        }
    }
    Err(err(format!("keydown on {target}: spell the element's tag")))
}

/// `setupKeyboardControls`: `event.key` is `undefined` unless the event is a `KeyboardEvent`
/// built with one.
fn event_key_clicks(init: &InitDict<'_>) -> Result<bool, String> {
    match init.get("key") {
        None => Ok(false),
        Some(key) => key
            .as_str()
            .map(|k| keyboard_clicks(&js(&k.to_string_lossy())))
            .ok_or_else(|| err("init.key must be a string")),
    }
}

struct InitDict<'a> {
    raw: &'a Json,
    kind: &'a str,
}

impl<'a> InitDict<'a> {
    fn get(&self, name: &str) -> Option<&'a Json> {
        const MODIFIERS: [&str; 4] = ["ctrlKey", "shiftKey", "altKey", "metaKey"];
        let mouse = [
            "clientX", "clientY", "screenX", "screenY", "button", "buttons",
        ];
        let member = match self.kind {
            "CustomEvent" => name == "detail",
            "MouseEvent" | "PointerEvent" | "DragEvent" => {
                name == "detail" || MODIFIERS.contains(&name) || mouse.contains(&name)
            }
            "WheelEvent" => {
                name == "detail"
                    || MODIFIERS.contains(&name)
                    || mouse.contains(&name)
                    || ["deltaX", "deltaY", "deltaZ", "deltaMode"].contains(&name)
            }
            "KeyboardEvent" => {
                MODIFIERS.contains(&name) || ["key", "code", "location", "repeat"].contains(&name)
            }
            _ => false,
        };
        member.then(|| self.raw.get(name)).flatten()
    }
}

fn field3d_controller(tabs: &mut Tabs<Host>, tab: usize) -> Result<&mut Field3dController, String> {
    tabs.controller_mut(&JsValue::Number(tab as f64))
        .map_err(|e| err(e.to_string()))?
        .downcast_mut::<Field3dController>()
        .ok_or_else(|| err(format!("tab {tab} is not Field3d")))
}

fn eval_tab_suffix(r: &mut Runner, suffix: &str, index: &JsValue) -> Result<JsValue, String> {
    let parse_arg = |arg: &str| -> Result<JsValue, String> {
        let json = json::parse(arg).map_err(|e| err(e.to_string()))?;
        js_json::to_js(&Value::from_json_lenient(&json))
    };
    if let Some(arg) = suffix
        .strip_prefix(".controller.restoreState(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let state = parse_arg(arg)?;
        r.tabs
            .controller_mut(index)
            .map_err(|e| err(e.to_string()))?
            .restore_state(&mut r.host, &state)
            .map_err(|e| e.message)?;
        return Ok(JsValue::Undefined);
    }
    if let Some(arg) = suffix
        .strip_prefix(".renderer.restoreState(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let state = parse_arg(arg)?;
        r.tabs
            .renderer_mut(index)
            .map_err(|e| err(e.to_string()))?
            .restore_state(&mut r.host, &state)
            .map_err(|e| e.message)?;
        return Ok(JsValue::Undefined);
    }
    if let Some(arg) = suffix
        .strip_prefix(".controller.setXRActive(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let JsValue::Bool(active) = parse_arg(arg)? else {
            return Err(err(format!("setXRActive({arg}) is not modeled")));
        };
        r.tabs
            .controller_mut(index)
            .map_err(|e| err(e.to_string()))?
            .downcast_mut::<Field3dController>()
            .ok_or_else(|| err("setXRActive on a tab that is not Field3d"))?
            .set_xr_active(active);
        return Ok(JsValue::Undefined);
    }
    if suffix == ".controller.saveState()" {
        return Ok(r
            .tabs
            .controller_mut(index)
            .map_err(|e| err(e.to_string()))?
            .save_state(&r.host));
    }
    for (name, list_id) in [
        ("leftSourceList", ListId::Left),
        ("rightSourceList", ListId::Right),
        ("discreteSourceList", ListId::Discrete),
    ] {
        let Some(arg) = suffix
            .strip_prefix(&format!(".controller.{name}.addField("))
            .and_then(|s| s.strip_suffix(')'))
        else {
            continue;
        };
        let Some(key) = parse_arg(arg)?.into_string() else {
            return Err(err("addField needs a string key"));
        };
        let controller = r
            .tabs
            .controller_mut(index)
            .map_err(|e| err(e.to_string()))?
            .downcast_mut::<LineGraphController>()
            .ok_or_else(|| err(format!("{name} on a tab that is not a line graph")))?;
        let host = &mut r.host;
        controller
            .with_list(list_id, |list, [a, b]| {
                with_env(host, |env| list.add_field(env, &[a, b], &key, None))
            })
            .map_err(|e| js_error(e).message)?;
        return Ok(JsValue::Undefined);
    }
    if let Some(op) = suffix.strip_prefix(".controller.sourceList.") {
        let controller = r
            .tabs
            .controller_mut(index)
            .map_err(|e| err(e.to_string()))?;
        let Some(list) = tab_source_list(controller) else {
            return Err(err(format!("unsupported tab eval suffix {suffix}")));
        };
        let host = &mut r.host;
        if let Some(arg) = op
            .strip_prefix("addField(")
            .and_then(|s| s.strip_suffix(')'))
        {
            let (key, parent) = match arg.rfind(',') {
                Some(pos) if arg[pos + 1..].trim().parse::<f64>().is_ok() => (
                    &arg[..pos],
                    Some(arg[pos + 1..].trim().parse::<f64>().unwrap() as usize),
                ),
                _ => (arg, None),
            };
            let JsValue::String(key) = &parse_arg(key)? else {
                return Err(err("addField needs a string key"));
            };
            with_env(host, |env| list.add_field(env, &[], key, parent))
                .map_err(|e| js_error(e).message)?;
            return Ok(JsValue::Undefined);
        }
        if let Some(arg) = op
            .strip_prefix("setState(")
            .and_then(|s| s.strip_suffix(')'))
        {
            let value = parse_arg(arg)?;
            with_env(host, |env| list.restore(env, &value)).map_err(|e| js_error(e).message)?;
            return Ok(JsValue::Undefined);
        }
        if op == "getState()" {
            return Ok(JsValue::Array(
                list.state().iter().map(ToJs::to_js).collect(),
            ));
        }
        if op == "refresh()" {
            with_env(host, |env| list.refresh(env)).map_err(|e| js_error(e).message)?;
            return Ok(JsValue::Undefined);
        }
        if let Some(arg) = op.strip_prefix("remove(").and_then(|s| s.strip_suffix(')')) {
            let i = arg
                .trim()
                .parse::<f64>()
                .map_err(|_| err("remove needs an index"))? as usize;
            list.remove(i).map_err(|e| js_error(e).message)?;
            return Ok(JsValue::Undefined);
        }
        if let Some(arg) = op
            .strip_prefix("toggleHidden(")
            .and_then(|s| s.strip_suffix(')'))
        {
            let i = arg
                .trim()
                .parse::<f64>()
                .map_err(|_| err("toggleHidden needs an index"))? as usize;
            with_env(host, |env| list.toggle_hidden(env, i)).map_err(|e| js_error(e).message)?;
            return Ok(JsValue::Undefined);
        }
        return Err(err(format!("unsupported tab eval suffix {suffix}")));
    }
    if suffix == ".controller.getActiveFields()" {
        let fields = r
            .tabs
            .controller_mut(index)
            .map_err(|e| err(e.to_string()))?
            .active_fields(&r.host);
        return Ok(JsValue::Array(
            fields.into_iter().map(JsValue::String).collect(),
        ));
    }
    Err(err(format!("unsupported tab eval suffix {suffix}")))
}

fn eval_selection_expr(sel: &mut SelectionImpl, expr: &str) -> Option<JsValue> {
    let expr = expr.strip_prefix("window.selection.")?;
    if let Some(arg) = expr
        .strip_prefix("setSelectedTime(")
        .and_then(|s| s.strip_suffix(')'))
    {
        sel.set_selected_time(arg.parse::<f64>().ok()?);
        return Some(JsValue::Undefined);
    }
    if let Some(arg) = expr
        .strip_prefix("setHoveredTime(")
        .and_then(|s| s.strip_suffix(')'))
    {
        sel.set_hovered_time(if arg == "null" || arg == "undefined" {
            None
        } else {
            arg.parse::<f64>().ok()
        });
        return Some(JsValue::Undefined);
    }
    match expr {
        "play()" => sel.play(),
        "pause()" => sel.pause(),
        "goIdle()" => sel.go_idle(),
        "togglePlayback()" => sel.toggle_playback(),
        "lock()" => sel.lock(),
        "unlock()" => sel.unlock(),
        "toggleLock()" => sel.toggle_lock(),
        _ => return None,
    }
    Some(JsValue::Undefined)
}

fn json_to_layout(fixture: &Json) -> Result<Layout, String> {
    let mut rules = Vec::new();
    if let Some(list) = fixture.get("layout").and_then(Json::as_array) {
        for rule in list {
            let selector = rule
                .get("selector")
                .and_then(Json::as_str)
                .ok_or_else(|| err("layout rule needs a selector"))?
                .to_string_lossy();
            let rect = if let Some(rect) = rule.get("rect").and_then(Json::as_array) {
                let n = |i: usize| rect.get(i).and_then(Json::as_f64).unwrap_or(0.0);
                Some([n(0), n(1), n(2), n(3)])
            } else if rule.get("text").and_then(Json::as_f64).is_some() {
                return Err(err("text layout rules are not modeled"));
            } else {
                None
            };
            rules.push((selector, rect));
        }
    }
    rules.push((
        "div.renderer-content".to_owned(),
        Some([300.0, 92.0, 800.0, 358.0]),
    ));
    rules.push(("div.renderer-content *".to_owned(), None));
    Ok(Layout { rules })
}

/// A record's kind for the tally: its first key, with the canvas a `canvas` record is for and
/// whether a geometry access is on the tab's own table container.
fn kind_of(record: &Record) -> String {
    let key = match &record.json {
        Json::Object(pairs) if !pairs.is_empty() => pairs[0].0.to_string_lossy(),
        _ => panic!("line {}: the record has no first key", record.line),
    };
    match key.as_str() {
        "canvas" => format!("canvas:{}", canvas_class(record)),
        "read" | "write" if on_tab_table(record) => TAB_GEOMETRY.to_owned(),
        "read" | "write" => HUB_GEOMETRY.to_owned(),
        _ => key,
    }
}

const TAB_GEOMETRY: &str = "tab geometry";
const HUB_GEOMETRY: &str = "hub geometry";

fn canvas_class(record: &Record) -> String {
    let id = record.string("canvas").expect("a canvas record has an id");
    let (class, _) = id.split_once('#').expect("a canvas id is class#n");
    class.to_owned()
}

fn on_tab_table(record: &Record) -> bool {
    record.string("el").is_some_and(|el| {
        el.contains("div.data-table-container:") || el.contains("div.console-table-container:")
    })
}

/// The recording's accesses to Console containers and rows. A real-geometry run also records
/// what the rest of the hub reads (tab bar, sidebar, timeline, source lists), which other suites
/// replay.
fn console_reads(records: &[Record]) -> Result<RecordedConsole, String> {
    let mut console = Vec::new();
    for record in records {
        if GeometryRecord::parse(record)?.is_some_and(|g| g.el.contains("console-table-container"))
        {
            console.push(record.clone());
        }
    }
    if console.is_empty() {
        return Err(err("a real-geometry Console fixture reads no layout"));
    }
    Ok(RecordedConsole(Rc::new(RefCell::new(
        ScriptedReads::from_records(&console)?,
    ))))
}

fn selection_step(
    sel: &mut SelectionImpl,
    parts: &[Json],
    live_time: &Rc<Cell<Option<f64>>>,
) -> Result<(), String> {
    let method = parts
        .first()
        .and_then(Json::as_str)
        .ok_or_else(|| err("selection method is a string"))?
        .to_string_lossy();
    let arg = |i: usize| -> Value {
        parts
            .get(1 + i)
            .map(Value::from_json_lenient)
            .unwrap_or(Value::Undefined)
    };
    let num = |i: usize| -> Result<f64, String> {
        arg(i).as_f64().ok_or_else(|| err("expected a number"))
    };
    match method.as_str() {
        "play" => sel.play(),
        "pause" => sel.pause(),
        "goIdle" => sel.go_idle(),
        "togglePlayback" => sel.toggle_playback(),
        "lock" => sel.lock(),
        "unlock" => sel.unlock(),
        "toggleLock" => sel.toggle_lock(),
        "stepCycle" => sel.step_cycle(truthy(&arg(0))),
        "setSelectedTime" => sel.set_selected_time(match arg(0) {
            Value::Null => 0.0,
            Value::Undefined => f64::NAN,
            v => v.as_f64().ok_or_else(|| err("expected a number"))?,
        }),
        "setHoveredTime" => sel.set_hovered_time(arg(0).as_f64()),
        "setPlaybackSpeed" => sel.set_playback_speed(num(0)?),
        "setPlaybackLooping" => sel.set_playback_looping(truthy(&arg(0))),
        "setGrabZoomRange" => sel.set_grab_zoom_range(match arg(0) {
            Value::Array(a) if a.len() == 2 => {
                Some([a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0)])
            }
            _ => None,
        }),
        "finishGrabZoom" => sel.finish_grab_zoom(),
        "setTimelineRange" => {
            let pair = match arg(0) {
                Value::Array(a) if a.len() == 2 => {
                    [a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0)]
                }
                _ => return Err(err("setTimelineRange needs a pair")),
            };
            sel.set_timeline_range(pair, truthy(&arg(1)));
        }
        "applyTimelineScroll" => {
            sel.apply_timeline_scroll(num(0)?, num(1)?, num(2)?);
        }
        "setLiveConnected" => {
            let live_time = live_time.clone();
            sel.set_live_connected(Rc::new(move || live_time.get().unwrap_or(f64::NAN)));
        }
        "setLiveDisconnected" => sel.set_live_disconnected(),
        other => return Err(format!("unknown selection method {other}")),
    }
    Ok(())
}

fn replay(
    spec_path: &Path,
    table: Option<SharedTable>,
    console_reads: Option<RecordedConsole>,
) -> Result<Vec<Emit>, String> {
    let spec = load_json(spec_path)?;
    let fixture = spec.get("fixture").ok_or_else(|| err("no fixture"))?;
    let start = fixture
        .get("startTime")
        .and_then(Json::as_f64)
        .unwrap_or(1767225600000.0);
    let window = fixture.get("window");
    let wnum = |key: &str, default: f64| {
        window
            .and_then(|w| w.get(key))
            .and_then(Json::as_f64)
            .unwrap_or(default)
    };
    let frame_period = wnum("framePeriod", 16.0);
    let inner_height = wnum("height", 650.0);
    let seed = fixture.get("seed").and_then(Json::as_f64).unwrap_or(1.0) as u32;
    let layout = json_to_layout(fixture)?;

    let clock_cell = Rc::new(Cell::new(start));
    let live_time: Rc<Cell<Option<f64>>> = Rc::new(Cell::new(None));
    let log = Rc::new(RefCell::new(Log::new(true)));
    let selection = SelectionImpl::new(log.clone(), Rc::new(ScriptedClock(clock_cell.clone())));
    let theme_dark = window
        .and_then(|w| w.get("theme"))
        .and_then(Json::as_str)
        .is_some_and(|t| t.to_string_lossy() == "dark");
    let device_pixel_ratio = wnum("devicePixelRatio", 1.0);
    if let Some(table) = &table {
        let layout = &mut table.0.borrow_mut().layout;
        layout.container = TABLE_CONTAINER;
        layout.device_pixel_ratio = device_pixel_ratio;
    }
    let mut images = HashMap::new();
    if let Some(Json::Object(map)) = fixture.get("images") {
        for (src, size) in map {
            if let Some(pair) = size.as_array()
                && pair.len() == 2
            {
                images.insert(
                    JsString::from(src.to_string_lossy()),
                    [
                        pair[0].as_f64().unwrap_or(0.0),
                        pair[1].as_f64().unwrap_or(0.0),
                    ],
                );
            }
        }
    }
    let mut host = Host {
        log: log.clone(),
        selection,
        preferences: Some(Preferences::default()),
        rng: DomRng(seed),
        inner_height,
        renderer_top: 92.0,
        messages: Vec::new(),
        assets_raw: JsValue::Null,
        assets: None,
        type_memory: JsValue::Object(JsObject::new()),
        dark: theme_dark,
        clock: clock_cell.clone(),
    };
    // Boot `set-assets`: `window.assets` is assigned before any controller exists, so
    // `tabs.newAssets()` at boot only reaches the stub tab.
    if let Some(assets) = fixture.get("assets") {
        let assets = match assets.as_str() {
            Some(path) => {
                let json = load_json(&spec_dir_join(spec_path, &path.to_string_lossy()))?;
                js_json::to_js(&Value::from_json_lenient(&json))?
            }
            None => js_json::to_js(&Value::from_json_lenient(assets))?,
        };
        host.assets_raw = assets.clone();
        host.assets = assets_from_js(&assets).map_err(err)?;
    }
    if let Some(memory) = fixture.get("typeMemory") {
        host.type_memory = js_json::to_js(&Value::from_json_lenient(memory))?;
    }
    if let Some(prefs) = fixture.get("preferences") {
        let prefs = js_json::to_js(&Value::from_json_lenient(prefs))?;
        host.preferences
            .as_mut()
            .unwrap()
            .merge(&prefs)
            .map_err(|e| err(format!("preferences: {e:?}")))?;
    }
    // `window.platform` comes from the boot `set-version`, upstream's darwin by default.
    let version = match fixture.get("version") {
        Some(v) => js_json::to_js(&Value::from_json_lenient(v))?,
        None => ops::object([("platform", JsValue::String(JsString::from("darwin")))]),
    };
    set_platform(&mut host.selection, &version);
    if let Some(boot) = fixture.get("boot") {
        host.preferences = None;
        for message in boot.as_array().ok_or_else(|| err("boot is a list"))? {
            let name = message
                .as_array()
                .and_then(|m| m.first())
                .and_then(Json::as_str)
                .map(|s| s.to_string_lossy())
                .ok_or_else(|| err("boot entries are [name, data]"))?;
            let data = message
                .as_array()
                .and_then(|m| m.get(1))
                .map(Value::from_json_lenient)
                .unwrap_or(Value::Undefined);
            match name.as_str() {
                "set-preferences" => {
                    let mut p = Preferences::default();
                    p.merge(&js_json::to_js(&data)?)
                        .map_err(|e| err(format!("boot set-preferences: {e:?}")))?;
                    host.preferences = Some(p);
                }
                "set-assets" => {
                    let data = js_json::to_js(&data)?;
                    host.assets = assets_from_js(&data).map_err(err)?;
                    host.assets_raw = data;
                }
                "restore-type-memory" => {
                    host.type_memory = js_json::to_js(&data)?;
                }
                "set-version" => set_platform(&mut host.selection, &js_json::to_js(&data)?),
                "set-fullscreen"
                | "set-battery"
                | "show-update-button"
                | "show-feedback-button"
                | "show-when-ready"
                | "set-active-satellites" => {}
                other => return Err(err(format!("boot message {other} is not modeled"))),
            }
        }
    }

    let ignored: Vec<String> = match fixture.get("ignoreMain").and_then(Json::as_array) {
        Some(names) => names
            .iter()
            .filter_map(Json::as_str)
            .map(|s| s.to_string_lossy())
            .collect(),
        None => ["save-state", "save-type-memory", "update-touch-bar-slider"]
            .map(str::to_owned)
            .into(),
    };

    let factory = HubTabFactory::with_hook(Box::new(Hook));
    host.rng.next_f64();
    let tabs = Tabs::new(Box::new(factory), &mut host);
    let mut runner = Runner {
        layout,
        host,
        tabs,
        emitted: Vec::new(),
        clock: clock_cell.clone(),
        now: start,
        start,
        last_frame: start,
        frame_period,
        show_pending: true,
        frame_loop_dead: false,
        active_drag: None,
        controls_order: Vec::new(),
        table,
        sidebar_width: 300.0,
        sidebar_expanded: Vec::new(),
        ignored,
        device_pixel_ratio,
        images,
        console_reads,
    };

    // `fixture.state` boots through `restore-state`.
    if let Some(state) = fixture.get("state") {
        if let Some(tabs_state) = state.get("tabs") {
            let tabs_state = js_json::to_js(&Value::from_json_lenient(tabs_state))?;
            runner
                .tabs
                .restore_state(&mut runner.host, &tabs_state)
                .map_err(|e| err(e.to_string()))?;
        }
        if let Some(sidebar) = state.get("sidebar") {
            if let Some(w) = sidebar.get("width").and_then(Json::as_f64) {
                runner.sidebar_width = w;
            }
            if let Some(expanded) = sidebar.get("expanded").and_then(Json::as_array) {
                runner.sidebar_expanded = expanded
                    .iter()
                    .filter_map(|e| e.as_str())
                    .map(|s| js(&s.to_string_lossy()))
                    .collect();
            }
        }
    }

    let steps = fixture
        .get("steps")
        .and_then(Json::as_array)
        .map(|a| a.to_vec())
        .unwrap_or_default();
    for (i, step) in steps.iter().enumerate() {
        let where_ = || format!("step {i}");
        runner.sync_controls_order(false);
        if let Some(log_step) = step.get("log") {
            let calls: Vec<Value> = match log_step.as_str() {
                Some(path) => {
                    let path = spec_dir_join(spec_path, &path.to_string_lossy());
                    let ops = load_json(&path)?;
                    ops.as_array()
                        .ok_or_else(|| err(format!("{}: not an array", path.display())))?
                        .iter()
                        .map(Value::from_json_lenient)
                        .collect()
                }
                None => log_step
                    .as_array()
                    .ok_or_else(|| err("log step is an array or a path"))?
                    .iter()
                    .map(Value::from_json_lenient)
                    .collect(),
            };
            if let Err(e) = run_log_step(&log, &calls) {
                runner.exception("log".to_owned(), e);
            }
            if !matches!(step.get("newFields"), Some(Json::Bool(false)))
                && let Err(e) = runner.tabs.refresh(&mut runner.host)
            {
                runner.exception("log".to_owned(), e.to_string());
            }
        } else if let Some(sel_step) = step.get("selection") {
            let parts = sel_step
                .as_array()
                .ok_or_else(|| err("selection step is [method, ...args]"))?;
            if let Err(e) = selection_step(&mut runner.host.selection, parts, &live_time) {
                let method = parts
                    .first()
                    .and_then(Json::as_str)
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default();
                runner.exception(format!("selection {method}"), e);
            }
        } else if let Some(live) = step.get("liveTime") {
            live_time.set(Value::from_json_lenient(live).as_f64());
        } else if let Some(main) = step.get("main") {
            let name = main
                .as_str()
                .ok_or_else(|| err("main step name is a string"))?
                .to_string_lossy();
            let data = step
                .get("data")
                .map(to_js)
                .transpose()?
                .unwrap_or(JsValue::Undefined);
            // `handleMainMessage` is async, so a throw surfaces as an unhandled rejection.
            if let Err(e) = runner.main_message(&name, &data) {
                runner.exception("unhandledRejection".to_owned(), e.to_string());
            }
        } else if let Some(event) = step.get("event") {
            let result = runner.dispatch_event(event);
            runner.flush_messages();
            if let Err(e) = result {
                let etype = event
                    .get("type")
                    .and_then(Json::as_str)
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default();
                runner.exception(format!("event {etype}"), e);
            }
        } else if let Some(ms) = step.get("advance").and_then(Json::as_f64) {
            runner.advance(ms);
        } else if let Some(n) = step.get("frames").and_then(Json::as_f64) {
            for _ in 0..n as usize {
                runner.advance(runner.frame_period);
            }
        } else if let Some(capture) = step.get("capture") {
            let label = step
                .get("label")
                .and_then(Json::as_str)
                .map(|s| s.to_string_lossy());
            runner.capture(capture, label)?;
        } else if let Some(eval) = step.get("eval").and_then(Json::as_str) {
            let expr = eval.to_string_lossy();
            if let Err(e) = run_eval(&mut runner, &expr) {
                runner.exception("eval".to_owned(), e);
            }
        } else {
            return Err(format!("{}: unknown step", where_()));
        }
    }
    Ok(runner.emitted)
}

/// `window.platform = data.platform` from `set-version`.
fn set_platform(selection: &mut SelectionImpl, data: &JsValue) {
    if let Some(platform) = data.get("platform").and_then(JsValue::as_str) {
        let mut ctx = selection.context().clone();
        ctx.platform = platform.to_string_lossy();
        selection.set_context(ctx);
    }
}

fn spec_dir_join(spec: &Path, rel: &str) -> PathBuf {
    spec.parent().unwrap().join(rel)
}

impl Runner {
    fn main_message(
        &mut self,
        name: &str,
        data: &JsValue,
    ) -> Result<(), asrs_hub::tabs::TabsError> {
        use asrs_hub::tabs::TabsError;
        match name {
            "new-tab" => self.tabs.add_tab(&mut self.host, data)?,
            "move-tab" => self.tabs.move_selection(&self.host, data)?,
            "shift-tab" => self.tabs.shift_selected(&self.host, data)?,
            "close-tab" => {
                let index = self.tabs.selected().clone();
                self.tabs.close(&self.host, &index, data)?;
            }
            "rename-tab" => {
                let index = data.get("index").cloned().unwrap_or_default();
                let name = data.get("name").cloned().unwrap_or_default();
                self.tabs.rename_tab(&index, name)?;
            }
            "toggle-controls" => self.tabs.toggle_controls_visible(&self.host)?,
            "new-satellite" => self.tabs.new_satellite()?,
            "set-active-satellites" => {
                let JsValue::Array(uuids) = data else {
                    return Err(TabsError::Thrown(JsError::type_error(
                        "set-active-satellites needs an array",
                    )));
                };
                self.tabs.set_active_satellites(uuids.clone());
            }
            "restore-state" => {
                let tabs = data.get("tabs").cloned().unwrap_or_default();
                self.tabs.restore_state(&mut self.host, &tabs)?;
            }
            "add-table-range" => {
                let renderer_uuid = match data.get("rendererUUID") {
                    Some(JsValue::String(s)) => s.clone(),
                    _ => {
                        return Err(TabsError::Thrown(JsError::type_error(
                            "add-table-range needs a rendererUUID",
                        )));
                    }
                };
                let range = match data.get("range") {
                    Some(JsValue::Array(r)) => match (r.first(), r.get(1)) {
                        (Some(JsValue::Number(a)), Some(JsValue::Number(b))) => Some([*a, *b]),
                        _ => None,
                    },
                    _ => None,
                };
                let controller = data.get("controllerUUID").cloned().unwrap_or_default();
                self.tabs
                    .add_table_range(&mut self.host, &controller, &renderer_uuid, range)?;
            }
            "set-preferences" => {
                let mut p = Preferences::default();
                p.merge(data).map_err(|e| {
                    TabsError::Thrown(JsError::type_error(format!("{}: {}", e.name, e.message)))
                })?;
                self.host.preferences = Some(p);
            }
            "call-selection-setter" => {
                let method = data
                    .get("method")
                    .and_then(JsValue::as_str)
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default();
                let args = match data.get("args") {
                    Some(JsValue::Array(a)) => a.clone(),
                    _ => Vec::new(),
                };
                let sel = &mut self.host.selection;
                let num = |i: usize| args.get(i).and_then(JsValue::as_f64).unwrap_or(f64::NAN);
                match method.as_str() {
                    "setSelectedTime" => sel.set_selected_time(num(0)),
                    "setHoveredTime" => sel.set_hovered_time(args[0].as_f64()),
                    "setPlaybackSpeed" => sel.set_playback_speed(num(0)),
                    "setPlaybackLooping" => {
                        sel.set_playback_looping(truthy(&js_json::from_js(&args[0])))
                    }
                    "lock" => sel.lock(),
                    "unlock" => sel.unlock(),
                    "toggleLock" => sel.toggle_lock(),
                    "play" => sel.play(),
                    "pause" => sel.pause(),
                    "goIdle" => sel.go_idle(),
                    "togglePlayback" => sel.toggle_playback(),
                    other => {
                        return Err(TabsError::Thrown(JsError::type_error(format!(
                            "unhandled selection call {other}"
                        ))));
                    }
                }
            }
            "set-assets" => {
                if asrs_js::json::stringify(&self.host.assets_raw) != asrs_js::json::stringify(data)
                {
                    let alert = assets_alert(&self.host.assets_raw, data);
                    self.host.assets = assets_from_js(data).map_err(unmodeled)?;
                    self.host.assets_raw = data.clone();
                    self.tabs.new_assets(&mut self.host)?;
                    if let Some(error) = alert.error {
                        self.host
                            .messages
                            .push(NamedMessage::new(error.name.name(), error.data));
                    }
                }
            }
            "restore-type-memory" => {
                self.host.type_memory = data.clone();
            }
            "video-data" => {
                for (uuid, controller) in self
                    .tabs
                    .controllers_with_uuid::<VideoController>(TabType::Video)
                {
                    controller.process_video_data(&mut self.host, data, uuid)?;
                }
            }
            "set-version" => set_platform(&mut self.host.selection, data),
            "set-2d-camera" => {
                if let Some(r) = self
                    .tabs
                    .selected_renderer_as::<Recorder>(TabType::Field2d)?
                    .and_then(|r| r.inner.downcast_mut::<Field2dRenderer>())
                {
                    r.set_2d_camera(&self.host, data.as_f64().unwrap_or(f64::NAN))?;
                }
            }
            "set-fullscreen"
            | "set-battery"
            | "show-update-button"
            | "show-feedback-button"
            | "show-when-ready"
            | "toggle-sidebar"
            | "set-playback-options" => {}
            other => {
                return Err(TabsError::Thrown(JsError::type_error(format!(
                    "unhandled main message {other}"
                ))));
            }
        }
        for message in self.tabs.take_messages() {
            self.emit_main(message.name.name(), js_json::from_js(&message.data));
        }
        self.sync_controls_order(name == "restore-state");
        Ok(())
    }

    fn dispatch_event(&mut self, event: &Json) -> Result<(), String> {
        let etype = event
            .get("type")
            .and_then(Json::as_str)
            .ok_or_else(|| err("event needs a type"))?
            .to_string_lossy();
        let target = event
            .get("target")
            .and_then(Json::as_str)
            .ok_or_else(|| err("event needs a target"))?
            .to_string_lossy();
        let index = event.get("index").and_then(Json::as_f64).unwrap_or(0.0) as usize;
        let kind = event
            .get("kind")
            .and_then(Json::as_str)
            .map_or_else(|| "Event".to_owned(), |k| k.to_string_lossy());
        let raw_init = event.get("init").unwrap_or(&Json::Null);
        // `new window[kind](type, init)`: a constructor reads only its own dictionary members.
        let init = InitDict {
            raw: raw_init,
            kind: &kind,
        };
        let iv = |name: &str| init.get(name).and_then(Json::as_f64).unwrap_or(0.0);
        let (x, y) = (iv("clientX"), iv("clientY"));
        let props = event.get("props");
        // `div.controls-content > :not([hidden])` scopes a selector to the selected
        // tab's controls; without it matches span every tab's, then the templates.
        let scoped = target.starts_with("div.controls-content > :not([hidden])");
        self.sync_dom();

        match (target.as_str(), etype.as_str()) {
            ("window", "drag-update") => {
                let detail = init
                    .get("detail")
                    .ok_or_else(|| err("drag-update needs detail"))?;
                let detail = js_json::to_js(&Value::from_json_lenient(detail))?;
                self.dispatch_drag(&detail);
            }
            ("window", _) => self.window_mouse(&etype, x, y),
            ("div.renderer-content > :not([hidden])", _) => {
                self.root_event(&etype, x, y);
                self.window_mouse(&etype, x, y);
            }
            (t, "mousemove" | "mouseleave") if t.ends_with("tbody") => {
                self.body_event(&etype, y);
                self.window_mouse(&etype, x, y);
            }
            (t, "input") if t.contains("data-table-jump-input") => self.set_jump_value(props),
            (t, "keydown") if t.contains("data-table-jump-input") => {
                self.set_jump_value(props);
                if init
                    .get("code")
                    .and_then(Json::as_str)
                    .map(|c| c.to_string_lossy())
                    == Some("Enter".to_owned())
                {
                    self.jump();
                }
            }
            (t, "click") if t.contains("data-table-jump-button") => self.jump(),
            // Match 0 is the hidden header template's button; match i + 1 closes field i.
            (t, "click") if t.contains("data-table-key-delete") => {
                if let Some(field) = index.checked_sub(1) {
                    self.close_field(Some(field as f64));
                }
            }
            (t, "click") if t.contains("console-table-key-delete") => self.close_field(None),
            (t, "click") if t.contains("highlight-button") => self.console_highlight_click(),
            // Match 0 is the header row; match i + 1 is data row i.
            (t, "click" | "contextmenu") if t.ends_with(" tr") => {
                if let Some(row) = index.checked_sub(1) {
                    self.row_event(&etype, row);
                }
            }
            (t, "scroll") => self.scroll_event(t, props),
            (t, "input") if t.contains("input.filter") => self.console_filter_input(props)?,
            // `asrs-tabs/tests/dom_tab_views.rs` replays the console's jump.
            (t, "keydown" | "click") if t.contains("console-table-jump") => {}
            (t, "change") if t.contains("input.max-speed") => {
                self.swerve_change(SwerveInput::MaxSpeed, index, scoped, props)?
            }
            (t, "change") if t.contains("input.size-x") => {
                self.swerve_change(SwerveInput::SizeX, index, scoped, props)?
            }
            (t, "change") if t.contains("input.size-y") => {
                self.swerve_change(SwerveInput::SizeY, index, scoped, props)?
            }
            (t, "change") if t.contains(".field-3d-settings .field-select") => {
                self.field3d_select_change(index, scoped, props)?
            }
            (t, "click") if t.contains(".field-3d-settings .xr-button") => {
                self.field3d_xr_click(index, scoped)?
            }
            (t, "keydown") if t.contains(".field-3d-settings .xr-button") => {
                if event_key_clicks(&init)? {
                    self.field3d_xr_click(index, scoped)?;
                }
            }
            (t, "click") if t.contains("orientation-switcher") => {
                self.orientation_click(index, scoped)?
            }
            (t, "keydown") if t.contains("orientation-switcher") => {
                if event_key_clicks(&init)? {
                    self.orientation_click(index, scoped)?;
                }
            }
            (t, "change") if t.contains("input.dimensions-width") => {
                self.points_change(Dimension::Width, index, scoped, props)?
            }
            (t, "change") if t.contains("input.dimensions-height") => {
                self.points_change(Dimension::Height, index, scoped, props)?
            }
            (t, "change") if t.contains("select.orientation") => {
                self.points_select(PointsSelect::Orientation, index, scoped, props)?
            }
            (t, "change") if t.contains("select.origin") => {
                self.points_select(PointsSelect::Origin, index, scoped, props)?
            }
            (t, "change") if t.contains("table.joystick-selector select") => {
                self.joystick_change(index, scoped, props)?
            }
            // One `button.hide` and one `button.remove` per `.source-list .item`, so `index`
            // is the item's position in the list.
            (t, "click") if t.contains(".source-list .list .item button.hide") => {
                self.source_item_click(index, false, scoped)?
            }
            (t, "click") if t.contains(".source-list .list .item button.remove") => {
                self.source_item_click(index, true, scoped)?
            }
            // None of these has a keydown listener that acts on what the fixtures type; the
            // event bubbles to the window's.
            (
                ".max-speed" | ".size-y" | ".dimensions-width" | ".side-bar-search" | ".filter"
                | "select.origin",
                "keydown",
            ) if init
                .get("code")
                .and_then(Json::as_str)
                .is_some_and(|c| c.to_string_lossy() == "Space") => {}
            // Video controls: `index` is the querySelectorAll position inside the
            // selected tab's controls (`div.controls-content > :not([hidden])`).
            (t, "click") if t.contains(".video-source button") => self.video_source_click(index)?,
            (t, "click") if t.contains(".video-timeline-section > button") => {
                self.video_lock_click()?
            }
            (t, "click") if t.contains(".video-timeline-controls button") => {
                self.video_transport_click(index)?
            }
            (t, "input") if t.contains("input.video-timeline-slider") => {
                self.video_slider_input(props)?
            }
            _ => return Err(format!("event {etype} on {target} is not modeled")),
        }
        // Every keydown bubbles to `SelectionImpl`'s window listener after the target's own.
        if etype == "keydown" {
            let flag = |name: &str| matches!(init.get(name), Some(Json::Bool(true)));
            let code = init
                .get("code")
                .and_then(Json::as_str)
                .map(|c| c.to_string_lossy())
                .unwrap_or_default();
            let key_target = key_target_of(&target)?;
            // `window.tabs` reads that SelectionImpl resolves per keydown upstream:
            // `getSelectedTabType()` and `isUnlockedVideoSelected()`.
            {
                let mut ctx = self.host.selection.context().clone();
                if let Some(i) = self.tabs.selected_index()
                    && let Some(tab) = self.tabs.tab(i)
                {
                    if let Some(kind) = tab.tab_type() {
                        ctx.selected_tab_type = kind;
                    }
                    ctx.unlocked_video_selected = tab.tab_type() == Some(TabType::Video)
                        && tab
                            .controller()
                            .downcast_ref::<VideoController>()
                            .is_some_and(|v| !v.is_locked());
                }
                self.host.selection.set_context(ctx);
            }
            let command_held = if self.host.selection.context().platform == "darwin" {
                flag("metaKey")
            } else {
                flag("ctrlKey")
            };
            self.host
                .selection
                .key_down(&code, flag("metaKey"), flag("ctrlKey"), key_target);
            // VideoController's window listener was registered when the tab was
            // created, after SelectionImpl's, so it runs second.
            if key_target == KeyTarget::Other && !command_held {
                let is_visible_video = self
                    .tabs
                    .selected_index()
                    .and_then(|i| self.tabs.tab(i))
                    .is_some_and(|t| t.tab_type() == Some(TabType::Video) && !t.content_hidden());
                if is_visible_video
                    && let Some(tab) = self.tabs.selected_tab_mut()
                    && let Some(video) = tab.controller.downcast_mut::<VideoController>()
                {
                    video.key_down(&mut self.host, &code);
                }
            }
        }
        Ok(())
    }

    /// The hub's window listeners that drive a drag session (`dragMove`, `dragEnd`).
    fn window_mouse(&mut self, etype: &str, x: f64, y: f64) {
        let end = match etype {
            "mousemove" => false,
            "mouseup" | "mouseleave" | "touchend" => true,
            _ => return,
        };
        let data = if end {
            self.active_drag.take()
        } else {
            self.active_drag.clone()
        };
        if let Some(data) = data {
            let mut detail = JsObject::new();
            detail.insert("data", data);
            detail.insert("end", JsValue::Bool(end));
            detail.insert("x", JsValue::Number(x));
            detail.insert("y", JsValue::Number(y));
            self.dispatch_drag(&JsValue::Object(detail));
        }
    }

    fn selected_table_dom(&self) -> Option<Rc<RefCell<TableDom>>> {
        match self.tab_dom(self.tabs.selected_index()?) {
            DomKind::Table(dom) => Some(dom),
            _ => None,
        }
    }

    fn selected_table_renderer(&mut self) -> Option<&mut TableRenderer> {
        let index = JsValue::Number(self.tabs.selected_index()? as f64);
        self.tabs
            .renderer_mut(&index)
            .ok()
            .and_then(|r| inner_mut::<TableRenderer>(r))
    }

    fn set_jump_value(&mut self, props: Option<&Json>) {
        let value = props
            .and_then(|p| p.get("value"))
            .and_then(|v| js_json::to_js(&Value::from_json_lenient(v)).ok())
            .and_then(JsValue::into_string);
        if let (Some(value), Some(dom)) = (value, self.selected_table_dom()) {
            dom.borrow_mut().jump_input.set_value(value);
        }
    }

    fn scroll_event(&mut self, target: &str, props: Option<&Json>) {
        let scroll_top = props
            .and_then(|p| p.get("scrollTop"))
            .and_then(Json::as_f64)
            .unwrap_or(f64::NAN);
        if let Some(i) = self.tabs.selected_index() {
            match self.tab_dom(i) {
                DomKind::Console(dom) if target.contains("console-table") => {
                    dom.borrow_mut().scroll_top = scroll_top;
                }
                _ => {}
            }
        }
    }

    fn body_event(&mut self, etype: &str, y: f64) {
        if let Some(r) = self.selected_table_renderer() {
            match etype {
                "mousemove" => r.body_mouse_move(y),
                _ => r.body_mouse_leave(),
            }
        }
    }

    fn row_event(&mut self, etype: &str, row: usize) {
        let Some(i) = self.tabs.selected_index() else {
            return;
        };
        let index = JsValue::Number(i as f64);
        if let Some(r) = self
            .tabs
            .renderer_mut(&index)
            .ok()
            .and_then(|r| inner_mut::<TableRenderer>(r))
        {
            match etype {
                "click" => r.row_click(&mut self.host, row),
                _ => r.row_context_menu(&mut self.host),
            }
        }
    }

    fn jump(&mut self) {
        let Some(i) = self.tabs.selected_index() else {
            return;
        };
        let index = JsValue::Number(i as f64);
        if let Some(r) = self
            .tabs
            .renderer_mut(&index)
            .ok()
            .and_then(|r| inner_mut::<TableRenderer>(r))
        {
            r.jump(&mut self.host);
        }
    }

    fn close_field(&mut self, index: Option<f64>) {
        let Some(i) = self.tabs.selected_index() else {
            return;
        };
        let ti = JsValue::Number(i as f64);
        match self.tabs.tab(i).and_then(|t| t.tab_type()) {
            Some(TabType::Table) => {
                if let Some(c) = self
                    .tabs
                    .controller_mut(&ti)
                    .ok()
                    .and_then(|c| c.downcast_mut::<TableController>())
                {
                    c.close_field(&index.map_or(JsValue::Undefined, JsValue::Number));
                }
            }
            Some(TabType::Console) => {
                if let Some(c) = self
                    .tabs
                    .controller_mut(&ti)
                    .ok()
                    .and_then(|c| c.downcast_mut::<ConsoleController>())
                {
                    c.close_field();
                }
            }
            _ => {}
        }
    }

    /// `props.value`, assigned to the target before the event is dispatched.
    fn prop_value(props: Option<&Json>) -> Result<Option<JsString>, String> {
        match props.and_then(|p| p.get("value")) {
            None => Ok(None),
            Some(v) => v
                .as_str()
                .map(|v| Some(js(&v.to_string_lossy())))
                .ok_or_else(|| err("props.value must be a string")),
        }
    }

    fn controller_as<T: 'static>(&mut self, tab: usize) -> Result<&mut T, String> {
        self.tabs
            .controller_mut(&JsValue::Number(tab as f64))
            .map_err(|e| err(e.to_string()))?
            .downcast_mut::<T>()
            .ok_or_else(|| err(format!("tab {tab} is not a {}", std::any::type_name::<T>())))
    }

    /// The tabs whose controls a selector reaches, in document order: the selected one when the
    /// selector starts at `div.controls-content > :not([hidden])`, every one otherwise.
    fn controls_tabs(&mut self, scoped: bool) -> Vec<usize> {
        if scoped {
            return self.tabs.selected_index().into_iter().collect();
        }
        self.sync_controls_order(false);
        let ids = self.tab_ids();
        self.controls_order
            .iter()
            .filter_map(|id| ids.iter().position(|i| i == id))
            .collect()
    }

    /// `div.controls-content > :not([hidden])` scopes a selector to the selected tab's
    /// controls; without it, matches span every tab's controls in `controls_order`
    /// (`count` of them per `kinds` tab), then `templates` more in the `#controllerN`
    /// templates, which have no listeners. The answer is (tab, index within its
    /// controls), or `None` when the match lands in a template.
    fn controls_match(
        &mut self,
        index: usize,
        scoped: bool,
        kinds: &[TabType],
        count: impl Fn(&dyn TabController<Host>) -> usize,
        templates: usize,
        what: &str,
    ) -> Result<Option<(usize, usize)>, String> {
        let order = self.controls_tabs(scoped);
        let mut at = index;
        for i in order {
            let Some(tab) = self.tabs.tab(i) else {
                continue;
            };
            if !tab.tab_type().is_some_and(|k| kinds.contains(&k)) {
                continue;
            }
            let n = count(tab.controller());
            if at < n {
                return Ok(Some((i, at)));
            }
            at -= n;
        }
        if !scoped && at < templates {
            return Ok(None);
        }
        Err(err(format!("no {what} match {index}")))
    }

    fn swerve_change(
        &mut self,
        input: SwerveInput,
        index: usize,
        scoped: bool,
        props: Option<&Json>,
    ) -> Result<(), String> {
        let value = Self::prop_value(props)?;
        let Some((tab, _)) = self.controls_match(
            index,
            scoped,
            &[TabType::Swerve],
            |_| 1,
            1,
            "a Swerve input",
        )?
        else {
            return Ok(());
        };
        let controller = self.controller_as::<SwerveController>(tab)?;
        if let Some(value) = value {
            controller.input_mut(input).set_value(value);
        }
        controller.input_changed(input);
        Ok(())
    }

    /// `.orientation-switcher > div`: two children per switcher, on the controls of both
    /// Swerve and Field2d tabs; `#controller2` and `#controller9` hold a switcher each.
    fn orientation_click(&mut self, index: usize, scoped: bool) -> Result<(), String> {
        let Some((tab, child)) = self.controls_match(
            index,
            scoped,
            &[TabType::Swerve, TabType::Field2d],
            |_| 2,
            4,
            "an orientation switcher",
        )?
        else {
            return Ok(());
        };
        match self.tabs.tab(tab).and_then(|t| t.tab_type()) {
            Some(TabType::Swerve) => match child {
                0 => self.controller_as::<SwerveController>(tab)?.rotate_left(),
                1 => self.controller_as::<SwerveController>(tab)?.rotate_right(),
                _ => {
                    return Err(err(format!("orientation switcher has no child {child}")));
                }
            },
            Some(TabType::Field2d) if child < 2 => self
                .controller_as::<Field2dController>(tab)?
                .click_orientation(child),
            _ => {
                return Err(err(format!("orientation switcher has no child {child}")));
            }
        }
        Ok(())
    }

    fn points_change(
        &mut self,
        dimension: Dimension,
        index: usize,
        scoped: bool,
        props: Option<&Json>,
    ) -> Result<(), String> {
        let value = Self::prop_value(props)?;
        let Some((tab, _)) = self.controls_match(
            index,
            scoped,
            &[TabType::Points],
            |_| 1,
            1,
            "a Points input",
        )?
        else {
            return Ok(());
        };
        let controller = self.controller_as::<PointsController>(tab)?;
        if let Some(value) = value {
            controller.dimension_mut(dimension).set_value(value);
        }
        controller.dimension_changed(dimension);
        Ok(())
    }

    /// The selects have no `change` listener; only the assigned value matters.
    fn points_select(
        &mut self,
        select: PointsSelect,
        index: usize,
        scoped: bool,
        props: Option<&Json>,
    ) -> Result<(), String> {
        let value = Self::prop_value(props)?;
        let Some((tab, _)) = self.controls_match(
            index,
            scoped,
            &[TabType::Points],
            |_| 1,
            1,
            "a Points select",
        )?
        else {
            return Ok(());
        };
        let controller = self.controller_as::<PointsController>(tab)?;
        if let Some(value) = value {
            controller.select_mut(select).set_value(&value);
        }
        Ok(())
    }

    /// The selects have no `change` listener; only the assigned value matters.
    fn joystick_change(
        &mut self,
        index: usize,
        scoped: bool,
        props: Option<&Json>,
    ) -> Result<(), String> {
        let value = Self::prop_value(props)?;
        // Each Joysticks tab holds one `select` per joystick; the `#controller8`
        // template holds 6.
        let Some((tab, at)) = self.controls_match(
            index,
            scoped,
            &[TabType::Joysticks],
            |controller| {
                controller
                    .downcast_ref::<JoysticksController>()
                    .map_or(0, |c| (0..).take_while(|&i| c.select(i).is_some()).count())
            },
            6,
            "a joystick select",
        )?
        else {
            return Ok(());
        };
        let controller = self.controller_as::<JoysticksController>(tab)?;
        let select = controller
            .select_mut(at)
            .ok_or_else(|| err(format!("no joystick select {at}")))?;
        if let Some(value) = value {
            select.set_value(&value);
        }
        Ok(())
    }

    /// The tab whose controls hold match `index` of a selector inside `.field-3d-settings`.
    /// `None` is the match one past the tabs: the hidden `#controller3` template after
    /// `.controls-content`, which has no listeners.
    fn field3d_controls_tab(
        &mut self,
        index: usize,
        scoped: bool,
    ) -> Result<Option<usize>, String> {
        Ok(self
            .controls_match(
                index,
                scoped,
                &[TabType::Field3d],
                |_| 1,
                1,
                ".field-3d-settings",
            )?
            .map(|(tab, _)| tab))
    }

    fn tab_ids(&self) -> Vec<usize> {
        self.tabs
            .tabs()
            .map(|t| t.controller() as *const dyn TabController<Host> as *const () as usize)
            .collect()
    }

    /// Runs after every step, so a closed tab's address cannot be reused unseen, except
    /// inside `restoreState`, which re-adds every tab in `tabList` order (`rebuilt`).
    fn sync_controls_order(&mut self, rebuilt: bool) {
        let ids = self.tab_ids();
        if rebuilt {
            self.controls_order = ids;
            return;
        }
        self.controls_order.retain(|id| ids.contains(id));
        for id in ids {
            if !self.controls_order.contains(&id) {
                self.controls_order.push(id);
            }
        }
    }

    fn field3d_select_change(
        &mut self,
        index: usize,
        scoped: bool,
        props: Option<&Json>,
    ) -> Result<(), String> {
        let value = Self::prop_value(props)?;
        let Some(tab) = self.field3d_controls_tab(index, scoped)? else {
            return Ok(());
        };
        let controller = field3d_controller(&mut self.tabs, tab)?;
        if let Some(value) = value {
            controller.set_field_value(&value);
        }
        controller
            .field_select_change(&mut self.host)
            .map_err(|e| format!("{}: {}", e.name, e.message))
    }

    fn field3d_xr_click(&mut self, index: usize, scoped: bool) -> Result<(), String> {
        let Some(tab) = self.field3d_controls_tab(index, scoped)? else {
            return Ok(());
        };
        let uuid = self.tabs.tab(tab).map(|t| t.uuid().clone()).unwrap();
        field3d_controller(&mut self.tabs, tab)?.xr_click(&mut self.host, &uuid);
        Ok(())
    }

    /// `.source-list .list .item button.*`: one match per item of every list the selector
    /// can reach: the selected tab's lists when it is scoped, every tab's in
    /// `controls_order` otherwise (the `#controllerN` lists are empty, so they match
    /// nothing). The click goes to the list that owns the match.
    fn source_item_click(
        &mut self,
        index: usize,
        remove: bool,
        scoped: bool,
    ) -> Result<(), String> {
        let order = self.controls_tabs(scoped);
        let mut at = index;
        for i in order {
            let Some(tab) = self.tabs.tab(i) else {
                continue;
            };
            let lists: Vec<(String, usize)> = tab
                .controller()
                .source_lists()
                .iter()
                .map(|list| (list.uuid().to_owned(), list.items().len()))
                .collect();
            for (uuid, count) in lists {
                if at < count {
                    let event = if remove {
                        ListPointer::RemoveClick(at)
                    } else {
                        ListPointer::HideClick(at)
                    };
                    let mut errors = Vec::new();
                    self.tabs
                        .controller_mut(&JsValue::Number(i as f64))
                        .map_err(|e| err(e.to_string()))?
                        .source_list_input(
                            &mut self.host,
                            ListInput::Pointer { uuid: &uuid, event },
                            &mut errors,
                        );
                    return match errors.into_iter().next() {
                        Some(e) => Err(e.message),
                        None => Ok(()),
                    };
                }
                at -= count;
            }
        }
        Err(err(format!("no .source-list .list .item match {index}")))
    }

    fn console_highlight_click(&mut self) {
        let Some(i) = self.tabs.selected_index() else {
            return;
        };
        let index = JsValue::Number(i as f64);
        if let Some(r) = self
            .tabs
            .renderer_mut(&index)
            .ok()
            .and_then(|r| inner_mut::<ConsoleRenderer>(r))
        {
            r.highlight_click(&self.host)
                .expect("the console rows format");
        }
    }

    fn console_filter_input(&mut self, props: Option<&Json>) -> Result<(), String> {
        let value = Self::prop_value(props)?.ok_or_else(|| err("filter input sets no value"))?;
        let i = self
            .tabs
            .selected_index()
            .ok_or_else(|| err("no tab is selected"))?;
        self.tabs
            .renderer_mut(&JsValue::Number(i as f64))
            .ok()
            .and_then(|r| inner_mut::<ConsoleRenderer>(r))
            .ok_or_else(|| err("filter input without a Console tab"))?
            .filter_input(&self.host, &value)
            .map_err(|e| format!("{}: {}", e.name, e.message))
    }

    /// `.video-source button` clicks on the selected Video tab. `index` is the
    /// `children[]` position: 0 local, 1 YouTube, 2 TBA (which also reads the
    /// button's `getBoundingClientRect`).
    fn video_source_click(&mut self, index: usize) -> Result<(), String> {
        if index == 2 {
            let rect = self.layout.rect_of(&video_source_button_el(3));
            let Some(tab) = self.tabs.selected_tab_mut() else {
                return Err("video source click with no selected tab".to_owned());
            };
            let Some(video) = tab.controller.downcast_mut::<VideoController>() else {
                return Err("video source click on a non-Video selected tab".to_owned());
            };
            video.select_tba(&mut self.host, tab.uuid, [rect.right(), rect.top()]);
            Ok(())
        } else {
            let Some(tab) = self.tabs.selected_tab_mut() else {
                return Err("video source click with no selected tab".to_owned());
            };
            let Some(video) = tab.controller.downcast_mut::<VideoController>() else {
                return Err("video source click on a non-Video selected tab".to_owned());
            };
            match index {
                0 => video.select_local(&mut self.host, tab.uuid),
                _ => video.select_youtube(&mut self.host, tab.uuid),
            }
            Ok(())
        }
    }

    /// `.video-timeline-section > button` clicks: LOCK_BUTTON / UNLOCK_BUTTON both
    /// call `toggleLock`.
    fn video_lock_click(&mut self) -> Result<(), String> {
        let Some(tab) = self.tabs.selected_tab_mut() else {
            return Err("video lock click with no selected tab".to_owned());
        };
        let Some(video) = tab.controller.downcast_mut::<VideoController>() else {
            return Err("video lock click on a non-Video selected tab".to_owned());
        };
        video.toggle_lock(&mut self.host);
        Ok(())
    }

    /// `.video-timeline-controls button` clicks. `index` is the `children[]` position:
    /// skip-back, frame-back, play, pause, frame-forward, skip-forward.
    fn video_transport_click(&mut self, index: usize) -> Result<(), String> {
        let Some(tab) = self.tabs.selected_tab_mut() else {
            return Err("video transport click with no selected tab".to_owned());
        };
        let Some(video) = tab.controller.downcast_mut::<VideoController>() else {
            return Err("video transport click on a non-Video selected tab".to_owned());
        };
        match index {
            0 => video.skip_time(&mut self.host, -5.0),
            1 => video.change_frame(-1.0),
            2 | 3 => video.toggle_play_pause(&mut self.host),
            4 => video.change_frame(1.0),
            _ => video.skip_time(&mut self.host, 5.0),
        }
        Ok(())
    }

    /// `input` on `input.video-timeline-slider`: the browser has already assigned the
    /// value (sanitized), then the listener re-anchors playback when playing.
    fn video_slider_input(&mut self, props: Option<&Json>) -> Result<(), String> {
        let Some(tab) = self.tabs.selected_tab_mut() else {
            return Err("video slider input with no selected tab".to_owned());
        };
        let Some(video) = tab.controller.downcast_mut::<VideoController>() else {
            return Err("video slider input on a non-Video selected tab".to_owned());
        };
        if let Some(value) = Self::prop_value(props)? {
            video.set_slider(value);
        }
        video.slider_input(&mut self.host);
        Ok(())
    }

    /// The canvases drawn since the last capture, in context creation order.
    fn take_drawings(&mut self) -> Vec<Drawing> {
        self.sync_controls_order(false);
        let ids = self.tab_ids();
        let mut out = Vec::new();
        for id in &self.controls_order {
            let recorder = ids
                .iter()
                .position(|i| i == id)
                .and_then(|i| self.tabs.tab(i))
                .and_then(|tab| tab.renderer().downcast_ref::<Recorder>());
            if let Some(recorder) = recorder
                && let Some(drawing) = recorder.drawing.borrow_mut().as_mut()
                && std::mem::take(&mut drawing.drawn_since_capture)
            {
                out.push(drawing.clone());
            }
        }
        out
    }

    fn capture(&mut self, spec: &Json, label: Option<String>) -> Result<(), String> {
        self.emitted.push(Emit::Record {
            values: vec![
                ("capture", cve_str(&label.unwrap_or_default())),
                ("time", Value::number(self.now - self.start)),
            ],
        });
        let kinds: Vec<&Json> = match spec.as_array() {
            Some(list) => list.iter().collect(),
            None => vec![spec],
        };
        for kind in kinds {
            let kind_name = match kind {
                Json::String(s) => s.to_string_lossy(),
                Json::Object(_) => kind
                    .get("kind")
                    .and_then(Json::as_str)
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default(),
                other => {
                    return Err(err(format!(
                        "capture {} is not modeled",
                        other.to_compact()
                    )));
                }
            };
            match kind_name.as_str() {
                "commands" => {
                    for i in 0..self.tabs.len() {
                        let Some(tab) = self.tabs.tab(i) else {
                            continue;
                        };
                        let Some(uuid) = tab.uuid().as_str().map(|s| s.to_string_lossy()) else {
                            continue;
                        };
                        let slot = tab
                            .renderer()
                            .downcast_ref::<Recorder>()
                            .map(|r| r.slot.clone())
                            .ok_or_else(|| err(format!("tab {i} has no recording renderer")))?;
                        let Some(command) = slot.borrow_mut().take() else {
                            continue;
                        };
                        let meta = Value::object([
                            ("index", Value::number(i as f64)),
                            ("type", js_json::from_js(tab.type_value())),
                            ("title", js_json::from_js(tab.title())),
                            ("uuid", js_json::from_js(&JsValue::String(js(&uuid)))),
                        ]);
                        self.emitted.push(Emit::Record {
                            values: vec![("command", meta), ("value", js_json::from_js(&command))],
                        });
                    }
                }
                "state" => {
                    let sidebar = Value::object([
                        ("width", Value::number(self.sidebar_width)),
                        (
                            "expanded",
                            Value::Array(
                                self.sidebar_expanded
                                    .iter()
                                    .map(|e| Value::String(Utf16String::from(e.to_string_lossy())))
                                    .collect(),
                            ),
                        ),
                    ]);
                    let state = Value::object([
                        ("sidebar", sidebar),
                        ("tabs", js_json::from_js(&self.tabs.save_state(&self.host))),
                    ]);
                    self.emitted.push(Emit::Record {
                        values: vec![("state", state)],
                    });
                }
                "selection" => {
                    let value = selection_value(&mut self.host.selection);
                    self.emitted.push(Emit::Record {
                        values: vec![("selection", value)],
                    });
                }
                "eval" => {
                    let expr = kind
                        .get("expr")
                        .and_then(Json::as_str)
                        .map(|s| s.to_string_lossy())
                        .unwrap_or_default();
                    match run_eval(self, &expr) {
                        Ok(value) => self.emitted.push(Emit::Record {
                            values: vec![
                                ("eval", cve_str(&expr)),
                                ("value", js_json::from_js(&value)),
                            ],
                        }),
                        Err(e) => self.emitted.push(Emit::Record {
                            values: vec![("eval", cve_str(&expr)), ("throw", cve_str(&e))],
                        }),
                    }
                }
                "dom" => {
                    let selector = kind
                        .get("selector")
                        .and_then(Json::as_str)
                        .map(|s| s.to_string_lossy())
                        .ok_or_else(|| err("dom capture has no selector"))?;
                    self.emitted.push(Emit::Record {
                        values: vec![("dom", cve_str(&selector))],
                    });
                }
                "canvas" => {
                    if kind.get("selector").is_some() {
                        return Err(err("a canvas capture with a selector is not modeled"));
                    }
                    let drawings = self.take_drawings();
                    self.emitted.push(Emit::Canvases(drawings));
                }
                other => return Err(err(format!("capture kind {other} is not modeled"))),
            }
        }
        Ok(())
    }
}

/// Fixtures that ran on Blink with real geometry and hold one Table tab; their other geometry
/// records are the sidebar's, tab bar's and timeline's, which other models replay.
const TABLE_FIXTURES: [&str; 7] = [
    "dom/tabs/table",
    "dom/tabs/table-edit",
    "dom/tabs/table-ranges",
    "dom/tabs/table-resize",
    "dom/tabs/table-wide",
    "dom/tabs/table-wide-2x",
    "dom/tabs/table-wide-hidpi",
];

/// `div.data-table-container` in the fixtures' 1100 x 650 window.
const TABLE_CONTAINER: Rect = Rect {
    x: 300.0,
    y: 88.0,
    width: 800.0,
    height: 562.0,
};

fn table_reads(records: &[Record]) -> Result<SharedTable, String> {
    let mut on_table = Vec::new();
    for record in records {
        let geometry = GeometryRecord::parse(record)?;
        if geometry.is_some_and(|g| g.el.contains("div.data-table-container:")) {
            on_table.push(record.clone());
        }
    }
    let table = SharedTable::default();
    table.0.borrow_mut().reads = ScriptedReads::from_records(&on_table)?;
    Ok(table)
}

fn compare(spec: &Path, vector_id: &str) -> Result<usize, String> {
    let records = vectors::load(vector_id)?;
    let table = TABLE_FIXTURES
        .contains(&vector_id)
        .then(|| table_reads(&records))
        .transpose()?;
    let reads = (table.is_none() && real_geometry(spec))
        .then(|| console_reads(&records))
        .transpose()?;
    let emitted = replay(spec, table.clone(), reads.clone())?;
    let mut tally = Tally::new(vector_id, &records, kind_of);
    let checked = (|| -> Result<(), String> {
        if let Some(table) = table {
            let mut t = table.0.borrow_mut();
            if let Some(mismatch) = t.mismatches.first() {
                return Err(format!("{} mismatches, {mismatch}", t.mismatches.len()));
            }
            let compared = std::mem::take(&mut t.reads).finish()?;
            if compared < 20 || t.checked < 20 {
                return Err(format!("only {compared} geometry records compared"));
            }
            (0..compared).for_each(|_| tally.compared(TAB_GEOMETRY));
        }
        if let Some(RecordedConsole(reads)) = reads {
            let compared = reads.take().finish()?;
            (0..compared).for_each(|_| tally.compared(TAB_GEOMETRY));
        }
        compare_emitted(&records, &emitted, &mut tally)
    })();
    if let Err(e) = checked {
        std::mem::forget(tally);
        return Err(e);
    }
    for (kind, by) in NOT_COMPARED {
        if records.iter().any(|r| kind_of(r) == kind) {
            tally.covered_elsewhere(kind, by);
        }
    }
    Ok(tally.finish())
}

/// Kinds whose payload no replay compares for these vectors (#397), with the suites that compare
/// the same production code on their own.
const NOT_COMPARED: [(&str, &str); 6] = [
    ("dom", "asrs-tabs's dom_tab_views, in part"),
    (HUB_GEOMETRY, "tabs_dom and sidebar_dom, in part"),
    ("canvas:timeline-canvas", "timeline_traces"),
    ("canvas:line-graph-canvas", "asrs-tabs's line_graph_dom"),
    ("canvas:heatmap-canvas", "asrs-tabs's heatmap_vectors"),
    (
        "canvas:canvas",
        "asrs-tabs's histogram_dom and heatmap_vectors",
    ),
];

fn compare_emitted(records: &[Record], emitted: &[Emit], tally: &mut Tally) -> Result<(), String> {
    let mut expected = records
        .iter()
        .filter(|r| ![TAB_GEOMETRY, HUB_GEOMETRY].contains(&kind_of(r).as_str()))
        .peekable();
    for emit in emitted {
        let values = match emit {
            Emit::Record { values } => values,
            Emit::Canvases(drawings) => {
                let mut drawings = drawings.iter();
                while let Some(record) = expected.next_if(|r| r.field("canvas").is_some()) {
                    let class = canvas_class(record);
                    if !COMPARED_CANVASES.contains(&class.as_str()) {
                        continue;
                    }
                    let drawing = drawings
                        .next()
                        .ok_or_else(|| format!("line {}: the port drew no {class}", record.line))?;
                    tally.compare(&kind_of(record), record, |fields| {
                        compare_canvas(fields, drawing)
                    })?;
                }
                if let Some(drawing) = drawings.next() {
                    return Err(format!(
                        "the port drew a {}, upstream did not",
                        drawing.class
                    ));
                }
                continue;
            }
        };
        let record = expected
            .next()
            .ok_or_else(|| err("port emitted more records than the vector has"))?;
        let compare_values = |fields: &Fields<'_>| -> Result<(), String> {
            for (key, value) in values {
                let expected_value = fields.value(key).map_err(|e| {
                    format!(
                        "line {} missing {key}: {e}; the port emitted {value:?}",
                        record.line
                    )
                })?;
                if let Some(report) = asrs_parity::diff::report(&expected_value, value) {
                    return Err(format!("line {} ({key}): {report}", record.line));
                }
            }
            Ok(())
        };
        let kind = kind_of(record);
        if kind == "dom" {
            compare_values(&Fields::new(record))?;
        } else {
            tally.compare(&kind, record, compare_values)?;
        }
    }
    match expected.next() {
        Some(record) => Err(format!(
            "line {}: the port emitted nothing for this record",
            record.line
        )),
        None => Ok(()),
    }
}

/// The oracle names a canvas passed to `drawImage` by its id, the port the heatmap's `@heatmap`.
fn name_heatmap(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(name_heatmap).collect()),
        Value::String(s) if s.to_string_lossy().starts_with("@canvas:heatmap-canvas#") => {
            Value::string("@heatmap")
        }
        other => other,
    }
}

/// Assigning `CANVAS.width` and then `CANVAS.height` resets the context twice a frame.
const RESETS_PER_FRAME: usize = 2;

fn compare_canvas(fields: &Fields<'_>, drawing: &Drawing) -> Result<(), String> {
    let id = fields.string("canvas")?;
    if id.split('#').next() != Some(drawing.class) {
        return Err(format!("{id}: the port drew a {}", drawing.class));
    }
    let calls = asrs_canvas::trace::encode(&drawing.canvas.calls)
        .lines()
        .map(|line| {
            json::parse(line)
                .map_err(|e| format!("port trace line {line}: {e:?}"))
                .and_then(|json| Value::from_json(&json).map_err(|e| e.0))
        })
        .collect::<Result<Vec<_>, String>>()?;
    for (key, value) in [
        (
            "frame",
            Value::number((drawing.frames * RESETS_PER_FRAME) as f64),
        ),
        ("width", Value::number(f64::from(drawing.canvas.width))),
        ("height", Value::number(f64::from(drawing.canvas.height))),
        ("calls", Value::Array(calls)),
    ] {
        let recorded = fields.value(key).map_err(|e| e.0)?;
        let recorded = name_heatmap(recorded);
        if let Some(report) = asrs_parity::diff::report(&recorded, &value) {
            return Err(format!("line {} {id} {key}: {report}", fields.line()));
        }
    }
    Ok(())
}

fn spec_files() -> Vec<(PathBuf, String)> {
    let dir = vectors::workspace_root()
        .join("parity/specs/dom")
        .join("tabs");
    let mut out: Vec<(PathBuf, String)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|n| n.ends_with(".entry.json"))
        })
        .map(|p| {
            let name = p
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .trim_end_matches(".entry.json")
                .to_owned();
            (p, format!("dom/tabs/{name}"))
        })
        .filter(|(p, id)| id != "dom/tabs/table-real" || !real_geometry(p))
        .collect();
    out.sort();
    out
}

/// Console fixtures answer layout from recorded reads; `asrs-tabs/tests/table_real_dom.rs`
/// replays `dom/tabs/table-real`.
fn real_geometry(spec: &Path) -> bool {
    let text = std::fs::read_to_string(spec).unwrap_or_else(|e| panic!("{}: {e}", spec.display()));
    let json = json::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", spec.display()));
    json.get("fixture")
        .and_then(|f| f.get("geometry"))
        .and_then(Json::as_str)
        .is_some_and(|g| g.to_string_lossy() == "real")
}

#[test]
fn dom_tabs_fixtures() {
    let specs = spec_files();
    assert!(!specs.is_empty(), "no dom/tabs specs");
    let mut failures = Vec::new();
    let mut compared = 0;
    for (spec, id) in &specs {
        match compare(spec, id) {
            Ok(n) => compared += n,
            Err(e) => failures.push(format!("{id}: {e}")),
        }
    }
    eprintln!(
        "dom/tabs: {} fixtures, {compared} records compared",
        specs.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
