#![allow(clippy::disallowed_methods)]

//! Replays `parity/specs/dom/tabs-model/<case>.entry.json` against `asrs_hub::tabs::Tabs` and
//! compares the emitted records with the committed `dom/tabs-model/<case>` vector, line for line.
//!
//! The vectors are Blink runs with real geometry. Every geometry read `Tabs` makes (title,
//! overlay, add button and renderer rects; the tab bar's `scrollLeft`, `scrollWidth` and
//! `clientWidth`) is answered, in order, from upstream's `read` records through
//! `asrs_parity::geometry::ScriptedReads`, and every `scrollLeft` assignment to the tab bar is
//! checked against upstream's `write` record. Each assignment's resulting offset, which the
//! app's tab bar computes with `assign_scroll_offset`, must equal the next recorded read. The
//! scroll overlay's own reads and writes are the DOM field `ScrollSensor` replaces: overlay
//! scrolls in the fixtures (`/*tabs:scroll dx dy*/`) reach the port as precise wheel input.
//! Geometry records of the sidebar, timeline and tab renderers belong to other models.
//!
//! Controllers and renderers are ported by other units, so every tab here gets stub halves.
//! The fixtures wrap upstream's `addTab` (`/*tabs:recorder*/`) to list each created tab with its
//! creation-time UUID, and every capture starts with `/*tabs:info*/`, which reports what each
//! created controller and renderer would save, its `showTimeline()` and its active fields. The
//! stubs answer from those records, keyed by creation order, and log every `restoreState` call
//! so `/*tabs:restores*/` checks that restores reach the right tab. Table renderers answer
//! `getTimestampRange()` with the values upstream's returned, in call order, and every
//! `getTimestampRange` and `addRendererRange` call is logged for `/*tabs:ranges*/`. Everything
//! `Tabs` owns (order, selection, titles, UUIDs, heights, the DOM it drives, messages, Table
//! range routing, exceptions) is the port's own output.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use asrs_hub::drag::Drag;
use asrs_hub::tabs::{TabDragData, TabFactory, TabParts, Tabs, TabsError, TabsHost};
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue};
use asrs_js::{math, number};
use asrs_log::log::Log;
use asrs_model::messages::NamedMessage;
use asrs_model::scroll_sensor::{Wheel, assign_scroll_offset};
use asrs_model::source_list::Rect;
use asrs_model::tab_controller::{Command, TabCommand, TabController, TabRenderer};
use asrs_model::tab_type::TabType;
use asrs_parity::cve1::Value;
use asrs_parity::geometry::{GeometryRecord, ScriptedReads};
use asrs_parity::json::Json;
use asrs_parity::utf16::Utf16String;
use asrs_parity::vectors::{self, Record};
use asrs_tabs::form::css::{Property, Style};

const FRAME_PERIOD: f64 = 16.0;
const OVERLAY_CLIENT_WIDTH: f64 = 636.0;
const OVERLAY_CLIENT_HEIGHT: f64 = 50.0;
const DEFAULT_IGNORED: [&str; 3] = ["save-state", "save-type-memory", "update-touch-bar-slider"];

fn js(s: &str) -> JsString {
    JsString::from(s)
}

fn cve_str(s: &str) -> Value {
    Value::String(Utf16String::from(s))
}

fn to_js(json: &Json) -> Result<JsValue, String> {
    js_json::to_js(&Value::from_json_lenient(json))
}

fn str_of(json: &Json) -> Option<String> {
    json.as_str().map(|s| s.to_string_lossy())
}

struct Seed {
    uuid: JsString,
    show_timeline: bool,
}

struct Current {
    controller: JsValue,
    renderer: JsValue,
    active_fields: Vec<JsString>,
}

struct Host {
    inner_height: f64,
    reads: RefCell<ScriptedReads>,
    bar: RefCell<BarScroll>,
    device_pixel_ratio: f64,
    seeds: Vec<Seed>,
    current: Vec<Current>,
    restores: Vec<(usize, &'static str, JsValue)>,
    renderer_uuids: Vec<Option<JsString>>,
    timestamp_ranges: RefCell<Vec<VecDeque<Option<[f64; 2]>>>>,
    range_calls: RefCell<Vec<JsValue>>,
    rendered: Vec<bool>,
    created: usize,
}

fn range_js(range: Option<[f64; 2]>) -> JsValue {
    range.map_or(JsValue::Null, |[a, b]| {
        JsValue::Array(vec![JsValue::Number(a), JsValue::Number(b)])
    })
}

fn range_of(value: &JsValue) -> Result<Option<[f64; 2]>, String> {
    match value {
        JsValue::Null => Ok(None),
        JsValue::Array(r) => match r.as_slice() {
            [JsValue::Number(a), JsValue::Number(b)] => Ok(Some([*a, *b])),
            _ => Err(format!("range {value:?}")),
        },
        other => Err(format!("range {other:?}")),
    }
}

/// The tab bar's scroll position as the app's host keeps it: each assignment and each layout
/// clamps and snaps it with `assign_scroll_offset`, against the extent upstream reads.
#[derive(Default)]
struct BarScroll {
    left: f64,
    scroll_width: f64,
    client_width: f64,
    last_read: Option<f64>,
    checks: usize,
    error: Option<String>,
}

impl BarScroll {
    fn check(&mut self, upstream: f64, port: f64) {
        self.checks += 1;
        if upstream.to_bits() != port.to_bits() && self.error.is_none() {
            self.error = Some(format!(
                "the tab bar's scrollLeft is {port} in the port, {upstream} upstream"
            ));
        }
    }

