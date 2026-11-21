#![allow(clippy::disallowed_methods)]

//! Replays `parity/specs/dom/statistics/<case>.entry.json` against `asrs_hub::tabs::Tabs` with
//! the real `asrs_tabs::statistics::StatisticsController` and `SelectionImpl`, and compares
//! the emitted records with `dom/statistics/<case>` vectors line for line.
//!
//! Tabs of other types get stub halves seeded from `/*tabs:info*/` eval captures, as the
//! tabs-model fixtures do. `show` is in every fixture's `ignoreMain`: the boot message is hub
//! plumbing, not the controller.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::cell::{Cell, Ref, RefCell};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use asrs_hub::tabs::{TabFactory, TabParts, Tabs, TabsError, TabsHost};
use asrs_js::string::{JsError, JsString};
use asrs_js::time::{self, Clock, Rng as _};
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::log::Log;
use asrs_model::assets::AdvantageScopeAssets;
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::preferences::Preferences;
use asrs_model::selection::Selection;
use asrs_model::selection_impl::{SelectionImpl, SharedLog};
use asrs_model::source_list::{Env, SourceListError};
use asrs_model::source_list_config::{SourceListTypeMemory, SourceListTypeMemoryEntry};
use asrs_model::tab_controller::{Command, TabCommand, TabController, TabRenderer};
use asrs_model::tab_type::TabType;
use asrs_parity::cve1::Value;
use asrs_parity::json::Json;
use asrs_parity::utf16::Utf16String;
use asrs_parity::vectors::{self, Record};
use asrs_tabs::cx::TabCx;
use asrs_tabs::statistics::{StatisticsController, StatisticsEnv};

const DEFAULT_IGNORED: [&str; 3] = ["save-state", "save-type-memory", "update-touch-bar-slider"];
type InputSetter = fn(&mut StatisticsController, JsString);
const INPUT_TARGETS: [(&str, InputSetter); 4] = [
    ("select.time-range", StatisticsController::time_range_input),
    ("input.range-min", StatisticsController::range_min_input),
    ("input.range-max", StatisticsController::range_max_input),
    ("input.step-size", StatisticsController::step_size_input),
];

fn cve_str(s: &str) -> Value {
    Value::String(Utf16String::from(s))
}

fn err(msg: impl Into<String>) -> String {
    msg.into()
}

fn to_js(json: &Json) -> Result<JsValue, String> {
    js_json::to_js(&Value::from_json_lenient(json))
}

fn str_of(json: &Json) -> Option<String> {
    json.as_str().map(|s| s.to_string_lossy())
}

/// `env.ts`'s `Math.random`: mulberry32 on a u32 counter, `nextU32() / 2**32`.
struct Mulberry(u64);

impl Mulberry {
    fn new(seed: u64) -> Self {
        Mulberry(seed & 0xffff_ffff)
    }

    fn next_u32(&mut self) -> u64 {
        self.0 = (self.0 + 0x6d2b79f5) & 0xffff_ffff;
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1) & 0xffff_ffff;
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61) & 0xffff_ffff) & 0xffff_ffff;
        (t ^ (t >> 14)) & 0xffff_ffff
    }
}

impl time::Rng for Mulberry {
    fn next_f64(&mut self) -> f64 {
        self.next_u32() as f64 / 4294967296.0
    }
}

struct ScriptedClock(Rc<Cell<f64>>);

impl Clock for ScriptedClock {
    fn now(&self) -> f64 {
        self.0.get()
    }
}

