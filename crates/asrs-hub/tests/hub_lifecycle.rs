#![allow(clippy::disallowed_methods)]

//! Replays `parity/specs/hub/lifecycle-*.entry.json` against `asrs_hub::hub::HubModel` and
//! compares every record with the committed `hub/lifecycle-*` vector, which
//! `oracle/src/hubSession.ts` records from upstream's real hub.ts, Tabs, Sidebar,
//! HistoricalDataSource and worker bundles in jsdom. The host here schedules work as that page
//! does (oracle/src/dom/env.ts timers and frames, workers drained oldest first after every
//! task, main's `historical-start` reply as a zero-delay timeout); decode workers are the real
//! `asrs_formats` workers on their own threads and the export worker is `asrs_formats::export`.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use asrs_formats::export::{ExportOptions, Format, Output, Sampling, export};
use asrs_formats::historical::{MainReply, MainRequest, WorkerName};
use asrs_formats::worker::{Abort, DecodeWorker, Request, WorkerMessage, WorkerThread};
use asrs_hub::hub::{
    ExportReply, HubFrame, HubHost, HubInterval, HubModel, HubTimeout, HubView, LiveSourceKind,
};
use asrs_hub::sidebar::{RowRect, RowValue, Sidebar};
use asrs_js::string::JsString;
use asrs_js::time::{Clock, Rng, SetClock, V8MathRandom};
use asrs_js::value::{JsValue, ToJs};
use asrs_log::log::{Log, SerializedLog};
use asrs_log::realm::JsRealm;
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage};
use asrs_model::selection::Selection;
use asrs_parity::cve1::Value;
use asrs_parity::json::{self, Json};
use asrs_parity::vectors::{self, Record};

const START_TIME: f64 = 1767225600000.0;
const FRAME_PERIOD: f64 = 16.0;
const WORKER_FROZEN_MS: f64 = 1_000_000_000_000.0;
const IGNORED_MAIN: [&str; 3] = ["save-state", "save-type-memory", "update-touch-bar-slider"];

fn err(message: impl Into<String>) -> String {
    message.into()
}

fn js(s: &str) -> JsString {
    JsString::from(s)
}

fn cve(value: &JsValue) -> Value {
    js_json::from_js(value)
}

fn cve_str(s: &str) -> Value {
    cve(&JsValue::String(js(s)))
}

/// `Math.random` as oracle/src/dom/env.ts seeds it.
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

struct PageClock(Rc<Cell<f64>>);

impl Clock for PageClock {
    fn now(&self) -> f64 {
        self.0.get()
    }
}

/// jsdom with oracle/src/dom/layout.json: no rule reaches sidebar rows, so each is 0x0.
struct View;

impl HubView for View {
    fn inner_height(&self) -> f64 {
        650.0
    }

    fn renderer_top(&self, _show_timeline: bool) -> f64 {
        92.0
    }

    fn dark(&self) -> bool {
        false
    }

    fn can_scroll_into_view(&self) -> bool {
        false
    }

    fn sidebar_rects(&self, sidebar: &Sidebar) -> Vec<RowRect> {
        vec![RowRect::default(); sidebar.rows().len()]
    }

    fn measure_value(&self, _value: &RowValue) -> f64 {
        0.0
    }
}

