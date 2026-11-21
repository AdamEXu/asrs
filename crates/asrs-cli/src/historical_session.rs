//! A page around `HistoricalDataSource` that orders work as the oracle's does
//! (oracle/src/historicalSession.ts and oracle/src/historical/scheduler.ts): timeouts first, in
//! the order they were set; otherwise the oldest busy worker's next message. Intervals and
//! animation frames run only when a step fires them, and main is emulated as
//! oracle/src/historical/mainProcess.ts does.
//!
//! `asrs-cli historical-session <sessions.jsonl>` runs scripted sessions and prints the same
//! trace as `oracle historicalSession`.

use std::collections::VecDeque;
use std::process::ExitCode;
use std::sync::Arc;

use asrs_formats::historical::{
    HistoricalDataSource, MainReply, MainRequest, PageContext, SourceEvent, SourceHost, Status,
    Timeout, WorkerName,
};
use asrs_formats::worker::{Abort, DecodeWorker, Request, WorkerMessage, WorkerThread};
use asrs_js::string::JsString;
use asrs_js::time::{Clock, Rng, SetClock, V8MathRandom};
use asrs_log::log::Log;
use asrs_log::realm::JsRealm;
use asrs_parity::Value;
use asrs_parity::json::{self, Json};
use asrs_services::historical_start;

use crate::decode::{WORKER_FROZEN_MS, dump_log};
use crate::log_field::string_value;

const USAGE: &str = "usage: asrs-cli historical-session <sessions.jsonl>";

const PAGE_EPOCH: f64 = 1_700_000_000_000.0;

const CONVERTED: [&str; 3] = [".hoot", ".revlog", ".wpilogxz"];

enum Queued {
    Source(Timeout),
    Main(MainReply),
}

struct Slot {
    source: String,
    thread: WorkerThread,
    pending: usize,
    posted: usize,
    terminated: bool,
    hung: bool,
}

struct Host {
    now: f64,
    timeouts: VecDeque<Queued>,
    intervals: Vec<String>,
    frames: Vec<String>,
    slots: Vec<Slot>,
    active: Vec<JsString>,
    root: usize,
    statuses: Vec<Status>,
    loaded: Vec<usize>,
    trace: Option<Vec<String>>,
}

impl Host {
    fn trace(&mut self, value: Value) {
        if let Some(lines) = &mut self.trace {
            lines.push(value.encode());
        }
    }

    fn slot(&mut self, source: &str) -> Option<(usize, &mut Slot)> {
        self.slots
            .iter_mut()
            .enumerate()
            .rfind(|(_, slot)| slot.source == source)
    }

    /// `historical-start-raw` in main: the bytes go to `child_<uuid>.<extension>`, which is then
    /// read like a file with nothing beside it.
    fn start_raw(&mut self, uuid: String, data: Arc<[u8]>, extension: &JsString) -> MainReply {
        let error = |message: &str| MainReply {
            uuid: uuid.clone(),
            files: vec![None],
            error: Some(JsString::from(message)),
        };
        let units = extension.units();
        let has = |s: &str| extension.includes(&JsString::from(s), None);
        if has("/") || has("\\") || has("..") {
            return error("Invalid log extension");
        }
        let mut temp_path = JsString::from(format!("child_{uuid}.").as_str());
        temp_path.push_str(&JsString::from_units(units.to_vec()));
        let ends = |s: &str| temp_path.ends_with(&JsString::from(s), None);
        if CONVERTED.iter().any(|e| ends(e)) {
            return error("oracle: conversion unavailable");
        }
        let files = if ends(".dslog") {
            vec![Some(data), None]
        } else {
            vec![Some(data)]
        };
        MainReply {
            uuid,
            files,
            error: None,
        }
    }
}

impl SourceHost for Host {
    fn now(&self) -> f64 {
        self.now
    }

