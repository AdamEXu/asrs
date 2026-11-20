//! The initial-load path of `HistoricalDataSource` (`openFile`, `handleMainMessage` up to the
//! worker's `initial` or `failed` reply), run synchronously. F09 replaces it with the decode
//! thread, lazy field requests, child logs and progress.

use asrs_log::log::Log;

use crate::dslog;
use crate::worker::{Abort, Response};

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

pub struct HistoricalDataSource {
    path: String,
    status: Status,
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
        }
    }

    /// The path sent with `historical-start`.
    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn status(&self) -> Status {
        self.status
    }

    /// `handleMainMessage` for this source's reply, then the worker's messages. A worker that
    /// aborts posts nothing more, so the source stays in `DecodingInitial`.
    pub fn handle_main_message(
        &mut self,
        log: &mut Log,
        files: &[Option<Vec<u8>>],
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
        let data: Vec<Option<&[u8]>> = files.iter().map(Option::as_deref).collect();
        let mut replies = Vec::new();
        let end = match worker {
            WorkerName::DsLog => dslog::run_worker(&data, &mut |r| replies.push(r)),
            other => return Err(UnportedWorker(other)),
        };
        for reply in replies {
            match reply {
                Response::Progress(_) => {}
                Response::Initial { log: payload, .. } => {
                    // `mergeWith(Log.fromSerialized(payload), "")` into a fresh log, until L08
                    // ports `mergeWith`: fields are rebuilt from the source's serialization.
                    *log = Log::from_serialized(Log::from_serialized(payload).to_serialized());
                    self.status = Status::Idle;
                }
                Response::Failed => self.status = Status::Error,
            }
        }
        Ok(end.err())
    }
}
