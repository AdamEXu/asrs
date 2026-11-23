#![allow(clippy::disallowed_methods)]

//! Replays `dom/histogram/*`, written by `oracle/tools/histogramSpecs.mjs` and recorded in Blink
//! from upstream's StatisticsRenderer and Chart.js 4.4.0, through `asrs_tabs::histogram`.
//!
//! Every outermost entry into the chart is compared call for call with the canvas calls upstream
//! made, after every layout the chart's geometry is compared bit for bit (tick values and
//! labels, scale boxes, bar rects, chart area, legend), and throws are matched in both
//! directions. Within an animation frame the order and kind of Chart.js's callbacks are compared
//! too. Records the harness does not know fail.

use std::collections::BTreeMap;

use asrs_canvas::text::{FixedAdvance, TextBackend};
use asrs_canvas::{Call, Raster, trace};
use asrs_js::string::{JsError, JsString};
use asrs_parity::vectors::{load, vectors_dir};
use asrs_parity::{Json, Record, Utf16String, Value};
use asrs_tabs::histogram::{
    Env, EventKind, Histogram, HistogramCommand, HistogramField, ScaleLayout, Work,
};

const OTHER_KINDS: &[&str] = &["main", "exception"];

fn case_ids() -> Vec<String> {
    let dir = vectors_dir().join("dom/histogram");
    let mut ids: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| {
            let name = entry.unwrap().file_name().into_string().unwrap();
            format!("dom/histogram/{}", name.split('.').next().unwrap())
        })
        .collect();
    ids.sort();
    assert!(ids.len() >= 18, "only {} histogram vectors", ids.len());
    ids
}

fn number(value: &Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("expected a number, got {}", value.kind()))
}

fn items(value: &Value) -> &[Value] {
    match value {
        Value::Array(items) => items,
        other => panic!("expected an array, got {}", other.kind()),
    }
}

fn numbers(value: &Value) -> Vec<f64> {
    items(value).iter().map(number).collect()
}

fn pair(value: &Value) -> (f64, f64) {
    match numbers(value)[..] {
        [a, b] => (a, b),
        ref other => panic!("expected a pair, got {other:?}"),
    }
}

fn string(value: &Value) -> JsString {
    JsString::from_units(value.as_str().expect("a string").units().to_vec())
}

fn get<'a>(value: &'a Value, key: &str) -> &'a Value {
    let Value::Object(pairs) = value else {
        panic!("expected an object, got {}", value.kind());
    };
    pairs
        .iter()
        .find(|(k, _)| k.as_string().as_deref() == Some(key))
        .map(|(_, v)| v)
        .unwrap_or_else(|| panic!("no {key}"))
}

fn keys(value: &Value) -> Vec<String> {
    let Value::Object(pairs) = value else {
        panic!("expected an object, got {}", value.kind());
    };
    pairs.iter().map(|(k, _)| k.as_string().unwrap()).collect()
}

fn same(expected: f64, actual: f64) -> bool {
    expected.to_bits() == actual.to_bits() || (expected.is_nan() && actual.is_nan())
}

fn assert_numbers(expected: &[f64], actual: &[f64], what: &str) {
    assert!(
        expected.len() == actual.len() && expected.iter().zip(actual).all(|(&e, &a)| same(e, a)),
        "{what}: expected {expected:?}, got {actual:?}"
    );
}

fn command(value: &Value) -> HistogramCommand {
    assert_eq!(
        keys(value),
        ["changeCounter", "bins", "stepSize", "fields"],
        "command keys"
    );
    HistogramCommand {
        change_counter: number(get(value, "changeCounter")),
        bins: numbers(get(value, "bins")),
        step_size: number(get(value, "stepSize")),
        fields: items(get(value, "fields"))
            .iter()
            .map(|f| HistogramField {
                title: string(get(f, "title")),
                color: string(get(f, "color")).to_string_lossy(),
                counts: numbers(get(f, "histogramCounts")),
            })
            .collect(),
    }
}