    fn send_main_message(&mut self, request: MainRequest) {
        let reply = match request {
            MainRequest::HistoricalStart { uuid, path } => {
                self.trace(Value::object([
                    ("event", Value::string("main")),
                    ("name", Value::string("historical-start")),
                    ("path", string_value(&path)),
                ]));
                let files = historical_start::read_files(&path.to_string_lossy());
                MainReply {
                    uuid,
                    files: files.into_iter().map(|f| f.map(Arc::from)).collect(),
                    error: None,
                }
            }
            MainRequest::HistoricalStartRaw {
                uuid,
                data,
                extension,
            } => {
                self.trace(Value::object([
                    ("event", Value::string("main")),
                    ("name", Value::string("historical-start-raw")),
                    ("extension", string_value(&extension)),
                    ("bytes", Value::number(data.len() as f64)),
                ]));
                self.start_raw(uuid, data, &extension)
            }
        };
        self.timeouts.push_back(Queued::Main(reply));
    }

    fn set_timeout(&mut self, timeout: Timeout) {
        self.timeouts.push_back(Queued::Source(timeout));
    }

    fn set_interval(&mut self, source: &str) {
        self.intervals.push(source.to_owned());
    }

    fn clear_interval(&mut self, source: &str) {
        self.intervals.retain(|s| s != source);
    }

    fn request_animation_frame(&mut self, source: &str) {
        self.frames.push(source.to_owned());
    }

    fn worker_realm(&mut self) -> (Box<dyn Rng + Send>, Box<dyn Clock + Send>) {
        (
            Box::new(V8MathRandom::from_entropy()),
            Box::new(SetClock(WORKER_FROZEN_MS)),
        )
    }

    fn spawn_worker(&mut self, source: &str, name: WorkerName, worker: Box<dyn DecodeWorker>) {
        let thread = WorkerThread::spawn(source, worker).expect("spawn a decode thread");
        self.trace(Value::object([
            ("event", Value::string("worker")),
            ("worker", Value::number(self.slots.len() as f64)),
            ("script", Value::string(name.script())),
        ]));
        self.slots.push(Slot {
            source: source.to_owned(),
            thread,
            pending: 0,
            posted: 0,
            terminated: false,
            hung: false,
        });
    }

    fn post_worker(&mut self, source: &str, request: Request) {
        let Some((index, _)) = self.slot(source) else {
            return;
        };
        let line = match &request {
            Request::Start(files) => Value::object([
                ("event", Value::string("post")),
                ("worker", Value::number(index as f64)),
                ("type", Value::string("start")),
                (
                    "files",
                    Value::Array(
                        files
                            .iter()
                            .map(|f| {
                                f.as_ref()
                                    .map_or(Value::Null, |f| Value::number(f.len() as f64))
                            })
                            .collect(),
                    ),
                ),
            ]),
            Request::ParseField(key) => Value::object([
                ("event", Value::string("post")),
                ("worker", Value::number(index as f64)),
                ("type", Value::string("parseField")),
                ("key", string_value(key)),
            ]),
        };
        self.trace(line);
        let slot = &mut self.slots[index];
        if !slot.terminated && !slot.hung {
            slot.pending += 1;
            slot.posted += 1;
            slot.thread.post(request);
        }
    }

    fn terminate_worker(&mut self, source: &str) {
        let Some((index, slot)) = self.slot(source) else {
            return;
        };
        slot.terminated = true;
        slot.thread.terminate();
        self.trace(Value::object([
            ("event", Value::string("terminate")),
            ("worker", Value::number(index as f64)),
        ]));
    }

    fn active_fields(&mut self) -> Vec<JsString> {
        self.active.clone()
    }

    fn event(&mut self, event: SourceEvent) {
        let source = ("source", Value::number(self.root as f64));
        let line = match event {
            SourceEvent::Status {
                status,
                custom_error,
            } => {
                self.statuses[self.root] = status;
                let mut pairs = vec![
                    ("event", Value::string("status")),
                    source,
                    ("status", Value::string(status.name())),
                ];
                if status == Status::Error {
                    pairs.push((
                        "customError",
                        custom_error.as_ref().map_or(Value::Null, string_value),
                    ));
                }
                Value::object(pairs)
            }
            SourceEvent::Progress(value) => Value::object([
                ("event", Value::string("progress")),
                source,
                ("value", Value::number(value)),
            ]),
            SourceEvent::Refresh { has_new_fields } => Value::object([
                ("event", Value::string("refresh")),
                source,
                ("hasNewFields", Value::Bool(has_new_fields)),
            ]),
            SourceEvent::AllFieldsLoaded => {
                self.loaded.push(self.root);
                return;
            }
        };
        self.trace(line);
    }
}

