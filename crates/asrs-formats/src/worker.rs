//! Worker-side messages of `HistoricalDataSource_WorkerResponse`. F02 adds `Fields`, the request
//! side and the `DecodeWorker` trait; F09 runs workers on their decode threads.

use asrs_log::log::SerializedLog;

pub enum Response {
    Progress(f64),
    Initial {
        log: SerializedLog,
        is_partial: bool,
    },
    Failed,
}

/// How a worker's `onmessage` ends without returning. Either way the hub receives nothing more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Abort {
    /// Any JS throw: a DataView RangeError, a thrown string, a TypeError on an `undefined` slot.
    Threw,
    /// Upstream loops forever. The port detects the cycle and stops instead of spinning.
    NeverReturns,
}
