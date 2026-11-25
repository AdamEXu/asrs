//! `HistoricalDataSource.ts`: reads a log through main, decodes it on a worker, merges the
//! worker's initial log into the hub's `Log` under a key prefix and requests fields lazily.
//!
//! Everything upstream does through browser APIs goes through [`SourceHost`]: main-process
//! messages, `setTimeout(.., 0)`, the 50 ms `setInterval`, `requestAnimationFrame`, `Date` and
//! the worker. The host hands each timer, frame and worker message back to the source tree
//! through the matching method of the root source, in the order it decides.

use std::collections::HashSet;
use std::sync::Arc;

use asrs_js::string::JsString;
use asrs_js::time::{Clock, Rng};
use asrs_log::log::log_key_utils::{
    apply_key_prefix, get_child_log_extension, is_child_log, is_merge_prefix, is_schema,
};
use asrs_log::log::log_util::get_urcl_keys;
use asrs_log::log::robot_state::{
    AKIT_TIMESTAMP_KEYS, ALLIANCE_KEYS, AUTONOMOUS_KEYS, DRIVER_STATION_KEYS, ENABLED_KEYS,
    EVENT_KEYS, MATCH_NUMBER_KEYS, MATCH_TYPE_KEYS, SYSTEM_TIME_KEYS, UTILITY_KEYS,
};
use asrs_log::log::{Log, LogError, LogField, LoggableType, RawValue};
use asrs_log::realm::JsRealm;
use asrs_log::util::{calc_mock_progress, create_uuid, scale_value};
use indexmap::{IndexMap, IndexSet};

use crate::csv::CsvWorker;
use crate::dslog::DsLogWorker;
use crate::rlog::RlogWorker;
use crate::roadrunner::RoadRunnerWorker;
use crate::worker::{Abort, DecodeWorker, Request, Response};
use crate::wpilog_worker::WpilogWorker;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Waiting,
    Reading,
    DecodingInitial,
    DecodingField,
    Idle,
    Error,
    Stopped,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Waiting => "Waiting",
            Status::Reading => "Reading",
            Status::DecodingInitial => "DecodingInitial",
            Status::DecodingField => "DecodingField",
            Status::Idle => "Idle",
            Status::Error => "Error",
            Status::Stopped => "Stopped",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerName {
    Rlog,
    Wpilog,
    RoadRunner,
    Csv,
    DsLog,
}

impl WorkerName {
    /// The bundle name upstream passes to `new Worker`.
    pub fn script(self) -> &'static str {
        match self {
            WorkerName::Rlog => "rlogWorker.js",
            WorkerName::Wpilog => "wpilogWorker.js",
            WorkerName::RoadRunner => "roadRunnerWorker.js",
            WorkerName::Csv => "csvWorker.js",
            WorkerName::DsLog => "dsLogWorker.js",
        }
    }
}

const WORKER_NAMES: [(&str, WorkerName); 9] = [
    (".rlog", WorkerName::Rlog),
    (".wpilog", WorkerName::Wpilog),
    (".wpilogxz", WorkerName::Wpilog),
    (".hoot", WorkerName::Wpilog),
    (".revlog", WorkerName::Wpilog),
    (".log", WorkerName::RoadRunner),
    (".csv", WorkerName::Csv),
    (".dslog", WorkerName::DsLog),
    (".dsevents", WorkerName::DsLog),
];

/// The last matching entry wins, as upstream's `forEach` over `Object.entries` overwrites.
pub fn select_worker(path: &JsString) -> Option<WorkerName> {
    WORKER_NAMES
        .iter()
        .rev()
        .find(|(extension, _)| path.ends_with(&JsString::from(*extension), None))
        .map(|&(_, name)| name)
}

/// `new Worker("../bundles/" + selectedWorkerName)`: a fresh worker realm with its own
/// `Math.random` stream and `Date`.
pub fn create_worker(
    name: WorkerName,
    random: Box<dyn Rng + Send>,
    clock: Box<dyn Clock + Send>,
) -> Box<dyn DecodeWorker> {
    match name {
        WorkerName::Wpilog => Box::new(WpilogWorker::new(JsRealm::new(random))),
        WorkerName::Csv => Box::new(CsvWorker),
        WorkerName::Rlog => Box::new(RlogWorker::new(JsRealm::new(random), clock)),
        WorkerName::DsLog => Box::new(DsLogWorker),
        WorkerName::RoadRunner => Box::new(RoadRunnerWorker::new(clock)),
    }
}