    fn relayout(&mut self, device_pixel_ratio: f64) {
        self.left = assign_scroll_offset(
            self.left,
            self.scroll_width - self.client_width,
            device_pixel_ratio,
        );
    }
}

const TAB_BAR: &str = "div.tab-bar:0";
const OVERLAY: &str = "div.tab-bar-scroll:4";
const RENDERER: &str = "div.renderer-content:5";
const ADD_BUTTON: &str = "button.add-tab:10";

fn rect_of([x, y, width, height]: [f64; 4]) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// Whether a geometry record is one `Tabs` makes: everything on the tab bar and its titles,
/// the renderer and add button rects, and the overlay's rect. The overlay's scroll position
/// reads and writes are `ScrollSensor`'s DOM field.
fn owned_by_tabs(g: &GeometryRecord) -> bool {
    let segments: Vec<&str> = g.el.split('>').collect();
    let classes = |segment: &str| -> Vec<String> {
        let name = segment.rsplit_once(':').map_or(segment, |(n, _)| n);
        name.split('.').skip(1).map(str::to_owned).collect()
    };
    let last = classes(segments[segments.len() - 1]);
    let parent = segments
        .len()
        .checked_sub(2)
        .map(|i| classes(segments[i]))
        .unwrap_or_default();
    let has = |list: &[String], c: &str| list.iter().any(|x| x == c);
    has(&last, "tab-bar")
        || (has(&parent, "tab-bar") && has(&last, "tab"))
        || (has(&last, "tab-bar-scroll") && g.name == "getBoundingClientRect")
        || has(&last, "renderer-content")
        || has(&last, "add-tab")
}

impl TabsHost for Host {
    fn inner_height(&self) -> f64 {
        self.inner_height
    }

    fn renderer_top(&self, _show_timeline: bool) -> f64 {
        self.reads.borrow_mut().read_rect(RENDERER)[1]
    }

    fn title_rect(&self, index: usize) -> Rect {
        rect_of(
            self.reads
                .borrow_mut()
                .read_rect(&format!("{TAB_BAR}>div.tab:{index}")),
        )
    }

    fn overlay_rect(&self) -> Rect {
        rect_of(self.reads.borrow_mut().read_rect(OVERLAY))
    }

    fn add_button_rect(&self) -> Rect {
        rect_of(self.reads.borrow_mut().read_rect(ADD_BUTTON))
    }

    fn tab_bar_scroll_left(&self) -> f64 {
        let upstream = self.reads.borrow_mut().read_f64("scrollLeft", TAB_BAR);
        self.bar.borrow_mut().last_read = Some(upstream);
        upstream
    }

    fn set_tab_bar_scroll_left(&self, value: f64) {
        self.reads
            .borrow_mut()
            .write_f64("scrollLeft", TAB_BAR, value);
        let mut bar = self.bar.borrow_mut();
        if let Some(read) = bar.last_read.take() {
            let left = bar.left;
            bar.check(read, left);
        }
        bar.left = assign_scroll_offset(
            value,
            bar.scroll_width - bar.client_width,
            self.device_pixel_ratio,
        );
    }

    fn tab_bar_scroll_width(&self) -> f64 {
        let width = self.reads.borrow_mut().read_f64("scrollWidth", TAB_BAR);
        self.bar.borrow_mut().scroll_width = width;
        width
    }

    fn tab_bar_client_width(&self) -> f64 {
        let width = self.reads.borrow_mut().read_f64("clientWidth", TAB_BAR);
        let mut bar = self.bar.borrow_mut();
        bar.client_width = width;
        bar.relayout(self.device_pixel_ratio);
        if let Some(read) = bar.last_read.take() {
            let left = bar.left;
            bar.check(read, left);
        }
        width
    }
}

struct StubController(usize);
struct StubRenderer(usize, Option<JsString>);

impl TabController<Host> for StubController {
    fn save_state(&self, cx: &Host) -> JsValue {
        cx.current
            .get(self.0)
            .map_or(JsValue::Undefined, |c| c.controller.clone())
    }

    fn restore_state(&mut self, cx: &mut Host, state: &JsValue) -> Result<(), JsError> {
        cx.restores.push((self.0, "controller", state.clone()));
        Ok(())
    }

    fn refresh(&mut self, _cx: &mut Host) -> Result<(), JsError> {
        Ok(())
    }

    fn new_assets(&mut self, _cx: &mut Host) -> Result<(), JsError> {
        Ok(())
    }

    fn active_fields(&self, cx: &Host) -> Vec<JsString> {
        cx.current
            .get(self.0)
            .map_or_else(Vec::new, |c| c.active_fields.clone())
    }

    fn show_timeline(&self, cx: &Host) -> bool {
        cx.seeds.get(self.0).is_some_and(|s| s.show_timeline)
    }

    fn command(&mut self, _cx: &mut Host, _uuid: &JsValue) -> Result<Command, JsError> {
        Ok(Rc::new(JsValue::Null))
    }

