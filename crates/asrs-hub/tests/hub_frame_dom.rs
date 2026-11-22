#![allow(clippy::disallowed_methods)]

//! Replays `parity/specs/dom/hub-frame/<case>.entry.json` against `asrs_hub::hub::HubModel`
//! with `asrs_hub::frame` and compares every record with the committed `dom/hub-frame/<case>`
//! vector, which upstream's real hub.html, hub.css and hub.ts produce in Blink with real
//! geometry.
//!
//! A `/*frame:layout*/` capture is the root element's custom properties, the `body` classes
//! and cursor, and each frame element's `hidden` attribute and `getBoundingClientRect()`; the
//! port answers it from the model through `FrameInputs::from_hub` and `FrameLayout`. Every
//! `read` record is upstream's own `RENDERER_CONTENT.getBoundingClientRect().top` in
//! `updateControlsHeight`, which the model asks its `HubView` for; the view answers from
//! `frame::renderer_top` and the answer must equal the recorded `top`. `/*frame:glow*/` evals
//! write the loading-glow properties directly (the model's progress values are D07's), so the
//! harness hands the same strings to the layout.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use asrs_formats::historical::{MainRequest, WorkerName};
use asrs_formats::worker::{DecodeWorker, Request};
use asrs_hub::frame::{
    BodyClasses, FrameElement, FrameInputs, FrameLayout, Handle, HubShell, client_coordinate,
    renderer_top,
};
use asrs_hub::hub::{
    HubFrame, HubHost, HubInterval, HubModel, HubTimeout, HubView, LiveSourceKind,
};
use asrs_hub::sidebar::{RowRect, RowValue, Sidebar};
use asrs_js::number;
use asrs_js::string::JsString;
use asrs_js::time::{Clock, Rng, SetClock};
use asrs_js::value::{JsValue, ToJs};
use asrs_log::log::SerializedLog;
use asrs_log::realm::JsRealm;
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage};
use asrs_model::selection::Selection;
use asrs_parity::cve1::Value;
use asrs_parity::json::{self, Json};
use asrs_parity::vectors::{self, Record};

const START_TIME: f64 = 1767225600000.0;
const IGNORED_MAIN: [&str; 3] = ["save-state", "save-type-memory", "update-touch-bar-slider"];
const VARS: [&str; 10] = [
    "--side-bar-width",
    "--show-side-bar",
    "--tab-controls-height",
    "--show-tab-controls",
    "--show-timeline",
    "--show-lock-buttons",
    "--show-update-button",
    "--show-feedback-button",
    "--show-loading-glow",
    "--loading-glow-progress",
];

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

/// The body classes the page has right now, which is all `renderer_top` depends on, and the
/// answers given, in call order.
#[derive(Default)]
struct Geometry {
    classes: BodyClasses,
    class_names: Vec<&'static str>,
    reads: Vec<(Vec<&'static str>, f64)>,
}

struct View {
    width: f64,
    height: f64,
    geometry: Rc<RefCell<Geometry>>,
}

impl HubView for View {
    fn inner_height(&self) -> f64 {
        self.height
    }

    fn renderer_top(&self, show_timeline: bool) -> f64 {
        let mut g = self.geometry.borrow_mut();
        let top = renderer_top(g.classes, show_timeline);
        let names = g.class_names.clone();
        g.reads.push((names, top));
        top
    }

    fn dark(&self) -> bool {
        false
    }

    fn can_scroll_into_view(&self) -> bool {
        true
    }

    fn sidebar_rects(&self, sidebar: &Sidebar) -> Vec<RowRect> {
        assert!(
            sidebar.rows().is_empty(),
            "the frame fixtures load no log, so the sidebar has no rows ({} wide)",
            self.width
        );
        Vec::new()
    }

    fn measure_value(&self, _value: &RowValue) -> f64 {
        panic!("the frame fixtures show no field values")
    }

    fn source_list_dom(&mut self) -> &mut dyn asrs_model::source_list::SourceListDom {
        asrs_model::source_list::Undisplayed::dom()
    }
}

#[derive(Default)]
struct Host {
    clock: Rc<Cell<f64>>,
    failures: Vec<String>,
}

impl HubHost for Host {
    fn now(&self) -> f64 {
        self.clock.get().trunc()
    }

    fn send_main_message(&mut self, message: NamedMessage<HubToMain>) {
        let name = message.name.name();
        if !IGNORED_MAIN.contains(&name) {
            self.failures
                .push(format!("main message {name} the page never sent"));
        }
    }