/// A message the page sends to main.
pub enum MainRequest {
    /// `historical-start`: read the file at `path` (and a `.dslog`'s `.dsevents`).
    HistoricalStart { uuid: String, path: JsString },
    /// `historical-start-raw`: write a child log to a temporary file and read that.
    HistoricalStartRaw {
        uuid: String,
        data: Arc<[u8]>,
        extension: JsString,
    },
}

/// `historical-data`: main's reply, which the hub hands to every root source.
#[derive(Clone)]
pub struct MainReply {
    pub uuid: String,
    pub files: Vec<Option<Arc<[u8]>>>,
    pub error: Option<JsString>,
}

/// What a source reports through the callbacks upstream passes to `openFile`. A child source's
/// status and progress callbacks do nothing and its refresh callback calls its parent's, so a
/// host sees `Refresh` from the whole tree and the rest only from the root.
#[derive(Clone, Debug, PartialEq)]
pub enum SourceEvent {
    /// `statusCallback(status)`, with what `getCustomError()` returns at that moment.
    Status {
        status: Status,
        custom_error: Option<JsString>,
    },
    Progress(f64),
    Refresh {
        has_new_fields: bool,
    },
    /// One `loadAllFields()` promise resolves; one event per pending call, oldest first.
    AllFieldsLoaded,
}

/// A `setTimeout(.., 0)` the host runs later through [`HistoricalDataSource::run_timeout`].
pub struct Timeout {
    source: String,
    task: Task,
}

enum Task {
    RawData(Arc<[u8]>),
    ChildLog {
        key: JsString,
        extension: JsString,
        chunks: Vec<RawValue>,
        next: usize,
    },
}

impl Timeout {
    /// The UUID of the source that scheduled it.
    pub fn source(&self) -> &str {
        &self.source
    }
}

/// The page around the sources. Timers, frames and workers are keyed by the UUID of the source
/// that owns them; each source has at most one of each.
pub trait SourceHost {
    /// `new Date().getTime()` on the page.
    fn now(&self) -> f64;
    fn send_main_message(&mut self, request: MainRequest);
    fn set_timeout(&mut self, timeout: Timeout);
    /// `window.setInterval(() => this.updateFieldRequest(), 50)`.
    fn set_interval(&mut self, source: &str);
    fn clear_interval(&mut self, source: &str);
    fn request_animation_frame(&mut self, source: &str);
    /// The `Math.random` and `Date` of a new worker realm.
    fn worker_realm(&mut self) -> (Box<dyn Rng + Send>, Box<dyn Clock + Send>);
    fn spawn_worker(&mut self, source: &str, name: WorkerName, worker: Box<dyn DecodeWorker>);
    fn post_worker(&mut self, source: &str, request: Request);
    /// `worker.terminate()`: no message of that worker may reach the source afterwards. Called
    /// again when an errored source is stopped.
    fn terminate_worker(&mut self, source: &str);
    /// `window.tabs.getActiveFields()` then `window.sidebar.getActiveFields()`.
    fn active_fields(&mut self) -> Vec<JsString>;
    fn event(&mut self, event: SourceEvent);
}

/// The page state a source works on: the hub's `Log` it writes into, the page realm, the host.
pub struct PageContext<'a> {
    pub log: &'a mut Log,
    pub realm: &'a mut JsRealm,
    pub host: &'a mut dyn SourceHost,
}

const RAW_EXTENSIONS: [&str; 4] = ["wpilog", "rlog", "csv", "log"];

const CHILD_CONCAT_YIELD: usize = 1000;

pub struct HistoricalDataSource {
    uuid: String,
    path: JsString,
    key_prefix: JsString,
    mock_progress: f64,
    mock_progress_active: bool,
    mock_start: f64,
    status: Status,
    reports_status: bool,
    load_all_callbacks: usize,
    custom_error: Option<JsString>,
    has_worker: bool,
    log_is_partial: bool,
    offset: f64,
    finished_fields: HashSet<JsString>,
    requested_fields: IndexSet<JsString>,
    has_interval: bool,
    last_raw_request_fields: IndexSet<JsString>,
    child_sources: IndexMap<JsString, HistoricalDataSource>,
    /// Children whose key was taken by a later child. Upstream drops them from `childSources`,
    /// but their timers and worker still hold them.
    replaced_children: Vec<HistoricalDataSource>,
}

