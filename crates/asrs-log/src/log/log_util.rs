use asrs_js::string::JsString;

use super::{Log, LogValue};

/// `getOrDefault` (shared/log/LogUtil.ts): the value in effect at `timestamp`, or
/// `default` when the field is missing, holds another type, or its only samples
/// are later. Upstream reads through `getRange`, which never forwards `startOffset`.
pub fn get_or_default<T: LogValue>(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    default: T,
    uuid: Option<&str>,
) -> T {
    if log.get_type(key) == Some(T::TYPE)
        && let Some(data) = log.get::<T>(key, timestamp, timestamp, uuid, None)
        && data.timestamps.first().is_some_and(|t| *t <= timestamp)
    {
        return data.values[0].clone();
    }
    default
}