fn check_state(expected: &Value, chart: &Histogram, what: &str) {
    assert_eq!(
        keys(expected),
        ["size", "area", "scales", "legend", "bars"],
        "{what}: state keys"
    );
    let (width, height) = chart.size();
    assert_numbers(
        &numbers(get(expected, "size")),
        &[width, height, chart.device_pixel_ratio()],
        &format!("{what}: size"),
    );
    match (get(expected, "area"), chart.area()) {
        (Value::Undefined, None) => assert!(keys(get(expected, "scales")).is_empty()),
        (area, Some(a)) => {
            assert_numbers(
                &numbers(area),
                &[a.left, a.top, a.right, a.bottom, a.width, a.height],
                &format!("{what}: chart area"),
            );
            let scales = get(expected, "scales");
            assert_eq!(keys(scales), ["x", "y"], "{what}: scales");
            for (id, actual) in ["x", "y"].into_iter().zip(chart.scales()) {
                check_scale(get(scales, id), &actual, &format!("{what}: {id} scale"));
            }
            assert_numbers(
                &numbers(get(expected, "legend")),
                &chart.legend(),
                &format!("{what}: legend"),
            );
        }
        (area, None) => panic!("{what}: no chart area, expected {area:?}"),
    }
    let expected = items(get(expected, "bars"));
    let actual = chart.bars();
    assert_eq!(expected.len(), actual.len(), "{what}: datasets");
    for (k, (expected, actual)) in expected.iter().zip(&actual).enumerate() {
        let expected = items(expected);
        assert_eq!(expected.len(), actual.len(), "{what}: bars of {k}");
        for (i, (e, a)) in expected.iter().zip(actual).enumerate() {
            assert_numbers(&numbers(e), a, &format!("{what}: bar {i} of {k}"));
        }
    }
}

fn check_scale(expected: &Value, actual: &ScaleLayout, what: &str) {
    assert_eq!(
        keys(expected),
        ["box", "range", "rotation", "ticks"],
        "{what}: keys"
    );
    let bounds = numbers(get(expected, "box"));
    assert_numbers(&bounds, &actual.bounds, &format!("{what} box"));
    let range = numbers(get(expected, "range"));
    assert_numbers(&range, &actual.range, &format!("{what} range"));
    assert_numbers(
        &[number(get(expected, "rotation"))],
        &[actual.label_rotation],
        &format!("{what} rotation"),
    );
    let ticks = items(get(expected, "ticks"));
    assert_eq!(ticks.len(), actual.ticks.len(), "{what}: tick count");
    for (i, (e, (value, label))) in ticks.iter().zip(&actual.ticks).enumerate() {
        let [expected, text] = items(e) else {
            panic!("{what}: tick {i} is not a pair");
        };
        assert_numbers(&[number(expected)], &[*value], &format!("{what} tick {i}"));
        assert_eq!(
            string(text).to_string_lossy(),
            *label,
            "{what}: label of tick {i}"
        );
    }
}

fn json_kind(json: &Json) -> String {
    json.as_str()
        .and_then(Utf16String::as_string)
        .expect("a string")
}

fn boolean(record: &Record, key: &str) -> bool {
    record
        .value(key)
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or_else(|| panic!("no boolean {key}"))
}

fn error_text(e: &JsError) -> String {
    format!("{}: {}", e.name, e.message)
}

type Callback = (Work, Result<(), JsError>, bool, Vec<Call>);

struct Replay {
    id: String,
    text: FixedAdvance,
    chart: Option<Histogram>,
    command: Option<HistogramCommand>,
    now: f64,
    /// The current frame's time, how many of its callbacks run after the hub's own, and how many
    /// of the queued ones the next frame runs before them.
    frame: f64,
    rest: usize,
    first: usize,
    work: Vec<Callback>,
    tally: BTreeMap<&'static str, usize>,
    calls: usize,
    /// Every call since the canvas was last reset.
    since_reset: Vec<Call>,
}

impl Replay {
    fn chart(&mut self) -> &mut Histogram {
        self.chart.as_mut().expect("a chart was created")
    }

