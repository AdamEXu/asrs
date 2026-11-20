//! The initial-load path of `HistoricalDataSource` (`openFile`, `handleMainMessage` up to the
//! worker's `initial` or `failed` reply), run synchronously. F09 replaces it with the decode
//! thread, lazy field requests, child logs and progress.

use std::sync::Arc;

use asrs_js::string::JsString;
use asrs_js::time::Rng;
use asrs_log::log::Log;
use asrs_log::realm::JsRealm;

use crate::dslog::DsLogWorker;
use crate::worker::{Abort, DecodeWorker, Request, Response, run_request};
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
pub fn select_worker(path: &str) -> Option<WorkerName> {
    WORKER_NAMES
        .iter()
        .rev()
        .find(|(extension, _)| path.ends_with(extension))
        .map(|&(_, name)| name)
}

#[derive(Debug)]
pub struct UnportedWorker(pub WorkerName);

/// `new Worker("../bundles/" + selectedWorkerName)`: a fresh worker realm with its own
/// `Math.random` stream.
pub fn create_worker(
    name: WorkerName,
    random: Box<dyn Rng + Send>,
) -> Result<Box<dyn DecodeWorker>, UnportedWorker> {
    match name {
        WorkerName::Wpilog => Ok(Box::new(WpilogWorker::new(JsRealm::new(random)))),
        WorkerName::DsLog => Ok(Box::new(DsLogWorker)),
        other => Err(UnportedWorker(other)),
    }
}

pub struct HistoricalDataSource {
    path: String,
    status: Status,
    log_is_partial: bool,
}

impl HistoricalDataSource {
    pub fn open_file(path: &str) -> Self {
        let path = match path.strip_suffix(".dsevents") {
            Some(stem) => format!("{stem}.dslog"),
            None => path.to_owned(),
        };
        HistoricalDataSource {
            path,
            status: Status::Reading,
            log_is_partial: false,
        }
    }

    /// The path sent with `historical-start`.
    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn log_is_partial(&self) -> bool {
        self.log_is_partial
    }

    /// `handleMainMessage` for this source's reply, then the worker's messages. A worker that
    /// aborts posts nothing more, so the source stays in `DecodingInitial`, as it does when
    /// merging the reply into the hub's log throws.
    pub fn handle_main_message(
        &mut self,
        log: &mut Log,
        realm: &mut JsRealm,
        files: Vec<Option<Vec<u8>>>,
        worker_random: Box<dyn Rng + Send>,
    ) -> Result<Option<Abort>, UnportedWorker> {
        if self.status != Status::Reading {
            return Ok(None);
        }
        self.status = Status::DecodingInitial;
        if files.iter().all(Option::is_none) {
            self.status = Status::Error;
            return Ok(None);
        }
        let Some(worker) = select_worker(&self.path) else {
            self.status = Status::Error;
            return Ok(None);
        };
        let mut worker = create_worker(worker, worker_random)?;
        let data = files.into_iter().map(|file| file.map(Arc::from)).collect();
        let (replies, end) = run_request(worker.as_mut(), Request::Start(data));
        for reply in replies {
            match reply {
                Response::Progress(_) => {}
                Response::Initial {
                    log: payload,
                    is_partial,
                } => {
                    // Each decode merges into a fresh hub log, so the merge offset is never
                    // computed; F09 passes L14's getMergeOffset here.
                    let merged = Log::from_serialized(payload.structured_clone(), realm).and_then(
                        |source| log.merge_with(&source, &JsString::new(), realm, |_, _| 0.0),
                    );
                    if merged.is_ok() {
                        self.log_is_partial = is_partial;
                        self.status = Status::Idle;
                    }
                }
                Response::Failed => self.status = Status::Error,
                Response::Fields(_) => {}
            }
        }
        Ok(end.err())
    }
}
