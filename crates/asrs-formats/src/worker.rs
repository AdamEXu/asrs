//! The worker half of `HistoricalDataSource`: what a format's `onmessage` can post back
//! after the `{type: "progress", value: 1}` message every one of them sends first.

use asrs_log::log::SerializedLog;

/// Any JS throw inside a reader or worker body: a DataView RangeError on a truncated read,
/// the unsupported-version string, or a TypeError from a missing slot's `undefined` data.
pub struct Thrown;

pub enum Outcome {
    /// `{type: "initial", log, isPartial}`.
    Initial {
        log: SerializedLog,
        is_partial: bool,
    },
    /// `{type: "failed"}`.
    Failed,
    /// The message handler threw, so the worker posts nothing: the source stays in
    /// DecodingInitial with an empty log.
    Threw,
}
