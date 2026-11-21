#![allow(clippy::disallowed_methods)]

//! Replays the `dom/field-2d/<case>` vectors (fixtures written by
//! `oracle/tools/field2dSpecs.mjs`) against `asrs_tabs::field_2d`. `slLog` records
//! build the log, `sl` ops drive the controller's `SourceList`, `f2d` records carry
//! each controller call with the selection reads upstream made during it, and
//! `f2dr` records carry renderer renders, camera calls, scroll/mouse events and the
//! DOM values upstream read. Commands, controls, saved state, main messages, list
//! dumps, pending scroll callbacks and exceptions are all compared bit-exact.

#[path = "../../asrs-cli/src/functions.rs"]
#[allow(dead_code)]
mod functions;
#[path = "../../asrs-cli/src/js_color.rs"]
#[allow(dead_code)]
mod js_color;
#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;
#[path = "../../asrs-cli/src/js_math.rs"]
#[allow(dead_code)]
mod js_math;
#[path = "../../asrs-cli/src/js_msgpack.rs"]
#[allow(dead_code)]
mod js_msgpack;
#[path = "../../asrs-cli/src/js_regex.rs"]
#[allow(dead_code)]
mod js_regex;
#[path = "../../asrs-cli/src/js_string.rs"]
#[allow(dead_code)]
mod js_string;
#[path = "../../asrs-cli/src/js_time.rs"]
#[allow(dead_code)]
mod js_time;
#[path = "../../asrs-cli/src/log_field.rs"]
#[allow(dead_code)]
mod log_field;
#[path = "../../asrs-cli/src/log_keys.rs"]
#[allow(dead_code)]
mod log_keys;
#[path = "../../asrs-cli/src/log_run.rs"]
#[allow(dead_code)]
mod log_run;
#[path = "../../asrs-cli/src/log_struct.rs"]
#[allow(dead_code)]
mod log_struct;
#[path = "../../asrs-cli/src/photon.rs"]
#[allow(dead_code)]
mod photon;
#[path = "../../asrs-cli/src/shared.rs"]
#[allow(dead_code)]
mod shared;
#[path = "../../asrs-cli/src/state.rs"]
#[allow(dead_code)]
mod state;

mod tabs_console {
    pub const FUNCTIONS: &[(&str, crate::functions::Function)] = &[];
    pub const CALLS: &[(&str, crate::functions::Function)] = &[];
}