    fn send_historical_request(&mut self, _request: MainRequest) {
        self.failures.push("historical request".into());
    }

    fn set_timeout(&mut self, _timeout: HubTimeout) {
        self.failures.push("setTimeout".into());
    }

    fn set_interval(&mut self, _interval: HubInterval, _period_ms: f64) {}

    fn clear_interval(&mut self, _interval: &HubInterval) {}

    fn request_animation_frame(&mut self, _frame: HubFrame) {}

    fn worker_realm(&mut self) -> (Box<dyn Rng + Send>, Box<dyn Clock + Send>) {
        (Box::new(DomRng(0)), Box::new(SetClock(0.0)))
    }

    fn spawn_worker(&mut self, _source: &str, _name: WorkerName, _worker: Box<dyn DecodeWorker>) {
        self.failures.push("worker".into());
    }

    fn post_worker(&mut self, _source: &str, _request: Request) {
        self.failures.push("worker message".into());
    }

    fn terminate_worker(&mut self, _source: &str) {}

    fn request_export(&mut self, _id: u64, _log: SerializedLog, _options: JsValue) {
        self.failures.push("export".into());
    }

    fn console(&mut self, warn: bool, text: String) {
        if warn {
            self.failures.push(format!("console.warn {text}"));
        }
    }

    fn exception(&mut self, task: &'static str, error: String) {
        self.failures.push(format!("exception in {task}: {error}"));
    }

    fn start_live_source(&mut self, _kind: LiveSourceKind, _address: &JsString) {
        self.failures.push("live source".into());
    }

    fn stop_live_source(&mut self) {}

    fn live_data(&mut self, _data: &JsValue) {
        self.failures.push("live-data".into());
    }

    fn start_publisher(&mut self, _is_sim: &JsValue) {
        self.failures.push("publisher".into());
    }

    fn stop_publisher(&mut self) {}
}

fn err(s: impl Into<String>) -> String {
    s.into()
}

fn to_js(json: &Json) -> Result<JsValue, String> {
    js_json::to_js(&Value::from_json_lenient(json))
}

fn num_str(n: f64) -> String {
    number::to_string(n).to_string()
}

fn flag(v: Option<bool>) -> String {
    match v {
        None => String::new(),
        Some(true) => "1".into(),
        Some(false) => "0".into(),
    }
}

struct Run {
    hub: HubModel,
    host: Host,
    shell: HubShell,
    geometry: Rc<RefCell<Geometry>>,
    width: f64,
    height: f64,
    dpr: f64,
    glow: Option<(String, JsString)>,
    records: Vec<Record>,
    next: usize,
    compared_reads: usize,
    compared_captures: usize,
}

impl Run {
    fn sync(geometry: &RefCell<Geometry>, hub: &HubModel) {
        let mut g = geometry.borrow_mut();
        g.class_names = hub.body_classes().to_vec();
        g.classes = BodyClasses::from_names(g.class_names.iter().copied());
    }

    fn record(&mut self, what: &str) -> Result<&Record, String> {
        let r = self
            .records
            .get(self.next)
            .ok_or_else(|| format!("{what}: the vector ended"))?;
        self.next += 1;
        Ok(r)
    }

    /// Matches the page's geometry reads since the last call with the next `read` records.
    fn reads(&mut self) -> Result<(), String> {
        let reads = std::mem::take(&mut self.geometry.borrow_mut().reads);
        for (classes, top) in reads {
            let record = self.record("read")?.clone();
            if record.string("read").as_deref() != Some("getBoundingClientRect") {
                return Err(format!(
                    "line {}: the port read renderer-content, upstream wrote {:?}",
                    record.line,
                    record.field("read")
                ));
            }
            let body: String = classes.iter().map(|c| format!(".{c}")).collect();
            let el = format!("body{body}:1>div.viewer:19>div.renderer-content:5");
            if record.string("el").as_deref() != Some(el.as_str()) {
                return Err(format!(
                    "line {}: read on {:?}, the port's page is {el}",
                    record.line,
                    record.string("el")
                ));
            }
            let value = record.value("value").map_err(|e| format!("{e:?}"))?;
            let recorded = value
                .as_array()
                .and_then(|r| r.get(1))
                .and_then(Value::as_f64)
                .ok_or_else(|| format!("line {}: read value", record.line))?;
            if recorded.to_bits() != top.to_bits() {
                return Err(format!(
                    "line {}: renderer-content top {recorded}, the port answered {top}",
                    record.line
                ));
            }
            self.compared_reads += 1;
        }
        Ok(())
    }

