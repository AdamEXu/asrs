#![allow(clippy::disallowed_methods)]

//! Replays the `dom/line-graph/<case>` vectors (H07 fixtures written by
//! `oracle/tools/lineGraphSpecs.mjs`) against `asrs_tabs::line_graph::LineGraphController`.
//! The records drive the port: `slLog` builds the log, `sl` ops reach the controller's lists,
//! `lg` records carry each controller call with the selection values upstream read during it.
//! Every command, preview, saved state, active field list, `ask-edit-axis` message, list dump
//! and exception is compared with upstream's.

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

use asrs_js::string::{JsError, JsString};
use asrs_js::time::Rng;
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::log::Log;
use asrs_log::realm::JsRealm;
use asrs_model::assets::AdvantageScopeAssets;
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::preferences::{Preferences, Timestamps};
use asrs_model::selection::{Selection, SelectionMode, TimeSupplier};
use asrs_model::source_list::{Env, ItemView, Length, Rect, SourceList};
use asrs_model::source_list_config::{
    SourceListItemState, SourceListTypeMemory, SourceListTypeMemoryEntry,
};
use asrs_model::tab_controller::TabController;
use asrs_parity::Value;
use asrs_parity::vectors::{Record, load, vectors_dir};
use asrs_tabs::cx::TabCx;
use asrs_tabs::line_graph::{LineGraphController, ListId};
use asrs_tabs::list_cx::{ListParts, SourceListCx};

const CASES: &str = "dom/line-graph";

struct NoRandom;

impl Rng for NoRandom {
    fn next_f64(&mut self) -> f64 {
        panic!("the line graph harness draws no Math.random values")
    }
}

/// Answers each selection read with the value upstream's call returned, in order, and notes
/// the first read that differs from upstream's sequence.
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
        self.unexpected("getCurrentLiveTime");
        None
    }
    fn get_render_time(&mut self) -> Option<f64> {
        self.unexpected("getRenderTime");
        None
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
        self.unexpected("getTimelineIsMaxZoom");
        false
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
    preferences: Preferences,
    dark: bool,
    rng: NoRandom,
    sent: Vec<NamedMessage<HubToMain>>,
}