impl HistoricalDataSource {
    /// `new HistoricalDataSource()`: draws its UUID from the page's `Math.random`.
    pub fn new(realm: &mut JsRealm) -> Self {
        HistoricalDataSource {
            uuid: create_uuid(realm.random.as_mut()),
            path: JsString::new(),
            key_prefix: JsString::new(),
            mock_progress: 0.0,
            mock_progress_active: true,
            mock_start: 0.0,
            status: Status::Waiting,
            reports_status: true,
            load_all_callbacks: 0,
            custom_error: None,
            has_worker: false,
            log_is_partial: false,
            offset: 0.0,
            finished_fields: HashSet::new(),
            requested_fields: IndexSet::new(),
            has_interval: false,
            last_raw_request_fields: IndexSet::new(),
            child_sources: IndexMap::new(),
            replaced_children: Vec::new(),
        }
    }

    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    pub fn path(&self) -> &JsString {
        &self.path
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn log_is_partial(&self) -> bool {
        self.log_is_partial
    }

    /// `getCustomError()`.
    pub fn custom_error(&self) -> Option<&JsString> {
        self.custom_error.as_ref()
    }

    /// `openFile(log, path, keyPrefix, ...)`.
    pub fn open_file(&mut self, path: &JsString, key_prefix: &JsString, cx: &mut PageContext<'_>) {
        self.key_prefix = key_prefix.clone();
        self.path = if path.ends_with(&JsString::from(".dsevents"), None) {
            let mut stem = path.slice(0.0, Some(-8.0));
            stem.push_str(&JsString::from("dslog"));
            stem
        } else {
            path.clone()
        };
        self.set_status(Status::Reading, cx);
        cx.host.send_main_message(MainRequest::HistoricalStart {
            uuid: self.uuid.clone(),
            path: self.path.clone(),
        });
        self.start_timers(cx);
    }

    /// `openRaw(log, data, extension, keyPrefix, ...)` for a child log.
    fn open_raw(
        &mut self,
        data: Arc<[u8]>,
        extension: &JsString,
        key_prefix: &JsString,
        cx: &mut PageContext<'_>,
    ) {
        let mut path = JsString::from("child.");
        path.push_str(extension);
        self.path = path;
        self.key_prefix = key_prefix.clone();
        self.set_status(Status::Reading, cx);
        if RAW_EXTENSIONS
            .iter()
            .any(|e| *extension == JsString::from(*e))
        {
            cx.host.set_timeout(Timeout {
                source: self.uuid.clone(),
                task: Task::RawData(data),
            });
        } else {
            cx.host.send_main_message(MainRequest::HistoricalStartRaw {
                uuid: self.uuid.clone(),
                data,
                extension: extension.clone(),
            });
        }
        self.start_timers(cx);
    }

    fn start_timers(&mut self, cx: &mut PageContext<'_>) {
        cx.host.set_interval(&self.uuid);
        self.has_interval = true;
        self.mock_start = cx.host.now();
        cx.host.request_animation_frame(&self.uuid);
    }

    /// `stop()`.
    pub fn stop(&mut self, cx: &mut PageContext<'_>) {
        self.set_status(Status::Stopped, cx);
        for child in self.child_sources.values_mut() {
            child.stop(cx);
        }
    }

    /// `getLoadingFields()`.
    pub fn get_loading_fields(&self) -> IndexSet<JsString> {
        let mut fields = self.requested_fields.clone();
        for (key, child) in &self.child_sources {
            if matches!(child.status, Status::Reading | Status::DecodingInitial) {
                fields.insert(key.clone());
            }
            fields.extend(child.get_loading_fields());
        }
        fields
    }

    /// Whether `uuid` belongs to this source or one of its descendants.
    pub fn contains(&self, uuid: &str) -> bool {
        self.uuid == uuid
            || self
                .child_sources
                .values()
                .chain(&self.replaced_children)
                .any(|child| child.contains(uuid))
    }

    fn find(&mut self, uuid: &str) -> Option<&mut HistoricalDataSource> {
        if self.uuid == uuid {
            return Some(self);
        }
        self.child_sources
            .values_mut()
            .chain(&mut self.replaced_children)
            .find_map(|child| child.find(uuid))
    }

    /// `handleMainMessage(message)`.
    pub fn handle_main_message(&mut self, reply: &MainReply, cx: &mut PageContext<'_>) {
        if reply.uuid != self.uuid {
            for child in self.child_sources.values_mut() {
                child.handle_main_message(reply, cx);
            }
            return;
        }
        if self.status != Status::Reading {
            return;
        }
        self.set_status(Status::DecodingInitial, cx);
        self.custom_error = reply.error.clone();
        if reply.files.iter().all(Option::is_none) {
            self.set_status(Status::Error, cx);
            return;
        }
        let Some(name) = select_worker(&self.path) else {
            self.set_status(Status::Error, cx);
            return;
        };
        let (random, clock) = cx.host.worker_realm();
        cx.host
            .spawn_worker(&self.uuid, name, create_worker(name, random, clock));
        self.has_worker = true;
        cx.host
            .post_worker(&self.uuid, Request::Start(reply.files.clone()));
    }

    /// Runs a timeout this tree scheduled.
    pub fn run_timeout(&mut self, timeout: Timeout, cx: &mut PageContext<'_>) {
        let Some(source) = self.find(&timeout.source) else {
            return;
        };
        match timeout.task {
            Task::RawData(data) => {
                let reply = MainReply {
                    uuid: source.uuid.clone(),
                    files: vec![Some(data)],
                    error: None,
                };
                source.handle_main_message(&reply, cx);
            }
            Task::ChildLog {
                key,
                extension,
                chunks,
                next,
            } => source.continue_child_log(key, extension, chunks, next, cx),
        }
    }

    /// The `setTimeout(async () => ...)` that concatenates a child log's raw values, awaiting a
    /// `setTimeout(.., 0)` after every 1000th chunk, then opens the child.
    fn continue_child_log(
        &mut self,
        key: JsString,
        extension: JsString,
        chunks: Vec<RawValue>,
        next: usize,
        cx: &mut PageContext<'_>,
    ) {
        if let Some(i) = (next..chunks.len()).find(|i| i.is_multiple_of(CHILD_CONCAT_YIELD)) {
            cx.host.set_timeout(Timeout {
                source: self.uuid.clone(),
                task: Task::ChildLog {
                    key,
                    extension,
                    chunks,
                    next: i + 1,
                },
            });
            return;
        }
        let data: Arc<[u8]> = chunks.concat().into();
        let mut child = HistoricalDataSource::new(cx.realm);
        child.reports_status = false;
        if let Some(replaced) = self.child_sources.insert(key.clone(), child) {
            self.replaced_children.push(replaced);
        }
        self.child_sources
            .get_mut(&key)
            .expect("the child was just inserted")
            .open_raw(data, &extension, &key, cx);
    }

    /// `updateFieldRequest()` from this source's 50 ms interval.
    pub fn fire_interval(&mut self, uuid: &str, cx: &mut PageContext<'_>) {
        if let Some(source) = self.find(uuid) {
            source.update_field_request(false, None, cx);
        }
    }

    /// `sendMockProgress` from this source's animation frame.
    pub fn animation_frame(&mut self, uuid: &str, cx: &mut PageContext<'_>) {
        let Some(source) = self.find(uuid) else {
            return;
        };
        if source.mock_progress_active {
            let time = (cx.host.now() - source.mock_start) / 1000.0;
            source.mock_progress = calc_mock_progress(time, 0.6);
            source.progress(source.mock_progress, cx);
            cx.host.request_animation_frame(&source.uuid);
        }
    }

    /// The worker's `onmessage` for one response. `Err` is a throw out of the handler (or a
    /// page realm that never returns), after which the rest of the handler does not run.
    pub fn worker_message(
        &mut self,
        uuid: &str,
        response: Response,
        cx: &mut PageContext<'_>,
    ) -> Result<(), Abort> {
        match self.find(uuid) {
            Some(source) => source.handle_worker_message(response, cx),
            None => Ok(()),
        }
    }

    fn handle_worker_message(
        &mut self,
        response: Response,
        cx: &mut PageContext<'_>,
    ) -> Result<(), Abort> {
        match response {
            Response::Progress(value) => {
                self.mock_progress_active = false;
                let progress = scale_value(value, [0.0, 1.0], [self.mock_progress, 1.0]);
                self.progress(progress, cx);
                return Ok(());
            }
            Response::Initial { log, is_partial } => {
                self.offset = Log::from_serialized(log.structured_clone(), cx.realm)
                    .and_then(|source| cx.log.merge_with(&source, &self.key_prefix, cx.realm))
                    .map_err(|error| match error {
                        LogError::Proto(_) | LogError::Thrown(_) | LogError::StackOverflow => {
                            Abort::Threw
                        }
                        LogError::NeverCompletes => Abort::NeverReturns,
                        LogError::Unmodeled(what) => Abort::Unmodeled(what),
                    })?;
                self.log_is_partial = is_partial;
            }
            Response::Failed => {
                self.set_status(Status::Error, cx);
                return Ok(());
            }
            Response::Fields(fields) if self.log_is_partial => {
                for field in fields {
                    let key = apply_key_prefix(&self.key_prefix, &field.key);
                    let mut data = field.data.structured_clone();
                    data.timestamps =
                        Arc::new(data.timestamps.iter().map(|t| t + self.offset).collect());
                    cx.log.set_field(&key, LogField::from_serialized(data));
                    if field.generated_parent {
                        cx.log.set_generated_parent(&key);
                    }
                    self.requested_fields.shift_remove(&key);
                    self.finished_fields.insert(key.clone());

                    let extension = get_child_log_extension(
                        cx.log.get_wpilib_type(&key),
                        cx.log.get_structured_type(&key),
                    );
                    if let Some(extension) = extension
                        && !self.child_sources.contains_key(&key)
                        && let Some(raw) = cx
                            .log
                            .get_field(&key)
                            .and_then(|f| f.get_raw(f64::NEG_INFINITY, f64::INFINITY, None, None))
                    {
                        cx.host.set_timeout(Timeout {
                            source: self.uuid.clone(),
                            task: Task::ChildLog {
                                key,
                                extension,
                                chunks: raw.values,
                                next: 0,
                            },
                        });
                    }
                }
            }
            Response::Fields(_) => {}
        }
        let status = if !self.requested_fields.is_empty() && self.log_is_partial {
            Status::DecodingField
        } else {
            Status::Idle
        };
        self.set_status(status, cx);
        if self.requested_fields.is_empty() || !self.log_is_partial {
            cx.host.event(SourceEvent::Refresh {
                has_new_fields: true,
            });
            for _ in 0..std::mem::take(&mut self.load_all_callbacks) {
                cx.host.event(SourceEvent::AllFieldsLoaded);
            }
        }
        Ok(())
    }

    /// Puts back `count` [`SourceEvent::AllFieldsLoaded`] resolutions whose callbacks never
    /// ran because a callback before them threw.
    pub fn requeue_load_all(&mut self, count: usize) {
        self.load_all_callbacks += count;
    }

    /// `loadAllFields()`. `true` when its promise is already resolved; otherwise it resolves
    /// with a later [`SourceEvent::AllFieldsLoaded`].
    pub fn load_all_fields(&mut self, cx: &mut PageContext<'_>) -> bool {
        self.update_field_request(true, None, cx);
        if self.requested_fields.is_empty() {
            true
        } else {
            self.load_all_callbacks += 1;
            false
        }
    }

    fn update_field_request(
        &mut self,
        load_everything: bool,
        forwarded_requests: Option<&IndexSet<JsString>>,
        cx: &mut PageContext<'_>,
    ) {
        if !matches!(self.status, Status::Idle | Status::DecodingField)
            || !self.has_worker
            || !self.log_is_partial
        {
            return;
        }
        let mut request_fields: IndexSet<JsString> = if load_everything {
            cx.log.get_field_keys().into_iter().collect()
        } else {
            let mut fields: IndexSet<JsString> = cx.host.active_fields().into_iter().collect();
            fields.extend(get_urcl_keys(cx.log));
            if let Some(forwarded) = forwarded_requests {
                fields.extend(forwarded.iter().cloned());
            }
            fields
        };

        let unchanged = request_fields.len() == self.last_raw_request_fields.len()
            && request_fields
                .iter()
                .all(|f| self.last_raw_request_fields.contains(f));
        if !unchanged {
            self.last_raw_request_fields = request_fields.clone();
            self.request_new_fields(load_everything, &mut request_fields, cx);
        }

        let status = if self.requested_fields.is_empty() {
            Status::Idle
        } else {
            Status::DecodingField
        };
        self.set_status(status, cx);
    }

    fn request_new_fields(
        &mut self,
        load_everything: bool,
        request_fields: &mut IndexSet<JsString>,
        cx: &mut PageContext<'_>,
    ) {
        let log = &*cx.log;
        let field_keys: Vec<&JsString> =
            log.field_keys().filter(|k| !log.is_generated(k)).collect();
        for &key in &field_keys {
            if is_schema(key) {
                request_fields.insert(key.clone());
            }
            if is_child_log(log.get_wpilib_type(key), log.get_structured_type(key)) {
                request_fields.insert(key.clone());
            }
        }
        for group in [
            ENABLED_KEYS,
            AUTONOMOUS_KEYS,
            UTILITY_KEYS,
            ALLIANCE_KEYS,
            DRIVER_STATION_KEYS,
            SYSTEM_TIME_KEYS,
            AKIT_TIMESTAMP_KEYS,
            EVENT_KEYS,
            MATCH_TYPE_KEYS,
            MATCH_NUMBER_KEYS,
        ] {
            request_fields.extend(group.iter().map(|k| JsString::from(*k)));
        }

        let initial: Vec<(JsString, JsString)> = request_fields
            .iter()
            .map(|f| (f.clone(), with_slash(f)))
            .collect();
        for &existing in &field_keys {
            if log.get_type(existing) == Some(LoggableType::Empty)
                || request_fields.contains(existing)
            {
                continue;
            }
            let existing_slash = with_slash(existing);
            let related = initial.iter().any(|(field, field_slash)| {
                existing.starts_with(field_slash, None) || field.starts_with(&existing_slash, None)
            });
            if related {
                request_fields.insert(existing.clone());
            }
        }

        let child_keys: Vec<(JsString, JsString)> = self
            .child_sources
            .keys()
            .map(|k| (k.clone(), with_slash(k)))
            .collect();
        let mut child_requests: Vec<IndexSet<JsString>> = vec![IndexSet::new(); child_keys.len()];
        let prefix_slash = with_slash(&self.key_prefix);
        let mut kept = IndexSet::new();
        for field in request_fields.drain(..) {
            let merge_prefix_local = self.key_prefix.is_empty() && is_merge_prefix(&field);
            let matches_prefix =
                self.key_prefix.is_empty() || field.starts_with(&prefix_slash, None);
            if self.requested_fields.contains(&field)
                || self.finished_fields.contains(&field)
                || log.get_field(&field).is_none()
                || log.is_generated(&field)
                || !matches_prefix
                || merge_prefix_local
            {
                continue;
            }
            match child_keys
                .iter()
                .position(|(_, child_slash)| field.starts_with(child_slash, None))
            {
                Some(child) => {
                    child_requests[child].insert(field);
                }
                None => {
                    kept.insert(field);
                }
            }
        }

        for ((child_key, _), requests) in child_keys.iter().zip(&child_requests) {
            let local: IndexSet<JsString> = requests
                .iter()
                .map(|f| f.slice(child_key.len() as f64, None))
                .collect();
            if let Some(child) = self.child_sources.get_mut(child_key) {
                child.update_field_request(load_everything, Some(&local), cx);
            }
        }

        let log = &*cx.log;
        let first = |field: &JsString| {
            is_schema(field)
                || field.includes(&JsString::from("URCL/Raw/Aliases"), None)
                || field.includes(&JsString::from("URCL/Raw/Persistent"), None)
                || is_child_log(log.get_wpilib_type(field), log.get_structured_type(field))
        };
        let (mut ordered, rest): (Vec<JsString>, Vec<JsString>) =
            kept.into_iter().partition(|f| first(f));
        ordered.extend(rest);

        let any = !ordered.is_empty();
        for field in ordered {
            let key = field.slice(self.key_prefix.len() as f64, None);
            self.requested_fields.insert(field);
            cx.host.post_worker(&self.uuid, Request::ParseField(key));
        }
        if any {
            cx.host.event(SourceEvent::Refresh {
                has_new_fields: false,
            });
        }
    }

    fn progress(&self, progress: f64, cx: &mut PageContext<'_>) {
        if self.reports_status {
            cx.host.event(SourceEvent::Progress(progress));
        }
    }

    /// `setStatus(status)`.
    fn set_status(&mut self, status: Status, cx: &mut PageContext<'_>) {
        if status == self.status || self.status == Status::Stopped {
            return;
        }
        self.status = status;
        if matches!(status, Status::Stopped | Status::Error) {
            if self.has_worker {
                cx.host.terminate_worker(&self.uuid);
            }
            self.mock_progress_active = false;
            if self.has_interval {
                cx.host.clear_interval(&self.uuid);
            }
        }
        if self.reports_status {
            cx.host.event(SourceEvent::Status {
                status,
                custom_error: self.custom_error.clone(),
            });
        }
    }
}

fn with_slash(key: &JsString) -> JsString {
    let mut out = key.clone();
    out.push_unit(u16::from(b'/'));
    out
}

#[cfg(test)]
mod tests {
    use asrs_js::time::{SetClock, SetRng};
    use asrs_log::log::proto_decoder::SerializedProtoDecoder;