struct Emit(Vec<(&'static str, Value)>);

enum Task {
    Interval(HubInterval, f64),
    Timeout(HubTimeout),
    Main(MainReply),
}

struct Timer {
    due: f64,
    seq: u64,
    task: Task,
}

enum Work {
    Decode {
        source: String,
        thread: WorkerThread,
        pending: usize,
    },
    Export {
        id: u64,
        replies: Vec<ExportReply>,
    },
}

struct Slot {
    work: Work,
    terminated: bool,
    hung: bool,
}

struct Host {
    clock: Rc<Cell<f64>>,
    base: PathBuf,
    timers: Vec<Timer>,
    seq: u64,
    frames: Vec<HubFrame>,
    last_frame: f64,
    slots: Vec<Slot>,
    emitted: Vec<Emit>,
    unmodeled: Vec<String>,
    ignored: Vec<String>,
}

impl Host {
    fn emit(&mut self, values: Vec<(&'static str, Value)>) {
        self.emitted.push(Emit(values));
    }

    fn add_timer(&mut self, delay: f64, task: Task) {
        let due = self.clock.get() + delay;
        let seq = self.seq;
        self.seq += 1;
        self.timers.push(Timer { due, seq, task });
    }

    fn decode_slot(&mut self, source: &str) -> Option<usize> {
        self.slots
            .iter()
            .rposition(|s| matches!(&s.work, Work::Decode { source: owner, .. } if owner == source))
    }
}

fn number(n: f64) -> Value {
    Value::number(n)
}

fn export_options(options: &JsValue) -> Option<ExportOptions> {
    let text = |key: &str| {
        options
            .get(key)
            .and_then(JsValue::as_str)
            .map(|s| s.to_string_lossy())
    };
    Some(ExportOptions {
        format: match text("format")?.as_str() {
            "csv-table" => Format::CsvTable,
            "csv-list" => Format::CsvList,
            "wpilog" => Format::Wpilog,
            "mcap" => Format::Mcap,
            _ => return None,
        },
        sampling: match text("samplingMode")?.as_str() {
            "changes" => Sampling::Changes,
            "fixed" => Sampling::Fixed,
            "akit" => Sampling::Akit,
            _ => return None,
        },
        sampling_period: options.get("samplingPeriod")?.as_f64()?,
        prefixes: options.get("prefixes")?.as_str()?.clone(),
        include_generated: matches!(options.get("includeGenerated"), Some(JsValue::Bool(true))),
    })
}

impl HubHost for Host {
    fn now(&self) -> f64 {
        self.clock.get()
    }

    fn send_main_message(&mut self, message: NamedMessage<HubToMain>) {
        let name = message.name.name();
        if !self.ignored.iter().any(|i| i == name) {
            self.emit(vec![("main", cve_str(name)), ("data", cve(&message.data))]);
        }
    }

    fn send_historical_request(&mut self, request: MainRequest) {
        match request {
            MainRequest::HistoricalStart { uuid, path } => {
                self.emit(vec![
                    ("main", cve_str("historical-start")),
                    (
                        "data",
                        Value::object([
                            ("uuid", cve_str(&uuid)),
                            ("path", cve(&JsValue::String(path.clone()))),
                        ]),
                    ),
                ]);
                let target = self.base.join(path.to_string_lossy());
                let files = vec![std::fs::read(target).ok().map(Arc::from)];
                self.add_timer(
                    0.0,
                    Task::Main(MainReply {
                        uuid,
                        files,
                        error: None,
                    }),
                );
            }
            MainRequest::HistoricalStartRaw { .. } => {
                self.unmodeled.push("historical-start-raw".to_owned());
            }
        }
    }

    fn set_timeout(&mut self, timeout: HubTimeout) {
        self.add_timer(0.0, Task::Timeout(timeout));
    }

    fn set_interval(&mut self, interval: HubInterval, period_ms: f64) {
        let period = period_ms.max(1.0);
        self.add_timer(period, Task::Interval(interval, period));
    }

    fn clear_interval(&mut self, interval: &HubInterval) {
        self.timers
            .retain(|t| !matches!(&t.task, Task::Interval(i, _) if i == interval));
    }

    fn request_animation_frame(&mut self, frame: HubFrame) {
        self.frames.push(frame);
    }

    fn worker_realm(&mut self) -> (Box<dyn Rng + Send>, Box<dyn Clock + Send>) {
        (
            Box::new(V8MathRandom::from_entropy()),
            Box::new(SetClock(WORKER_FROZEN_MS)),
        )
    }

    fn spawn_worker(&mut self, source: &str, name: WorkerName, worker: Box<dyn DecodeWorker>) {
        let thread = WorkerThread::spawn(source, worker).expect("spawn a decode thread");
        self.emit(vec![
            ("worker", number(self.slots.len() as f64)),
            ("script", cve_str(name.script())),
        ]);
        self.slots.push(Slot {
            work: Work::Decode {
                source: source.to_owned(),
                thread,
                pending: 0,
            },
            terminated: false,
            hung: false,
        });
    }

    fn post_worker(&mut self, source: &str, request: Request) {
        let Some(index) = self.decode_slot(source) else {
            return;
        };
        let record = match &request {
            Request::Start(files) => vec![
                ("post", number(index as f64)),
                ("type", cve_str("start")),
                (
                    "files",
                    Value::Array(
                        files
                            .iter()
                            .map(|f| f.as_ref().map_or(Value::Null, |f| number(f.len() as f64)))
                            .collect(),
                    ),
                ),
            ],
            Request::ParseField(key) => vec![
                ("post", number(index as f64)),
                ("type", cve_str("parseField")),
                ("key", cve(&JsValue::String(key.clone()))),
            ],
        };
        self.emit(record);
        let slot = &mut self.slots[index];
        if let Work::Decode {
            thread, pending, ..
        } = &mut slot.work
            && !slot.terminated
            && !slot.hung
        {
            *pending += 1;
            thread.post(request);
        }
    }

    fn terminate_worker(&mut self, source: &str) {
        let Some(index) = self.decode_slot(source) else {
            return;
        };
        let slot = &mut self.slots[index];
        slot.terminated = true;
        if let Work::Decode { thread, .. } = &mut slot.work {
            thread.terminate();
        }
        self.emit(vec![("terminate", number(index as f64))]);
    }

    fn request_export(&mut self, id: u64, log: SerializedLog, options: JsValue) {
        let index = self.slots.len();
        self.emit(vec![
            ("worker", number(index as f64)),
            ("script", cve_str("exportWorker.js")),
        ]);
        self.emit(vec![
            ("post", number(index as f64)),
            ("type", cve_str("request")),
        ]);
        let mut realm = JsRealm::new(Box::new(V8MathRandom::from_entropy()));
        let output = match (
            Log::from_serialized(log, &mut realm),
            export_options(&options),
        ) {
            (Ok(log), Some(options)) => export(&log, &options).ok(),
            _ => None,
        };
        let replies = match output {
            Some(Output::Text(text)) => vec![
                ExportReply::Progress(1.0),
                ExportReply::Payload(JsValue::String(text)),
            ],
            Some(Output::Bytes(_)) => {
                self.unmodeled.push("a binary export payload".to_owned());
                Vec::new()
            }
            None => vec![ExportReply::Failed],
        };
        self.slots.push(Slot {
            work: Work::Export { id, replies },
            terminated: false,
            hung: false,
        });
    }

    fn console(&mut self, warn: bool, text: String) {
        if warn {
            self.emit(vec![("console", cve_str("warn")), ("text", cve_str(&text))]);
        }
    }

    fn exception(&mut self, task: &'static str, _error: String) {
        self.emit(vec![("exception", cve_str(task))]);
    }

    fn start_live_source(&mut self, _kind: LiveSourceKind, _address: &JsString) {
        self.unmodeled.push("a live source".to_owned());
    }

    fn stop_live_source(&mut self) {}

    fn live_data(&mut self, _data: &JsValue) {
        self.unmodeled.push("live-data".to_owned());
    }

    fn start_publisher(&mut self, _is_sim: &JsValue) {
        self.unmodeled.push("a publisher".to_owned());
    }

    fn stop_publisher(&mut self) {}
}

struct Runner {
    host: Host,
    hub: HubModel,
}

impl Runner {
    /// The page between tasks: each worker message in turn, from the oldest worker that has
    /// one or is still busy.
    fn settle(&mut self) -> Result<(), String> {
        loop {
            let Some(index) = self.host.slots.iter().position(|s| {
                !s.terminated
                    && !s.hung
                    && match &s.work {
                        Work::Decode { pending, .. } => *pending > 0,
                        Work::Export { replies, .. } => !replies.is_empty(),
                    }
            }) else {
                return Ok(());
            };
            let slot = &mut self.host.slots[index];
            match &mut slot.work {
                Work::Decode {
                    source,
                    thread,
                    pending,
                } => match thread.recv() {
                    Some(WorkerMessage::Response(response)) => {
                        let source = source.clone();
                        self.hub.worker_message(&source, response, &mut self.host);
                    }
                    Some(WorkerMessage::Done(end)) => {
                        *pending -= 1;
                        match end {
                            Ok(()) | Err(Abort::Threw) => {}
                            Err(Abort::NeverReturns) => slot.hung = true,
                            Err(Abort::Unmodeled(what)) => return Err(format!("worker: {what}")),
                        }
                    }
                    None => slot.hung = true,
                },
                Work::Export { id, replies } => {
                    let id = *id;
                    let reply = replies.remove(0);
                    if !matches!(reply, ExportReply::Progress(_)) {
                        replies.clear();
                        slot.terminated = true;
                        self.host.emit(vec![("terminate", number(index as f64))]);
                    }
                    self.hub.export_reply(id, reply, &mut self.host);
                }
            }
        }
    }

    fn run_timer(&mut self, timer: Timer) -> Result<(), String> {
        match timer.task {
            Task::Interval(interval, period) => {
                self.host.timers.push(Timer {
                    due: timer.due + period,
                    seq: self.host.seq,
                    task: Task::Interval(interval.clone(), period),
                });
                self.host.seq += 1;
                self.hub.fire_interval(&interval, &mut self.host);
            }
            Task::Timeout(timeout) => self.hub.run_timeout(timeout, &mut self.host),
            Task::Main(reply) => self.hub.historical_data(&reply, &mut self.host),
        }
        self.settle()
    }

    /// oracle/src/dom/env.ts `advance`: timers and frames in time order, timers first on a tie.
    fn advance(&mut self, ms: f64) -> Result<(), String> {
        let end = self.host.clock.get() + ms;
        loop {
            let next = self
                .host
                .timers
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| a.due.total_cmp(&b.due).then(a.seq.cmp(&b.seq)))
                .map(|(i, t)| (i, t.due));
            let frame_at = self.host.last_frame + FRAME_PERIOD;
            match next {
                Some((i, due)) if due <= end && due <= frame_at => {
                    self.host.clock.set(self.host.clock.get().max(due));
                    let timer = self.host.timers.remove(i);
                    self.run_timer(timer)?;
                }
                _ if frame_at <= end => {
                    self.host.clock.set(frame_at);
                    self.host.last_frame = frame_at;
                    for frame in std::mem::take(&mut self.host.frames) {
                        self.hub.animation_frame(&frame, &mut self.host);
                    }
                    self.settle()?;
                }
                _ => {
                    self.host.clock.set(end);
                    return Ok(());
                }
            }
        }
    }

    fn main(&mut self, name: &str, data: JsValue) -> Result<(), String> {
        let name =
            MainToHub::from_name(name).ok_or_else(|| err(format!("unknown message {name}")))?;
        self.hub
            .handle_main_message(&NamedMessage::new(name, data), &mut self.host);
        self.settle()
    }

    fn capture(&mut self, kind: &str) -> Result<(), String> {
        let record = match kind {
            "title" => ("title", cve(&JsValue::String(self.hub.title().clone()))),
            "loading" => {
                let loading = self.hub.loading();
                let show = match loading.show {
                    None => "",
                    Some(true) => "1",
                    Some(false) => "0",
                };
                let progress = loading.progress.clone().unwrap_or_default();
                (
                    "loading",
                    Value::object([
                        ("show", cve_str(show)),
                        ("progress", cve(&JsValue::String(progress))),
                    ]),
                )
            }
            "selection" => {
                let selection = self.hub.page_mut().selection_mut();
                let opt = |v: Option<f64>| v.map_or(Value::Null, number);
                let mode = selection.get_mode().number();
                let selected = selection.get_selected_time();
                let hovered = selection.get_hovered_time();
                let [a, b] = selection.get_timeline_range();
                let max_zoom = selection.get_timeline_is_max_zoom();
                (
                    "selection",
                    Value::object([
                        ("mode", number(mode)),
                        ("selectedTime", opt(selected)),
                        ("hoveredTime", opt(hovered)),
                        ("timelineRange", Value::Array(vec![number(a), number(b)])),
                        ("timelineIsMaxZoom", Value::Bool(max_zoom)),
                    ]),
                )
            }
            "log" => {
                let mut loading: Vec<JsString> = self.hub.loading_fields().into_iter().collect();
                loading.sort_by(|a, b| a.units().cmp(b.units()));
                let log = self.hub.page().log();
                let [a, b] = log.get_timestamp_range();
                let keys = log
                    .get_field_keys()
                    .into_iter()
                    .map(|k| cve(&JsValue::String(k)))
                    .collect();
                (
                    "log",
                    Value::object([
                        ("keys", Value::Array(keys)),
                        ("range", Value::Array(vec![number(a), number(b)])),
                        (
                            "loading",
                            Value::Array(
                                loading
                                    .into_iter()
                                    .map(|k| cve(&JsValue::String(k)))
                                    .collect(),
                            ),
                        ),
                    ]),
                )
            }
            "state" => ("state", cve(&self.hub.save_state())),
            "window" => {
                let flag = |v: Option<bool>| match v {
                    None => "",
                    Some(true) => "1",
                    Some(false) => "0",
                };
                (
                    "window",
                    Value::object([
                        (
                            "classes",
                            Value::Array(
                                self.hub.body_classes().iter().map(|c| cve_str(c)).collect(),
                            ),
                        ),
                        ("updateButton", cve_str(flag(self.hub.update_button()))),
                        ("feedbackButton", cve_str(flag(self.hub.feedback_button()))),
                    ]),
                )
            }
            other => return Err(err(format!("capture kind {other} is not modeled"))),
        };
        self.host.emit(vec![record]);
        Ok(())
    }
}

fn to_js(json: &Json) -> Result<JsValue, String> {
    js_json::to_js(&Value::from_json_lenient(json))
}

fn replay(spec: &Path) -> Result<(Vec<Emit>, Vec<String>), String> {
    let text = std::fs::read_to_string(spec).map_err(|e| e.to_string())?;
    let spec_json = json::parse(&text).map_err(|e| e.to_string())?;
    let fixture = spec_json
        .get("fixture")
        .ok_or_else(|| err("spec has no fixture"))?;
    for key in [
        "boot",
        "preferences",
        "assets",
        "version",
        "window",
        "startTime",
    ] {
        if fixture.get(key).is_some() {
            return Err(format!("fixture key {key} is not modeled"));
        }
    }
    let seed = fixture.get("seed").and_then(Json::as_f64).unwrap_or(1.0) as u32;
    let clock = Rc::new(Cell::new(START_TIME));
    let mut host = Host {
        clock: clock.clone(),
        base: spec.parent().unwrap().to_path_buf(),
        timers: Vec::new(),
        seq: 0,
        frames: Vec::new(),
        last_frame: START_TIME,
        slots: Vec::new(),
        emitted: Vec::new(),
        unmodeled: Vec::new(),
        ignored: match fixture.get("ignoreMain").and_then(Json::as_array) {
            Some(names) => names
                .iter()
                .map(|n| n.as_str().map(|s| s.to_string_lossy()))
                .collect::<Option<_>>()
                .ok_or_else(|| err("ignoreMain lists names"))?,
            None => IGNORED_MAIN.map(str::to_owned).into(),
        },
    };
    let mut rng = DomRng(seed);
    // One `Math.random()` happens while hub.ts's imports evaluate, before the hub draws any
    // UUID (the same draw dom_tabs.rs makes).
    rng.next_f64();
    let realm = JsRealm::new(Box::new(rng));
    let hub = HubModel::new(realm, Rc::new(PageClock(clock)), Box::new(View), &mut host);
    let mut runner = Runner { host, hub };

    let version = asrs_js::ops::object([
        ("platform", JsValue::string("darwin")),
        ("platformRelease", JsValue::string("25.0.0")),
        ("platformArch", JsValue::string("arm64")),
        (
            "appVersion",
            JsValue::string(asrs_model::build_constants::LITE_VERSION),
        ),
    ]);
    let boot: Vec<(&str, JsValue)> = vec![
        ("set-assets", JsValue::Null),
        ("set-fullscreen", JsValue::Bool(false)),
        ("set-battery", JsValue::Bool(false)),
        ("set-version", version),
        ("show-update-button", JsValue::Bool(false)),
        ("show-feedback-button", JsValue::Bool(false)),
        ("show-when-ready", JsValue::Undefined),
        (
            "set-preferences",
            asrs_model::preferences::Preferences::default().to_js(),
        ),
        ("set-active-satellites", JsValue::Array(Vec::new())),
    ];
    for (name, data) in boot {
        runner.main(name, data)?;
    }

    let steps = fixture
        .get("steps")
        .and_then(Json::as_array)
        .ok_or_else(|| err("fixture has no steps"))?;
    for (index, step) in steps.iter().enumerate() {
        if let Some(name) = step.get("main").and_then(Json::as_str) {
            let data = match step.get("data") {
                Some(data) => to_js(data)?,
                None => JsValue::Undefined,
            };
            runner.main(&name.to_string_lossy(), data)?;
        } else if let Some(ms) = step.get("advance").and_then(Json::as_f64) {
            runner.advance(ms)?;
        } else if let Some(frames) = step.get("frames").and_then(Json::as_f64) {
            runner.advance(frames * FRAME_PERIOD)?;
        } else if let Some(kinds) = step.get("capture").and_then(Json::as_array) {
            let label = match step.get("label") {
                Some(label) => Value::from_json_lenient(label),
                None => number(index as f64),
            };
            runner.host.emit(vec![("capture", label)]);
            for kind in kinds {
                let kind = kind
                    .as_str()
                    .ok_or_else(|| err("capture kinds are strings"))?;
                runner.capture(&kind.to_string_lossy())?;
            }
        } else {
            return Err(format!("step {index} is not modeled"));
        }
    }
    let unmodeled = std::mem::take(&mut runner.host.unmodeled);
    Ok((std::mem::take(&mut runner.host.emitted), unmodeled))
}

fn record_keys(record: &Record) -> Vec<String> {
    match &record.json {
        Json::Object(pairs) => pairs.iter().map(|(k, _)| k.to_string_lossy()).collect(),
        _ => Vec::new(),
    }
}

fn compare(spec: &Path, id: &str) -> Result<usize, String> {
    let (emitted, unmodeled) = replay(spec)?;
    if !unmodeled.is_empty() {
        return Err(format!(
            "the fixture reaches unmodeled host work: {unmodeled:?}"
        ));
    }
    let records = vectors::load(id)?;
    for (i, record) in records.iter().enumerate() {
        let emit = emitted.get(i).ok_or_else(|| {
            format!(
                "line {}: the port emitted nothing for {}",
                record.line, record.text
            )
        })?;
        let keys: Vec<String> = emit.0.iter().map(|(k, _)| (*k).to_owned()).collect();
        if keys != record_keys(record) {
            return Err(format!(
                "line {}: upstream {} but the port emitted keys {keys:?}",
                record.line, record.text
            ));
        }
        for (key, value) in &emit.0 {
            let expected = record.value(key).map_err(|e| e.0)?;
            if !vectors::values_match(&expected, value)
                && let Some(report) = asrs_parity::diff::report(&expected, value)
            {
                return Err(format!("line {} ({key}): {report}", record.line));
            }
        }
    }
    if emitted.len() > records.len() {
        return Err(format!(
            "the port emitted {} records past the vector's {}",
            emitted.len() - records.len(),
            records.len()
        ));
    }
    Ok(records.len())
}

#[test]
fn hub_lifecycle_sessions() {
    let dir = vectors::workspace_root().join("parity/specs/hub");
    let mut specs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("lifecycle-") && n.ends_with(".entry.json"))
        })
        .collect();
    specs.sort();
    assert!(!specs.is_empty(), "no hub lifecycle specs");
    let mut failures = Vec::new();
    let mut compared = 0;
    for spec in &specs {
        let name = spec
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .trim_end_matches(".entry.json");
        match compare(spec, &format!("hub/{name}")) {
            Ok(n) => compared += n,
            Err(e) => failures.push(format!("hub/{name}: {e}")),
        }
    }
    eprintln!(
        "hub lifecycle: {} sessions, {compared} records compared",
        specs.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
