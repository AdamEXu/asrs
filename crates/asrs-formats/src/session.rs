//! A `HistoricalDataSource` tree driven to a fully loaded `Log`, the way the oracle's
//! `HistoricalSession` drives it for export: `openFile`, then 50 ms `updateFieldRequest`
//! passes and `loadAllFields` until a whole pass posts nothing. Worker exchanges run
//! synchronously, each `parseField` reply applied before the next request goes out;
//! a `log:` field spawns its child source once the pass that decoded it is done.

use std::collections::VecDeque;
use std::sync::Arc;

use asrs_js::string::JsString;
use asrs_js::time::Rng;
use asrs_log::log::log_key_utils::{
    apply_key_prefix, get_child_log_extension, is_child_log, is_merge_prefix, is_schema,
};
use asrs_log::log::log_util::get_urcl_keys;
use asrs_log::log::robot_state::{
    AKIT_TIMESTAMP_KEYS, ALLIANCE_KEYS, AUTONOMOUS_KEYS, DRIVER_STATION_KEYS, ENABLED_KEYS,
    EVENT_KEYS, MATCH_NUMBER_KEYS, MATCH_TYPE_KEYS, SYSTEM_TIME_KEYS, UTILITY_KEYS,
};
use asrs_log::log::{Log, LogField, LoggableType};
use asrs_log::realm::JsRealm;
use indexmap::IndexSet;

use crate::historical::{Status, UnportedWorker, WorkerName, select_worker};
use crate::worker::{Abort, DecodeWorker, FieldResponse, Request, Response, run_request};

/// Extensions `openRaw` decodes directly; anything else goes through `historical-start-raw`.
const DIRECT_CHILD_EXTENSIONS: &[&str] = &["wpilog", "rlog", "csv", "log"];
const CONVERTED_EXTENSIONS: &[&str] = &[".hoot", ".revlog", ".wpilogxz"];
const ALWAYS_REQUESTED: &[&[&str]] = &[
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
];

/// A worker factory: `new Worker("../bundles/<name>.js")` with its own realm, random and clock.
pub trait WorkerFactory {
    fn create(&mut self, name: WorkerName) -> Result<Box<dyn DecodeWorker>, UnportedWorker>;
}

impl<F> WorkerFactory for F
where
    F: FnMut(WorkerName) -> Result<Box<dyn DecodeWorker>, UnportedWorker>,
{
    fn create(&mut self, name: WorkerName) -> Result<Box<dyn DecodeWorker>, UnportedWorker> {
        self(name)
    }
}

/// `historical-start-raw`: main writes the bytes to `child_<uuid>.<extension>` and opens it.
/// Invalid extensions and formats it cannot convert reply with null files.
pub fn raw_child_files(data: &[u8], extension: &str) -> Vec<Option<Arc<[u8]>>> {
    if extension.contains('/') || extension.contains('\\') || extension.contains("..") {
        return vec![None];
    }
    let temp = format!("child_x.{extension}");
    if CONVERTED_EXTENSIONS.iter().any(|e| temp.ends_with(e)) {
        vec![None]
    } else if temp.ends_with(".dslog") {
        vec![Some(Arc::from(data)), None]
    } else {
        vec![Some(Arc::from(data))]
    }
}

struct Spawn {
    parent: usize,
    key: JsString,
    extension: JsString,
    data: Vec<u8>,
}

struct Source {
    path: String,
    key_prefix: JsString,
    status: Status,
    worker: Option<Box<dyn DecodeWorker>>,
    /// True once the worker threw inside a request: upstream never notices and keeps
    /// posting to it, so every later reply is simply lost.
    worker_dead: bool,
    is_partial: bool,
    merge_offset: f64,
    requested: IndexSet<JsString>,
    finished: IndexSet<JsString>,
    last_raw: IndexSet<JsString>,
    children: Vec<usize>,
}

impl Source {
    fn new(path: String, key_prefix: JsString) -> Self {
        Source {
            path,
            key_prefix,
            status: Status::Reading,
            worker: None,
            is_partial: false,
            worker_dead: false,
            merge_offset: 0.0,
            requested: IndexSet::new(),
            finished: IndexSet::new(),
            last_raw: IndexSet::new(),
            children: Vec::new(),
        }
    }
}

