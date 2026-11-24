#![allow(clippy::disallowed_methods)]

//! Replays the `dom/field-2d/<case>` vectors (fixtures written by
//! `oracle/tools/field2dSpecs.mjs`) against `asrs_tabs::field_2d`. `slLog` records
//! build the log, `sl` ops drive the controller's `SourceList`, `f2d` records carry
//! each controller call with the selection reads upstream made during it, and
//! `f2dr` records carry renderer renders (with the camera fields after each), camera
//! calls, wheel/mouse events and the DOM values upstream read. Commands, controls, saved
//! state, camera state, main messages, list dumps, type memory, scroll callbacks and
//! exceptions are all compared bit-exact, and any record the harness does not know fails.

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
#[path = "../../asrs-cli/src/js_ops.rs"]
#[allow(dead_code)]
mod js_ops;
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
use asrs_model::source_list::Undisplayed;
use asrs_model::source_list::{Env, ItemView, Length, SourceList};
use asrs_model::source_list_config::SourceListItemState;
use asrs_model::tab_controller::{TabController, TabRenderer};
use asrs_parity::Value;
use asrs_parity::vectors::{Record, load, vectors_dir};
use asrs_tabs::cx::TabCx;
use asrs_tabs::field_2d::{
    Field2dCommand, Field2dController, Field2dDom, Field2dRenderer, ScrollArgs,
};
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
        match &self.next(name) {
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
    memory: JsValue,
    selection: ScriptedSelection,
    preferences: Option<Preferences>,
    assets: Option<AdvantageScopeAssets>,
    dark: bool,
    rng: NoRandom,
    sent: VecDeque<NamedMessage<HubToMain>>,
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
    fn tab_rng(&mut self) -> &mut dyn Rng {
        &mut self.rng
    }
    fn tab_assets(&self) -> Option<&AdvantageScopeAssets> {
        self.assets.as_ref()
    }
    fn send_main_message(&mut self, message: NamedMessage<HubToMain>) {
        self.sent.push_back(message);
    }

    fn send_open_xr(&mut self, _uuid: JsValue) {
        unreachable!("the field2d harness has no Field3d tab")
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
            dom: Undisplayed::dom(),
        }
    }
}

fn js(record: &Record, key: &str) -> Result<JsValue, String> {
    js_json::to_js(&record.value(key).map_err(|e| e.0)?)
}