    fn after_step(&mut self, what: &str) -> Result<(), String> {
        if !self.host.failures.is_empty() {
            return Err(format!("{what}: {}", self.host.failures.join("; ")));
        }
        self.reads()
    }

    fn main(&mut self, name: &str, data: JsValue) -> Result<(), String> {
        let name = MainToHub::from_name(name).ok_or_else(|| format!("unknown message {name}"))?;
        Run::sync(&self.geometry, &self.hub);
        self.hub
            .handle_main_message(&NamedMessage::new(name, data), &mut self.host);
        Run::sync(&self.geometry, &self.hub);
        self.after_step(name.name())
    }

    fn event(&mut self, event: &Json) -> Result<(), String> {
        let target = event
            .get("target")
            .and_then(Json::as_str)
            .map(|s| s.to_string_lossy())
            .ok_or_else(|| err("event target"))?;
        let kind = event
            .get("type")
            .and_then(Json::as_str)
            .map(|s| s.to_string_lossy())
            .ok_or_else(|| err("event type"))?;
        let coordinate = |key: &str| {
            client_coordinate(
                event
                    .get("init")
                    .and_then(|i| i.get(key))
                    .and_then(Json::as_f64)
                    .unwrap_or(0.0),
            )
        };
        let (x, y) = (coordinate("clientX"), coordinate("clientY"));
        let handle = match target.as_str() {
            "div.side-bar-handle" => Some(Handle::SideBar),
            "div.controls-handle" => Some(Handle::Controls),
            "window" => None,
            other => return Err(format!("event on {other}")),
        };
        Run::sync(&self.geometry, &self.hub);
        let now = self.host.now();
        match (handle, kind.as_str()) {
            (Some(handle), "mousedown") => self.shell.handle_mouse_down(&mut self.hub, handle),
            (Some(handle), "click") => self
                .shell
                .handle_click(&mut self.hub, handle, now)
                .map_err(|e| format!("{e}"))?,
            (None, "mousemove") => {
                let geometry = self.geometry.clone();
                self.shell
                    .window_mouse_move(&mut self.hub, x, y, &mut |hub| Run::sync(&geometry, hub))
                    .map_err(|e| format!("{e}"))?
            }
            (None, "mouseup") => self
                .shell
                .window_mouse_up(&mut self.hub)
                .map_err(|e| format!("{e}"))?,
            (target, kind) => return Err(format!("{kind} on {target:?}")),
        }
        self.after_step(&kind)
    }

    fn eval(&mut self, expr: &str) -> Result<(), String> {
        if let Some(rest) = expr.strip_prefix("/*frame:glow*/ ") {
            let quoted: Vec<&str> = rest.split('"').skip(1).step_by(2).collect();
            let [show_name, show, progress_name, progress] = quoted.as_slice() else {
                return Err(format!("glow eval {expr}"));
            };
            if *show_name != "--show-loading-glow" || *progress_name != "--loading-glow-progress" {
                return Err(format!("glow eval {expr}"));
            }
            self.glow = Some(((*show).to_owned(), JsString::from(*progress)));
        } else if let Some(index) = expr
            .strip_prefix("window.tabs.setSelected(")
            .and_then(|r| r.strip_suffix(')'))
        {
            let index = number::to_number(JsString::from(index).units());
            Run::sync(&self.geometry, &self.hub);
            let (tabs, page) = self.hub.tabs_mut();
            tabs.set_selected(page, &JsValue::Number(index))
                .map_err(|e| e.to_string())?;
        } else {
            return Err(format!("eval the harness does not model: {expr}"));
        }
        self.after_step("eval")
    }