/// How a session ends early.
pub(crate) enum End {
    Unmodeled(&'static str),
    /// A worker never returns and the session was made with `stop_on_hang`.
    NeverReturns,
}

pub(crate) struct Session {
    host: Host,
    roots: Vec<HistoricalDataSource>,
    open: Vec<usize>,
    log: Log,
    realm: JsRealm,
    stop_on_hang: bool,
}

impl Session {
    /// `stop_on_hang` ends `settle` with `End::NeverReturns` where a worker never returns, where
    /// the oracle's `decode` would wait until its timeout; otherwise the worker just goes quiet,
    /// as in `oracle historicalSession`.
    pub(crate) fn new(trace: bool, stop_on_hang: bool) -> Self {
        Session {
            host: Host {
                now: PAGE_EPOCH,
                timeouts: VecDeque::new(),
                intervals: Vec::new(),
                frames: Vec::new(),
                slots: Vec::new(),
                active: Vec::new(),
                root: 0,
                statuses: Vec::new(),
                loaded: Vec::new(),
                trace: trace.then(Vec::new),
            },
            roots: Vec::new(),
            open: Vec::new(),
            log: Log::new(true),
            realm: JsRealm::new(Box::new(V8MathRandom::from_entropy())),
            stop_on_hang,
        }
    }