/// `createUUID` calls each controller type's constructor makes (`UUID` plus `SourceList`s).
fn uuid_draws(tab_type: Option<TabType>) -> usize {
    match tab_type {
        Some(TabType::LineGraph) => 4,
        Some(
            TabType::Statistics
            | TabType::Field2d
            | TabType::Field3d
            | TabType::Swerve
            | TabType::Mechanism
            | TabType::Points,
        ) => 2,
        _ => 1,
    }
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
    log: SharedLog,
    selection: SelectionImpl,
    clock: Rc<Cell<f64>>,
    rng: Mulberry,
    type_memory: SourceListTypeMemory,
    dark: bool,
    seeds: Vec<Seed>,
    current: Vec<Current>,
    restores: Vec<(usize, &'static str, JsValue)>,
    rendered: Vec<Option<JsValue>>,
    created: usize,
    messages: Vec<NamedMessage<HubToMain>>,
}

impl TabsHost for Host {
    fn inner_height(&self) -> f64 {
        self.inner_height
    }

    fn renderer_top(&self, _show_timeline: bool) -> f64 {
        self.renderer_top
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
        None
    }

    fn tab_assets(&self) -> Option<&AdvantageScopeAssets> {
        None
    }

    fn tab_rng(&mut self) -> &mut dyn time::Rng {
        &mut self.rng
    }

    fn send_main_message(&mut self, message: NamedMessage<HubToMain>) {
        self.messages.push(message);
    }
}

impl StatisticsEnv for Host {
    fn with_source_env<R>(&mut self, f: impl FnOnce(&mut Env<'_>) -> R) -> R {
        let log = self.log.borrow();
        let mut env = Env {
            log: &log,
            type_memory: &mut self.type_memory,
            dark: self.dark,
        };
        f(&mut env)
    }

    fn now(&self) -> f64 {
        self.clock.get()
    }
}

/// The stub halves every non-statistics tab gets; `k` is the creation index `/*tabs:info*/`
/// reports under.
struct StubController(usize);
struct StubRenderer(usize);

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

    fn render(&mut self, cx: &mut Host, command: &dyn TabCommand) -> Result<(), JsError> {
        if cx.rendered.len() <= self.0 {
            cx.rendered.resize(self.0 + 1, None);
        }
        cx.rendered[self.0] = Some(command.to_js());
        Ok(())
    }
}

/// `StatisticsController` plus the recorder's `restoreState` interception.
struct StatsHalf {
    inner: StatisticsController,
    k: usize,
}

impl TabController<Host> for StatsHalf {
    fn save_state(&self, cx: &Host) -> JsValue {
        self.inner.save_state(cx)
    }

    fn restore_state(&mut self, cx: &mut Host, state: &JsValue) -> Result<(), JsError> {
        cx.restores.push((self.k, "controller", state.clone()));
        self.inner.restore_state(cx, state)
    }

    fn refresh(&mut self, cx: &mut Host) -> Result<(), JsError> {
        self.inner.refresh(cx)
    }

    fn new_assets(&mut self, cx: &mut Host) -> Result<(), JsError> {
        self.inner.new_assets(cx)
    }

    fn active_fields(&self, cx: &Host) -> Vec<JsString> {
        self.inner.active_fields(cx)
    }

    fn show_timeline(&self, cx: &Host) -> bool {
        self.inner.show_timeline(cx)
    }

    fn command(&mut self, cx: &mut Host, uuid: &JsValue) -> Result<Command, JsError> {
        self.inner.command(cx, uuid)
    }
}

struct StatFactory;

impl TabFactory<Host> for StatFactory {
    fn create(&mut self, cx: &mut Host, tab_type: Option<TabType>) -> TabParts<Host> {
        let k = cx.created;
        cx.created += 1;
        if tab_type == Some(TabType::Statistics) {
            TabParts {
                uuid: JsString::from(time::create_uuid(&mut cx.rng)),
                controller: Box::new(StatsHalf {
                    inner: StatisticsController::new(cx),
                    k,
                }),
                renderer: Box::new(StubRenderer(k)),
            }
        } else {
            let mut uuid = JsString::new();
            for i in 0..uuid_draws(tab_type) {
                let drawn = JsString::from(time::create_uuid(&mut cx.rng));
                if i == 0 {
                    uuid = drawn;
                }
            }
            if let Some(seed) = cx.seeds.get(k) {
                uuid = seed.uuid.clone();
            }
            TabParts {
                uuid,
                controller: Box::new(StubController(k)),
                renderer: Box::new(StubRenderer(k)),
            }
        }
    }
}

struct Emit {
    values: Vec<(&'static str, Value)>,
}

impl Emit {
    fn new(values: Vec<(&'static str, Value)>) -> Self {
        Emit { values }
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
            let uuid = match js_json::to_js(&row[0])? {
                JsValue::String(s) => s,
                other => return Err(format!("creation uuid {other:?}")),
            };
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
            let fields = match js_json::to_js(&row[4])? {
                JsValue::Array(items) => items
                    .into_iter()
                    .map(|v| match v {
                        JsValue::String(s) => Ok(s),
                        other => Err(format!("active field {other:?}")),
                    })
                    .collect::<Result<_, String>>()?,
                other => return Err(format!("active fields {other:?}")),
            };
            Ok(Current {
                controller: js_json::to_js(&row[1])?,
                renderer: js_json::to_js(&row[2])?,
                active_fields: fields,
            })
        })
        .collect()
}

fn selection_value(sel: &mut SelectionImpl) -> Value {
    let nullable = |t: Option<f64>| t.map_or(Value::Null, Value::number);
    let mode = sel.get_mode();
    let hovered = sel.get_hovered_time();
    let selected = sel.get_selected_time();
    let render = sel.get_render_time();
    let live = sel.get_current_live_time();
    let grab = sel.get_grab_zoom_range();
    let range = sel.get_timeline_range();
    let max_zoom = sel.get_timeline_is_max_zoom();
    let controls = sel.controls();
    Value::object([
        ("mode", Value::number(mode.number())),
        ("hoveredTime", nullable(hovered)),
        ("selectedTime", nullable(selected)),
        ("renderTime", nullable(render)),
        ("currentLiveTime", nullable(live)),
        (
            "grabZoomRange",
            grab.map_or(Value::Null, |r| {
                Value::Array(vec![Value::number(r[0]), Value::number(r[1])])
            }),
        ),
        (
            "timelineRange",
            Value::Array(vec![Value::number(range[0]), Value::number(range[1])]),
        ),
        ("timelineIsMaxZoom", Value::Bool(max_zoom)),
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

fn type_memory_of(value: &JsValue) -> SourceListTypeMemory {
    let mut out = SourceListTypeMemory::new();
    if let JsValue::Object(ids) = value {
        for (id, entries) in ids.iter() {
            let mut inner = JsObject::new();
            if let JsValue::Object(entries) = entries {
                for (key, entry) in entries.iter() {
                    if let Some(entry) = SourceListTypeMemoryEntry::from_js(entry) {
                        inner.insert(key.clone(), entry);
                    }
                }
            }
            out.insert(id.clone(), inner);
        }
    }
    out
}

/// `e instanceof Error ? `${name}: ${message}` : String(e)`; `source_error` marks thrown
/// non-Error values with an empty name.
fn thrown_text(e: &JsError) -> String {
    if e.name.is_empty() {
        e.message.clone()
    } else {
        format!("{}: {}", e.name, e.message)
    }
}

/// `SourceListError` out of a page call, as `thrown_text` formats it.
fn source_error(e: SourceListError) -> JsError {
    match e {
        SourceListError::UnknownType(t) => {
            JsError::new("", format!("Unknown type \"{}\"", t.to_string_lossy()))
        }
        SourceListError::Thrown(text) => {
            if let Some((name, message)) = text.split_once(": ") {
                match name {
                    "TypeError" => return JsError::type_error(message),
                    "RangeError" => return JsError::range(message),
                    "Error" => return JsError::new("Error", message),
                    _ => {}
                }
            }
            JsError::new("", text)
        }
    }
}

struct Runner {
    tabs: Tabs<Host>,
    host: Host,
    emitted: Vec<Emit>,
    ignored: HashSet<String>,
    infos: Vec<Value>,
    info_seen: usize,
    start: f64,
    last_frame: f64,
    frame_period: f64,
    live_time: Rc<Cell<f64>>,
}

impl Runner {
    /// Run `f` on the `StatisticsController` of tab `index`.
    fn stats<R>(
        &mut self,
        index: usize,
        f: impl FnOnce(&mut StatisticsController, &mut Host) -> R,
    ) -> Result<R, String> {
        let controller = self
            .tabs
            .controller_mut(&JsValue::Number(index as f64))
            .ok()
            .and_then(|c| c.downcast_mut::<StatsHalf>().map(|h| &mut h.inner))
            .ok_or_else(|| format!("tab {index} is not statistics"))?;
        Ok(f(controller, &mut self.host))
    }

    /// The nth tab whose controller is a `StatisticsController`, for selectors like
    /// `select.time-range` that match every statistics tab's controls in DOM order.
    fn stats_nth(&mut self, nth: usize) -> Result<usize, String> {
        (0..self.tabs.len())
            .filter(|&i| {
                self.tabs
                    .tab(i)
                    .is_some_and(|t| t.controller().downcast_ref::<StatsHalf>().is_some())
            })
            .nth(nth)
            .ok_or_else(|| format!("no statistics tab {nth}"))
    }

    fn drain(&mut self) {
        for NamedMessage { name, data } in self.tabs.take_messages() {
            if self.ignored.contains(name.name()) {
                continue;
            }
            self.emitted.push(Emit::new(vec![
                ("main", cve_str(name.name())),
                ("data", js_json::from_js(&data)),
            ]));
        }
        let controller_messages = std::mem::take(&mut self.host.messages);
        for NamedMessage { name, data } in controller_messages
            .into_iter()
            .chain(self.host.selection.take_messages())
        {
            if self.ignored.contains(name.name()) {
                continue;
            }
            self.emitted.push(Emit::new(vec![
                ("main", cve_str(name.name())),
                ("data", js_json::from_js(&data)),
            ]));
        }
    }

    fn report(&mut self, where_: &str, result: Result<(), JsError>) -> Result<(), String> {
        self.drain();
        match result {
            Err(e) if e.name == "Unmodeled" => {
                Err(err(format!("unmodeled in {where_}: {}", e.message)))
            }
            Err(e) => {
                self.emitted.push(Emit::new(vec![
                    ("exception", cve_str(where_)),
                    ("error", cve_str(&thrown_text(&e))),
                ]));
                Ok(())
            }
            Ok(()) => Ok(()),
        }
    }

    fn report_tabs(&mut self, where_: &str, result: Result<(), TabsError>) -> Result<(), String> {
        match result {
            Ok(()) => self.report(where_, Ok(())),
            Err(TabsError::Thrown(e)) => self.report(where_, Err(e)),
            Err(TabsError::Unmodeled(what)) => {
                Err(err(format!("unmodeled upstream state: {what}")))
            }
        }
    }

    fn frame(&mut self) -> Result<(), String> {
        let out = self.tabs.periodic(&mut self.host);
        for update in out.satellite_updates {
            if self.ignored.contains("update-satellite") {
                continue;
            }
            let message = update.to_message();
            self.emitted.push(Emit::new(vec![
                ("main", cve_str(message.name.name())),
                ("data", js_json::from_js(&message.data)),
            ]));
        }
        let tab_error = out.error;
        let mut list_errors = Vec::new();
        for i in 0..self.tabs.len() {
            if let Ok(r) = self.stats(i, |c, host| c.source_list_frame(host)) {
                list_errors.extend(r.err());
            }
        }
        let sel = &mut self.host.selection;
        sel.get_timeline_range();
        sel.get_hovered_time();
        sel.get_selected_time();
        sel.get_mode();
        sel.get_render_time();
        sel.get_grab_zoom_range();
        sel.get_current_live_time();
        match tab_error {
            Some(e) => self.report_tabs("requestAnimationFrame", Err(e))?,
            None => self.drain(),
        }
        for e in list_errors {
            self.report("requestAnimationFrame", Err(e))?;
        }
        Ok(())
    }

    fn advance(&mut self, ms: f64) -> Result<(), String> {
        let end = self.host.clock.get() + ms;
        loop {
            let frame_at = self.last_frame + self.frame_period;
            if frame_at <= end {
                self.last_frame = frame_at;
                self.host.clock.set(frame_at);
                self.frame()?;
            } else {
                self.host.clock.set(end);
                return Ok(());
            }
        }
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
                let JsValue::Array(uuids) = data else {
                    return Err(err("set-active-satellites needs an array"));
                };
                tabs.set_active_satellites(uuids);
                Ok(())
            }
            "restore-state" => {
                tabs.restore_state(cx, &data.get("tabs").cloned().unwrap_or_default())
            }
            "set-assets" => tabs.new_assets(cx),
            "restore-type-memory" => {
                cx.type_memory = type_memory_of(&data);
                Ok(())
            }
            "set-version" => {
                let mut ctx = cx.selection.context().clone();
                ctx.platform = data
                    .get("platform")
                    .and_then(JsValue::as_str)
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default();
                cx.selection.set_context(ctx);
                Ok(())
            }
            "set-fullscreen"
            | "set-battery"
            | "show-update-button"
            | "show-feedback-button"
            | "show-when-ready"
            | "set-preferences"
            | "set-focused"
            | "set-active-xr-uuid"
            | "call-selection-setter" => Ok(()),
            other => return Err(err(format!("unsupported main message {other}"))),
        };
        // hub.ts handles main messages in an async function, so a throw is an unhandled
        // rejection, which the oracle records as `${reason.name}: ${reason.message}`.
        let result = result.map_err(|e| match e {
            TabsError::Thrown(e) if e.name.is_empty() => {
                TabsError::Thrown(JsError::new("undefined", "undefined"))
            }
            other => other,
        });
        self.report_tabs("unhandledRejection", result)
    }

    /// `window.selection.<method>(<json args>)` on a `selection` step's `[method, ...args]`.
    fn selection_call(&mut self, method: &str, args: &[Value]) -> Result<(), JsError> {
        let arg = |i: usize| -> JsValue {
            js_json::to_js(args.get(i).unwrap_or(&Value::Null)).unwrap_or_default()
        };
        let sel = &mut self.host.selection;
        match method {
            "setSelectedTime" => {
                if let JsValue::Number(t) = arg(0) {
                    sel.set_selected_time(t)
                }
            }
            "setHoveredTime" => match arg(0) {
                JsValue::Number(t) => sel.set_hovered_time(Some(t)),
                JsValue::Null => sel.set_hovered_time(None),
                _ => {}
            },
            "setTimelineRange" => {
                let range = match arg(0) {
                    JsValue::Array(r) => match r.as_slice() {
                        [JsValue::Number(a), JsValue::Number(b)] => [*a, *b],
                        _ => [0.0, 0.0],
                    },
                    _ => [0.0, 0.0],
                };
                let lock = matches!(arg(1), JsValue::Bool(true));
                sel.set_timeline_range(range, lock);
            }
            "setGrabZoomRange" => {
                let range = match arg(0) {
                    JsValue::Array(r) => match r.as_slice() {
                        [JsValue::Number(a), JsValue::Number(b)] => Some([*a, *b]),
                        _ => None,
                    },
                    _ => None,
                };
                sel.set_grab_zoom_range(range);
            }
            "finishGrabZoom" => sel.finish_grab_zoom(),
            "setLiveConnected" => {
                let live = self.live_time.clone();
                sel.set_live_connected(Rc::new(move || live.get()));
            }
            "setLiveDisconnected" => sel.set_live_disconnected(),
            "goIdle" => sel.go_idle(),
            "play" => sel.play(),
            "pause" => sel.pause(),
            "togglePlayback" => sel.toggle_playback(),
            "lock" => sel.lock(),
            "unlock" => sel.unlock(),
            "toggleLock" => sel.toggle_lock(),
            "stepCycle" => sel.step_cycle(matches!(arg(0), JsValue::Bool(true))),
            "setPlaybackSpeed" => {
                if let JsValue::Number(n) = arg(0) {
                    sel.set_playback_speed(n);
                }
            }
            "setPlaybackLooping" => sel.set_playback_looping(matches!(arg(0), JsValue::Bool(true))),
            other => {
                return Err(JsError::type_error(format!(
                    "window.selection.{other} is not a function"
                )));
            }
        }
        Ok(())
    }

    /// An `eval` step: the statement forms the fixtures drive.
    fn eval(&mut self, expr: &str) -> Result<(), String> {
        if expr.starts_with("/*tabs:recorder*/") || expr.starts_with("/*tabs:tag*/") {
            return Ok(());
        }
        if let Some(inner) = expr
            .strip_prefix("window.tabs.setSelected(")
            .and_then(|s| s.strip_suffix(')'))
        {
            let i: f64 = inner.parse().map_err(|_| err(format!("eval {expr}")))?;
            let r = self.tabs.set_selected(&self.host, &JsValue::Number(i));
            return self.report_tabs("eval", r);
        }
        if let Some((tab_i, rest)) = Self::split_controller(&self.tabs, expr) {
            let result = self.controller_call(tab_i, &rest, expr)?;
            return self.report("eval", result);
        }
        if expr.starts_with("window.selection.") {
            if let Some(rest) = expr.strip_prefix("window.selection.")
                && let Some((method, arg_s)) = rest.split_once('(')
            {
                let arg_s = arg_s
                    .strip_suffix(')')
                    .ok_or_else(|| err(format!("eval {expr}")))?;
                let list =
                    asrs_parity::json::parse(&format!("[{arg_s}]")).map_err(|e| e.to_string())?;
                let args: Vec<Value> = list
                    .as_array()
                    .map(|a| a.iter().map(Value::from_json_lenient).collect())
                    .unwrap_or_default();
                let method = method.to_owned();
                let r = self.selection_call(&method, &args);
                return self.report("eval", r);
            }
            return Err(err(format!("unsupported eval {expr}")));
        }
        Err(err(format!("unsupported eval {expr}")))
    }

    /// `window.tabs.tabList[<i>].controller.<rest>`; `<i>` may be `window.tabs.selectedTab`.
    fn split_controller(tabs: &Tabs<Host>, expr: &str) -> Option<(usize, String)> {
        let rest = expr.strip_prefix("window.tabs.tabList[")?;
        let (index, rest) = rest.split_once("].controller.")?;
        let index = if index == "window.tabs.selectedTab" {
            tabs.selected_index()?
        } else {
            index.parse().ok()?
        };
        Some((index, rest.to_owned()))
    }

    /// `.controller.<member>(<json>)` and `.controller.<input>.value = <json>` statements.
    fn controller_call(
        &mut self,
        tab_i: usize,
        rest: &str,
        expr: &str,
    ) -> Result<Result<(), JsError>, String> {
        if let Some((member, arg_s)) = rest.split_once('(') {
            let arg_s = arg_s
                .strip_suffix(')')
                .ok_or_else(|| err(format!("eval {expr}")))?;
            let json = asrs_parity::json::parse(arg_s).map_err(|e| e.to_string())?;
            let arg = to_js(&json)?;
            return Ok(match member {
                "sourceList.addField" => {
                    let JsValue::String(key) = arg else {
                        return Err(err(format!("addField arg in {expr}")));
                    };
                    self.stats(tab_i, |c, host| {
                        host.with_source_env(|env| {
                            c.source_list_mut()
                                .add_field(env, &[], &key, None)
                                .map_err(source_error)
                        })
                    })?
                }
                "sourceList.setState" => {
                    self.stats(tab_i, |c, host| c.set_source_state(host, &arg))?
                }
                "restoreState" => self.stats(tab_i, |c, host| c.restore_state(host, &arg))?,
                other => return Err(err(format!("unsupported eval member {other}"))),
            });
        }
        if let Some((input, value_s)) = rest.split_once(".value = ") {
            let json = asrs_parity::json::parse(value_s).map_err(|e| e.to_string())?;
            let value = asrs_js::ops::to_string(&to_js(&json)?);
            let f = match input {
                "RANGE_MIN" => {
                    StatisticsController::set_range_min as fn(&mut StatisticsController, JsString)
                }
                "RANGE_MAX" => {
                    StatisticsController::set_range_max as fn(&mut StatisticsController, JsString)
                }
                "STEP_SIZE" => {
                    StatisticsController::set_step_size as fn(&mut StatisticsController, JsString)
                }
                "TIME_RANGE" => {
                    StatisticsController::set_time_range as fn(&mut StatisticsController, JsString)
                }
                other => return Err(err(format!("unsupported eval assign {other}"))),
            };
            self.stats(tab_i, |c, _| f(c, value.clone()))?;
            return Ok(Ok(()));
        }
        Err(err(format!("unsupported eval {expr}")))
    }

    /// The `/*tabs:info*/` record with each live statistics tab's `saveState()`,
    /// `showTimeline()` and `getActiveFields()` taken from the port; the stub tabs answer from
    /// the record itself, and column 0 is the creation-time UUID the recorder logged.
    fn with_port_stats_rows(&self, info: Value) -> Value {
        let Value::Array(mut rows) = info else {
            return info;
        };
        for tab in self.tabs.tabs() {
            let Some(half) = tab.controller().downcast_ref::<StatsHalf>() else {
                continue;
            };
            let Some(Value::Array(row)) = rows.get_mut(half.k) else {
                continue;
            };
            let fields = half
                .active_fields(&self.host)
                .into_iter()
                .map(JsValue::String)
                .collect();
            let port = [
                (1, half.save_state(&self.host)),
                (3, JsValue::Bool(half.show_timeline(&self.host))),
                (4, JsValue::Array(fields)),
            ];
            for (column, value) in port {
                if let Some(cell) = row.get_mut(column) {
                    *cell = js_json::from_js(&value);
                }
            }
        }
        Value::Array(rows)
    }

    /// A `{kind: "eval"}` capture: `/*tabs:info*/` records verbatim, `/*tabs:restores*/`, and
    /// controller/Tabs probes answered from the port.
    fn eval_capture(&mut self, expr: &str) -> Result<Value, EvalError> {
        if expr.starts_with("/*tabs:info*/") {
            let info =
                self.infos.get(self.info_seen).cloned().ok_or_else(|| {
                    EvalError::Fatal(err("more info captures than the vector has"))
                })?;
            self.info_seen += 1;
            let current = current_from(&info).map_err(EvalError::Fatal)?;
            if current.len() != self.host.created {
                return Err(EvalError::Fatal(format!(
                    "port created {} tabs, upstream {}",
                    self.host.created,
                    current.len()
                )));
            }
            self.host.current = current;
            return Ok(self.with_port_stats_rows(info));
        }
        if expr.starts_with("/*tabs:restores*/") {
            return Ok(Value::Array(
                self.host
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
            ));
        }
        let fatal = |e: String| EvalError::Fatal(e);
        if expr == "window.tabs.saveState()" {
            return Ok(js_json::from_js(&self.tabs.save_state(&self.host)));
        }
        if expr == "[...window.tabs.getActiveFields()]" || expr == "window.tabs.getActiveFields()" {
            let fields = self
                .tabs
                .active_fields(&self.host, &self.host.log.borrow())
                .iter()
                .map(|s| js_json::from_js(&JsValue::String(s.clone())))
                .collect();
            return Ok(Value::Array(fields));
        }
        let (index, member) = Self::split_controller(&self.tabs, expr)
            .ok_or_else(|| fatal(format!("unsupported eval {expr}")))?;
        let member = member.strip_suffix("()").unwrap_or(&member);
        let Some(tab) = self.tabs.tab(index) else {
            return Err(EvalError::Thrown(
                "TypeError: Cannot read properties of undefined (reading 'controller')".to_owned(),
            ));
        };
        let uuid = tab.uuid().clone();
        // Members every controller answers.
        match member {
            "showTimeline" => {
                return Ok(js_json::from_js(&JsValue::Bool(
                    tab.controller().show_timeline(&self.host),
                )));
            }
            "getActiveFields" => {
                return Ok(js_json::from_js(&JsValue::Array(
                    tab.controller()
                        .active_fields(&self.host)
                        .into_iter()
                        .map(JsValue::String)
                        .collect(),
                )));
            }
            "saveState" => return Ok(js_json::from_js(&tab.controller().save_state(&self.host))),
            "UUID" => return Ok(js_json::from_js(&uuid)),
            _ => {}
        }
        let controller = tab
            .controller()
            .downcast_ref::<StatsHalf>()
            .map(|h| &h.inner)
            .ok_or_else(|| fatal(format!("tab {index} is not statistics")))?;
        let inputs = controller.histogram_inputs();
        let js = |s: &JsString| js_json::from_js(&JsValue::String(s.clone()));
        Ok(match member {
            "shouldUpdate" => js_json::from_js(&JsValue::Bool(controller.should_update())),
            "RANGE_MIN.value" => js(&inputs.range_min),
            "RANGE_MAX.value" => js(&inputs.range_max),
            "STEP_SIZE.value" => js(&inputs.step_size),
            "RANGE_MIN.step" => js(&inputs.range_min_step),
            "RANGE_MAX.step" => js(&inputs.range_max_step),
            "STEP_SIZE.step" => js(&inputs.step_size_step),
            "TIME_RANGE.value" => js(&inputs.time_range.clone().unwrap_or_default()),
            "TIME_RANGE.options[0].disabled"
            | "TIME_RANGE.options[1].disabled"
            | "TIME_RANGE.options[2].disabled"
            | "TIME_RANGE.options[3].disabled"
            | "TIME_RANGE.options[4].disabled" => js_json::from_js(&JsValue::Bool(false)),
            "TIME_RANGE.options[5].disabled" | "TIME_RANGE.options[6].disabled" => {
                js_json::from_js(&JsValue::Bool(controller.live_options_disabled()))
            }
            "sourceList.getState" => js_json::from_js(&JsValue::Array(
                controller
                    .source_list()
                    .state()
                    .iter()
                    .map(ToJs::to_js)
                    .collect(),
            )),
            "sourceList.getState(true)" => {
                let displayed = controller
                    .source_list()
                    .displayed_state(&self.host.log.borrow());
                js_json::from_js(&JsValue::Array(displayed.iter().map(ToJs::to_js).collect()))
            }
            other => return Err(fatal(format!("unsupported eval member {other}"))),
        })
    }
}

enum EvalError {
    /// The evaluated call threw; the record is `throw` with the thrown text.
    Thrown(String),
    Fatal(String),
}

/// `w.log[method](...args)`: `LogError`s and bad calls become `exception("log", ...)` records,
/// so they surface as `JsError`s here.
fn run_log_step(log: &SharedLog, calls: &[Value]) -> Result<(), JsError> {
    let bad = |what: &str| JsError::type_error(what);
    for call in calls {
        let parts = call
            .as_array()
            .ok_or_else(|| bad("log step entries are [method, ...args]"))?;
        let (method, args) = parts
            .split_first()
            .ok_or_else(|| bad("empty log step entry"))?;
        let method = method
            .as_str()
            .ok_or_else(|| bad("log step method is a string"))?
            .to_string_lossy();
        let key = |i: usize| -> Result<JsString, JsError> {
            match args.get(i) {
                Some(Value::String(s)) => Ok(JsString::from_units(s.units().to_vec())),
                other => Err(bad(&format!("log op key {other:?}"))),
            }
        };
        let num = |i: usize| -> Result<f64, JsError> {
            args.get(i).and_then(Value::as_f64).ok_or_else(|| {
                bad(&format!(
                    "log op arg {}",
                    args.get(i).unwrap_or(&Value::Null).encode()
                ))
            })
        };
        let js = |i: usize| -> Result<JsValue, JsError> {
            js_json::to_js(args.get(i).unwrap_or(&Value::Null)).map_err(|t| bad(&t))
        };
        let log = &mut *log.borrow_mut();
        match method.as_str() {
            "putBoolean" => log.put_boolean(
                &key(0)?,
                num(1)?,
                matches!(args.get(2), Some(Value::Bool(true))),
            ),
            "putNumber" => log.put_number(&key(0)?, num(1)?, num(2)?),
            "putString" => {
                let s = match args.get(2) {
                    Some(Value::String(s)) => JsString::from_units(s.units().to_vec()),
                    other => return Err(bad(&format!("putString value {other:?}"))),
                };
                log.put_string(&key(0)?, num(1)?, s)
            }
            "putBooleanArray" => log.put_boolean_array(
                &key(0)?,
                num(1)?,
                args.get(2)
                    .and_then(Value::as_array)
                    .ok_or_else(|| bad("putBooleanArray value is an array"))?
                    .iter()
                    .map(|v| matches!(v, Value::Bool(true)))
                    .collect(),
            ),
            "putNumberArray" => log.put_number_array(
                &key(0)?,
                num(1)?,
                args.get(2)
                    .and_then(Value::as_array)
                    .ok_or_else(|| bad("putNumberArray value is an array"))?
                    .iter()
                    .map(|v| v.as_f64().unwrap_or(f64::NAN))
                    .collect(),
            ),
            "putStringArray" => log.put_string_array(
                &key(0)?,
                num(1)?,
                args.get(2)
                    .and_then(Value::as_array)
                    .ok_or_else(|| bad("putStringArray value is an array"))?
                    .iter()
                    .map(|v| match v {
                        Value::String(s) => JsString::from_units(s.units().to_vec()),
                        _ => JsString::new(),
                    })
                    .collect(),
            ),
            "putRaw" => {
                let bytes = match args.get(2) {
                    Some(Value::Bytes(b)) => b.clone(),
                    other => return Err(bad(&format!("putRaw value {other:?}"))),
                };
                log.put_raw(
                    &key(0)?,
                    num(1)?,
                    std::sync::Arc::<[u8]>::from(bytes.into_boxed_slice()),
                )
                .map_err(|e| JsError::new("Error", format!("{e:?}")))?
            }
            "putPose" => {
                let pose = js(2)?;
                let translation = match asrs_js::ops::get(&pose, "translation")? {
                    JsValue::Array(t) => match t.as_slice() {
                        [JsValue::Number(x), JsValue::Number(y)] => [*x, *y],
                        _ => [0.0, 0.0],
                    },
                    _ => [0.0, 0.0],
                };
                let rotation = match asrs_js::ops::get(&pose, "rotation")? {
                    JsValue::Number(r) => r,
                    _ => 0.0,
                };
                log.put_pose(&key(0)?, num(1)?, translation, rotation)
            }
            "putUnknownStruct" => log
                .put_unknown_struct(&key(0)?, num(1)?, &js(2)?, false)
                .map_err(|e| JsError::new("Error", format!("{e:?}")))?,
            "createBlankField" => {
                let ty = match num(1)? as u8 {
                    0 => asrs_log::log::LoggableType::Raw,
                    1 => asrs_log::log::LoggableType::Boolean,
                    2 => asrs_log::log::LoggableType::Number,
                    3 => asrs_log::log::LoggableType::String,
                    4 => asrs_log::log::LoggableType::BooleanArray,
                    5 => asrs_log::log::LoggableType::NumberArray,
                    6 => asrs_log::log::LoggableType::StringArray,
                    _ => asrs_log::log::LoggableType::Empty,
                };
                log.create_blank_field(&key(0)?, ty)
            }
            "setStructuredType" => log.set_structured_type(
                &key(0)?,
                match args.get(1) {
                    Some(Value::String(s)) => Some(JsString::from_units(s.units().to_vec())),
                    _ => None,
                },
            ),
            "setGeneratedParent" => log.set_generated_parent(&key(0)?),
            "clearBeforeTime" => {
                log.clear_before_time(num(0)?, matches!(args.get(1), Some(Value::Bool(true))))
            }
            "deleteField" => log.delete_field(&key(0)?),
            "updateRangeWithTimestamp" => log.update_range_with_timestamp(num(0)?),
            other => return Err(bad(&format!("log.{other} is not a function"))),
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

fn capture(runner: &mut Runner, spec: &Json) -> Result<(), String> {
    let kind = match spec {
        Json::String(_) => str_of(spec).unwrap_or_default(),
        _ => spec.get("kind").and_then(str_of).unwrap_or_default(),
    };
    match kind.as_str() {
        "commands" => {
            for i in 0..runner.tabs.len() {
                let Some(k) = runner
                    .tabs
                    .tab(i)
                    .and_then(|t| t.renderer().downcast_ref::<StubRenderer>())
                    .map(|r| r.0)
                else {
                    continue;
                };
                let Some(command) = runner.host.rendered.get(k).cloned().flatten() else {
                    continue;
                };
                runner.host.rendered[k] = None;
                let tab = runner.tabs.tab(i).ok_or("tab vanished")?;
                let mut meta = JsObject::new();
                meta.insert("index", JsValue::Number(i as f64));
                meta.insert("type", tab.type_value().clone());
                meta.insert("title", tab.title().clone());
                meta.insert("uuid", tab.uuid().clone());
                let mut values: Vec<(&'static str, Value)> =
                    vec![("command", js_json::from_js(&JsValue::Object(meta)))];
                if tab.controller().downcast_ref::<StatsHalf>().is_some() {
                    values.push(("value", js_json::from_js(&command)));
                }
                runner.emitted.push(Emit { values });
            }
            Ok(())
        }
        "state" => {
            let mut sidebar = JsObject::new();
            sidebar.insert("width", JsValue::Number(300.0));
            sidebar.insert("expanded", JsValue::Array(vec![]));
            let mut state = JsObject::new();
            state.insert("sidebar", JsValue::Object(sidebar));
            state.insert("tabs", runner.tabs.save_state(&runner.host));
            runner.emitted.push(Emit::new(vec![(
                "state",
                js_json::from_js(&JsValue::Object(state)),
            )]));
            Ok(())
        }
        "selection" => {
            let sel = &mut runner.host.selection;
            runner
                .emitted
                .push(Emit::new(vec![("selection", selection_value(sel))]));
            Ok(())
        }
        "eval" => {
            let expr = spec
                .get("expr")
                .and_then(str_of)
                .ok_or("eval capture needs expr")?;
            match runner.eval_capture(&expr) {
                Ok(value) => runner
                    .emitted
                    .push(Emit::new(vec![("eval", cve_str(&expr)), ("value", value)])),
                Err(EvalError::Thrown(text)) => runner.emitted.push(Emit::new(vec![
                    ("eval", cve_str(&expr)),
                    ("throw", cve_str(&text)),
                ])),
                Err(EvalError::Fatal(e)) => return Err(e),
            }
            Ok(())
        }
        other => Err(format!("unsupported capture {other}")),
    }
}

fn replay(spec_path: &Path, records: &[Record]) -> Result<Vec<Emit>, String> {
    let text = std::fs::read_to_string(spec_path).map_err(|e| e.to_string())?;
    let spec = asrs_parity::json::parse(&text).map_err(|e| e.to_string())?;
    let fixture = spec.get("fixture").ok_or("spec has no fixture")?;
    let window = fixture.get("window");
    let window_number = |key: &str, default: f64| {
        window
            .and_then(|w| w.get(key))
            .and_then(Json::as_f64)
            .unwrap_or(default)
    };
    let frame_period = window_number("framePeriod", 16.0);
    let inner_height = window_number("height", 650.0);
    let dark = window
        .and_then(|w| w.get("theme"))
        .and_then(Json::as_str)
        .is_some_and(|t| t.to_string_lossy() == "dark");
    let start = fixture
        .get("startTime")
        .and_then(Json::as_f64)
        .unwrap_or(1767225600000.0);
    let renderer_top = fixture
        .get("layout")
        .and_then(Json::as_array)
        .unwrap_or(&[])
        .iter()
        .find(|r| r.get("selector").and_then(str_of).as_deref() == Some("div.renderer-content"))
        .and_then(|r| {
            r.get("rect")
                .and_then(Json::as_array)?
                .get(1)
                .and_then(Json::as_f64)
        })
        .unwrap_or(92.0);
    let ignored: HashSet<String> = match fixture.get("ignoreMain").and_then(Json::as_array) {
        Some(names) => names.iter().filter_map(str_of).collect(),
        None => DEFAULT_IGNORED.iter().map(|s| (*s).to_owned()).collect(),
    };

    let seed = fixture.get("seed").and_then(Json::as_f64).unwrap_or(1.0) as u64;
    let clock = Rc::new(Cell::new(start));
    let log: SharedLog = Rc::new(RefCell::new(Log::new(true)));
    let live_time = Rc::new(Cell::new(0.0f64));
    let infos = info_records(records)?;
    let mut rng = Mulberry::new(seed);
    rng.next_f64(); // BigNumber.random's implementation check runs at bundle init.
    let mut host = Host {
        inner_height,
        renderer_top,
        log: log.clone(),
        selection: SelectionImpl::new(log.clone(), Rc::new(ScriptedClock(clock.clone()))),
        clock: clock.clone(),
        rng,
        type_memory: SourceListTypeMemory::new(),
        dark,
        seeds: seeds(&infos)?,
        current: Vec::new(),
        restores: Vec::new(),
        rendered: Vec::new(),
        created: 0,
        messages: Vec::new(),
    };
    let tabs = Tabs::new(Box::new(StatFactory), &mut host);
    let mut runner = Runner {
        tabs,
        host,
        emitted: Vec::new(),
        ignored,
        infos,
        info_seen: 0,
        start,
        last_frame: start,
        frame_period,
        live_time,
    };

    // The default boot sequence, or the fixture's own `boot` list.
    let boot: Vec<(String, Json)> = match fixture.get("boot") {
        Some(list) => list
            .as_array()
            .ok_or("boot is a list")?
            .iter()
            .map(|m| {
                let m = m.as_array().ok_or("boot entry is [name, data]")?;
                let name = m
                    .first()
                    .and_then(str_of)
                    .ok_or("boot entry name is a string")?;
                Ok((name, m.get(1).cloned().unwrap_or(Json::Null)))
            })
            .collect::<Result<_, String>>()?,
        None => [
            (
                "set-assets".to_owned(),
                fixture.get("assets").cloned().unwrap_or(Json::Null),
            ),
            ("set-fullscreen".to_owned(), Json::Bool(false)),
            ("set-battery".to_owned(), Json::Bool(false)),
            (
                "set-version".to_owned(),
                fixture.get("version").cloned().unwrap_or(Json::Null),
            ),
            ("show-update-button".to_owned(), Json::Bool(false)),
            ("show-feedback-button".to_owned(), Json::Bool(false)),
            ("show-when-ready".to_owned(), Json::Null),
            ("set-preferences".to_owned(), Json::Null),
            ("set-active-satellites".to_owned(), Json::Array(vec![])),
        ]
        .into_iter()
        .chain(
            fixture
                .get("typeMemory")
                .map(|v| ("restore-type-memory".to_owned(), v.clone())),
        )
        .chain(
            fixture
                .get("state")
                .map(|v| ("restore-state".to_owned(), v.clone())),
        )
        .collect(),
    };
    for (name, data) in &boot {
        let data = to_js(&load_json(spec_path, data)?)?;
        runner.main(name, data)?;
    }

    let steps = fixture.get("steps").and_then(Json::as_array).unwrap_or(&[]);
    for (index, step) in steps.iter().enumerate() {
        let where_ = |e: String| format!("step {index}: {e}");
        if let Some(name) = step.get("main").and_then(str_of) {
            let data = match (step.get("cve1"), step.get("data")) {
                (Some(cve), _) => js_json::to_js(&Value::from_json(cve).map_err(|e| e.0)?)?,
                (None, Some(data)) => to_js(&load_json(spec_path, data)?)?,
                (None, None) => JsValue::Undefined,
            };
            runner.main(&name, data).map_err(where_)?;
        } else if let Some(ops) = step.get("log") {
            let ops = load_json(spec_path, ops)?;
            let calls: Vec<Value> = ops
                .as_array()
                .ok_or("log ops are an array")?
                .iter()
                .map(Value::from_json_lenient)
                .collect();
            run_log_step(&log, &calls)
                .map_err(|e| where_(format!("log op threw {}", thrown_text(&e))))?;
            let r = runner.tabs.refresh(&mut runner.host);
            runner.report_tabs("log", r).map_err(where_)?;
        } else if let Some(sel_step) = step.get("selection") {
            let parts = sel_step
                .as_array()
                .ok_or("selection step is [method, ...args]")?;
            let (method, args) = parts.split_first().ok_or("empty selection step")?;
            let method = method
                .as_str()
                .ok_or("selection method is a string")?
                .to_string_lossy();
            let args: Vec<Value> = args.iter().map(Value::from_json_lenient).collect();
            let r = runner.selection_call(&method, &args);
            runner
                .report(&format!("selection {method}"), r)
                .map_err(where_)?;
        } else if let Some(live) = step.get("liveTime") {
            let v = Value::from_json_lenient(live);
            let t = v
                .as_f64()
                .ok_or_else(|| format!("step {index}: liveTime {v:?}"))?;
            runner.live_time.set(t);
        } else if let Some(event) = step.get("event") {
            let target = event
                .get("target")
                .and_then(str_of)
                .ok_or("event needs a target")?;
            let nth = event.get("index").and_then(Json::as_f64).unwrap_or(0.0) as usize;
            let etype = event
                .get("type")
                .and_then(str_of)
                .ok_or("event needs a type")?;
            if etype != "change" {
                return Err(where_(format!("unsupported event {etype}")));
            }
            let Some((_, f)) = INPUT_TARGETS
                .iter()
                .find(|(sel, _)| *sel == target.as_str())
            else {
                return Err(where_(format!("unsupported event target {target}")));
            };
            let value = event
                .get("props")
                .and_then(|p| p.get("value"))
                .map(|j| to_js(j).unwrap_or_default())
                .unwrap_or_default();
            let value = asrs_js::ops::to_string(&value);
            let index = runner.stats_nth(nth).map_err(where_)?;
            let f = *f;
            runner.stats(index, |c, _| f(c, value.clone()))?;
            runner.drain();
        } else if let Some(expr) = step.get("eval").and_then(str_of) {
            runner.eval(&expr).map_err(where_)?;
        } else if let Some(ms) = step.get("advance").and_then(Json::as_f64) {
            runner.advance(ms).map_err(where_)?;
        } else if let Some(n) = step.get("frames").and_then(Json::as_f64) {
            runner.advance(n * frame_period).map_err(where_)?;
        } else if let Some(specs) = step.get("capture") {
            let label = step
                .get("label")
                .map_or(Value::number(index as f64), Value::from_json_lenient);
            runner.emitted.push(Emit::new(vec![
                ("capture", label),
                (
                    "time",
                    Value::number(runner.host.clock.get() - runner.start),
                ),
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
            let expected = record
                .value(key)
                .map_err(|e| format!("line {}: missing {key}: {}", record.line, e.0))?;
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
    let dir = vectors::workspace_root().join("parity/specs/dom/statistics");
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
            Some((p, format!("dom/statistics/{name}")))
        })
        .collect();
    out.sort();
    out
}

#[test]
fn statistics_dom_matches_upstream() {
    let specs = specs();
    assert!(!specs.is_empty(), "no statistics specs");
    let mut failures = Vec::new();
    let mut lines = 0;
    for (spec, id) in &specs {
        let result = vectors::load(id).and_then(|records| {
            lines += records.len();
            compare(&replay(spec, &records)?, &records)
        });
        if let Err(e) = result {
            failures.push(format!("{id}: {e}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    eprintln!(
        "dom/statistics: {} fixtures, {lines} records, 0 mismatches",
        specs.len()
    );
}