use std::cell::{Ref, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use asrs_js::string::{JsError, JsString};
use asrs_js::time::Rng;
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::log::Log;
use asrs_log::realm::JsRealm;
use asrs_model::assets::AdvantageScopeAssets;
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::preferences::{CoordinateSystemPreference, Preferences, Timestamps};
use asrs_model::selection::{Selection, SelectionMode, TimeSupplier};
use asrs_model::source_list::{Env, ItemView, Length, SourceList};
use asrs_model::source_list_config::{
    SourceListItemState, SourceListTypeMemory, SourceListTypeMemoryEntry,
};
use asrs_model::tab_controller::{TabController, TabRenderer};
use asrs_parity::Value;
use asrs_parity::vectors::{Record, load, vectors_dir};
use asrs_tabs::cx::TabCx;
use asrs_tabs::field_2d::{Field2dCommand, Field2dController, Field2dDom, Field2dRenderer};
use asrs_tabs::list_cx::{ListParts, SourceListCx};

const CASES: &str = "dom/field-2d";

struct NoRandom;

impl Rng for NoRandom {
    fn next_f64(&mut self) -> f64 {
        panic!("the field2d harness draws no Math.random values")
    }
}

/// Answers each selection read with the value upstream's call returned, in order.
#[derive(Default)]
struct ScriptedSelection {
    reads: RefCell<VecDeque<(String, JsValue)>>,
    error: RefCell<Option<String>>,
}

impl ScriptedSelection {
    fn load(&self, reads: VecDeque<(String, JsValue)>) {
        *self.reads.borrow_mut() = reads;
    }

    fn fail(&self, message: String) {
        self.error.borrow_mut().get_or_insert(message);
    }

    fn next(&self, name: &str) -> JsValue {
        match self.reads.borrow_mut().pop_front() {
            Some((n, v)) if n == name => v,
            other => {
                self.fail(format!("port read {name}, upstream read {other:?}"));
                JsValue::Null
            }
        }
    }

    fn finish(&self) -> Result<(), String> {
        if let Some(e) = self.error.borrow_mut().take() {
            return Err(e);
        }
        let left = std::mem::take(&mut *self.reads.borrow_mut());
        if !left.is_empty() {
            return Err(format!(
                "upstream made selection reads the port did not: {left:?}"
            ));
        }
        Ok(())
    }

    fn time(&self, name: &str) -> Option<f64> {
        match self.next(name) {
            JsValue::Number(n) => Some(n),
            _ => None,
        }
    }

    fn pair(&self, name: &str) -> Option<[f64; 2]> {
        match self.next(name) {
            JsValue::Array(a) => match a.as_slice() {
                [JsValue::Number(x), JsValue::Number(y)] => Some([*x, *y]),
                _ => None,
            },
            _ => None,
        }
    }

    fn unexpected(&self, name: &str) {
        self.fail(format!("the port called selection.{name}"));
    }
}

impl Selection for ScriptedSelection {
    fn get_mode(&self) -> SelectionMode {
        let n = self.next("getMode").as_f64().unwrap_or(f64::NAN);
        SelectionMode::ALL
            .into_iter()
            .find(|m| m.number() == n)
            .unwrap_or(SelectionMode::Idle)
    }
    fn get_hovered_time(&self) -> Option<f64> {
        self.time("getHoveredTime")
    }
    fn set_hovered_time(&mut self, _: Option<f64>) {
        self.unexpected("setHoveredTime")
    }
    fn get_selected_time(&mut self) -> Option<f64> {
        self.time("getSelectedTime")
    }
    fn set_selected_time(&mut self, _: f64) {
        self.unexpected("setSelectedTime")
    }
    fn go_idle(&mut self) {
        self.unexpected("goIdle")
    }
    fn play(&mut self) {
        self.unexpected("play")
    }
    fn pause(&mut self) {
        self.unexpected("pause")
    }
    fn toggle_playback(&mut self) {
        self.unexpected("togglePlayback")
    }
    fn lock(&mut self) {
        self.unexpected("lock")
    }
    fn unlock(&mut self) {
        self.unexpected("unlock")
    }
    fn toggle_lock(&mut self) {
        self.unexpected("toggleLock")
    }
    fn step_cycle(&mut self, _: bool) {
        self.unexpected("stepCycle")
    }
    fn set_live_connected(&mut self, _: TimeSupplier) {
        self.unexpected("setLiveConnected")
    }
    fn set_live_disconnected(&mut self) {
        self.unexpected("setLiveDisconnected")
    }
    fn get_current_live_time(&self) -> Option<f64> {
        self.time("getCurrentLiveTime")
    }
    fn get_render_time(&mut self) -> Option<f64> {
        self.time("getRenderTime")
    }
    fn set_playback_speed(&mut self, _: f64) {
        self.unexpected("setPlaybackSpeed")
    }
    fn set_playback_looping(&mut self, _: bool) {
        self.unexpected("setPlaybackLooping")
    }
    fn set_grab_zoom_range(&mut self, _: Option<[f64; 2]>) {
        self.unexpected("setGrabZoomRange")
    }
    fn get_grab_zoom_range(&self) -> Option<[f64; 2]> {
        self.pair("getGrabZoomRange")
    }
    fn finish_grab_zoom(&mut self) {
        self.unexpected("finishGrabZoom")
    }
    fn get_timeline_range(&mut self) -> [f64; 2] {
        self.pair("getTimelineRange").unwrap_or([f64::NAN; 2])
    }
    fn get_timeline_is_max_zoom(&self) -> bool {
        match self.next("getTimelineIsMaxZoom") {
            JsValue::Bool(b) => b,
            _ => false,
        }
    }
    fn set_timeline_range(&mut self, _: [f64; 2], _: bool) {
        self.unexpected("setTimelineRange")
    }
    fn apply_timeline_scroll(&mut self, _: f64, _: f64, _: f64) {
        self.unexpected("applyTimelineScroll")
    }
}

struct Host {
    log: RefCell<Log>,
    memory: SourceListTypeMemory,
    selection: ScriptedSelection,
    preferences: Option<Preferences>,
    /// Raw `window.assets` JSON for the set-assets loadFailures transition.
    assets_raw: JsValue,
    assets: Option<AdvantageScopeAssets>,
    dark: bool,
    rng: NoRandom,
    sent: VecDeque<NamedMessage<HubToMain>>,
}

impl Host {
    fn had_load_failures(raw: &JsValue) -> bool {
        raw.as_object()
            .and_then(|o| o.get("loadFailures"))
            .and_then(JsValue::as_array)
            .is_some_and(|a| !a.is_empty())
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
    fn tab_rng(&mut self) -> &mut dyn Rng {
        &mut self.rng
    }
    fn tab_assets(&self) -> Option<&AdvantageScopeAssets> {
        self.assets.as_ref()
    }
    fn send_main_message(&mut self, message: NamedMessage<HubToMain>) {
        self.sent.push_back(message);
    }
}

impl SourceListCx for Host {
    fn tab_dark(&self) -> bool {
        self.dark
    }
    fn list_parts(&mut self) -> ListParts<'_> {
        ListParts {
            log: self.log.borrow(),
            type_memory: &mut self.memory,
            selection: &mut self.selection,
            dark: self.dark,
        }
    }
}

fn js(record: &Record, key: &str) -> Result<JsValue, String> {
    js_json::to_js(&record.value(key).map_err(|e| e.0)?)
}

fn at(args: &[JsValue], i: usize) -> &JsValue {
    args.get(i).unwrap_or(&JsValue::Undefined)
}

/// `putPose` reaches upstream as `(key, ts, pose)` but the port's logRun takes
/// `(key, ts, translation, rotation)`; rewrite the op before it is applied.
fn translate_pose_op(op: &[Value]) -> Result<Vec<Value>, String> {
    if op
        .first()
        .and_then(Value::as_str)
        .is_some_and(|s| s.eq_str("putPose"))
    {
        if let [name, key, ts, pose, ..] = op {
            let translation = pose.get("translation").ok_or("putPose translation")?;
            let xy = translation.as_array().ok_or("putPose translation pair")?;
            let [x, y] = xy else {
                return Err("putPose translation pair".to_owned());
            };
            let rotation = pose.get("rotation").ok_or("putPose rotation")?;
            return Ok(vec![
                name.clone(),
                key.clone(),
                ts.clone(),
                Value::Array(vec![x.clone(), y.clone()]),
                rotation.clone(),
            ]);
        }
        return Err("putPose args".to_owned());
    }
    Ok(op.to_vec())
}

fn num(v: &JsValue) -> Result<f64, String> {
    v.as_f64()
        .ok_or_else(|| format!("expected a number, got {v:?}"))
}

fn text(v: &JsValue) -> Result<String, String> {
    v.as_str()
        .map(JsString::to_string_lossy)
        .ok_or_else(|| format!("expected a string, got {v:?}"))
}

fn index(v: &JsValue) -> Result<usize, String> {
    let x = num(v)?;
    if x < 0.0 || x.fract() != 0.0 {
        return Err(format!("expected an index, got {x}"));
    }
    Ok(x as usize)
}

fn item_state(v: &JsValue) -> Result<SourceListItemState, String> {
    SourceListItemState::from_js(v).ok_or_else(|| format!("not an item state: {v:?}"))
}

fn encode(v: &JsValue) -> String {
    js_json::from_js(v).encode()
}

fn error_text(e: &JsError) -> String {
    if e.name.is_empty() {
        e.message.clone()
    } else {
        format!("{}: {}", e.name, e.message)
    }
}

fn threw(record: &Record) -> Option<String> {
    record.string("threw")
}

fn same_error(what: &str, upstream: Option<String>, port: Option<String>) -> Result<(), String> {
    if upstream == port {
        Ok(())
    } else {
        Err(format!(
            "{what}: upstream threw {upstream:?}, port {port:?}"
        ))
    }
}

fn item_dump(view: &ItemView) -> JsValue {
    let mut out: Vec<JsValue> = Vec::new();
    match &view.rendered {
        None => {
            out.push("".to_js());
            out.push(JsValue::Null);
            out.extend(["", "", "", ""].map(|s| s.to_js()));
            out.push("symbols/visibility.svg".to_js());
            out.push("Toggle field visibility".to_js());
            out.push(false.to_js());
        }
        Some(r) => {
            out.push(r.symbol_path.to_js());
            out.push(r.color.to_js());
            out.push(r.type_name.to_js());
            out.push(r.log_key.to_js());
            out.push(if r.available { "" } else { "line-through" }.to_js());
            out.push(r.log_key.to_js());
            out.push(r.hide_icon().to_js());
            out.push(r.hide_title().to_js());
            out.push((!r.visible).to_js());
        }
    }
    out.push(view.child.to_js());
    out.push(view.parent_highlight.to_js());
    let hidden = view.preview_shown != Some(true);
    out.push(hidden.to_js());
    out.push(hidden.to_js());
    out.push(view.preview_text.to_js());
    out.push(
        match view.preview_shown {
            None => "",
            Some(true) => "50px",
            Some(false) => "30px",
        }
        .to_js(),
    );
    JsValue::Array(out)
}

fn list_dump(list: &SourceList, log: &Log) -> JsValue {
    let mut o = JsObject::new();
    o.insert("title", list.title().to_js());
    o.insert("state", list.state().to_js());
    o.insert("displayed", list.displayed_state(log).to_js());
    o.insert("stopped", list.is_stopped().to_js());
    let hand = if list.hand_icon_shown() {
        ["0.15", "opacity 1s ease-in 1s"]
    } else {
        ["0", ""]
    };
    o.insert("handIcon", hand.to_vec().to_js());
    let highlight = list.drag_highlight();
    let (left, top, width, height) = match highlight.placement {
        None => (String::new(), String::new(), String::new(), String::new()),
        Some((top, height)) => (
            Length::Percent(0.0).css(),
            top.css(),
            Length::Percent(100.0).css(),
            height.css(),
        ),
    };
    o.insert(
        "highlight",
        JsValue::Array(vec![
            highlight.hidden.to_js(),
            left.to_js(),
            top.to_js(),
            width.to_js(),
            height.to_js(),
        ]),
    );
    let edit = list.has_edit_button();
    o.insert("buttons", vec![!edit, edit, edit].to_js());
    o.insert(
        "items",
        JsValue::Array(list.items().map(item_dump).collect()),
    );
    JsValue::Object(o)
}

#[derive(Default)]
struct Stats {
    commands: usize,
    renders: usize,
    saves: usize,
    calls: usize,
    mains: usize,
    controls: usize,
    events: usize,
    dumps: usize,
    ops: usize,
}

struct Harness {
    host: Host,
    realm: JsRealm,
    controllers: Vec<Field2dController>,
    renderers: Vec<(Field2dRenderer, Rc<RefCell<Field2dDom>>)>,
    /// `renderer.lastCommand` seen so far: `set2DCamera` re-renders only when set.
    had_last_command: Vec<bool>,
    list_uuids: HashMap<usize, String>,
    bound: HashMap<String, usize>,
    /// `main` records upstream emitted before the port call that produced them ran.
    upstream_main: VecDeque<(String, String)>,
    pending_errors: VecDeque<String>,
    stats: Stats,
}

fn reads(record: &Record) -> Result<VecDeque<(String, JsValue)>, String> {
    js(record, "reads")?
        .as_array()
        .ok_or("reads")?
        .iter()
        .map(|read| {
            let read = read.as_array().ok_or("a read is [name, value]")?;
            Ok((text(at(read, 0))?, at(read, 1).clone()))
        })
        .collect()
}

fn record_id(record: &Record) -> Result<usize, String> {
    index(&js(record, "id")?)
}

impl Harness {
    fn new() -> Harness {
        Harness {
            host: Host {
                log: RefCell::new(Log::new(true)),
                memory: JsObject::new(),
                selection: ScriptedSelection::default(),
                preferences: Some(Preferences::default()),
                assets_raw: JsValue::Null,
                assets: None,
                dark: false,
                rng: NoRandom,
                sent: VecDeque::new(),
            },
            realm: JsRealm::new(Box::new(NoRandom)),
            controllers: Vec::new(),
            renderers: Vec::new(),
            had_last_command: Vec::new(),
            list_uuids: HashMap::new(),
            bound: HashMap::new(),
            upstream_main: VecDeque::new(),
            pending_errors: VecDeque::new(),
            stats: Stats::default(),
        }
    }

    fn apply_log(&mut self, ops: &Value) -> Result<(), String> {
        let mut results = Vec::new();
        let mut log = self.host.log.borrow_mut();
        for op in ops.as_array().ok_or("slLog is a list of ops")? {
            let op = op.as_array().ok_or("a log op is an array")?;
            let op = translate_pose_op(op)?;
            log_run::apply(&mut log, &mut self.realm, &op, &mut results)?;
        }
        Ok(())
    }

    fn list_of(&self, sl: usize) -> Option<usize> {
        self.bound.get(self.list_uuids.get(&sl)?).copied()
    }

    /// `dark`/`prefs`/`assets` context fields carried on `f2d` and `f2dr` records.
    /// Returns `true` when the record carries assets that newly have loadFailures.
    fn apply_context(&mut self, record: &Record) -> Result<bool, String> {
        if let Ok(Value::Bool(dark)) = record.value("dark") {
            self.host.dark = dark;
        }
        if let Ok(prefs) = record.value("prefs") {
            self.host.preferences = match prefs {
                Value::Object(_) => {
                    let p = js(record, "prefs")?;
                    let o = p.as_object().ok_or("prefs")?;
                    let mut preferences = Preferences::default();
                    if let Some(cs) = o.get("cs") {
                        if let Some(s) = cs.as_str() {
                            preferences.coordinate_system =
                                CoordinateSystemPreference::from_js_str(s).ok_or("prefs.cs")?;
                        }
                    }
                    if let Some(ftc) = o.get("ftc").and_then(JsValue::as_bool) {
                        preferences.prefers_ftc = ftc;
                    }
                    if let Some(ts) = o.get("ts").and_then(JsValue::as_str) {
                        preferences.timestamps = if ts.to_string_lossy() == "original" {
                            Timestamps::Original
                        } else {
                            Timestamps::StartZero
                        };
                    }
                    Some(preferences)
                }
                _ => None,
            };
        }
        if let Ok(_assets) = record.value("assets") {
            let old_failed = Host::had_load_failures(&self.host.assets_raw);
            self.host.assets_raw = js(record, "assets")?;
            self.host.assets = if self.host.assets_raw.is_null() {
                None
            } else {
                Some(AdvantageScopeAssets::from_js(&self.host.assets_raw))
            };
            let new_failed = Host::had_load_failures(&self.host.assets_raw);
            return Ok(new_failed && !old_failed);
        }
        Ok(false)
    }

    /// After a port call, pair queued port messages with `main` records upstream
    /// buffered because they were emitted inside the call the port just replayed.
    fn drain_main(&mut self) -> Result<(), String> {
        while self.host.sent.front().is_some() && self.upstream_main.front().is_some() {
            let (name, expected) = self.upstream_main.pop_front().unwrap();
            let msg = self.host.sent.pop_front().unwrap();
            if msg.name.name() != name {
                return Err(format!(
                    "port sent {:?}, upstream recorded main {name}",
                    msg.name.name()
                ));
            }
            let actual = encode(&msg.data);
            if actual != expected {
                return Err(format!(
                    "main {name}\n upstream {expected}\n port     {actual}"
                ));
            }
            self.stats.mains += 1;
        }
        Ok(())
    }

    fn main_record(&mut self, record: &Record) -> Result<(), String> {
        let name = record.string("main").unwrap_or_default();
        let data = encode(&js(record, "data")?);
        if let Some(msg) = self.host.sent.front() {
            if msg.name.name() == name {
                let msg = self.host.sent.pop_front().unwrap();
                let actual = encode(&msg.data);
                if actual != data {
                    return Err(format!("main {name}\n upstream {data}\n port     {actual}"));
                }
                self.stats.mains += 1;
                return Ok(());
            }
            return Err(format!(
                "port sent {:?}, upstream recorded main {name}",
                msg.name.name()
            ));
        }
        // `show` is the boot message upstream sent before the recorder was installed.
        if name == "show" {
            return Ok(());
        }
        self.upstream_main.push_back((name, data));
        Ok(())
    }

    fn command(&mut self, record: &Record) -> Result<(), String> {
        let id = record_id(record)?;
        self.apply_context(record)?;
        self.host.selection.load(reads(record)?);
        let uuid = js(record, "uuid")?;
        let result = TabController::command(&mut self.controllers[id], &mut self.host, &uuid);
        self.host.selection.finish()?;
        self.stats.commands += 1;
        match result {
            Ok(command) => {
                same_error("getCommand", threw(record), None)?;
                let expected = encode(&js(record, "value")?);
                let actual = encode(&command.to_js());
                if expected != actual {
                    return Err(format!("command\n upstream {expected}\n port     {actual}"));
                }
            }
            Err(e) => same_error("getCommand", threw(record), Some(error_text(&e)))?,
        }
        self.drain_main()
    }

    fn controls(&mut self, record: &Record) -> Result<(), String> {
        let id = record_id(record)?;
        self.apply_context(record)?;
        let controls = &self.controllers[id].controls;
        let options = |list: &Vec<(JsString, JsString)>| {
            JsValue::Array(
                list.iter()
                    .map(|(v, l)| JsValue::Array(vec![v.to_js(), l.to_js()]))
                    .collect(),
            )
        };
        let mut actual = JsObject::new();
        actual.insert("value", controls.field.to_js());
        actual.insert("frc", options(&controls.frc));
        actual.insert("ftc", options(&controls.ftc));
        actual.insert("srcHidden", controls.source_hidden.to_js());
        actual.insert("isFtc", controls.is_ftc.to_js());
        actual.insert("size", (controls.size as f64).to_js());
        let expected = encode(&js(record, "controls")?);
        let actual = encode(&JsValue::Object(actual));
        self.stats.controls += 1;
        if expected != actual {
            return Err(format!(
                "controls\n upstream {expected}\n port     {actual}"
            ));
        }
        Ok(())
    }

    fn f2d_bracket(
        &mut self,
        record: &Record,
        inner: Vec<Record>,
        end: &Record,
    ) -> Result<(), String> {
        let id = record_id(record)?;
        let name = record.string("f2d").unwrap_or_default();
        let error_assets = self.apply_context(record)?;
        let args = js(record, "args")?;
        let args = args.as_array().ok_or("bracket args")?.to_vec();
        let result = match name.as_str() {
            "restoreState" => TabController::restore_state(
                &mut self.controllers[id],
                &mut self.host,
                at(&args, 0),
            ),
            "refresh" => TabController::refresh(&mut self.controllers[id], &mut self.host),
            "newAssets" => TabController::new_assets(&mut self.controllers[id], &mut self.host),
            other => return Err(format!("unknown controller call {other}")),
        };
        self.stats.calls += 1;
        same_error(&name, threw(end), result.err().map(|e| error_text(&e)))?;
        if name == "newAssets" && error_assets && threw(end).is_none() {
            // hub's set-assets handler alerts on new loadFailures after newAssets.
            let mut data = JsObject::new();
            data.insert("title", "Failed to load assets".to_js());
            data.insert(
                "content",
                "Some asset files failed to load. Check the developer console for details.".to_js(),
            );
            self.host
                .sent
                .push_back(NamedMessage::new(HubToMain::Error, JsValue::Object(data)));
        }
        for r in &inner {
            if r.field("sl").is_some() && r.field("op").is_some() {
                continue;
            }
            self.record(r)?;
        }
        self.drain_main()
    }

    fn handle_f2d(
        &mut self,
        record: &Record,
        kind: &str,
        records: &mut std::slice::Iter<'_, Record>,
    ) -> Result<(), String> {
        match kind {
            "create" => {
                self.apply_context(record)?;
                let uuid = text(&js(record, "uuid")?)?;
                let id = self.controllers.len();
                self.bound.insert(uuid.clone(), id);
                self.controllers
                    .push(Field2dController::with_uuid(&mut self.host, uuid));
                Ok(())
            }
            "command" => self.command(record),
            "save" => {
                let id = record_id(record)?;
                self.apply_context(record)?;
                self.stats.saves += 1;
                let expected = encode(&js(record, "value")?);
                let actual = encode(&TabController::save_state(
                    &self.controllers[id],
                    &self.host,
                ));
                if expected != actual {
                    return Err(format!(
                        "saved state\n upstream {expected}\n port     {actual}"
                    ));
                }
                self.drain_main()
            }
            "activeFields" => {
                let id = record_id(record)?;
                self.apply_context(record)?;
                let expected = encode(&js(record, "value")?);
                let fields = TabController::active_fields(&self.controllers[id], &self.host);
                let actual = encode(&JsValue::Array(
                    fields.into_iter().map(JsValue::String).collect(),
                ));
                if expected != actual {
                    return Err(format!(
                        "active fields\n upstream {expected}\n port     {actual}"
                    ));
                }
                self.drain_main()
            }
            "controls" => self.controls(record),
            "select" => {
                let id = record_id(record)?;
                self.apply_context(record)?;
                let value = js(record, "value")?
                    .as_str()
                    .cloned()
                    .ok_or("select value")?;
                self.controllers[id].select_field(&mut self.host, value);
                self.drain_main()
            }
            "source" => {
                let id = record_id(record)?;
                self.apply_context(record)?;
                self.controllers[id].open_source(&mut self.host);
                self.drain_main()
            }
            "orientation" => {
                let id = record_id(record)?;
                self.apply_context(record)?;
                self.controllers[id].click_orientation(index(&js(record, "index")?)?);
                self.drain_main()
            }
            "size" => {
                let id = record_id(record)?;
                self.apply_context(record)?;
                self.controllers[id].click_size(index(&js(record, "index")?)?);
                self.drain_main()
            }
            "restoreState" | "refresh" | "newAssets" => self.bracket(record, "f2d", records),
            other => Err(format!("unexpected f2d record {other}")),
        }
    }

    fn apply_dom(&mut self, id: usize, dom: &JsValue) -> Result<(), String> {
        let o = dom.as_object().ok_or("dom snapshot")?;
        let (_, dom_rc) = &self.renderers[id];
        let mut d = dom_rc.borrow_mut();
        d.container = [
            0.0,
            0.0,
            num(o.get("cw").ok_or("dom.cw")?)?,
            num(o.get("ch").ok_or("dom.ch")?)?,
        ];
        let scroll = o
            .get("scroll")
            .and_then(JsValue::as_array)
            .ok_or("dom.scroll")?;
        d.scroll_container = [
            num(at(scroll, 0))?,
            num(at(scroll, 1))?,
            num(at(scroll, 2))?,
            num(at(scroll, 3))?,
        ];
        d.device_pixel_ratio = num(o.get("dpr").ok_or("dom.dpr")?)?;
        d.satellite = o
            .get("satellite")
            .and_then(JsValue::as_bool)
            .unwrap_or(false);
        d.now = num(o.get("now").ok_or("dom.now")?)?;
        if let Some(images) = o.get("images").and_then(JsValue::as_object) {
            d.images.clear();
            for (src, size) in images.iter() {
                let size = size.as_array().ok_or("image size")?;
                d.images
                    .insert(src.clone(), [num(at(&size, 0))?, num(at(&size, 1))?]);
            }
        }
        Ok(())
    }

    fn render(&mut self, record: &Record, in_frame: bool) -> Result<(), String> {
        let id = record_id(record)?;
        self.apply_context(record)?;
        self.apply_dom(id, &js(record, "dom")?)?;
        if in_frame {
            // setTimeout image "load" events land between frames.
            self.renderers[id].0.flush_image_loads();
        }
        let command = Field2dCommand::from_js(&js(record, "command")?)
            .ok_or("render command does not parse")?;
        self.renderers[id].0.render_command(&self.host, &command);
        self.had_last_command[id] = true;
        self.stats.renders += 1;
        same_error("render", threw(record), None)?;
        let dom = js(record, "dom")?;
        let o = dom.as_object().ok_or("dom")?;
        let loads = num(o.get("loads").ok_or("dom.loads")?)? as u64;
        if loads != self.renderers[id].0.image_load_count() {
            return Err(format!(
                "imageLoadCount upstream {loads}, port {}",
                self.renderers[id].0.image_load_count()
            ));
        }
        Ok(())
    }

    fn f2dr_bracket(
        &mut self,
        record: &Record,
        inner: Vec<Record>,
        end: &Record,
    ) -> Result<(), String> {
        let id = record_id(record)?;
        let name = record.string("f2dr").unwrap_or_default();
        self.apply_context(record)?;
        let args = js(record, "args")?;
        let args = args.as_array().ok_or("bracket args")?.to_vec();
        let expected_renders = inner
            .iter()
            .filter(|r| r.string("f2dr").as_deref() == Some("render"))
            .count();
        let result: Result<(), JsError> = match name.as_str() {
            "restoreState" => {
                TabRenderer::restore_state(&mut self.renderers[id].0, &mut self.host, at(&args, 0))
            }
            "set2DCamera" => {
                let index = num(at(&args, 0))?;
                let should_render = (0.0..=2.0).contains(&index) && self.had_last_command[id];
                if expected_renders != usize::from(should_render) {
                    return Err(format!(
                        "set2DCamera({index}): upstream rendered {expected_renders} times inside"
                    ));
                }
                self.renderers[id].0.set_2d_camera(&mut self.host, index);
                Ok(())
            }
            other => return Err(format!("unknown renderer call {other}")),
        };
        self.stats.calls += 1;
        same_error(&name, threw(end), result.err().map(|e| error_text(&e)))?;
        for r in &inner {
            if r.string("f2dr").as_deref() == Some("render") {
                continue;
            }
            self.record(r)?;
        }
        self.drain_main()
    }

    fn handle_f2dr(
        &mut self,
        record: &Record,
        kind: &str,
        records: &mut std::slice::Iter<'_, Record>,
    ) -> Result<(), String> {
        match kind {
            "create" => {
                let dom = Rc::new(RefCell::new(Field2dDom::default()));
                self.renderers
                    .push((Field2dRenderer::new(dom.clone()), dom));
                self.had_last_command.push(false);
                Ok(())
            }
            "render" => self.render(record, true),
            "save" => {
                let id = record_id(record)?;
                self.apply_context(record)?;
                self.stats.saves += 1;
                let expected = encode(&js(record, "value")?);
                let actual = encode(&TabRenderer::save_state(&self.renderers[id].0, &self.host));
                if expected != actual {
                    return Err(format!(
                        "renderer save\n upstream {expected}\n port     {actual}"
                    ));
                }
                self.drain_main()
            }
            "aspectRatio" => {
                let id = record_id(record)?;
                self.apply_context(record)?;
                let expected = js(record, "value")?;
                let actual = TabRenderer::aspect_ratio(&self.renderers[id].0, &self.host);
                let actual = actual.map_or(JsValue::Null, |v| v.to_js());
                if encode(&expected) != encode(&actual) {
                    return Err(format!(
                        "aspectRatio upstream {expected:?}, port {actual:?}"
                    ));
                }
                Ok(())
            }
            "scroll" => {
                let id = record_id(record)?;
                let args = js(record, "args")?;
                let args = args.as_array().ok_or("scroll args")?;
                let (dx, dy, is_pan, cx, cy) = (
                    num(at(args, 0))?,
                    num(at(args, 1))?,
                    at(args, 2).as_bool().ok_or("scroll isPan")?,
                    num(at(args, 3))?,
                    num(at(args, 4))?,
                );
                let pending = self.renderers[id]
                    .0
                    .pending_scrolls
                    .first()
                    .copied()
                    .ok_or("upstream dispatched a scroll callback the port did not queue")?;
                let expected = [dx, dy, f64::from(is_pan), cx, cy];
                for (a, b) in expected.iter().zip(pending.iter()) {
                    if a.to_bits() != b.to_bits() {
                        return Err(format!(
                            "scroll callback args {pending:?}, upstream {expected:?}"
                        ));
                    }
                }
                self.renderers[id].0.pending_scrolls.remove(0);
                self.stats.events += 1;
                self.renderers[id]
                    .0
                    .scroll(&mut self.host, dx, dy, is_pan, cx, cy);
                self.drain_main()
            }
            "scrollEvent" => {
                let id = record_id(record)?;
                self.stats.events += 1;
                self.renderers[id].0.scroll_event(
                    num(&js(record, "scrollLeft")?)?,
                    num(&js(record, "scrollTop")?)?,
                    num(&js(record, "now")?)?,
                );
                Ok(())
            }
            "mouse" => {
                let id = record_id(record)?;
                self.stats.events += 1;
                let kind = record
                    .string("type")
                    .unwrap_or_default()
                    .strip_prefix("mouse")
                    .unwrap_or_default()
                    .to_owned();
                self.renderers[id].0.mouse(
                    &mut self.host,
                    &kind,
                    num(&js(record, "button")?)?,
                    js(record, "shiftKey")?.as_bool().unwrap_or(false),
                    num(&js(record, "x")?)?,
                    num(&js(record, "y")?)?,
                );
                self.drain_main()
            }
            "restoreState" | "set2DCamera" => self.bracket(record, "f2dr", records),
            other => Err(format!("unexpected f2dr record {other}")),
        }
    }

    /// Consumes the records between a begin marker and its matching end marker.
    fn bracket(
        &mut self,
        record: &Record,
        family: &str,
        records: &mut std::slice::Iter<'_, Record>,
    ) -> Result<(), String> {
        let name = record.string(family).unwrap_or_default();
        let id = record.field("id").cloned();
        let mut inner = Vec::new();
        let end = loop {
            let next = records
                .next()
                .ok_or_else(|| format!("{name} never ended"))?;
            if next.string(family).as_deref() == Some(name.as_str())
                && next.string("phase").as_deref() == Some("end")
                && next.field("id").cloned() == id
            {
                break next;
            }
            inner.push(next.clone());
        };
        match family {
            "f2d" => self.f2d_bracket(record, inner, end),
            _ => self.f2dr_bracket(record, inner, end),
        }
    }

    fn frame(
        &mut self,
        record: &Record,
        ctrl: usize,
        args: &[JsValue],
    ) -> Result<Option<String>, String> {
        self.host.dark = args
            .get(1)
            .and_then(JsValue::as_bool)
            .ok_or("frame isDark")?;
        let time = match at(args, 2) {
            JsValue::Null => None,
            v => Some(num(v)?),
        };
        if let Some(previews) = record.field("numberPreviews") {
            let previews = js_json::to_js(&Value::from_json(previews).map_err(|e| e.0)?)?;
            if !previews.as_array().ok_or("numberPreviews")?.is_empty() {
                return Err("field2d lists have no number previews".to_owned());
            }
        }
        let controller = &mut self.controllers[ctrl];
        let host = &mut self.host;
        let mut env = Env {
            log: &host.log.borrow(),
            type_memory: &mut host.memory,
            dark: host.dark,
        };
        let result = controller.source_list_mut().frame(&mut env, time, None);
        Ok(result.err().map(|e| e.to_string()))
    }

    fn op(&mut self, record: &Record, ctrl: usize) -> Result<(), String> {
        self.stats.ops += 1;
        let op = js(record, "op")?;
        let args = op.as_array().ok_or("an op is an array")?.to_vec();
        let name = text(at(&args, 0))?;
        if let Ok(Value::Bool(dark)) = record.value("dark") {
            self.host.dark = dark;
        }
        if name == "frame" {
            let port = self.frame(record, ctrl, &args)?;
            return same_error(&name, threw(record), port);
        }
        let controller = &mut self.controllers[ctrl];
        let host = &mut self.host;
        let env = |host: &mut Host,
                   f: &mut dyn FnMut(
            &mut Env<'_>,
        )
            -> Result<(), asrs_model::source_list::SourceListError>| {
            let mut env = Env {
                log: &host.log.borrow(),
                type_memory: &mut host.memory,
                dark: host.dark,
            };
            f(&mut env)
        };
        let port: Option<String> = match name.as_str() {
            "setTitle" => {
                let title = text(at(&args, 1))?;
                controller.source_list_mut().set_title(&title);
                None
            }
            "setState" => {
                let items = at(&args, 1)
                    .as_array()
                    .ok_or("setState items")?
                    .iter()
                    .map(item_state)
                    .collect::<Result<Vec<_>, _>>()?;
                let mut items = Some(items);
                env(host, &mut |e| {
                    controller
                        .source_list_mut()
                        .set_state(e, items.take().unwrap_or_default())
                })
                .err()
                .map(|e| e.to_string())
            }
            "clear" => {
                controller.source_list_mut().clear();
                None
            }
            "refresh" => env(host, &mut |e| controller.source_list_mut().refresh(e))
                .err()
                .map(|e| e.to_string()),
            "stop" => {
                controller.source_list_mut().stop();
                None
            }
            "addField" => {
                let key = at(&args, 1).as_str().cloned().ok_or("addField key")?;
                let parent = match at(&args, 2) {
                    JsValue::Undefined => None,
                    v => Some(index(v)?),
                };
                env(host, &mut |e| {
                    controller.source_list_mut().add_field(e, &[], &key, parent)
                })
                .err()
                .map(|e| e.to_string())
            }
            "toggleHidden" => {
                let i = index(at(&args, 1))?;
                env(host, &mut |e| {
                    controller.source_list_mut().toggle_hidden(e, i)
                })
                .err()
                .map(|e| e.to_string())
            }
            "nameClick" => {
                let (i, now) = (index(at(&args, 1))?, num(at(&args, 2))?);
                env(host, &mut |e| {
                    controller.source_list_mut().name_click(e, i, now)
                })
                .err()
                .map(|e| e.to_string())
            }
            "remove" => {
                let i = index(at(&args, 1))?;
                controller.source_list_mut().remove(i);
                None
            }
            "promptType" | "contextMenu" | "typeResponse" | "clearClick" | "clearResponse"
            | "mouseDown" | "mouseUp" | "mouseMove" | "itemDrag" | "fieldDrag"
            | "setOptionValues" => {
                return Err(format!(
                    "the fixture reached list op {name}, which this harness does not replay"
                ));
            }
            other => return Err(format!("unsupported list op {other:?}")),
        };
        let hooked = matches!(
            name.as_str(),
            "setTitle" | "setState" | "clear" | "refresh" | "stop" | "addField" | "frame"
        );
        if hooked || record.field("threw").is_some() {
            same_error(&name, threw(record), port)?;
        } else if let Some(error) = port {
            self.pending_errors.push_back(error);
        }
        Ok(())
    }

    fn record(&mut self, record: &Record) -> Result<(), String> {
        let mut empty = [].iter();
        self.record_in(record, &mut empty)
    }

    fn record_in(
        &mut self,
        record: &Record,
        records: &mut std::slice::Iter<'_, Record>,
    ) -> Result<(), String> {
        if let Some(kind) = record.string("f2d") {
            return self.handle_f2d(record, &kind, records);
        }
        if let Some(kind) = record.string("f2dr") {
            return self.handle_f2dr(record, &kind, records);
        }
        if let Some(ops) = record.field("slLog") {
            let ops = Value::from_json(ops).map_err(|e| e.0)?;
            return self.apply_log(&ops);
        }
        if record.field("slTypeMemory").is_some() {
            let value = js(record, "slTypeMemory")?;
            self.host.memory = SourceListTypeMemoryEntry::memory_from_js(&value)
                .ok_or("slTypeMemory does not parse")?;
            return Ok(());
        }
        if record.string("main").is_some() {
            return self.main_record(record);
        }
        if record.string("exception").as_deref() == Some("event") {
            let error = record.string("error").unwrap_or_default();
            return match self.pending_errors.pop_front() {
                Some(port) if port == error => Ok(()),
                port => Err(format!(
                    "event exception: upstream {error:?}, port {port:?}"
                )),
            };
        }
        if record.field("capture").is_some() {
            self.drain_main()?;
            if !self.host.sent.is_empty() {
                return Err(format!(
                    "port sent main messages upstream never recorded: {:?}",
                    self.host.sent
                ));
            }
            if !self.upstream_main.is_empty() {
                return Err(format!(
                    "upstream emitted main records the port did not: {:?}",
                    self.upstream_main
                ));
            }
            if !self.pending_errors.is_empty() {
                return Err(format!(
                    "port errors upstream never threw: {:?}",
                    self.pending_errors
                ));
            }
        }
        let Some(sl) = record.field("sl") else {
            return Ok(());
        };
        let sl = index(&js_json::to_js(&Value::from_json(sl).map_err(|e| e.0)?)?)?;
        if record.field("create").is_some() {
            let create = js(record, "create")?;
            let uuid = text(
                create
                    .as_object()
                    .and_then(|o| o.get("uuid"))
                    .ok_or("create uuid")?,
            )?;
            self.list_uuids.insert(sl, uuid);
            return Ok(());
        }
        let Some(ctrl) = self.list_of(sl) else {
            return Ok(());
        };
        if record.field("op").is_some() {
            return self.op(record, ctrl);
        }
        if record.field("dump").is_some() {
            self.stats.dumps += 1;
            let mut expected = js(record, "dump")?;
            expected.as_object_mut().and_then(|o| o.remove("config"));
            let expected = encode(&expected);
            let actual = encode(&list_dump(
                self.controllers[ctrl].source_list(),
                &self.host.log.borrow(),
            ));
            if expected != actual {
                return Err(format!(
                    "list dump\n upstream {expected}\n port     {actual}"
                ));
            }
        }
        Ok(())
    }
}

fn case_ids() -> Vec<String> {
    let dir = vectors_dir().join(CASES);
    let mut ids: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            Some(format!("{CASES}/{}", name.split('.').next()?))
        })
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