    fn add_renderer_range(
        &mut self,
        cx: &mut Host,
        renderer_uuid: &JsString,
        range: Option<[f64; 2]>,
    ) -> Result<(), JsError> {
        cx.range_calls.borrow_mut().push(JsValue::Array(vec![
            JsValue::Number(self.0 as f64),
            JsValue::string("add"),
            JsValue::String(renderer_uuid.clone()),
            range_js(range),
        ]));
        Ok(())
    }
}

impl TabRenderer<Host> for StubRenderer {
    fn save_state(&self, cx: &Host) -> JsValue {
        cx.current
            .get(self.0)
            .map_or(JsValue::Undefined, |c| c.renderer.clone())
    }

    fn restore_state(&mut self, cx: &mut Host, state: &JsValue) -> Result<(), JsError> {
        cx.restores.push((self.0, "renderer", state.clone()));
        Ok(())
    }

    fn aspect_ratio(&self, _cx: &Host) -> Option<f64> {
        None
    }

    fn render(&mut self, cx: &mut Host, _command: &dyn TabCommand) -> Result<(), JsError> {
        if cx.rendered.len() <= self.0 {
            cx.rendered.resize(self.0 + 1, false);
        }
        cx.rendered[self.0] = true;
        Ok(())
    }

    fn renderer_uuid(&self) -> Option<&JsString> {
        self.1.as_ref()
    }

    fn timestamp_range(&self, cx: &Host) -> Option<[f64; 2]> {
        let range = cx
            .timestamp_ranges
            .borrow_mut()
            .get_mut(self.0)
            .and_then(VecDeque::pop_front)
            .flatten();
        cx.range_calls.borrow_mut().push(JsValue::Array(vec![
            JsValue::Number(self.0 as f64),
            JsValue::string("get"),
            range_js(range),
        ]));
        range
    }
}

struct StubFactory;

impl TabFactory<Host> for StubFactory {
    fn create(
        &mut self,
        cx: &mut Host,
        _tab_type: Option<TabType>,
    ) -> Result<TabParts<Host>, JsError> {
        Ok({
            let k = cx.created;
            cx.created += 1;
            TabParts {
                uuid: cx
                    .seeds
                    .get(k)
                    .map_or_else(|| js(&format!("unseeded-{k}")), |s| s.uuid.clone()),
                controller: Box::new(StubController(k)),
                renderer: Box::new(StubRenderer(k, cx.renderer_uuids.get(k).cloned().flatten())),
            }
        })
    }
}

#[derive(Clone)]
struct TabDragItem {
    text: JsString,
    selected: bool,
}

/// One emitted record: the keys compared against the vector line, and whether the vector's
/// `data.command` is left out (the stubs' commands are not upstream's).
struct Emit {
    values: Vec<(&'static str, Value)>,
    without_command: bool,
}

impl Emit {
    fn new(values: Vec<(&'static str, Value)>) -> Self {
        Emit {
            values,
            without_command: false,
        }
    }
}

fn info_records(records: &[Record]) -> Result<Vec<Value>, String> {
    records
        .iter()
        .filter(|r| {
            r.string("eval")
                .is_some_and(|e| e.starts_with("/*tabs:info*/"))
        })
        .map(|r| r.value("value").map_err(|e| e.0))
        .collect()
}

fn seeds(infos: &[Value]) -> Result<Vec<Seed>, String> {
    let mut seeds = Vec::new();
    for info in infos {
        let rows = info.as_array().ok_or("info is not an array")?;
        for row in rows.iter().skip(seeds.len()) {
            let row = row.as_array().ok_or("info row is not an array")?;
            let uuid = js_json::to_js(&row[0])?
                .into_string()
                .ok_or_else(|| "creation uuid is not a string".to_owned())?;
            seeds.push(Seed {
                uuid,
                show_timeline: row[3].as_bool().ok_or("showTimeline is not a boolean")?,
            });
        }
    }
    Ok(seeds)
}

fn current_from(info: &Value) -> Result<Vec<Current>, String> {
    info.as_array()
        .ok_or("info is not an array")?
        .iter()
        .map(|row| {
            let row = row.as_array().ok_or("info row is not an array")?;
            let fields = js_json::to_js(&row[4])?
                .into_array()
                .ok_or_else(|| "active fields is not an array".to_owned())?
                .into_iter()
                .map(|v| match v.into_string() {
                    Some(s) => Ok(s),
                    None => Err("active field is not a string".to_owned()),
                })
                .collect::<Result<_, String>>()?;
            Ok(Current {
                controller: js_json::to_js(&row[1])?,
                renderer: js_json::to_js(&row[2])?,
                active_fields: fields,
            })
        })
        .collect()
}

/// The renderer UUIDs and, per created tab, what its `getTimestampRange()` returned in order,
/// from the `/*tabs:ranges*/` captures.
type RangeSeeds = (Vec<Option<JsString>>, Vec<VecDeque<Option<[f64; 2]>>>);

fn range_seeds(records: &[Record]) -> Result<RangeSeeds, String> {
    let mut uuids = Vec::new();
    let mut returns: Vec<VecDeque<Option<[f64; 2]>>> = Vec::new();
    for record in records.iter().filter(|r| {
        r.string("eval")
            .is_some_and(|e| e.starts_with("/*tabs:ranges*/"))
    }) {
        let value = js_json::to_js(&record.value("value").map_err(|e| e.0)?)?;
        let [JsValue::Array(ids), JsValue::Array(calls)] =
            value.as_array().ok_or("ranges is not an array")?
        else {
            return Err("ranges is not [uuids, calls]".to_owned());
        };
        uuids = ids
            .iter()
            .map(|id| match id {
                JsValue::String(s) => Ok(Some(s.clone())),
                JsValue::Null => Ok(None),
                other => Err(format!("renderer uuid {other:?}")),
            })
            .collect::<Result<_, String>>()?;
        for call in calls {
            let call = call.as_array().ok_or("range call is not an array")?;
            if let [JsValue::Number(k), JsValue::String(kind), range] = call
                && kind.to_string_lossy() == "get"
            {
                let k = *k as usize;
                if returns.len() <= k {
                    returns.resize_with(k + 1, VecDeque::new);
                }
                returns[k].push_back(range_of(range)?);
            }
        }
    }
    Ok((uuids, returns))
}

/// `style.left = n + "px"` read back, as Blink serializes it.
fn px(n: f64) -> Value {
    let mut style = Style::new();
    let assigned = format!("{}px", number::to_string(n));
    style.set(Property::Left, &js(&assigned));
    js_str_value(&style.get(Property::Left))
}

fn opt_px(n: Option<f64>) -> Value {
    n.map_or_else(|| cve_str(""), px)
}

fn flag(b: Option<bool>) -> Value {
    cve_str(match b {
        None => "",
        Some(true) => "1",
        Some(false) => "0",
    })
}

fn js_str_value(s: &JsString) -> Value {
    Value::String(Utf16String::from_units(s.units().to_vec()))
}

fn error_text(e: &TabsError) -> Result<String, String> {
    match e {
        TabsError::Thrown(e) => Ok(format!("{}: {}", e.name, e.message)),
        TabsError::Unmodeled(what) => Err(format!("unmodeled upstream state: {what}")),
    }
}

struct Runner {
    tabs: Tabs<Host>,
    host: Host,
    log: Log,
    drag: Drag<TabDragData, TabDragItem>,
    ignored: Vec<String>,
    infos: Vec<Value>,
    info_seen: usize,
    start_time: f64,
    now: f64,
    last_frame: f64,
    event_x: f64,
    emitted: Vec<Emit>,
}

impl Runner {
    fn creation_index(&self, index: usize) -> Option<usize> {
        let tab = self.tabs.tab(index)?;
        tab.controller()
            .downcast_ref::<StubController>()
            .map(|c| c.0)
    }