    use super::*;

    #[derive(Default)]
    struct Recorder {
        log: Vec<String>,
        intervals: Vec<String>,
    }

    impl SourceHost for Recorder {
        fn now(&self) -> f64 {
            0.0
        }
        fn send_main_message(&mut self, request: MainRequest) {
            if let MainRequest::HistoricalStart { path, .. } = request {
                self.log.push(format!("main {}", path.to_string_lossy()));
            }
        }
        fn set_timeout(&mut self, _: Timeout) {
            self.log.push("timeout".into());
        }
        fn set_interval(&mut self, source: &str) {
            self.intervals.push(source.to_owned());
        }
        fn clear_interval(&mut self, source: &str) {
            self.intervals.retain(|s| s != source);
        }
        fn request_animation_frame(&mut self, _: &str) {}
        fn worker_realm(&mut self) -> (Box<dyn Rng + Send>, Box<dyn Clock + Send>) {
            (Box::new(SetRng::new(vec![0.5])), Box::new(SetClock(0.0)))
        }
        fn spawn_worker(&mut self, _: &str, name: WorkerName, _: Box<dyn DecodeWorker>) {
            self.log.push(format!("worker {}", name.script()));
        }
        fn post_worker(&mut self, _: &str, _: Request) {
            self.log.push("post".into());
        }
        fn terminate_worker(&mut self, _: &str) {
            self.log.push("terminate".into());
        }
        fn active_fields(&mut self) -> Vec<JsString> {
            Vec::new()
        }
        fn event(&mut self, event: SourceEvent) {
            if let SourceEvent::Status {
                status,
                custom_error,
            } = event
            {
                let error = custom_error.map(|e| format!(" ({})", e.to_string_lossy()));
                self.log
                    .push(format!("{}{}", status.name(), error.unwrap_or_default()));
            }
        }
    }