/// The shared log and its root-plus-children source tree.
pub struct Session {
    pub log: Log,
    realm: JsRealm,
    sources: Vec<Source>,
    spawn_queue: VecDeque<Spawn>,
}

impl Session {
    /// `openFile`: the `.dsevents` rewrite happens before main sees the path, so `files`
    /// must already be the reply for the rewritten path.
    pub fn open<F>(
        path: &str,
        files: Vec<Option<Arc<[u8]>>>,
        realm_random: Box<dyn Rng + Send>,
        make_worker: &mut F,
    ) -> Result<Self, Abort>
    where
        F: WorkerFactory,
    {
        // `path.slice(0, -8) + "dslog"`: the dot stays, so `.dsevents` becomes `.dslog`.
        let path = path
            .strip_suffix("dsevents")
            .map_or_else(|| path.to_owned(), |stem| format!("{stem}dslog"));
        let mut session = Session {
            log: Log::new(true),
            realm: JsRealm::new(realm_random),
            sources: vec![Source::new(path, JsString::new())],
            spawn_queue: VecDeque::new(),
        };
        session.start_source(0, files, make_worker)?;
        Ok(session)
    }

    /// `loadEverything`: interval passes, then `loadAllFields`, until a pass posts nothing.
    /// Returns `Ok(())`; the `Abort` is a worker that threw or never returned upstream.
    pub fn load_everything<F>(&mut self, make_worker: &mut F) -> Result<(), Abort>
    where
        F: WorkerFactory,
    {
        loop {
            let mut activity = false;
            loop {
                let mut tick = false;
                let count = self.sources.len();
                for index in 0..count {
                    tick |= self.update_field_request(index, false, None, make_worker)?;
                }
                tick |= self.settle_spawns(make_worker)?;
                activity |= tick;
                if !tick {
                    break;
                }
            }
            activity |= self.update_field_request(0, true, None, make_worker)?;
            activity |= self.settle_spawns(make_worker)?;
            if !activity {
                return Ok(());
            }
        }
    }