    fn drain_messages(&mut self) {
        for NamedMessage { name, data } in self.tabs.take_messages() {
            if self.ignored.iter().any(|n| n == name.name()) {
                continue;
            }
            self.emitted.push(Emit::new(vec![
                ("main", cve_str(name.name())),
                ("data", js_json::from_js(&data)),
            ]));
        }
    }

    fn report(&mut self, where_: &str, result: Result<(), TabsError>) -> Result<(), String> {
        self.drain_messages();
        if let Err(e) = result {
            let text = error_text(&e)?;
            self.emitted.push(Emit::new(vec![
                ("exception", cve_str(where_)),
                ("error", cve_str(&text)),
            ]));
        }
        Ok(())
    }

    fn frame(&mut self) -> Result<(), String> {
        let now = self.start_time + self.now;
        let out = self.tabs.periodic(&mut self.host, now);
        for update in out.satellite_updates {
            if self.ignored.iter().any(|n| n == "update-satellite") {
                continue;
            }
            let message = update.to_message();
            let mut data = js_json::from_js(&message.data);
            if let Value::Object(pairs) = &mut data {
                for (k, v) in pairs.iter_mut() {
                    if k.eq_str("command") {
                        *v = Value::Null;
                    }
                }
            }
            self.emitted.push(Emit {
                values: vec![("main", cve_str(message.name.name())), ("data", data)],
                without_command: true,
            });
        }
        match out.error {
            Some(e) => self.report("requestAnimationFrame", Err(e)),
            None => Ok(()),
        }
    }

    /// The real Blink frame the harness waits for after every step and every callback, where
    /// native `scroll` events are dispatched; the fake clock's frames never dispatch them.
    fn settle(&mut self) {
        let now = self.start_time + self.now;
        self.tabs.scroll_step(&self.host, now);
    }

    fn advance(&mut self, ms: f64) -> Result<(), String> {
        let end = self.now + ms;
        while self.last_frame + FRAME_PERIOD <= end {
            self.last_frame += FRAME_PERIOD;
            self.now = self.last_frame;
            self.frame()?;
            self.settle();
        }
        self.now = end;
        Ok(())
    }

