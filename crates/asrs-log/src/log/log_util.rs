//! `shared/log/LogUtil.ts`.

use asrs_js::json::{self, quote_into};
use asrs_js::number;
use asrs_js::string::JsString;
use asrs_js::value::JsValue;

use super::fuse::Fuse;
use super::log_value_sets::RawValue;
use super::{Log, LogValue, LogValueSet, LogValueSetAny};

pub const ARRAY_TEXT_SIZE_LIMIT: usize = 500;
pub const TYPE_KEY: &str = ".type";
pub const MAX_SEARCH_RESULTS: usize = 128;

/// A log value as `getLogValueText` prints it.
pub trait LogValueText {
    fn log_value_text(&self) -> String;
}

/// `JSON.stringify` of one array element or scalar.
trait JsonText {
    fn push_json(&self, out: &mut String);
}

impl JsonText for bool {
    fn push_json(&self, out: &mut String) {
        out.push_str(if *self { "true" } else { "false" });
    }
}

impl JsonText for f64 {
    fn push_json(&self, out: &mut String) {
        if self.is_finite() {
            out.push_str(&number::to_string(*self));
        } else {
            out.push_str("null");
        }
    }
}

impl JsonText for JsString {
    fn push_json(&self, out: &mut String) {
        quote_into(self.units(), out);
    }
}

macro_rules! scalar_text {
    ($($ty:ty),*) => {$(
        impl LogValueText for $ty {
            fn log_value_text(&self) -> String {
                let mut out = String::new();
                self.push_json(&mut out);
                out
            }
        }

        impl LogValueText for Vec<$ty> {
            fn log_value_text(&self) -> String {
                let mut out = String::from("[");
                for (i, item) in self.iter().take(ARRAY_TEXT_SIZE_LIMIT).enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    item.push_json(&mut out);
                }
                // Upstream pushes the string "..." before stringifying, so it keeps its quotes.
                if self.len() > ARRAY_TEXT_SIZE_LIMIT {
                    out.push_str(", \"...\"");
                }
                out.push(']');
                out
            }
        }
    )*};
}

scalar_text!(bool, f64, JsString);

impl LogValueText for RawValue {
    fn log_value_text(&self) -> String {
        if self.is_empty() {
            return "(empty)".to_owned();
        }
        let mut parts: Vec<String> = self
            .iter()
            .take(ARRAY_TEXT_SIZE_LIMIT)
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if self.len() > ARRAY_TEXT_SIZE_LIMIT {
            parts.push("...".to_owned());
        }
        parts.join("-")
    }
}

fn push_foreign(value: &JsValue, out: &mut String) {
    match json::stringify(value) {
        Ok(Some(text)) => out.push_str(&text),
        // A template or `join` prints what `JSON.stringify(undefined)` returns.
        _ => out.push_str("undefined"),
    }
}

impl LogValueText for JsValue {
    fn log_value_text(&self) -> String {
        let mut out = String::new();
        match self {
            JsValue::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().take(ARRAY_TEXT_SIZE_LIMIT).enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    push_foreign(item, &mut out);
                }
                if items.len() > ARRAY_TEXT_SIZE_LIMIT {
                    out.push_str(", \"...\"");
                }
                out.push(']');
            }
            other => push_foreign(other, &mut out),
        }
        out
    }
}

/// An `Empty` field's samples are `null`.
impl LogValueText for () {
    fn log_value_text(&self) -> String {
        "null".to_owned()
    }
}

/// `getLogValueText`: the display string for a log value.
pub fn get_log_value_text<T: LogValueText>(value: &T) -> String {
    value.log_value_text()
}