impl TabCx for Host {
    fn tab_log(&self) -> Ref<'_, Log> {
        self.log.borrow()
    }
    fn tab_selection(&mut self) -> &mut dyn Selection {
        &mut self.selection
    }
    fn tab_preferences(&self) -> Option<&Preferences> {
        Some(&self.preferences)
    }

    fn tab_preferences_mut(&mut self) -> Option<&mut Preferences> {
        Some(&mut self.preferences)
    }

    fn tab_rng(&mut self) -> &mut dyn Rng {
        &mut self.rng
    }
    fn tab_assets(&self) -> Option<&AdvantageScopeAssets> {
        None
    }
    fn send_main_message(&mut self, message: NamedMessage<HubToMain>) {
        self.sent.push(message);
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
    static UNDEFINED: JsValue = JsValue::Undefined;
    args.get(i).unwrap_or(&UNDEFINED)
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

struct Preview {
    key: JsString,
    time: f64,
    reads: VecDeque<(String, JsValue)>,
    result: Result<JsValue, String>,
}

#[derive(Default)]
struct Stats {
    commands: usize,
    previews: usize,
    saves: usize,
    calls: usize,
    prompts: usize,
    dumps: usize,
    ops: usize,
}

struct Harness {
    host: Host,
    realm: JsRealm,
    controllers: Vec<LineGraphController>,
    previews: Vec<VecDeque<Preview>>,
    list_uuids: HashMap<usize, String>,
    bound: HashMap<String, (usize, ListId)>,
    pending_prompt: Option<JsValue>,
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

fn lg_id(record: &Record) -> Result<usize, String> {
    index(&js(record, "id")?)
}

impl Harness {
    fn new() -> Harness {
        Harness {
            host: Host {
                log: RefCell::new(Log::new(true)),
                memory: JsObject::new(),
                selection: ScriptedSelection::default(),
                preferences: Preferences::default(),
                dark: false,
                rng: NoRandom,
                sent: Vec::new(),
            },
            realm: JsRealm::new(Box::new(NoRandom)),
            controllers: Vec::new(),
            previews: Vec::new(),
            list_uuids: HashMap::new(),
            bound: HashMap::new(),
            pending_prompt: None,
            pending_errors: VecDeque::new(),
            stats: Stats::default(),
        }
    }

    fn apply_log(&mut self, ops: &Value) -> Result<(), String> {
        let mut results = Vec::new();
        let mut log = self.host.log.borrow_mut();
        for op in ops.as_array().ok_or("slLog is a list of ops")? {
            let op = op.as_array().ok_or("a log op is an array")?;
            log_run::apply(&mut log, &mut self.realm, op, &mut results)?;
        }
        Ok(())
    }

    fn list_of(&self, sl: usize) -> Option<(usize, ListId)> {
        self.bound.get(self.list_uuids.get(&sl)?).copied()
    }

    fn set_context(&mut self, record: &Record) -> Result<(), String> {
        if let Ok(Value::Bool(dark)) = record.value("dark") {
            self.host.dark = dark;
        }
        if let Ok(ts) = record.value("ts") {
            self.host.preferences.timestamps = match ts.as_str() {
                Some(s) if s.eq_str("original") => Timestamps::Original,
                _ => Timestamps::StartZero,
            };
        }
        Ok(())
    }

    /// A `getCommand` call: runs the port with upstream's selection reads and compares.
    fn command(&mut self, record: &Record) -> Result<(), String> {
        let id = lg_id(record)?;
        self.set_context(record)?;
        self.host.selection.load(reads(record)?);
        let result = self.controllers[id].get_command(&mut self.host);
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
                Ok(())
            }
            Err(e) => same_error("getCommand", threw(record), Some(error_text(&e))),
        }
    }

    fn bracket(&mut self, record: &Record, inner: Vec<Record>, end: &Record) -> Result<(), String> {
        let id = lg_id(record)?;
        let name = record.string("lg").unwrap_or_default();
        self.set_context(record)?;
        let args = js(record, "args")?;
        let args = args.as_array().ok_or("bracket args")?.to_vec();
        let mut nested: VecDeque<&Record> = inner
            .iter()
            .filter(|r| r.string("lg").as_deref() == Some("command"))
            .collect();
        let mut rest = Vec::new();
        for r in &inner {
            if r.field("sl").is_some() && r.field("op").is_some() {
                continue;
            }
            if r.string("lg").as_deref() == Some("command") {
                continue;
            }
            rest.push(r);
        }
        let result = match name.as_str() {
            "restoreState" => TabController::restore_state(
                &mut self.controllers[id],
                &mut self.host,
                at(&args, 0),
            ),
            "refresh" => TabController::refresh(&mut self.controllers[id], &mut self.host),
            "clearAxis" => {
                self.controllers[id].clear_axis(at(&args, 0));
                Ok(())
            }
            "setRobotModeVisible" => {
                self.controllers[id].set_robot_mode_visible(at(&args, 0).clone());
                Ok(())
            }
            "editAxis" => {
                if let Some(command) = nested.pop_front() {
                    // getCommand is idempotent on the controller's state, so running it here
                    // first lets its result be compared before edit_axis repeats it.
                    self.command(command)?;
                    self.set_context(command)?;
                    self.host.selection.load(reads(command)?);
                }
                let result = self.controllers[id].edit_axis(
                    &mut self.host,
                    at(&args, 0),
                    at(&args, 1),
                    at(&args, 2),
                    at(&args, 3),
                );
                self.host.selection.finish()?;
                result
            }
            other => return Err(format!("unknown controller call {other}")),
        };
        if !nested.is_empty() {
            return Err(format!(
                "{name}: upstream ran getCommand inside it, the port did not"
            ));
        }
        self.stats.calls += 1;
        same_error(&name, threw(end), result.err().map(|e| error_text(&e)))?;
        for r in rest {
            self.record(r)?;
        }
        Ok(())
    }

    fn frame(
        &mut self,
        record: &Record,
        ctrl: usize,
        list: ListId,
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
        let mut expected: VecDeque<(JsString, f64, Result<JsValue, String>)> = VecDeque::new();
        if record.field("numberPreviews").is_some() {
            for call in js(record, "numberPreviews")?
                .as_array()
                .ok_or("numberPreviews")?
            {
                let call = call.as_array().ok_or("a number preview call")?;
                expected.push_back((
                    at(call, 0).as_str().cloned().ok_or("preview key")?,
                    num(at(call, 1))?,
                    Ok(at(call, 2).clone()),
                ));
            }
        }
        let previews: Vec<Preview> = self.previews[ctrl].drain(..).collect();
        if let Some(last) = previews.last()
            && previews.len() == expected.len() + 1
            && last.result.is_err()
        {
            expected.push_back((last.key.clone(), last.time, last.result.clone()));
        }
        if previews.len() != expected.len() {
            return Err(format!(
                "{} getPreview records for {} number previews",
                previews.len(),
                expected.len()
            ));
        }
        let mut all_reads = VecDeque::new();
        for (preview, (key, time, value)) in previews.iter().zip(&expected) {
            if &preview.key != key
                || preview.time.to_bits() != time.to_bits()
                || &preview.result != value
            {
                return Err("getPreview records disagree with the list's preview calls".to_owned());
            }
            all_reads.extend(preview.reads.iter().cloned());
        }
        self.host.selection.load(all_reads);
        let mut mismatch: Option<String> = None;
        let result = self.controllers[ctrl].frame_list_observed(
            &mut self.host,
            list,
            time,
            |key, t, result| {
                let port = match result {
                    Ok(text) => Ok(text.as_ref().map_or(JsValue::Null, |s| s.to_js())),
                    Err(e) => Err(e.to_string()),
                };
                match expected.pop_front() {
                    Some((k, et, value))
                        if &k == key && et.to_bits() == t.to_bits() && value == port => {}
                    other => {
                        mismatch.get_or_insert(format!(
                            "preview ({}, {t}) = {port:?}, upstream {other:?}",
                            key.to_string_lossy()
                        ));
                    }
                }
            },
        );
        self.host.selection.finish()?;
        if let Some(m) = mismatch {
            return Err(m);
        }
        if !expected.is_empty() {
            return Err(format!("upstream made more preview calls: {expected:?}"));
        }
        self.stats.previews += previews.len();
        Ok(result.err().map(|e| error_text(&e)))
    }

    fn op(&mut self, record: &Record, ctrl: usize, list: ListId) -> Result<(), String> {
        self.stats.ops += 1;
        let op = js(record, "op")?;
        let args = op.as_array().ok_or("an op is an array")?.to_vec();
        let name = text(at(&args, 0))?;
        if let Ok(Value::Bool(dark)) = record.value("dark") {
            self.host.dark = dark;
        }
        if name == "frame" {
            let port = self.frame(record, ctrl, list, &args)?;
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
            "setState" => {
                let items = at(&args, 1)
                    .as_array()
                    .ok_or("setState items")?
                    .iter()
                    .map(item_state)
                    .collect::<Result<Vec<_>, _>>()?;
                let mut items = Some(items);
                env(host, &mut |e| {
                    controller.with_list(list, |l, _| {
                        l.set_state(e, items.take().unwrap_or_default())
                    })
                })
                .err()
                .map(|e| e.to_string())
            }
            "clear" => {
                controller.with_list(list, |l, _| l.clear());
                None
            }
            "refresh" => env(host, &mut |e| {
                controller.with_list(list, |l, _| l.refresh(e))
            })
            .err()
            .map(|e| e.to_string()),
            "stop" => {
                controller.with_list(list, |l, _| l.stop());
                None
            }
            "addField" => {
                let key = at(&args, 1).as_str().cloned().ok_or("addField key")?;
                let parent = match at(&args, 2) {
                    JsValue::Undefined => None,
                    v => Some(index(v)?),
                };
                let recorded = encode(at(&args, 3));
                let mut supplemental = String::new();
                let result = env(host, &mut |e| {
                    controller.with_list(list, |l, others| {
                        supplemental =
                            encode(&JsValue::Array(others.iter().map(|s| s.to_js()).collect()));
                        l.add_field(e, &others, &key, parent)
                    })
                });
                if supplemental != recorded {
                    return Err(format!(
                        "addField supplemental states\n upstream {recorded}\n port     {supplemental}"
                    ));
                }
                result.err().map(|e| e.to_string())
            }
            "promptType" => {
                let i = index(at(&args, 1))?;
                controller.with_list(list, |l, _| {
                    l.prompt_type(i);
                });
                None
            }
            "contextMenu" => {
                let i = index(at(&args, 1))?;
                controller.with_list(list, |l, _| {
                    l.context_menu(i);
                });
                None
            }
            "typeResponse" => {
                let state = item_state(at(&args, 1))?;
                let mut state = Some(state);
                env(host, &mut |e| {
                    controller.with_list(list, |l, _| {
                        l.type_response(e, state.take().expect("one call"))
                    })
                })
                .err()
                .map(|e| e.to_string())
            }
            "toggleHidden" => {
                let i = index(at(&args, 1))?;
                env(host, &mut |e| {
                    controller.with_list(list, |l, _| l.toggle_hidden(e, i))
                })
                .err()
                .map(|e| e.to_string())
            }
            "nameClick" => {
                let (i, now) = (index(at(&args, 1))?, num(at(&args, 2))?);
                env(host, &mut |e| {
                    controller.with_list(list, |l, _| l.name_click(e, i, now))
                })
                .err()
                .map(|e| e.to_string())
            }
            "remove" => {
                let i = index(at(&args, 1))?;
                controller.with_list(list, |l, _| l.remove(i));
                None
            }
            "clearClick" => {
                controller.with_list(list, |l, _| l.request_clear());
                None
            }
            "clearResponse" => {
                controller.with_list(list, |l, _| l.clear_response());
                None
            }
            "mouseDown" | "mouseUp" | "mouseMove" | "setTitle" => {
                return Err(format!(
                    "the fixture reached list op {name}, which this harness does not replay"
                ));
            }
            other => return Err(format!("unsupported list op {other:?}")),
        };
        let hooked = matches!(
            name.as_str(),
            "setState" | "clear" | "refresh" | "stop" | "addField" | "frame"
        );
        if hooked || record.field("threw").is_some() {
            same_error(&name, threw(record), port)?;
        } else if let Some(error) = port {
            self.pending_errors.push_back(error);
        }
        Ok(())
    }

    fn handle_lg(
        &mut self,
        record: &Record,
        kind: &str,
        records: &mut std::slice::Iter<'_, Record>,
    ) -> Result<(), String> {
        match kind {
            "create" => {
                let uuids = js(record, "uuids")?;
                let uuids: Vec<String> = uuids
                    .as_array()
                    .ok_or("uuids")?
                    .iter()
                    .map(text)
                    .collect::<Result<_, _>>()?;
                let [left, right, discrete]: [String; 3] =
                    uuids.try_into().map_err(|_| "three uuids")?;
                let id = self.controllers.len();
                if lg_id(record)? != id {
                    return Err("controllers registered out of order".to_owned());
                }
                for (uuid, list) in [
                    (&left, ListId::Left),
                    (&right, ListId::Right),
                    (&discrete, ListId::Discrete),
                ] {
                    self.bound.insert(uuid.clone(), (id, list));
                }
                self.controllers
                    .push(LineGraphController::with_uuids([left, right, discrete]));
                self.previews.push(VecDeque::new());
                Ok(())
            }
            "command" => self.command(record),
            "preview" => {
                let id = lg_id(record)?;
                let args = js(record, "args")?;
                let args = args.as_array().ok_or("preview args")?;
                self.previews[id].push_back(Preview {
                    key: at(args, 0).as_str().cloned().ok_or("preview key")?,
                    time: num(at(args, 1))?,
                    reads: reads(record)?,
                    result: match threw(record) {
                        Some(error) => Err(error),
                        None => Ok(js(record, "value")?),
                    },
                });
                Ok(())
            }
            "save" => {
                let id = lg_id(record)?;
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
                Ok(())
            }
            "activeFields" => {
                let id = lg_id(record)?;
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
                Ok(())
            }
            "edit" => {
                let id = lg_id(record)?;
                let list = match record.string("list").as_deref() {
                    Some("left") => ListId::Left,
                    Some("right") => ListId::Right,
                    Some("discrete") => ListId::Discrete,
                    other => return Err(format!("edit button on list {other:?}")),
                };
                let r = js(record, "rect")?;
                let r = r.as_array().ok_or("rect")?;
                let rect = Rect {
                    x: num(at(r, 0))?,
                    y: num(at(r, 1))?,
                    width: num(at(r, 2))?,
                    height: num(at(r, 3))?,
                };
                self.controllers[id]
                    .ask_edit_axis(&mut self.host, list, rect)
                    .map_err(|e| format!("ask_edit_axis threw {}", error_text(&e)))?;
                match std::mem::take(&mut self.host.sent).as_slice() {
                    [
                        NamedMessage {
                            name: HubToMain::AskEditAxis,
                            data,
                        },
                    ] => {
                        self.pending_prompt = Some(data.clone());
                        Ok(())
                    }
                    other => Err(format!("the edit button sent {other:?}")),
                }
            }
            name => {
                if record.string("phase").as_deref() != Some("begin") {
                    return Err(format!("unexpected lg record {name}"));
                }
                let id = record.field("id").cloned();
                let mut inner = Vec::new();
                let end = loop {
                    let next = records
                        .next()
                        .ok_or_else(|| format!("{name} never ended"))?;
                    if next.string("lg").as_deref() == Some(name)
                        && next.string("phase").as_deref() == Some("end")
                        && next.field("id").cloned() == id
                    {
                        break next;
                    }
                    inner.push(next.clone());
                };
                self.bracket(record, inner, end)
            }
        }
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
        if let Some(kind) = record.string("lg") {
            return self.handle_lg(record, &kind, records);
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
        if record.string("main").as_deref() == Some("ask-edit-axis") {
            self.stats.prompts += 1;
            let expected = encode(&js(record, "data")?);
            let actual = self
                .pending_prompt
                .take()
                .ok_or("upstream asked to edit an axis, the port did not")?;
            let actual = encode(&actual);
            if expected != actual {
                return Err(format!(
                    "ask-edit-axis\n upstream {expected}\n port     {actual}"
                ));
            }
            return Ok(());
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
            if self.pending_prompt.is_some() {
                return Err("the port asked to edit an axis, upstream did not".to_owned());
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
        let Some((ctrl, list)) = self.list_of(sl) else {
            return Ok(());
        };
        if record.field("op").is_some() {
            return self.op(record, ctrl, list);
        }
        if record.field("dump").is_some() {
            self.stats.dumps += 1;
            let mut expected = js(record, "dump")?;
            expected.as_object_mut().and_then(|o| o.remove("config"));
            let expected = encode(&expected);
            let actual = encode(&list_dump(
                self.controllers[ctrl].list(list),
                &self.host.log.borrow(),
            ));
            if expected != actual {
                return Err(format!(
                    "{list:?} list dump\n upstream {expected}\n port     {actual}"
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
    if harness.pending_prompt.is_some() {
        return Err(format!("{id}: the port asked to edit an axis at the end"));
    }
    if harness.controllers.is_empty() {
        return Err(format!("{id}: no line graph controller was recorded"));
    }
    Ok(harness.stats)
}

#[test]
fn line_graph_fixtures() {
    let ids = case_ids();
    assert!(!ids.is_empty(), "no {CASES} vectors");
    let mut failures = Vec::new();
    let mut total = Stats::default();
    for id in &ids {
        match run_case(id) {
            Ok(s) => {
                eprintln!(
                    "{id}: {} commands, {} previews, {} saves, {} calls, {} prompts, {} dumps, {} list ops",
                    s.commands, s.previews, s.saves, s.calls, s.prompts, s.dumps, s.ops
                );
                total.commands += s.commands;
                total.previews += s.previews;
                total.saves += s.saves;
                total.calls += s.calls;
                total.prompts += s.prompts;
                total.dumps += s.dumps;
                total.ops += s.ops;
            }
            Err(e) => failures.push(e),
        }
    }
    eprintln!(
        "{} fixtures: {} commands, {} previews, {} saves, {} calls, {} prompts, {} dumps, {} list ops, {} failing",
        ids.len(),
        total.commands,
        total.previews,
        total.saves,
        total.calls,
        total.prompts,
        total.dumps,
        total.ops,
        failures.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