fn at(args: &[JsValue], i: usize) -> &JsValue {
    static UNDEFINED: JsValue = JsValue::Undefined;
    args.get(i).unwrap_or(&UNDEFINED)
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
    o.insert(
        "displayed",
        list.displayed_state(log).expect("modeled items").to_js(),
    );
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
    cameras: usize,
    frames: usize,
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
    /// Scroll callbacks the port dispatched, per renderer, awaiting upstream's `scroll`.
    scrolls: Vec<VecDeque<ScrollArgs>>,
    /// Animation frames the port's scroll callbacks requested before this frame's tab loop;
    /// upstream must render once in each before the next tab loop runs.
    frame_requests: Vec<u32>,
    /// The tab loop asked this renderer for a command, so its next render is the tab's own.
    tab_render_due: Vec<bool>,
    /// What the last replayed controller or renderer call threw, for an `eval` exception.
    last_threw: Option<String>,
    /// What the port's render or list frame threw inside an animation frame, for upstream's
    /// `requestAnimationFrame` exception.
    frame_threw: Option<String>,
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
                memory: JsValue::Object(JsObject::new()),
                selection: ScriptedSelection::default(),
                preferences: None,
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
            scrolls: Vec::new(),
            frame_requests: Vec::new(),
            tab_render_due: Vec::new(),
            last_threw: None,
            frame_threw: None,
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
    fn apply_context(&mut self, record: &Record) -> Result<(), String> {
        if let Ok(Value::Bool(dark)) = record.value("dark") {
            self.host.dark = dark;
        }
        if let Ok(prefs) = record.value("prefs") {
            self.host.preferences = match prefs {
                Value::Object(_) => {
                    let p = js(record, "prefs")?;
                    let o = p.as_object().ok_or("prefs")?;
                    let mut preferences = self.host.preferences.clone().unwrap_or_default();
                    if let Some(ftc) = o.get("ftc").and_then(JsValue::as_bool)
                        && self.host.preferences.is_some()
                        && preferences.prefers_ftc != ftc
                    {
                        return Err(format!(
                            "window.preferences.prefersFTC upstream {ftc}, port {}",
                            preferences.prefers_ftc
                        ));
                    }
                    if let Some(cs) = o.get("cs")
                        && let Some(s) = cs.as_str()
                    {
                        preferences.coordinate_system =
                            CoordinateSystemPreference::from_js_str(s).ok_or("prefs.cs")?;
                    }
                    if self.host.preferences.is_none()
                        && let Some(ftc) = o.get("ftc").and_then(JsValue::as_bool)
                    {
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
        if record.value("assets").is_ok() {
            let raw = js(record, "assets")?;
            self.host.assets = (!raw.is_null()).then(|| AdvantageScopeAssets::from_js(&raw));
        }
        Ok(())
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
        // The tab loop runs once per animation frame, after the timers that deliver the
        // `IMAGE` load events queued by the previous frame's renders.
        if let Some((renderer, _)) = self.renderers.get_mut(id) {
            renderer.flush_image_loads();
            if self.frame_requests[id] != 0 {
                return Err(format!(
                    "the port requested {} animation frames upstream did not render",
                    self.frame_requests[id]
                ));
            }
            self.frame_requests[id] = renderer.take_frame_requests();
            self.tab_render_due[id] = true;
        }
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
        self.apply_context(record)?;
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
        self.last_threw = threw(end);
        same_error(&name, threw(end), result.err().map(|e| error_text(&e)))?;
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
        if let Some(images) = o.get("images").and_then(JsValue::as_object) {
            d.images.clear();
            for (src, size) in images.iter() {
                let size = size.as_array().ok_or("image size")?;
                d.images
                    .insert(src.clone(), [num(at(size, 0))?, num(at(size, 1))?]);
            }
        }
        Ok(())
    }

    fn render(&mut self, record: &Record) -> Result<(), String> {
        let id = record_id(record)?;
        if !std::mem::take(&mut self.tab_render_due[id]) {
            if self.frame_requests[id] == 0 {
                return Err(
                    "upstream rendered in an animation frame the port did not request".to_owned(),
                );
            }
            self.frame_requests[id] -= 1;
        }
        self.apply_context(record)?;
        self.apply_dom(id, &js(record, "dom")?)?;
        let recorded = js(record, "command")?;
        let command = Field2dCommand::from_js(&recorded).ok_or("render command does not parse")?;
        if encode(&command.to_js()) != encode(&recorded) {
            return Err(format!(
                "render command does not round-trip\n recorded {}\n parsed   {}",
                encode(&recorded),
                encode(&command.to_js())
            ));
        }
        let result = self.renderers[id].0.render_command(&self.host, &command);
        self.had_last_command[id] = true;
        self.stats.renders += 1;
        if let Err(e) = &result {
            self.frame_threw = Some(error_text(e));
        }
        self.frame_matches(id, record, result.err())?;
        self.camera(id, record)?;
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

    /// A `render` record's throw, and the frame it painted if it reset the canvas: pixel size,
    /// the `CANVAS.style` strings, the call trace and the heatmap store behind `@heatmap`.
    fn frame_matches(
        &mut self,
        id: usize,
        record: &Record,
        error: Option<JsError>,
    ) -> Result<(), String> {
        same_error("render", threw(record), error.as_ref().map(error_text))?;
        let mut dom = self.renderers[id].1.borrow_mut();
        match (record.field("calls"), dom.painted.take()) {
            (None, None) => Ok(()),
            (Some(_), None) => Err("upstream painted a frame, the port kept the last".to_owned()),
            (None, Some(_)) => Err("the port painted a frame, upstream kept the last".to_owned()),
            (Some(_), Some(canvas)) => {
                self.stats.frames += 1;
                let size = encode(&JsValue::Array(vec![
                    (canvas.width as f64).to_js(),
                    (canvas.height as f64).to_js(),
                ]));
                let expected_size = encode(&js(record, "size")?);
                if size != expected_size {
                    return Err(format!(
                        "canvas size: upstream {expected_size}, port {size}"
                    ));
                }
                let style = dom
                    .canvas_style
                    .ok_or("the port painted without setting the canvas style")?;
                let style = encode(&JsValue::Array(
                    style.css().iter().map(|s| s.as_str().to_js()).collect(),
                ));
                let expected_style = encode(&js(record, "style")?);
                if style != expected_style {
                    return Err(format!(
                        "canvas style\n upstream {expected_style}\n port     {style}"
                    ));
                }
                compare_calls(&record.value("calls").map_err(|e| e.0)?, &canvas)?;
                drop(dom);
                let store = heatmap_store(self.renderers[id].0.heatmap().get_data());
                let expected_store = encode(&js(record, "heatmap")?);
                if encode(&store) != expected_store {
                    return Err(format!(
                        "heatmap getData()\n upstream {expected_store}\n port     {}",
                        encode(&store)
                    ));
                }
                Ok(())
            }
        }
    }

    fn camera(&self, id: usize, record: &Record) -> Result<(), String> {
        let expected = encode(&js(record, "camera")?);
        let actual = encode(&self.renderers[id].0.camera_state());
        if expected != actual {
            return Err(format!(
                "camera state\n upstream {expected}\n port     {actual}"
            ));
        }
        Ok(())
    }

    fn scroll_args(record: &Record) -> Result<ScrollArgs, String> {
        let args = js(record, "args")?;
        let args = args.as_array().ok_or("scroll args")?;
        Ok(ScrollArgs {
            dx: num(at(args, 0))?,
            dy: num(at(args, 1))?,
            is_pan: at(args, 2).as_bool().ok_or("scroll isPan")?,
            cursor: [num(at(args, 3))?, num(at(args, 4))?],
        })
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
        let inner_renders: Vec<&Record> = inner
            .iter()
            .filter(|r| r.string("f2dr").as_deref() == Some("render"))
            .collect();
        let expected_renders = inner_renders.len();
        if let Some(render) = inner_renders.first() {
            self.apply_dom(id, &js(render, "dom")?)?;
        }
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
                let result = self.renderers[id].0.set_2d_camera(&self.host, index);
                if let Some(render) = inner_renders.first() {
                    self.stats.renders += 1;
                    self.frame_matches(id, render, result.clone().err())?;
                    self.camera(id, render)?;
                }
                result
            }
            other => return Err(format!("unknown renderer call {other}")),
        };
        self.stats.calls += 1;
        self.last_threw = threw(end);
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
                self.scrolls.push(VecDeque::new());
                self.frame_requests.push(0);
                self.tab_render_due.push(false);
                Ok(())
            }
            "render" => self.render(record),
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
                let expected = Self::scroll_args(record)?;
                let port = self.scrolls[id]
                    .pop_front()
                    .ok_or("upstream dispatched a scroll callback the port did not")?;
                if port.is_pan != expected.is_pan
                    || port.dx.to_bits() != expected.dx.to_bits()
                    || port.dy.to_bits() != expected.dy.to_bits()
                    || port.cursor.map(f64::to_bits) != expected.cursor.map(f64::to_bits)
                {
                    return Err(format!(
                        "scroll callback upstream {expected:?}, port {port:?}"
                    ));
                }
                self.stats.events += 1;
                self.drain_main()
            }
            "scrollEvent" => {
                // The DOM scroll field is U08's to replace: a `scroll` event is a wheel
                // input only when the sensor turned it into a callback, whose deltas the
                // host's wheel mapping supplies. The cursor is the port's own.
                let id = record_id(record)?;
                self.stats.events += 1;
                let next = records.clone().next();
                if let Some(next) = next
                    && next.string("f2dr").as_deref() == Some("scroll")
                    && record_id(next)? == id
                {
                    let upstream = Self::scroll_args(next)?;
                    if upstream.is_pan {
                        return Err("a scroll event dispatched a pan callback".to_owned());
                    }
                    let args = self.renderers[id]
                        .0
                        .wheel(&self.host, upstream.dx, upstream.dy);
                    self.scrolls[id].push_back(args);
                }
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
                // A drag's callback is the constructor-captured one, which upstream cannot
                // record; the fixtures' `camera` snapshots check its effect instead.
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
            "camera" => {
                let id = record_id(record)?;
                self.stats.cameras += 1;
                self.camera(id, record)
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
            dom: Undisplayed::dom(),
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
            same_error(&name, threw(record), port.clone())?;
            if port.is_some() {
                self.frame_threw = port;
            }
            return Ok(());
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
                dom: Undisplayed::dom(),
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
                let removed = controller.source_list_mut().remove(i);
                removed.err().map(|e| e.to_string())
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
            same_error(&name, threw(record), port.clone())?;
            if port.is_some() {
                self.last_threw = port;
            }
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
            self.host.memory = js(record, "slTypeMemory")?;
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
        if record.string("exception").as_deref() == Some("requestAnimationFrame") {
            let error = record.string("error").unwrap_or_default();
            return match self.frame_threw.take() {
                Some(port) if port == error => Ok(()),
                port => Err(format!(
                    "animation frame exception: upstream {error:?}, port {port:?}"
                )),
            };
        }
        if record.string("exception").as_deref() == Some("eval") {
            let error = record.string("error").unwrap_or_default();
            return match self.last_threw.take() {
                Some(port) if port == error => Ok(()),
                port => Err(format!("eval exception: upstream {error:?}, port {port:?}")),
            };
        }
        if record.field("slMemory").is_some() {
            let expected = encode(&js(record, "slMemory")?);
            let actual = encode(&self.host.memory);
            if expected != actual {
                return Err(format!(
                    "type memory\n upstream {expected}\n port     {actual}"
                ));
            }
            return Ok(());
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
            return Ok(());
        }
        let Some(sl) = record.field("sl") else {
            return Err("a record the harness does not know".to_owned());
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
        // Lists of the hub's other tabs.
        let Some(ctrl) = self.list_of(sl) else {
            return Ok(());
        };
        if record.field("op").is_some() {
            return self.op(record, ctrl);
        }
        if record.field("dump").is_none() {
            return Err("an sl record that is neither create, op nor dump".to_owned());
        }
        {
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

fn heatmap_store(store: Option<asrs_canvas::heatmap::StoreData>) -> JsValue {
    let Some(store) = store else {
        return JsValue::Null;
    };
    let mut o = JsObject::new();
    o.insert("min", store.min.to_js());
    o.insert("max", store.max.to_js());
    o.insert(
        "data",
        JsValue::Array(
            store
                .data
                .iter()
                .map(|p| {
                    let mut point = JsObject::new();
                    point.insert("x", p.x.as_str().to_js());
                    point.insert("y", p.y.as_str().to_js());
                    point.insert("radius", p.radius.to_js());
                    point.insert("value", p.value.to_js());
                    JsValue::Object(point)
                })
                .collect(),
        ),
    );
    JsValue::Object(o)
}

fn compare_calls(expected: &Value, canvas: &asrs_canvas::Canvas2d) -> Result<(), String> {
    let actual = asrs_canvas::trace::encode(&canvas.calls)
        .lines()
        .map(|line| {
            asrs_parity::json::parse(line)
                .map_err(|e| format!("port trace line {line}: {e:?}"))
                .and_then(|json| Value::from_json(&json).map_err(|e| e.0))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let Value::Array(expected) = expected else {
        return Err("calls is not a list".to_owned());
    };
    for (i, (e, a)) in expected.iter().zip(actual.iter()).enumerate() {
        if !asrs_parity::vectors::values_match(e, a) {
            return Err(format!(
                "canvas call {i}:\n upstream {}\n port     {}",
                e.encode(),
                a.encode()
            ));
        }
    }
    if expected.len() != actual.len() {
        return Err(format!(
            "upstream made {} canvas calls, the port {}",
            expected.len(),
            actual.len()
        ));
    }
    Ok(())
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
    if let Some(e) = &harness.frame_threw {
        return Err(format!(
            "{id}: the port's render threw {e:?}, upstream's did not"
        ));
    }
    if harness.scrolls.iter().any(|q| !q.is_empty()) {
        return Err(format!(
            "{id}: port wheel callbacks upstream never dispatched"
        ));
    }
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
    let recorded_frames = records
        .iter()
        .filter(|r| r.string("f2dr").as_deref() == Some("render") && r.field("calls").is_some())
        .count();
    if harness.stats.frames != recorded_frames {
        return Err(format!(
            "{id}: compared {} painted frames, upstream recorded {recorded_frames}",
            harness.stats.frames
        ));
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
                    "{id}: {} commands, {} renders, {} painted frames, {} saves, {} calls, {} mains, {} controls, {} events, {} dumps, {} list ops, {} cameras",
                    s.commands,
                    s.renders,
                    s.frames,
                    s.saves,
                    s.calls,
                    s.mains,
                    s.controls,
                    s.events,
                    s.dumps,
                    s.ops,
                    s.cameras
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
                total.cameras += s.cameras;
                total.frames += s.frames;
            }
            Err(e) => failures.push(e),
        }
    }
    eprintln!(
        "{} fixtures: {} commands, {} renders, {} painted frames, {} saves, {} calls, {} mains, {} controls, {} events, {} dumps, {} list ops, {} cameras, {} failing",
        ids.len(),
        total.commands,
        total.renders,
        total.frames,
        total.saves,
        total.calls,
        total.mains,
        total.controls,
        total.events,
        total.dumps,
        total.ops,
        total.cameras,
        failures.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