    fn main(&mut self, name: &str, data: JsValue) -> Result<(), String> {
        let cx = &mut self.host;
        let tabs = &mut self.tabs;
        let result = match name {
            "new-tab" => tabs.add_tab(cx, &data),
            "move-tab" => tabs.move_selection(cx, &data),
            "shift-tab" => tabs.shift_selected(cx, &data),
            "close-tab" => {
                let index = tabs.selected().clone();
                tabs.close(cx, &index, &data)
            }
            "rename-tab" => tabs.rename_tab(
                &data.get("index").cloned().unwrap_or_default(),
                data.get("name").cloned().unwrap_or_default(),
            ),
            "toggle-controls" => tabs.toggle_controls_visible(cx),
            "new-satellite" => tabs.new_satellite(),
            "set-active-satellites" => {
                let Some(uuids) = data.into_array() else {
                    return Err("set-active-satellites needs an array".to_owned());
                };
                tabs.set_active_satellites(uuids);
                Ok(())
            }
            "restore-state" => {
                tabs.restore_state(cx, &data.get("tabs").cloned().unwrap_or_default())
            }
            "add-table-range" => {
                let renderer_uuid = match data.get("rendererUUID") {
                    Some(JsValue::String(s)) => s.clone(),
                    _ => return Err("add-table-range needs a rendererUUID".to_owned()),
                };
                let range = match data.get("range") {
                    Some(JsValue::Array(r)) => match (r.first(), r.get(1)) {
                        (Some(JsValue::Number(a)), Some(JsValue::Number(b))) => Some([*a, *b]),
                        _ => return Err("bad range".to_owned()),
                    },
                    _ => None,
                };
                tabs.add_table_range(
                    cx,
                    &data.get("controllerUUID").cloned().unwrap_or_default(),
                    &renderer_uuid,
                    range,
                )
            }
            other => return Err(format!("unsupported main message {other}")),
        };
        self.report("unhandledRejection", result)
    }