    /// Runs `body` on the chart with the recorded `Date.now()`.
    fn run<T>(&mut self, body: impl FnOnce(&mut Histogram, &mut Env<'_>) -> T) -> T {
        let text: &mut dyn TextBackend = &mut self.text;
        let mut env = Env {
            text,
            now: self.now,
        };
        body(self.chart.as_mut().expect("a chart was created"), &mut env)
    }

    fn check(
        &mut self,
        record: &Record,
        result: &Result<(), JsError>,
        (reset, calls): (bool, Vec<Call>),
        what: &str,
    ) {
        let what = format!("{} {what}", self.id);
        let threw = record.value("threw").ok().map(|v| string(&v));
        assert_eq!(
            threw.map(|t| t.to_string_lossy()),
            result.as_ref().err().map(error_text),
            "{what}: exception"
        );
        let expected_reset = boolean(record, "reset");
        assert_eq!(expected_reset, reset, "{what}: canvas reset");
        self.check_calls(record, &calls, &what);
        if reset {
            self.since_reset.clear();
        }
        self.since_reset.extend(calls);
    }

    fn check_calls(&mut self, record: &Record, calls: &[Call], what: &str) {
        let recorded = record
            .field("calls")
            .and_then(Json::as_array)
            .expect("calls");
        let expected: Vec<String> = recorded.iter().map(Json::to_compact).collect();
        let actual = trace::encode(calls);
        let actual: Vec<&str> = actual.lines().collect();
        if let Some(i) = (0..expected.len().max(actual.len()))
            .find(|&i| expected.get(i).map(String::as_str) != actual.get(i).copied())
        {
            panic!(
                "{what}: call {i} of {}: expected {:?}, got {:?} ({} calls)",
                expected.len(),
                expected.get(i),
                actual.get(i),
                actual.len()
            );
        }
        self.calls += calls.len();
        let canvas = pair(&record.value("canvas").expect("canvas"));
        let chart = self.chart();
        let size = (
            f64::from(chart.canvas().width),
            f64::from(chart.canvas().height),
        );
        assert_eq!(canvas, size, "{what}: canvas size");
    }

    fn check_state(&mut self, record: &Record, what: &str) {
        if let Ok(state) = record.value("state") {
            let what = format!("{} {what}", self.id);
            check_state(&state, self.chart(), &what);
            *self.tally.entry("state").or_default() += 1;
        }
    }

    fn no_pending_work(&self, what: &str) {
        let left: Vec<Work> = self.work.iter().map(|w| w.0).collect();
        assert!(
            left.is_empty(),
            "{} {what}: port also ran {left:?}",
            self.id
        );
    }

    fn run_callbacks(&mut self, count: usize, what: &str) {
        for _ in 0..count {
            let (work, result) = self
                .run(|chart, env| chart.run_callback(env))
                .expect("a queued callback");
            let (reset, calls) = self.chart().take_calls();
            if work == Work::Idle {
                assert!(calls.is_empty(), "{} {what}: idle frame drew", self.id);
                continue;
            }
            self.work.push((work, result, reset, calls));
        }
    }

    fn frame_work(&mut self, record: &Record, kind: Work, what: &str) {
        assert!(
            !self.work.is_empty(),
            "{} {what}: port did nothing",
            self.id
        );
        let (work, result, reset, calls) = self.work.remove(0);
        assert_eq!(work, kind, "{} {what}", self.id);
        self.check(record, &result, (reset, calls), what);
    }

    fn record(&mut self, n: usize, record: &Record) {
        let Some(kind) = record.field("hist") else {
            self.no_pending_work(&format!("record {n}"));
            let key = OTHER_KINDS
                .iter()
                .find(|k| record.field(k).is_some())
                .unwrap_or_else(|| panic!("{} record {n}: unknown record", self.id));
            *self.tally.entry(key).or_default() += 1;
            return;
        };
        let kind = json_kind(kind);
        let what = format!("record {n} ({kind})");
        let value = |key: &str| {
            record
                .value(key)
                .unwrap_or_else(|e| panic!("{what}: {key}: {e}"))
        };
        if kind != "create" && kind != "frame" {
            assert_eq!(number(&value("id")), 0.0, "{what}: one chart per fixture");
        }
        let attach = kind == "resize" && !self.chart.as_ref().is_some_and(Histogram::is_attached);
        if let Ok(t) = record.value("t") {
            self.now = number(&t);
            if self.now > self.frame && !attach {
                self.no_pending_work(&what);
                self.frame = self.now;
                let queued = self.chart.as_ref().map_or(0, Histogram::callbacks);
                self.rest = queued - self.first;
                let first = std::mem::take(&mut self.first);
                self.run_callbacks(first, &what);
            }
            assert!(attach || self.now == self.frame, "{what}: not in its frame");
        }
        let in_frame = matches!(kind.as_str(), "event" | "draw" | "resize") && !attach;
        if !in_frame {
            self.no_pending_work(&what);
        }
        let key = match kind.as_str() {
            "create" => {
                assert!(self.chart.is_none(), "{what}: one chart per fixture");
                let mut chart = Histogram::new(number(&value("dpr")));
                let calls = chart.canvas().calls.clone();
                chart.record_calls();
                self.chart = Some(chart);
                let what = format!("{} {what}", self.id);
                self.check_calls(record, &calls, &what);
                "create"
            }
            "resize" if attach => {
                assert_eq!(
                    items(&value("args")),
                    [Value::Undefined, Value::Undefined],
                    "{what}: the attach resize has no size"
                );
                let container = pair(&value("container"));
                let result = self.run(|chart, env| chart.attach(container, env));
                let calls = self.chart().take_calls();
                self.check(record, &result, calls, &what);
                self.check_state(record, &what);
                "attach"
            }
            "resize" => {
                self.frame_work(record, Work::Resize, &what);
                self.check_state(record, &what);
                "resize"
            }
            "event" => {
                self.frame_work(record, Work::Event, &what);
                "event"
            }
            "draw" => {
                self.frame_work(record, Work::Draw, &what);
                "draw"
            }
            "render" => {
                if let Ok(value) = record.value("command") {
                    self.command = Some(command(&value));
                }
                let dark = boolean(record, "dark");
                let command = self.command.clone().expect("a command");
                let result = self.run(|chart, env| chart.render(&command, dark, env));
                let calls = self.chart().take_calls();
                self.check(record, &result, calls, &what);
                if result.is_ok() {
                    self.check_state(record, &what);
                }
                "render"
            }
            "observe" => {
                let size = pair(&value("size"));
                self.chart().container_resized(size);
                "observe"
            }
            "dpr" => {
                let dpr = number(&value("dpr"));
                self.chart().window_resized(dpr);
                "dpr"
            }
            "dom" => {
                let kind = match json_kind(record.field("type").expect("type")).as_str() {
                    "mousemove" => EventKind::MouseMove,
                    "mouseout" => EventKind::MouseOut,
                    "click" => EventKind::Click,
                    other => panic!("{what}: unknown event {other}"),
                };
                let (x, y) = pair(&value("at"));
                self.chart().pointer(kind, x, y);
                "dom"
            }
            "frame" => {
                if self.chart.is_some() {
                    self.first = self.chart().callbacks() - self.rest;
                    let rest = std::mem::take(&mut self.rest);
                    self.run_callbacks(rest, &what);
                }
                "frame"
            }
            other => panic!("{} record {n}: unknown kind {other}", self.id),
        };
        *self.tally.entry(key).or_default() += 1;
    }
}

fn replay(id: &str) -> Replay {
    let vector = load(id).unwrap_or_else(|e| panic!("{id}: {e}"));
    let mut replay = Replay {
        id: id.to_owned(),
        text: FixedAdvance,
        chart: None,
        command: None,
        now: f64::NAN,
        frame: f64::NEG_INFINITY,
        rest: 0,
        first: 0,
        work: Vec::new(),
        tally: BTreeMap::new(),
        calls: 0,
        since_reset: Vec::new(),
    };
    for (n, record) in vector.iter().enumerate() {
        replay.record(n, record);
    }
    replay.no_pending_work("end");
    let compared: usize = replay.tally.values().sum();
    let states = vector
        .iter()
        .filter(|r| r.field("state").is_some() && r.field("threw").is_none())
        .count();
    assert_eq!(compared, vector.len() + states, "{id}: compared records");
    replay
}

#[test]
fn charts_draw_what_upstream_draws() {
    let mut total: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut calls = 0;
    for id in case_ids() {
        let replay = replay(&id);
        for (kind, n) in replay.tally {
            *total.entry(kind).or_default() += n;
        }
        calls += replay.calls;
    }
    println!("{total:?}, {calls} calls");
    for kind in [
        "create",
        "attach",
        "resize",
        "render",
        "event",
        "draw",
        "observe",
        "dpr",
        "dom",
        "state",
        "exception",
    ] {
        assert!(total.contains_key(kind), "no {kind} record in any vector");
    }
}

#[test]
fn the_kept_display_list_paints_what_the_whole_history_paints() {
    for id in case_ids() {
        let mut replay = replay(&id);
        let since_reset = std::mem::take(&mut replay.since_reset);
        let canvas = replay.chart().canvas();
        if canvas.width == 0 || canvas.height == 0 {
            continue;
        }
        let paint = |calls: &[Call]| {
            let mut raster = Raster::new(canvas.width, canvas.height).expect("a canvas");
            raster.run(calls);
            raster.into_pixmap()
        };
        assert!(canvas.calls.len() <= since_reset.len(), "{id}");
        assert!(
            paint(&canvas.calls) == paint(&since_reset),
            "{id}: the kept calls paint something else"
        );
    }
}