fn run_case(id: &str) -> Result<Stats, String> {
    let records = load(id)?;
    let mut harness = Harness::new();
    let mut iter = records.iter();
    while let Some(record) = iter.next() {
        harness
            .record_in(record, &mut iter)
            .map_err(|e| format!("{id} line {}: {e}", record.line))?;
    }
    harness.drain_main().map_err(|e| format!("{id}: {e}"))?;
    if !harness.host.sent.is_empty() {
        return Err(format!(
            "{id}: port sent main messages upstream never recorded"
        ));
    }
    if !harness.upstream_main.is_empty() {
        return Err(format!(
            "{id}: upstream emitted main records the port did not"
        ));
    }
    if harness.controllers.is_empty() {
        return Err(format!("{id}: no field2d controller was recorded"));
    }
    Ok(harness.stats)
}

#[test]
fn field_2d_fixtures() {
    let ids = case_ids();
    assert!(!ids.is_empty(), "no {CASES} vectors");
    let mut failures = Vec::new();
    let mut total = Stats::default();
    for id in &ids {
        match run_case(id) {
            Ok(s) => {
                eprintln!(
                    "{id}: {} commands, {} renders, {} saves, {} calls, {} mains, {} controls, {} events, {} dumps, {} list ops",
                    s.commands,
                    s.renders,
                    s.saves,
                    s.calls,
                    s.mains,
                    s.controls,
                    s.events,
                    s.dumps,
                    s.ops
                );
                total.commands += s.commands;
                total.renders += s.renders;
                total.saves += s.saves;
                total.calls += s.calls;
                total.mains += s.mains;
                total.controls += s.controls;
                total.events += s.events;
                total.dumps += s.dumps;
                total.ops += s.ops;
            }
            Err(e) => failures.push(e),
        }
    }
    eprintln!(
        "{} fixtures: {} commands, {} renders, {} saves, {} calls, {} mains, {} controls, {} events, {} dumps, {} list ops, {} failing",
        ids.len(),
        total.commands,
        total.renders,
        total.saves,
        total.calls,
        total.mains,
        total.controls,
        total.events,
        total.dumps,
        total.ops,
        failures.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