    fn open(
        path: &str,
        host: &mut Recorder,
        log: &mut Log,
        realm: &mut JsRealm,
    ) -> HistoricalDataSource {
        let mut source = HistoricalDataSource::new(realm);
        let mut cx = PageContext { log, realm, host };
        source.open_file(&JsString::from(path), &JsString::new(), &mut cx);
        source
    }

    fn reply(
        source: &HistoricalDataSource,
        files: Vec<Option<Arc<[u8]>>>,
        error: Option<&str>,
    ) -> MainReply {
        MainReply {
            uuid: source.uuid().to_owned(),
            files,
            error: error.map(JsString::from),
        }
    }

    #[test]
    fn worker_selection_takes_the_last_matching_extension() {
        let select = |p: &str| select_worker(&JsString::from(p));
        assert_eq!(select("a.dsevents"), Some(WorkerName::DsLog));
        assert_eq!(select("child.x.log"), Some(WorkerName::RoadRunner));
        assert_eq!(select("a.wpilogxz"), Some(WorkerName::Wpilog));
        assert_eq!(select("a.txt"), None);
    }

    #[test]
    fn unreadable_files_and_unknown_extensions_end_in_error() {
        let (mut log, mut realm) = (
            Log::new(true),
            JsRealm::new(Box::new(SetRng::new(vec![0.25]))),
        );
        let mut host = Recorder::default();
        let mut source = open("/x/a.dsevents", &mut host, &mut log, &mut realm);
        let missing = reply(&source, vec![None, None], None);
        let mut cx = PageContext {
            log: &mut log,
            realm: &mut realm,
            host: &mut host,
        };
        source.handle_main_message(&missing, &mut cx);
        source.handle_main_message(&missing, &mut cx);
        assert_eq!(
            host.log,
            ["Reading", "main /x/a.dslog", "DecodingInitial", "Error"]
        );
        assert!(host.intervals.is_empty());

        let mut host = Recorder::default();
        let mut source = open("/x/a.txt", &mut host, &mut log, &mut realm);
        let data = reply(&source, vec![Some(Arc::from(&b"1"[..]))], Some("why"));
        let mut cx = PageContext {
            log: &mut log,
            realm: &mut realm,
            host: &mut host,
        };
        source.handle_main_message(&data, &mut cx);
        assert_eq!(
            host.log,
            ["Reading", "main /x/a.txt", "DecodingInitial", "Error (why)"]
        );
    }

