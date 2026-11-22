#![allow(clippy::disallowed_methods)]

//! Replays `parity/specs/dom/tabs-model/<case>.entry.json` against `asrs_hub::tabs::Tabs` and
//! compares the emitted records with the committed `dom/tabs-model/<case>` vector, line for line.
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
use asrs_js::number;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue};
use asrs_log::log::Log;
use asrs_model::messages::NamedMessage;
use asrs_model::source_list::Rect;
use asrs_model::tab_controller::{Command, TabCommand, TabController, TabRenderer};
use asrs_model::tab_type::TabType;
use asrs_parity::cve1::Value;
use asrs_parity::json::Json;
use asrs_parity::utf16::Utf16String;
use asrs_parity::vectors::{self, Record};

const FRAME_PERIOD: f64 = 16.0;
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
    renderer_top: f64,
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

impl TabsHost for Host {
    fn inner_height(&self) -> f64 {
        self.inner_height
    }

    fn renderer_top(&self, _show_timeline: bool) -> f64 {
        self.renderer_top
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

struct LayoutRule {
    selector: String,
    rect: Rect,
}

fn px(n: f64) -> Value {
    cve_str(&format!("{}px", number::to_string(n)))
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
    layout: Vec<LayoutRule>,
    tags: Vec<Option<usize>>,
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
    fn rule(&self, selector: &str) -> Option<Rect> {
        self.layout
            .iter()
            .find(|r| r.selector == selector)
            .map(|r| r.rect)
    }

    fn creation_index(&self, index: usize) -> Option<usize> {
        let tab = self.tabs.tab(index)?;
        tab.controller()
            .downcast_ref::<StubController>()
            .map(|c| c.0)
    }

    fn tab_rects(&self) -> Vec<Rect> {
        (0..self.tabs.len())
            .map(|i| {
                self.creation_index(i)
                    .and_then(|k| self.tags.get(k).copied().flatten())
                    .and_then(|tag| self.rule(&format!("div.tab[data-i=\"{tag}\"]")))
                    .unwrap_or_default()
            })
            .collect()
    }

    fn overlay(&self) -> Rect {
        self.rule("div.tab-bar-scroll").unwrap_or(Rect {
            x: 310.0,
            y: 0.0,
            width: 636.0,
            height: 50.0,
        })
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
        let out = self.tabs.periodic(&mut self.host);
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

    fn advance(&mut self, ms: f64) -> Result<(), String> {
        let end = self.now + ms;
        while self.last_frame + FRAME_PERIOD <= end {
            self.last_frame += FRAME_PERIOD;
            self.now = self.last_frame;
            self.frame()?;
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
            let rects = self.tab_rects();
            let overlay = self.overlay();
            let r = self.tabs.drag_update(&self.host, &update, &rects, &overlay);
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

fn spec_layout(fixture: &Json) -> Vec<LayoutRule> {
    let mut rules = Vec::new();
    for rule in fixture
        .get("layout")
        .and_then(Json::as_array)
        .unwrap_or(&[])
    {
        let (Some(selector), Some(rect)) = (
            rule.get("selector").and_then(str_of),
            rule.get("rect").and_then(Json::as_array),
        ) else {
            continue;
        };
        let n = |i: usize| rect.get(i).and_then(Json::as_f64).unwrap_or(0.0);
        rules.push(LayoutRule {
            selector,
            rect: Rect {
                x: n(0),
                y: n(1),
                width: n(2),
                height: n(3),
            },
        });
    }
    rules
}

fn view(runner: &Runner) -> Value {
    let tabs = &runner.tabs;
    let controls = tabs.controls();
    let highlight = tabs.drag_highlight();
    let bar = tabs
        .bar()
        .map(|e| Value::Array(vec![cve_str(&e.class_name()), js_str_value(&e.text)]))
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

fn replay(spec_path: &Path, records: &[Record]) -> Result<Vec<Emit>, String> {
    let text = std::fs::read_to_string(spec_path).map_err(|e| e.to_string())?;
    let spec = asrs_parity::json::parse(&text).map_err(|e| e.to_string())?;
    let fixture = spec.get("fixture").ok_or("spec has no fixture")?;
    let height = fixture
        .get("window")
        .and_then(|w| w.get("height"))
        .and_then(Json::as_f64)
        .unwrap_or(650.0);
    let layout = spec_layout(fixture);
    let ignored = match fixture.get("ignoreMain").and_then(Json::as_array) {
        Some(names) => names.iter().filter_map(str_of).collect(),
        None => DEFAULT_IGNORED.iter().map(|s| (*s).to_owned()).collect(),
    };
    let infos = info_records(records)?;
    let (renderer_uuids, timestamp_ranges) = range_seeds(records)?;
    let mut host = Host {
        inner_height: height,
        renderer_top: layout
            .iter()
            .find(|r| r.selector == "div.renderer-content")
            .map_or(92.0, |r| r.rect.y),
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
        layout,
        tags: Vec::new(),
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
            if expr.starts_with("/*tabs:tag*/") {
                for i in 0..runner.tabs.len() {
                    if let Some(k) = runner.creation_index(i) {
                        if runner.tags.len() <= k {
                            runner.tags.resize(k + 1, None);
                        }
                        runner.tags[k] = Some(i);
                    }
                }
            } else if !expr.starts_with("/*tabs:recorder*/") {
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
    }
    Ok(runner.emitted)
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
    let (x, y) = (num("clientX"), num("clientY"));
    runner.event_x = x;
    match (target.as_str(), kind.as_str()) {
        ("div.tab-bar-scroll", "mousedown") => runner.tabs.overlay_mouse_down(x, y),
        ("div.tab-bar-scroll", "mouseup") => {
            let rects = runner.tab_rects();
            let r = runner.tabs.overlay_mouse_up(&runner.host, x, y, &rects);
            runner.report("event", r)?;
        }
        ("div.tab-bar-scroll", "mousemove") => {
            let rects = runner.tab_rects();
            if let Some(start) = runner.tabs.overlay_mouse_move(x, y, &rects) {
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
            let rects = runner.tab_rects();
            runner.tabs.overlay_context_menu(x, y, &rects);
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
            let rect = runner.rule("button.add-tab").unwrap_or_default();
            runner.tabs.add_button(&rect);
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
    for (spec, id) in &specs {
        let result = load_records(id).and_then(|records| {
            lines += records.len();
            compare(&replay(spec, &records)?, &records)
        });
        if let Err(e) = result {
            failures.push(format!("{id}: {e}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    eprintln!(
        "dom/tabs-model: {} fixtures, {lines} records, 0 mismatches",
        specs.len()
    );
}