    fn layout_value(&self) -> Value {
        let mut inputs = FrameInputs::from_hub(
            &self.hub,
            self.width,
            self.height,
            self.dpr,
            self.shell.fps(),
            (0.0, 0.0),
        );
        let mut vars: Vec<String> = vec![
            format!("{}px", num_str(inputs.side_bar_width)),
            if inputs.show_side_bar { "1" } else { "0" }.into(),
            inputs
                .tab_controls_height
                .map_or(String::new(), |h| format!("{}px", num_str(h))),
            flag(inputs.show_tab_controls),
            flag(inputs.show_timeline),
            flag(inputs.show_lock_buttons),
            flag(inputs.show_update_button),
            flag(inputs.show_feedback_button),
            flag(inputs.show_loading_glow),
            inputs
                .loading_glow_progress
                .as_ref()
                .map_or(String::new(), |p| p.to_string_lossy()),
        ];
        if let Some((show, progress)) = &self.glow {
            inputs.show_loading_glow = Some(show == "1");
            inputs.loading_glow_progress = Some(progress.clone());
            vars[8] = show.clone();
            vars[9] = progress.to_string_lossy();
        }
        debug_assert_eq!(vars.len(), VARS.len());
        let layout = FrameLayout::compute(&inputs);
        let rows = FrameElement::ALL
            .iter()
            .map(|&e| {
                let [x, y, w, h] = layout.client_rect(e);
                Value::Array(vec![
                    Value::string(e.selector()),
                    Value::Bool(e.hidden_attribute(&inputs.hidden)),
                    Value::number(x),
                    Value::number(y),
                    Value::number(w),
                    Value::number(h),
                ])
            })
            .collect();
        let drag = self.shell.drag();
        let position = drag.item_position();
        let px = |v: Option<f64>| v.map_or(String::new(), |v| format!("{}px", num_str(v)));
        Value::Array(vec![
            Value::Array(vars.iter().map(|v| Value::string(v)).collect()),
            Value::string(&self.hub.body_classes().join(" ")),
            Value::string(self.shell.cursor().css()),
            Value::Array(rows),
            Value::Array(vec![
                Value::Bool(drag.item_hidden()),
                Value::string(&px(position.map(|p| p.0))),
                Value::string(&px(position.map(|p| p.1))),
            ]),
        ])
    }

    fn capture(&mut self, label: &Json, specs: &[Json]) -> Result<(), String> {
        let record = self.record("capture")?.clone();
        let expected_label = Value::from_json_lenient(label);
        let recorded = record.value("capture").map_err(|e| format!("{e:?}"))?;
        if !vectors::values_match(&recorded, &expected_label) {
            return Err(format!(
                "line {}: capture {}",
                record.line,
                recorded.encode()
            ));
        }
        let time = record.value("time").map_err(|e| format!("{e:?}"))?;
        if !vectors::values_match(&time, &Value::number(0.0)) {
            return Err(format!(
                "line {}: capture time {}",
                record.line,
                time.encode()
            ));
        }
        for spec in specs {
            let expr = spec
                .get("expr")
                .and_then(Json::as_str)
                .map(|s| s.to_string_lossy())
                .ok_or_else(|| err("capture without an eval"))?;
            if !expr.starts_with("/*frame:layout*/") {
                return Err(format!("capture {expr}"));
            }
            let record = self.record("eval")?.clone();
            if record.string("eval").as_deref() != Some(expr.as_str()) {
                return Err(format!("line {}: eval differs", record.line));
            }
            let expected = record.value("value").map_err(|e| format!("{e:?}"))?;
            let actual = self.layout_value();
            if !vectors::values_match(&expected, &actual) {
                return Err(format!(
                    "line {} ({}): {}",
                    record.line,
                    recorded.encode(),
                    first_difference(&expected, &actual)
                ));
            }
            self.compared_captures += 1;
        }
        Ok(())
    }
}

fn first_difference(expected: &Value, actual: &Value) -> String {
    if let (Some(e), Some(a)) = (expected.as_array(), actual.as_array())
        && e.len() == a.len()
    {
        for (i, (e, a)) in e.iter().zip(a).enumerate() {
            if !vectors::values_match(e, a) {
                return format!("[{i}] {}", first_difference(e, a));
            }
        }
    }
    format!(
        "\n  expected {}\n  actual   {}",
        expected.encode(),
        actual.encode()
    )
}

fn replay(spec: &Path, id: &str) -> Result<(usize, usize), String> {
    let text = std::fs::read_to_string(spec).map_err(|e| e.to_string())?;
    let root = json::parse(&text).map_err(|e| format!("{e:?}"))?;
    let fixture = root.get("fixture").ok_or_else(|| err("no fixture"))?;
    let window = fixture.get("window");
    let dim = |key: &str, default: f64| {
        window
            .and_then(|w| w.get(key))
            .and_then(Json::as_f64)
            .unwrap_or(default)
    };
    let (width, height, dpr) = (
        dim("width", 1100.0),
        dim("height", 650.0),
        dim("devicePixelRatio", 1.0),
    );
    if fixture
        .get("geometry")
        .and_then(Json::as_str)
        .map(|s| s.to_string_lossy())
        .as_deref()
        != Some("real")
    {
        return Err(err("hub-frame fixtures use real geometry"));
    }
    let seed = fixture.get("seed").and_then(Json::as_f64).unwrap_or(0.0) as u32;
    let clock = Rc::new(Cell::new(START_TIME));
    let mut host = Host {
        clock: clock.clone(),
        failures: Vec::new(),
    };
    let geometry = Rc::new(RefCell::new(Geometry::default()));
    let mut rng = DomRng(seed);
    rng.next_f64();
    let hub = HubModel::new(
        JsRealm::new(Box::new(rng)),
        Rc::new(PageClock(clock)),
        Box::new(View {
            width,
            height,
            geometry: geometry.clone(),
        }),
        &mut host,
    );
    let mut run = Run {
        hub,
        host,
        shell: HubShell::default(),
        geometry,
        width,
        height,
        dpr,
        glow: None,
        records: vectors::load(id)?,
        next: 0,
        compared_reads: 0,
        compared_captures: 0,
    };
    run.after_step("HubModel::new")?;

    let version = match fixture.get("version") {
        Some(v) => to_js(v)?,
        None => asrs_js::ops::object([
            ("platform", JsValue::string("darwin")),
            ("platformRelease", JsValue::string("25.0.0")),
            ("platformArch", JsValue::string("arm64")),
            (
                "appVersion",
                JsValue::string(asrs_model::build_constants::LITE_VERSION),
            ),
        ]),
    };
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
        run.main(name, data)?;
    }