    #[test]
    fn stopped_is_terminal_and_other_uuids_are_ignored() {
        let (mut log, mut realm) = (
            Log::new(true),
            JsRealm::new(Box::new(SetRng::new(vec![0.75]))),
        );
        let mut host = Recorder::default();
        let mut source = open("a.csv", &mut host, &mut log, &mut realm);
        let mut other = reply(&source, vec![Some(Arc::from(&b"x"[..]))], None);
        other.uuid.push('!');
        let mut cx = PageContext {
            log: &mut log,
            realm: &mut realm,
            host: &mut host,
        };
        source.handle_main_message(&other, &mut cx);
        source.stop(&mut cx);
        let data = reply(&source, vec![Some(Arc::from(&b"x"[..]))], None);
        source.handle_main_message(&data, &mut cx);
        source.stop(&mut cx);
        assert_eq!(source.status(), Status::Stopped);
        assert_eq!(host.log, ["Reading", "main a.csv", "Stopped"]);
        assert!(host.intervals.is_empty());
    }

    #[test]
    fn an_unchanged_request_set_posts_nothing_and_load_all_puts_schemas_first() {
        let (mut log, mut realm) = (
            Log::new(true),
            JsRealm::new(Box::new(SetRng::new(vec![0.5]))),
        );
        let mut host = Recorder::default();
        let mut source = open("a.wpilog", &mut host, &mut log, &mut realm);
        let data = reply(&source, vec![Some(Arc::from(&b"x"[..]))], None);
        let mut worker_log = Log::new(true);
        for key in [
            "/Data",
            "/.schema/struct:A",
            "/DriverStation/Enabled",
            "/Other",
        ] {
            worker_log.create_blank_field(&JsString::from(key), LoggableType::Raw);
        }
        let mut cx = PageContext {
            log: &mut log,
            realm: &mut realm,
            host: &mut host,
        };
        source.handle_main_message(&data, &mut cx);
        let initial = Response::Initial {
            log: Box::new(worker_log.to_serialized()),
            is_partial: true,
        };
        let uuid = source.uuid().to_owned();
        source.worker_message(&uuid, initial, &mut cx).unwrap();
        source.fire_interval(&uuid, &mut cx);
        assert_eq!(source.status(), Status::Idle);
        assert!(!source.load_all_fields(&mut cx));
        assert_eq!(source.status(), Status::DecodingField);
        let loading: Vec<String> = source
            .get_loading_fields()
            .iter()
            .map(|k| k.to_string_lossy())
            .collect();
        assert_eq!(
            loading,
            [
                "/.schema/struct:A",
                "/Data",
                "/DriverStation/Enabled",
                "/Other"
            ]
        );
    }