    /// One `updateFieldRequest` call. Returns whether it posted a worker request or a child
    /// source was created anywhere in the recursion.
    #[allow(clippy::only_used_in_recursion)]
    fn update_field_request<F>(
        &mut self,
        index: usize,
        load_everything: bool,
        forwarded: Option<IndexSet<JsString>>,
        make_worker: &mut F,
    ) -> Result<bool, Abort>
    where
        F: WorkerFactory,
    {
        let ready = matches!(
            self.sources[index].status,
            Status::Idle | Status::DecodingField
        ) && self.sources[index].worker.is_some()
            && self.sources[index].is_partial;
        if !ready {
            return Ok(false);
        }

        let mut request_fields = IndexSet::new();
        if load_everything {
            request_fields.extend(self.log.get_field_keys());
        } else {
            request_fields.extend(get_urcl_keys(&self.log));
            if let Some(forwarded) = forwarded {
                request_fields.extend(forwarded);
            }
        }

        let mut posted = false;
        if !sets_equal(&request_fields, &self.sources[index].last_raw) {
            self.sources[index].last_raw = request_fields.clone();

            let field_keys: Vec<JsString> = self
                .log
                .get_field_keys()
                .into_iter()
                .filter(|key| !self.log.is_generated(key))
                .collect();
            for key in &field_keys {
                if is_schema(key) {
                    request_fields.insert(key.clone());
                }
                if is_child_log(
                    self.log.get_wpilib_type(key),
                    self.log.get_structured_type(key),
                ) {
                    request_fields.insert(key.clone());
                }
            }
            for list in ALWAYS_REQUESTED {
                for key in *list {
                    request_fields.insert(JsString::from(*key));
                }
            }

            let initial: Vec<JsString> = request_fields.iter().cloned().collect();
            for existing in &field_keys {
                if self.log.get_type(existing) == Some(LoggableType::Empty) {
                    continue;
                }
                if initial.iter().any(|field| {
                    existing == field
                        || starts_with_slash(existing, field)
                        || starts_with_slash(field, existing)
                }) {
                    request_fields.insert(existing.clone());
                }
            }

            let prefix_empty = self.sources[index].key_prefix.is_empty();
            let prefix = self.sources[index].key_prefix.clone();
            let mut keep = IndexSet::new();
            let mut child_requests: Vec<IndexSet<JsString>> = self.sources[index]
                .children
                .iter()
                .map(|_| IndexSet::new())
                .collect();
            for field in &request_fields {
                let is_merge_prefix_local = prefix_empty && is_merge_prefix(field);
                let matches_prefix = prefix_empty || starts_with_slash(field, &prefix);
                let mut managed = None;
                if !self.sources[index].requested.contains(field)
                    && !self.sources[index].finished.contains(field)
                    && self.log.get_field(field).is_some()
                    && !self.log.is_generated(field)
                    && matches_prefix
                    && !is_merge_prefix_local
                {
                    managed = self.sources[index].children.iter().position(|&child| {
                        starts_with_slash(field, &self.sources[child].key_prefix)
                    });
                    if managed.is_none() {
                        keep.insert(field.clone());
                    }
                }
                if let Some(position) = managed {
                    child_requests[position].insert(field.clone());
                }
            }

            let children = self.sources[index].children.clone();
            for (position, child) in children.into_iter().enumerate() {
                let child_prefix = self.sources[child].key_prefix.clone();
                let local: IndexSet<JsString> = child_requests[position]
                    .iter()
                    .map(|field| field.slice(child_prefix.len() as f64, None))
                    .collect();
                posted |=
                    self.update_field_request(child, load_everything, Some(local), make_worker)?;
            }

            let priority = |field: &JsString| {
                is_schema(field)
                    || field.includes(&JsString::from("URCL/Raw/Aliases"), None)
                    || field.includes(&JsString::from("URCL/Raw/Persistent"), None)
                    || is_child_log(
                        self.log.get_wpilib_type(field),
                        self.log.get_structured_type(field),
                    )
            };
            let first: Vec<JsString> = keep.iter().filter(|f| priority(f)).cloned().collect();
            let rest: Vec<JsString> = keep.iter().filter(|f| !priority(f)).cloned().collect();
            for field in first.into_iter().chain(rest) {
                let key = field.slice(prefix.len() as f64, None);
                self.sources[index].requested.insert(field);
                self.send_parse_field(index, key)?;
                posted = true;
            }
        }

        let source = &mut self.sources[index];
        source.status = if source.requested.is_empty() {
            Status::Idle
        } else {
            Status::DecodingField
        };
        Ok(posted)
    }

    /// `postMessage({type: "parseField"})` plus every reply, applied in order.
    fn send_parse_field(&mut self, index: usize, key: JsString) -> Result<(), Abort> {
        if self.sources[index].worker_dead {
            return Ok(());
        }
        let Some(worker) = self.sources[index].worker.as_mut() else {
            return Ok(());
        };
        let (replies, end) = run_request(worker.as_mut(), Request::ParseField(key));
        for reply in replies {
            self.handle_response(index, reply)?;
        }
        match end {
            Err(Abort::Threw) => {
                self.sources[index].worker_dead = true;
                Ok(())
            }
            end => end,
        }
    }

    /// The `start` exchange for one source. Returns whether a worker was created.
    fn start_source<F>(
        &mut self,
        index: usize,
        files: Vec<Option<Arc<[u8]>>>,
        make_worker: &mut F,
    ) -> Result<bool, Abort>
    where
        F: WorkerFactory,
    {
        if self.sources[index].status != Status::Reading {
            return Ok(false);
        }
        self.sources[index].status = Status::DecodingInitial;
        if files.iter().all(Option::is_none) {
            self.sources[index].status = Status::Error;
            return Ok(false);
        }
        let Some(name) = select_worker(&self.sources[index].path) else {
            self.sources[index].status = Status::Error;
            return Ok(false);
        };
        let mut worker = make_worker
            .create(name)
            .map_err(|_| Abort::Unmodeled("worker not ported"))?;
        let (replies, end) = run_request(worker.as_mut(), Request::Start(files));
        for reply in replies {
            self.handle_response(index, reply)?;
        }
        self.sources[index].worker = Some(worker);
        match end {
            Err(Abort::Threw) => self.sources[index].worker_dead = true,
            end => end?,
        }
        Ok(true)
    }