    let steps = fixture
        .get("steps")
        .and_then(Json::as_array)
        .ok_or_else(|| err("no steps"))?;
    for (index, step) in steps.iter().enumerate() {
        let result = if let Some(name) = step.get("main").and_then(Json::as_str) {
            let data = match step.get("data") {
                Some(d) => to_js(d)?,
                None => JsValue::Undefined,
            };
            run.main(&name.to_string_lossy(), data)
        } else if let Some(event) = step.get("event") {
            run.event(event)
        } else if let Some(expr) = step.get("eval").and_then(Json::as_str) {
            run.eval(&expr.to_string_lossy())
        } else if let Some(call) = step.get("selection").and_then(Json::as_array) {
            match call
                .first()
                .and_then(Json::as_str)
                .map(|s| s.to_string_lossy())
                .as_deref()
            {
                Some("setLiveConnected") => {
                    run.hub
                        .page_mut()
                        .selection_mut()
                        .set_live_connected(Rc::new(|| f64::NAN));
                    run.after_step("setLiveConnected")
                }
                other => Err(format!("selection step {other:?}")),
            }
        } else if let Some(specs) = step.get("capture").and_then(Json::as_array) {
            let label = step
                .get("label")
                .cloned()
                .unwrap_or(Json::Number(index as f64));
            run.capture(&label, specs)
        } else {
            Err(format!("unknown step {}", step.to_compact()))
        };
        result.map_err(|e| format!("step {index}: {e}"))?;
    }
    if run.next != run.records.len() {
        return Err(format!(
            "compared {} of {} records; next is line {}",
            run.next,
            run.records.len(),
            run.records[run.next].line
        ));
    }
    let recorded_reads = run
        .records
        .iter()
        .filter(|r| r.field("read").is_some())
        .count();
    let recorded_captures = run
        .records
        .iter()
        .filter(|r| r.field("eval").is_some())
        .count();
    assert_eq!(run.compared_reads, recorded_reads, "{id}: reads compared");
    assert_eq!(
        run.compared_captures, recorded_captures,
        "{id}: captures compared"
    );
    assert!(run.compared_captures > 0, "{id}: nothing captured");
    Ok((run.compared_reads, run.compared_captures))
}

#[test]
fn hub_frame_vectors() {
    let dir = vectors::workspace_root().join("parity/specs/dom/hub-frame");
    let mut specs: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with(".entry.json"))
        .collect();
    specs.sort();
    assert_eq!(specs.len(), 3, "dom/hub-frame specs");
    let mut failures = Vec::new();
    for spec in &specs {
        let name = spec
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.trim_end_matches(".entry.json"))
            .unwrap_or_default();
        let id = format!("dom/hub-frame/{name}");
        match replay(spec, &id) {
            Ok((reads, captures)) => eprintln!("{id}: {reads} reads, {captures} captures equal"),
            Err(e) => failures.push(format!("{id}: {e}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn file_drop_sends_open_files_request() {
    assert!(HubShell::drop_files(Vec::new()).is_none());
    let message =
        HubShell::drop_files(vec![JsString::from("/a.wpilog"), JsString::from("/b.rlog")])
            .expect("a drop with files");
    assert_eq!(message.name, HubToMain::OpenFilesRequest);
    let files = message
        .data
        .get("files")
        .and_then(JsValue::as_array)
        .expect("files");
    assert_eq!(files.len(), 2);
}