/// `getLogValueText` for the sample at `index` of a range, or `None` when there is none.
pub fn get_log_value_text_at(set: &LogValueSetAny, index: usize) -> Option<String> {
    fn at<T: LogValueText>(set: &LogValueSet<T>, index: usize) -> Option<String> {
        set.values.get(index).map(get_log_value_text)
    }
    match set {
        LogValueSetAny::Raw(s) => at(s, index),
        LogValueSetAny::Boolean(s) => at(s, index),
        LogValueSetAny::Number(s) => at(s, index),
        LogValueSetAny::String(s) => at(s, index),
        LogValueSetAny::BooleanArray(s) => at(s, index),
        LogValueSetAny::NumberArray(s) => at(s, index),
        LogValueSetAny::StringArray(s) => at(s, index),
        LogValueSetAny::Empty(s) => at(s, index),
        LogValueSetAny::Foreign(_, s) => at(s, index),
    }
}

/// `getOrDefault` without the default: the value of `key` in effect at `timestamp`, or `None`
/// when the field is missing, has another type, or has no sample at or before it.
pub fn get_value_at<T: LogValue>(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    uuid: Option<&str>,
) -> Option<T> {
    if log.get_type(key) != Some(T::TYPE) {
        return None;
    }
    let set = log.get::<T>(key, timestamp, timestamp, uuid, None)?;
    if !set.timestamps.first().is_some_and(|&t| t <= timestamp) {
        return None;
    }
    set.values.into_iter().next()
}

/// [`get_value_at`] as stored, for a field whose column is foreign.
pub fn get_foreign_at(log: &Log, key: &JsString, timestamp: f64) -> Option<JsValue> {
    let set = log.get_foreign_range(key, timestamp, timestamp)?;
    if set.timestamps.first()? > &timestamp {
        return None;
    }
    set.values.into_iter().next()
}

/// `getOrDefault`.
pub fn get_or_default<T: LogValue>(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    default: T,
    uuid: Option<&str>,
) -> T {
    get_value_at(log, key, timestamp, uuid).unwrap_or(default)
}

/// `logValuesEqual`.
pub fn log_values_equal<T: LogValue>(a: &T, b: &T) -> bool {
    a.log_values_equal(b)
}

/// `getURCLKeys`: field keys whose wpilib type starts with `URCL`.
pub fn get_urcl_keys(log: &Log) -> Vec<JsString> {
    let prefix = [b'U' as u16, b'R' as u16, b'C' as u16, b'L' as u16];
    log.field_keys()
        .filter(|key| {
            log.get_wpilib_type(key)
                .is_some_and(|t| t.units().starts_with(&prefix))
        })
        .cloned()
        .collect()
}

/// `searchFields`: the field keys Fuse.js ranks for `query`, best first, at most
/// [`MAX_SEARCH_RESULTS`].
pub fn search_fields(log: &Log, query: &JsString) -> Vec<JsString> {
    if query.is_empty() {
        return Vec::new();
    }
    let keys = log.get_field_keys();
    Fuse::new(&keys)
        .search(query)
        .into_iter()
        .take(MAX_SEARCH_RESULTS)
        .map(|result| keys[result.ref_index].clone())
        .collect()
}

/// `LogFieldIterator`: a forward-only cursor into a `{timestamps, values}` set for
/// monotonically advancing time lookups.
pub struct LogFieldIterator<'a, T> {
    data: Option<(&'a [f64], &'a [T])>,
    idx: usize,
}

impl<'a, T> LogFieldIterator<'a, T> {
    pub fn new(data: Option<&'a LogValueSet<T>>) -> Self {
        LogFieldIterator {
            data: data.map(|set| (set.timestamps.as_slice(), set.values.as_slice())),
            idx: 0,
        }
    }

    /// The value in effect at `time`, advancing the cursor but never moving it back.
    pub fn get_at_time(&mut self, time: f64) -> Option<&'a T> {
        let (timestamps, values) = self.data?;
        while self.idx + 1 < timestamps.len() && timestamps[self.idx + 1] <= time {
            self.idx += 1;
        }
        if self.idx < timestamps.len() && timestamps[self.idx] <= time {
            Some(&values[self.idx])
        } else {
            None
        }
    }
}