    /// The worker `onmessage` switch.
    fn handle_response(&mut self, index: usize, response: Response) -> Result<(), Abort> {
        match response {
            Response::Progress(_) => {}
            Response::Initial { log, is_partial } => {
                let merged = Log::from_serialized(log.structured_clone(), &mut self.realm)
                    .and_then(|source| {
                        self.log.merge_with(
                            &source,
                            &self.sources[index].key_prefix,
                            &mut self.realm,
                        )
                    });
                if let Ok(offset) = merged {
                    self.sources[index].merge_offset = offset;
                    self.sources[index].is_partial = is_partial;
                }
            }
            Response::Failed => {
                self.sources[index].status = Status::Error;
                return Ok(());
            }
            Response::Fields(fields) => self.apply_fields(index, fields),
        }
        let source = &mut self.sources[index];
        if source.status != Status::Error {
            source.status = if !source.requested.is_empty() && source.is_partial {
                Status::DecodingField
            } else {
                Status::Idle
            };
        }
        Ok(())
    }

    /// The `fields` branch: apply the key prefix, shift by the merge offset, install the
    /// field and queue a child source when it is a `log:` field.
    fn apply_fields(&mut self, index: usize, fields: Vec<FieldResponse>) {
        if !self.sources[index].is_partial {
            return;
        }
        let offset = self.sources[index].merge_offset;
        let prefix = self.sources[index].key_prefix.clone();
        for field in fields {
            let key = apply_key_prefix(&prefix, &field.key);
            let mut data = field.data;
            data.timestamps = Arc::new(data.timestamps.iter().map(|t| t + offset).collect());
            self.log.set_field(&key, LogField::from_serialized(data));
            if field.generated_parent {
                self.log.set_generated_parent(&key);
            }
            self.sources[index].requested.shift_remove(&key);
            self.sources[index].finished.insert(key.clone());
            let Some(extension) = get_child_log_extension(
                self.log.get_wpilib_type(&key),
                self.log.get_structured_type(&key),
            ) else {
                continue;
            };
            let spawned = self.sources[index]
                .children
                .iter()
                .any(|&child| self.sources[child].key_prefix == key);
            if spawned {
                continue;
            }
            let Some(raw) = self
                .log
                .get_raw(&key, f64::NEG_INFINITY, f64::INFINITY, None, None)
            else {
                continue;
            };
            let data: Vec<u8> = raw
                .values
                .iter()
                .flat_map(|chunk| chunk.iter().copied())
                .collect();
            self.spawn_queue.push_back(Spawn {
                parent: index,
                key,
                extension,
                data,
            });
        }
    }

    /// Creates the queued child sources, one per `log:` field, in arrival order.
    /// Returns whether any worker was created.
    fn settle_spawns<F>(&mut self, make_worker: &mut F) -> Result<bool, Abort>
    where
        F: WorkerFactory,
    {
        let mut posted = false;
        while let Some(spawn) = self.spawn_queue.pop_front() {
            let extension = spawn.extension.to_string_lossy();
            let index = self.sources.len();
            self.sources
                .push(Source::new(format!("child.{extension}"), spawn.key.clone()));
            self.sources[spawn.parent].children.push(index);
            let files = if DIRECT_CHILD_EXTENSIONS.contains(&extension.as_str()) {
                vec![Some(Arc::from(spawn.data.as_slice()))]
            } else {
                raw_child_files(&spawn.data, &extension)
            };
            posted |= self.start_source(index, files, make_worker)?;
        }
        Ok(posted)
    }
}

fn sets_equal(a: &IndexSet<JsString>, b: &IndexSet<JsString>) -> bool {
    a.len() == b.len() && a.iter().all(|key| b.contains(key))
}

/// `a.startsWith(b + "/")`: `a` is strictly under `b`.
fn starts_with_slash(a: &JsString, b: &JsString) -> bool {
    a.units().len() > b.units().len()
        && a.units().starts_with(b.units())
        && a.units()[b.units().len()] == u16::from(b'/')
}