    #[test]
    fn an_initial_log_whose_from_serialized_throws_stays_decoding() {
        let (mut log, mut realm) = (
            Log::new(true),
            JsRealm::new(Box::new(SetRng::new(vec![0.5]))),
        );
        let mut host = Recorder::default();
        let mut source = open("a.wpilog", &mut host, &mut log, &mut realm);
        let data = reply(&source, vec![Some(Arc::from(&b"x"[..]))], None);
        let mut cx = PageContext {
            log: &mut log,
            realm: &mut realm,
            host: &mut host,
        };
        source.handle_main_message(&data, &mut cx);
        // A FileDescriptorProto with no `name` holding one "Anon" message type: the
        // filename dedup does not apply, so adding it twice throws `duplicate name`.
        let anon: &[u8] = &[
            0x22, 0x11, 0x0a, 0x04, b'A', b'n', b'o', b'n', 0x12, 0x09, 0x0a, 0x01, b'x', 0x18,
            0x01, 0x20, 0x01, 0x28, 0x05,
        ];
        let mut serialized = Log::new(true).to_serialized();
        serialized.proto_decoder = SerializedProtoDecoder(vec![anon.to_vec(), anon.to_vec()]);
        let error = Log::from_serialized(serialized.clone(), cx.realm).err();
        assert!(
            matches!(&error, Some(LogError::Proto(e)) if e.js_message().to_string_lossy()
                == "duplicate name 'Anon' in Root"),
            "{error:?}"
        );
        let uuid = source.uuid().to_owned();
        let initial = Response::Initial {
            log: Box::new(serialized),
            is_partial: false,
        };
        assert_eq!(
            source.worker_message(&uuid, initial, &mut cx),
            Err(Abort::Threw)
        );
        assert_eq!(source.status(), Status::DecodingInitial);
    }
}