    fn with_root<R>(
        &mut self,
        root: usize,
        f: impl FnOnce(&mut HistoricalDataSource, &mut PageContext<'_>) -> R,
    ) -> R {
        self.host.root = root;
        let mut cx = PageContext {
            log: &mut self.log,
            realm: &mut self.realm,
            host: &mut self.host,
        };
        f(&mut self.roots[root], &mut cx)
    }

    fn route(&self, uuid: &str) -> Option<usize> {
        self.roots.iter().position(|root| root.contains(uuid))
    }

    /// The `.then` of every `loadAllFields()` promise that resolved during the last task.
    fn microtasks(&mut self) {
        for root in std::mem::take(&mut self.host.loaded) {
            self.host.trace(Value::object([
                ("event", Value::string("loaded")),
                ("source", Value::number(root as f64)),
            ]));
        }
    }

    fn page_result(&mut self, result: Result<(), Abort>) -> Result<(), End> {
        match result {
            Ok(()) => Ok(()),
            Err(Abort::Threw) => {
                self.host
                    .trace(Value::object([("event", Value::string("threw"))]));
                Ok(())
            }
            Err(Abort::NeverReturns) => Err(End::Unmodeled("the page realm never returns")),
            Err(Abort::Unmodeled(what)) => Err(End::Unmodeled(what)),
        }
    }

    pub(crate) fn open(&mut self, path: &JsString, prefix: &JsString) -> usize {
        let index = self.roots.len();
        self.roots.push(HistoricalDataSource::new(&mut self.realm));
        self.host.statuses.push(Status::Waiting);
        self.open.push(index);
        self.with_root(index, |source, cx| source.open_file(path, prefix, cx));
        self.microtasks();
        index
    }

    pub(crate) fn status(&self, root: usize) -> Status {
        self.host.statuses[root]
    }

    pub(crate) fn source(&self, root: usize) -> &HistoricalDataSource {
        &self.roots[root]
    }

    pub(crate) fn log(&self) -> &Log {
        &self.log
    }

    pub(crate) fn settle(&mut self) -> Result<(), End> {
        loop {
            if let Some(task) = self.host.timeouts.pop_front() {
                match task {
                    Queued::Source(timeout) => {
                        if let Some(root) = self.route(timeout.source()) {
                            self.with_root(root, |source, cx| source.run_timeout(timeout, cx));
                        }
                    }
                    Queued::Main(reply) => {
                        for root in self.open.clone() {
                            self.with_root(root, |source, cx| {
                                source.handle_main_message(&reply, cx)
                            });
                        }
                    }
                }
                self.microtasks();
                continue;
            }
            let Some(index) = self
                .host
                .slots
                .iter()
                .position(|s| !s.terminated && !s.hung && s.pending > 0)
            else {
                return Ok(());
            };
            let slot = &mut self.host.slots[index];
            match slot.thread.recv() {
                Some(WorkerMessage::Response(response)) => {
                    let uuid = slot.source.clone();
                    if let Some(root) = self.route(&uuid) {
                        let result = self.with_root(root, |source, cx| {
                            source.worker_message(&uuid, response, cx)
                        });
                        self.page_result(result)?;
                    }
                    self.microtasks();
                }
                Some(WorkerMessage::Done(end)) => {
                    slot.pending -= 1;
                    match end {
                        Err(Abort::NeverReturns) if self.stop_on_hang => {
                            return Err(End::NeverReturns);
                        }
                        Err(Abort::NeverReturns) => slot.hung = true,
                        Err(Abort::Unmodeled(what)) => return Err(End::Unmodeled(what)),
                        Ok(()) | Err(Abort::Threw) => {}
                    }
                }
                None => slot.hung = true,
            }
        }
    }

    pub(crate) fn tick(&mut self) {
        for uuid in self.host.intervals.clone() {
            if !self.host.intervals.contains(&uuid) {
                continue;
            }
            if let Some(root) = self.route(&uuid) {
                self.with_root(root, |source, cx| source.fire_interval(&uuid, cx));
            }
            self.microtasks();
        }
    }

    fn frame(&mut self, ms: f64) {
        self.host.now += ms;
        for uuid in std::mem::take(&mut self.host.frames) {
            if let Some(root) = self.route(&uuid) {
                self.with_root(root, |source, cx| source.animation_frame(&uuid, cx));
            }
            self.microtasks();
        }
    }

    pub(crate) fn load_all(&mut self) {
        for root in self.open.clone() {
            if self.with_root(root, |source, cx| source.load_all_fields(cx)) {
                self.host.loaded.push(root);
            }
        }
        self.microtasks();
    }

    fn activity(&self) -> (usize, usize) {
        (
            self.host.slots.len(),
            self.host.slots.iter().map(|s| s.posted).sum(),
        )
    }

    /// Ticks until a round does nothing, loads all fields, and repeats until that does nothing
    /// either (oracle/src/historical/session.ts `loadEverything`).
    pub(crate) fn load_everything(&mut self) -> Result<(), End> {
        loop {
            let before = self.activity();
            loop {
                let tick_start = self.activity();
                self.tick();
                self.settle()?;
                if self.activity() == tick_start {
                    break;
                }
            }
            self.load_all();
            self.settle()?;
            if self.activity() == before {
                return Ok(());
            }
        }
    }

    fn stop(&mut self, which: Option<usize>) {
        let stopped = match which {
            Some(root) => vec![root],
            None => self.open.clone(),
        };
        if which.is_none() {
            self.open.clear();
        }
        for root in stopped {
            self.with_root(root, |source, cx| source.stop(cx));
        }
        self.microtasks();
    }

    fn pick(&self, indices: &[f64], parents: &[f64]) -> Vec<JsString> {
        let keys = self.log.get_field_keys();
        if keys.is_empty() {
            return Vec::new();
        }
        let n = keys.len() as i64;
        let at = |i: f64| &keys[(i as i64).rem_euclid(n) as usize];
        let mut picked: Vec<JsString> = indices.iter().map(|&i| at(i).clone()).collect();
        let slash = JsString::from("/");
        for &i in parents {
            let key = at(i);
            let index = key.last_index_of(&slash, None);
            if index > 0 {
                picked.push(key.slice(0.0, Some(index as f64)));
            }
        }
        picked
    }

    fn dump(&self, phase: &str) -> Vec<String> {
        let mut loading: Vec<JsString> = Vec::new();
        for &root in &self.open {
            for field in self.roots[root].get_loading_fields() {
                if !loading.contains(&field) {
                    loading.push(field);
                }
            }
        }
        let statuses: Vec<&str> = self.host.statuses.iter().map(|s| s.name()).collect();
        dump_log(
            &self.log.to_serialized(),
            phase,
            &statuses.join(","),
            &loading,
        )
    }

    fn step(&mut self, index: usize, step: &Json) -> Result<(), StepError> {
        let op = text(step, "op")?;
        self.host.trace(Value::object([
            ("step", Value::number(index as f64)),
            ("op", Value::string(&op)),
        ]));
        match op.as_str() {
            "open" => {
                let path = js_text(step, "path")?;
                let prefix = js_text(step, "prefix")?;
                self.open(&path, &prefix);
            }
            "settle" => {}
            "tick" => self.tick(),
            "frame" => self.frame(number(step, "ms")?),
            "pick" => {
                self.host.active =
                    self.pick(&numbers(step, "indices")?, &numbers(step, "parents")?);
                let fields = Value::Array(self.host.active.iter().map(string_value).collect());
                self.host.trace(Value::object([
                    ("event", Value::string("active")),
                    ("fields", fields),
                ]));
            }
            "loadAll" => self.load_all(),
            "loadEverything" => return Ok(self.load_everything()?),
            "stop" => self.stop(
                step.get("source")
                    .and_then(Json::as_f64)
                    .map(|s| s as usize),
            ),
            "dump" => {
                let lines = self.dump(&text(step, "phase")?);
                if let Some(trace) = &mut self.host.trace {
                    trace.extend(lines);
                }
                return Ok(());
            }
            other => return Err(format!("unknown op {other:?}").into()),
        }
        if !matches!(step.get("settle"), Some(Json::Bool(false))) {
            self.settle()?;
        }
        Ok(())
    }

    fn take_trace(&mut self) -> Vec<String> {
        self.host
            .trace
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        for slot in &mut self.host.slots {
            slot.thread.terminate();
        }
    }
}

enum StepError {
    End(End),
    Invalid(String),
}

impl From<End> for StepError {
    fn from(end: End) -> Self {
        StepError::End(end)
    }
}

impl From<String> for StepError {
    fn from(message: String) -> Self {
        StepError::Invalid(message)
    }
}

fn text(step: &Json, key: &str) -> Result<String, String> {
    js_text(step, key).map(|s| s.to_string_lossy())
}

fn js_text(step: &Json, key: &str) -> Result<JsString, String> {
    step.get(key)
        .and_then(Json::as_str)
        .map(|s| JsString::from_units(s.units().to_vec()))
        .ok_or_else(|| format!("step needs a string {key:?}"))
}

fn number(step: &Json, key: &str) -> Result<f64, String> {
    step.get(key)
        .and_then(Json::as_f64)
        .ok_or_else(|| format!("step needs a number {key:?}"))
}

fn numbers(step: &Json, key: &str) -> Result<Vec<f64>, String> {
    step.get(key)
        .and_then(Json::as_array)
        .ok_or_else(|| format!("step needs an array {key:?}"))?
        .iter()
        .map(|v| {
            v.as_f64()
                .ok_or_else(|| format!("{key:?} must hold numbers"))
        })
        .collect()
}

pub fn run(args: &[String]) -> ExitCode {
    let [file] = args else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("{file}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let mut out = String::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let spec = match json::parse(line) {
            Ok(spec) => spec,
            Err(error) => {
                eprintln!("{file}: {error}");
                return ExitCode::FAILURE;
            }
        };
        let name = spec
            .get("name")
            .and_then(Json::as_str)
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        let mut lines = vec![Value::object([("session", Value::string(&name))]).encode()];
        let mut session = Session::new(true, false);
        let steps = spec
            .get("steps")
            .and_then(Json::as_array)
            .unwrap_or_default();
        for (i, step) in steps.iter().enumerate() {
            let result = session.step(i, step);
            lines.extend(session.take_trace());
            if let Err(error) = result {
                let line = match error {
                    StepError::End(End::Unmodeled(what)) => {
                        Value::object([("unmodeled", Value::string(what))])
                    }
                    StepError::End(End::NeverReturns) => {
                        Value::object([("error", Value::string("a worker never returns"))])
                    }
                    StepError::Invalid(message) => {
                        Value::object([("error", Value::string(&message))])
                    }
                };
                lines.push(line.encode());
                break;
            }
        }
        drop(session);
        lines.push(Value::object([("end", Value::string(&name))]).encode());
        for line in lines {
            out.push_str(&line);
            out.push('\n');
        }
    }
    print!("{out}");
    ExitCode::SUCCESS
}