    fn window_mouse(&mut self, kind: &str, y: f64) -> Result<(), String> {
        match kind {
            "mousemove" => {
                let r = self.tabs.window_mouse_move(&self.host, y);
                self.report("event", r)?;
            }
            "mouseup" => self.tabs.window_mouse_up(),
            _ => return Ok(()),
        }
        match kind {
            "mousemove" => {
                let x = self.event_x;
                self.drag.move_to(x, y);
            }
            _ => self.drag.end(),
        }
        for update in self.drag.take_updates() {
            let r = self.tabs.drag_update(&self.host, &update);
            self.report("event", r)?;
            let mut data = JsObject::new();
            data.insert("tabIndex", JsValue::Number(update.data.tab_index));
            self.emitted.push(Emit::new(vec![
                ("drag", js_json::from_js(&JsValue::Object(data))),
                ("end", Value::Bool(update.end)),
                ("x", Value::number(update.x)),
                ("y", Value::number(update.y)),
            ]));
        }
        Ok(())
    }
}

fn view(runner: &Runner) -> Value {
    let tabs = &runner.tabs;
    let controls = tabs.controls();
    let highlight = tabs.drag_highlight();
    let bar = tabs
        .bar()
        .map(|e| {
            Value::Array(vec![
                cve_str(&e.class_name()),
                js_str_value(&e.inner_text()),
            ])
        })
        .collect();
    let content = tabs
        .tabs()
        .map(|t| {
            Value::Array(vec![
                Value::Bool(t.content_hidden()),
                Value::Bool(t.content_hidden()),
            ])
        })
        .collect();
    let (left, top) = runner
        .drag
        .item_position()
        .map_or((cve_str(""), cve_str("")), |(l, t)| (px(l), px(t)));
    let children = runner
        .drag
        .item()
        .map(|item| {
            vec![Value::Array(vec![
                cve_str(if item.selected {
                    "tab tab-selected"
                } else {
                    "tab"
                }),
                js_str_value(&item.text),
            ])]
        })
        .unwrap_or_default();
    Value::object([
        ("bar", Value::Array(bar)),
        ("content", Value::Array(content)),
        ("tabControlsHeight", opt_px(controls.tab_controls_height)),
        ("showTabControls", flag(controls.show_tab_controls)),
        ("showTimeline", flag(tabs.show_timeline())),
        (
            "timelineHidden",
            Value::Bool(tabs.show_timeline().is_some_and(|s| !s)),
        ),
        ("handleHidden", Value::Bool(controls.handle_hidden)),
        ("controlsHidden", Value::Bool(controls.content_hidden)),
        ("handleActive", Value::Bool(tabs.controls_handle_active())),
        (
            "highlight",
            Value::Array(vec![Value::Bool(highlight.hidden), opt_px(highlight.left)]),
        ),
        (
            "dragItem",
            Value::Array(vec![
                Value::Bool(runner.drag.item_hidden()),
                left,
                top,
                Value::Array(children),
            ]),
        ),
    ])
}

fn capture(runner: &mut Runner, spec: &Json) -> Result<(), String> {
    let kind = match spec {
        Json::String(_) => str_of(spec).unwrap_or_default(),
        _ => spec.get("kind").and_then(str_of).unwrap_or_default(),
    };
    match kind.as_str() {
        "commands" => {
            for i in 0..runner.tabs.len() {
                let Some(k) = runner.creation_index(i) else {
                    continue;
                };
                if !runner.host.rendered.get(k).copied().unwrap_or(false) {
                    continue;
                }
                runner.host.rendered[k] = false;
                let tab = runner.tabs.tab(i).ok_or("tab vanished")?;
                let mut meta = JsObject::new();
                meta.insert("index", JsValue::Number(i as f64));
                meta.insert("type", tab.type_value().clone());
                meta.insert("title", tab.title().clone());
                meta.insert("uuid", tab.uuid().clone());
                runner.emitted.push(Emit::new(vec![(
                    "command",
                    js_json::from_js(&JsValue::Object(meta)),
                )]));
            }
            Ok(())
        }
        "eval" => {
            let expr = spec
                .get("expr")
                .and_then(str_of)
                .ok_or("eval capture needs expr")?;
            let value = if expr.starts_with("/*tabs:info*/") {
                let info = runner
                    .infos
                    .get(runner.info_seen)
                    .cloned()
                    .ok_or("more info captures than the vector has")?;
                runner.info_seen += 1;
                let current = current_from(&info)?;
                if current.len() != runner.host.created {
                    return Err(format!(
                        "port created {} tabs, upstream {}",
                        runner.host.created,
                        current.len()
                    ));
                }
                runner.host.current = current;
                info
            } else if expr == "window.tabs.saveState()" {
                js_json::from_js(&runner.tabs.save_state(&runner.host))
            } else if expr.starts_with("/*tabs:view*/") {
                view(runner)
            } else if expr.starts_with("/*tabs:bar*/") {
                let opacity = |o: Option<&str>| cve_str(o.unwrap_or(""));
                let [left, right] = runner.tabs.shadows();
                Value::Array(vec![
                    opacity(left),
                    opacity(right),
                    Value::number(runner.host.bar.borrow().left),
                ])
            } else if expr.starts_with("/*tabs:restores*/") {
                Value::Array(
                    runner
                        .host
                        .restores
                        .iter()
                        .map(|(k, half, state)| {
                            Value::Array(vec![
                                Value::number(*k as f64),
                                cve_str(half),
                                js_json::from_js(state),
                            ])
                        })
                        .collect(),
                )
            } else if expr.starts_with("/*tabs:ranges*/") {
                let host = &runner.host;
                let uuids = (0..host.created)
                    .map(|k| {
                        host.renderer_uuids
                            .get(k)
                            .cloned()
                            .flatten()
                            .map_or(JsValue::Null, JsValue::String)
                    })
                    .collect();
                let calls = host.range_calls.borrow_mut().drain(..).collect();
                js_json::from_js(&JsValue::Array(vec![
                    JsValue::Array(uuids),
                    JsValue::Array(calls),
                ]))
            } else if expr == "[...window.tabs.getActiveFields()]" {
                Value::Array(
                    runner
                        .tabs
                        .active_fields(&runner.host, &runner.log)
                        .iter()
                        .map(js_str_value)
                        .collect(),
                )
            } else if let Some(arg) = expr
                .strip_prefix("window.tabs.isValidUUID(")
                .and_then(|s| s.strip_suffix(')'))
            {
                let arg = asrs_parity::json::parse(arg).map_err(|e| e.to_string())?;
                Value::Bool(runner.tabs.is_valid_uuid(&to_js(&arg)?))
            } else {
                return Err(format!("unsupported eval {expr}"));
            };
            runner
                .emitted
                .push(Emit::new(vec![("eval", cve_str(&expr)), ("value", value)]));
            Ok(())
        }
        other => Err(format!("unsupported capture {other}")),
    }
}

fn apply_log(log: &mut Log, ops: &Json) -> Result<(), String> {
    for op in ops.as_array().ok_or("log ops are an array")? {
        let op = op.as_array().ok_or("log op is an array")?;
        let method = op.first().and_then(str_of).ok_or("log op method")?;
        let key = op
            .get(1)
            .and_then(Json::as_str)
            .map(|s| JsString::from_units(s.units().to_vec()));
        let key = key.ok_or("log op key")?;
        let time = op.get(2).and_then(Json::as_f64).ok_or("log op time")?;
        match (method.as_str(), op.get(3)) {
            ("putBoolean", Some(Json::Bool(b))) => log.put_boolean(&key, time, *b),
            ("putNumber", Some(Json::Number(n))) => log.put_number(&key, time, *n),
            _ => return Err(format!("unsupported log op {method}")),
        }
    }
    Ok(())
}

fn load_json(spec: &Path, value: &Json) -> Result<Json, String> {
    match value {
        Json::String(s) => {
            let path = spec.parent().unwrap().join(s.to_string_lossy());
            let text =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            asrs_parity::json::parse(&text).map_err(|e| e.to_string())
        }
        other => Ok(other.clone()),
    }
}

struct Replayed {
    emitted: Vec<Emit>,
    geometry: usize,
    scroll_checks: usize,
}

fn replay(spec_path: &Path, records: &[Record]) -> Result<Replayed, String> {
    let text = std::fs::read_to_string(spec_path).map_err(|e| e.to_string())?;
    let spec = asrs_parity::json::parse(&text).map_err(|e| e.to_string())?;
    let fixture = spec.get("fixture").ok_or("spec has no fixture")?;
    let height = fixture
        .get("window")
        .and_then(|w| w.get("height"))
        .and_then(Json::as_f64)
        .unwrap_or(650.0);
    let window = fixture.get("window");
    let device_pixel_ratio = window
        .and_then(|w| w.get("devicePixelRatio"))
        .and_then(Json::as_f64)
        .unwrap_or(1.0);
    let geometry: Vec<Record> = records
        .iter()
        .filter(|r| {
            GeometryRecord::parse(r)
                .ok()
                .flatten()
                .is_some_and(|g| owned_by_tabs(&g))
        })
        .cloned()
        .collect();
    let ignored = match fixture.get("ignoreMain").and_then(Json::as_array) {
        Some(names) => names.iter().filter_map(str_of).collect(),
        None => DEFAULT_IGNORED.iter().map(|s| (*s).to_owned()).collect(),
    };
    let infos = info_records(records)?;
    let (renderer_uuids, timestamp_ranges) = range_seeds(records)?;
    let mut host = Host {
        inner_height: height,
        reads: RefCell::new(ScriptedReads::from_records(&geometry)?),
        bar: RefCell::new(BarScroll::default()),
        device_pixel_ratio,
        seeds: seeds(&infos)?,
        current: Vec::new(),
        restores: Vec::new(),
        renderer_uuids,
        timestamp_ranges: RefCell::new(timestamp_ranges),
        range_calls: RefCell::new(Vec::new()),
        rendered: Vec::new(),
        created: 0,
    };
    let tabs = Tabs::new(Box::new(StubFactory), &mut host);
    let mut runner = Runner {
        tabs,
        host,
        log: Log::new(true),
        drag: Drag::default(),
        ignored,
        infos,
        info_seen: 0,
        start_time: fixture
            .get("startTime")
            .and_then(Json::as_f64)
            .unwrap_or(1_767_225_600_000.0),
        now: 0.0,
        last_frame: 0.0,
        emitted: Vec::new(),
        event_x: 0.0,
    };
    runner.tabs.tab_bar_sensor_mut().set_layout(
        [OVERLAY_CLIENT_WIDTH, OVERLAY_CLIENT_HEIGHT],
        device_pixel_ratio,
    );
    runner.main("set-active-satellites", JsValue::Array(Vec::new()))?;

    let steps = fixture.get("steps").and_then(Json::as_array).unwrap_or(&[]);
    for (index, step) in steps.iter().enumerate() {
        let where_ = |e: String| format!("step {index}: {e}");
        if let Some(name) = step.get("main").and_then(str_of) {
            let data = match (step.get("cve1"), step.get("data")) {
                (Some(cve), _) => js_json::to_js(&Value::from_json(cve).map_err(|e| e.0)?)?,
                (None, Some(data)) => to_js(data)?,
                (None, None) => JsValue::Undefined,
            };
            runner.main(&name, data).map_err(where_)?;
        } else if let Some(ops) = step.get("log") {
            apply_log(&mut runner.log, &load_json(spec_path, ops)?).map_err(where_)?;
            let r = runner.tabs.refresh(&mut runner.host);
            runner.report("log", r).map_err(where_)?;
        } else if let Some(ms) = step.get("advance").and_then(Json::as_f64) {
            runner.advance(ms).map_err(where_)?;
        } else if let Some(n) = step.get("frames").and_then(Json::as_f64) {
            runner.advance(n * FRAME_PERIOD).map_err(where_)?;
        } else if let Some(expr) = step.get("eval").and_then(str_of) {
            if let Some(rest) = expr.strip_prefix("/*tabs:scroll ") {
                let deltas: Vec<f64> = rest
                    .split("*/")
                    .next()
                    .unwrap_or_default()
                    .split(' ')
                    .map(|n| number::parse_float(js(n).units()))
                    .collect();
                let [dx, dy] = deltas[..] else {
                    return Err(where_(format!("bad scroll tag {expr}")));
                };
                let now = runner.start_time + runner.now;
                runner.tabs.tab_bar_wheel(
                    Wheel {
                        delta: [-dx, -dy],
                        precise: true,
                    },
                    now,
                );
            } else if !expr.starts_with("/*tabs:recorder*/") && !expr.starts_with("/*tabs:tag*/") {
                return Err(where_(format!("unsupported eval {expr}")));
            }
        } else if let Some(event) = step.get("event") {
            run_event(&mut runner, event).map_err(where_)?;
        } else if let Some(specs) = step.get("capture") {
            let label = step
                .get("label")
                .map_or(Value::number(index as f64), Value::from_json_lenient);
            runner.emitted.push(Emit::new(vec![
                ("capture", label),
                ("time", Value::number(runner.now)),
            ]));
            for spec in specs.as_array().ok_or("capture is a list")? {
                capture(&mut runner, spec).map_err(where_)?;
            }
        } else {
            return Err(where_("unknown step".to_owned()));
        }
        runner.settle();
    }
    let geometry = runner
        .host
        .reads
        .into_inner()
        .finish()
        .map_err(|e| format!("geometry: {e}"))?;
    let bar = runner.host.bar.into_inner();
    if let Some(e) = bar.error {
        return Err(e);
    }
    Ok(Replayed {
        emitted: runner.emitted,
        geometry,
        scroll_checks: bar.checks,
    })
}

fn run_event(runner: &mut Runner, event: &Json) -> Result<(), String> {
    let target = event.get("target").and_then(str_of).ok_or("event target")?;
    let kind = event.get("type").and_then(str_of).ok_or("event type")?;
    let index = event.get("index").and_then(Json::as_f64).unwrap_or(0.0) as usize;
    let init = event.get("init");
    let num = |key: &str| {
        init.and_then(|i| i.get(key))
            .and_then(Json::as_f64)
            .unwrap_or(0.0)
    };
    let (x, y) = (math::floor(num("clientX")), math::floor(num("clientY")));
    runner.event_x = x;
    match (target.as_str(), kind.as_str()) {
        ("div.tab-bar-scroll", "mousedown") => runner.tabs.overlay_mouse_down(x, y),
        ("div.tab-bar-scroll", "mouseup") => {
            let r = runner.tabs.overlay_mouse_up(&runner.host, x, y);
            runner.report("event", r)?;
        }
        ("div.tab-bar-scroll", "mousemove") => {
            if let Some(start) = runner.tabs.overlay_mouse_move(&runner.host, x, y) {
                runner.drag.start(
                    start.x,
                    start.y,
                    start.offset_x,
                    start.offset_y,
                    TabDragItem {
                        text: start.text,
                        selected: start.selected,
                    },
                    TabDragData {
                        tab_index: start.tab_index as f64,
                    },
                );
            }
        }
        ("div.tab-bar-scroll", "mouseout") => runner.tabs.overlay_mouse_out(),
        ("div.tab-bar-scroll", "contextmenu") => {
            runner.tabs.overlay_context_menu(&runner.host, x, y);
            runner.drain_messages();
        }
        ("div.controls-handle", "mousedown") => runner.tabs.controls_handle_mouse_down(),
        ("div.controls-handle", "click") => {
            let r = runner
                .tabs
                .controls_handle_click(&runner.host, runner.start_time + runner.now);
            runner.report("event", r)?;
        }
        ("div.tab-bar > div.tab", "keydown") => {
            let key = init
                .and_then(|i| i.get("key"))
                .and_then(str_of)
                .unwrap_or_default();
            let r = runner
                .tabs
                .title_key_down(&runner.host, index, &key)
                .map(|_| ());
            runner.report("event", r)?;
        }
        ("button.close", "click") => {
            let r = runner.tabs.close_button(&runner.host);
            runner.report("event", r)?;
        }
        ("button.popup", "click") => {
            let r = runner.tabs.popup_button();
            runner.report("event", r)?;
        }
        ("button.add-tab", "click") => {
            runner.tabs.add_button(&runner.host);
            runner.drain_messages();
        }
        ("window", _) => {}
        other => return Err(format!("unsupported event {other:?}")),
    }
    let bubbles_to_window = target != "window" || matches!(kind.as_str(), "mousemove" | "mouseup");
    if bubbles_to_window {
        runner.window_mouse(&kind, y)?;
    }
    Ok(())
}

fn compare(emitted: &[Emit], records: &[Record]) -> Result<(), String> {
    if emitted.len() != records.len() {
        let first_diff = emitted
            .iter()
            .zip(records)
            .position(|(e, r)| {
                e.values
                    .iter()
                    .any(|(k, v)| r.value(k).ok().as_ref() != Some(v))
            })
            .unwrap_or(emitted.len().min(records.len()));
        return Err(format!(
            "emitted {} records, vector has {} (first difference near line {})",
            emitted.len(),
            records.len(),
            first_diff + 1
        ));
    }
    for (emit, record) in emitted.iter().zip(records) {
        for (key, value) in &emit.values {
            let mut expected = record
                .value(key)
                .map_err(|e| format!("line {}: missing {key}: {}", record.line, e.0))?;
            if emit.without_command
                && let Value::Object(pairs) = &mut expected
            {
                for (k, v) in pairs.iter_mut() {
                    if k.eq_str("command") {
                        *v = Value::Null;
                    }
                }
            }
            if *value != expected {
                return Err(format!(
                    "line {} ({key}):\n  expected {}\n  actual   {}",
                    record.line,
                    expected.encode(),
                    value.encode()
                ));
            }
        }
    }
    Ok(())
}

fn specs() -> Vec<(PathBuf, String)> {
    let dir = vectors::workspace_root().join("parity/specs/dom/tabs-model");
    let mut out: Vec<(PathBuf, String)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter_map(|p| {
            let name = p
                .file_name()?
                .to_str()?
                .strip_suffix(".entry.json")?
                .to_owned();
            Some((p, format!("dom/tabs-model/{name}")))
        })
        .collect();
    out.sort();
    out
}

fn load_records(id: &str) -> Result<Vec<Record>, String> {
    match std::env::var("ASRS_TABS_DOM_DIR") {
        Ok(dir) => {
            let name = id.rsplit('/').next().unwrap_or(id);
            let path = Path::new(&dir).join(format!("{name}.jsonl"));
            let text =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            vectors::parse_records(&text)
        }
        Err(_) => vectors::load(id),
    }
}

#[test]
fn tabs_dom_matches_upstream() {
    let specs = specs();
    assert!(!specs.is_empty(), "no tabs-model specs");
    let mut failures = Vec::new();
    let mut lines = 0;
    let (mut geometry, mut scroll_checks) = (0, 0);
    for (spec, id) in &specs {
        let result = load_records(id).and_then(|records| {
            lines += records.len();
            let replayed = replay(spec, &records)?;
            geometry += replayed.geometry;
            scroll_checks += replayed.scroll_checks;
            let others: Vec<Record> = records
                .iter()
                .filter(|r| r.field("read").is_none() && r.field("write").is_none())
                .cloned()
                .collect();
            compare(&replayed.emitted, &others)
        });
        if let Err(e) = result {
            failures.push(format!("{id}: {e}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    eprintln!(
        "dom/tabs-model: {} fixtures, {lines} records ({geometry} Tabs geometry accesses, \
         {scroll_checks} tab bar scroll offsets), 0 mismatches",
        specs.len()
    );
    assert!(geometry > 0 && scroll_checks > 0);
}
