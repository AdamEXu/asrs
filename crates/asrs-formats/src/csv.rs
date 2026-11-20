//! Port of `src/hub/dataSources/csv/CSVDecoder.ts` and `csvWorker.ts`: reads the table and
//! list layouts upstream's own exporter writes back into a Log.

use std::sync::Arc;

use asrs_js::json;
use asrs_js::number;
use asrs_js::regex::sites::{is_csv_byte, is_csv_bytes};
use asrs_js::string::{JsString, TextDecoder};
use asrs_js::value::JsValue;
use asrs_log::log::{Log, LogError};

use crate::worker::{Abort, DecodeWorker, Request, Response};

fn js(text: &str) -> JsString {
    JsString::from(text)
}

/// A `LogError` leaving a write that is not inside a `try`: `Proto` is a JS throw, which the
/// worker's catch turns into `failed`.
fn thrown(error: LogError) -> Abort {
    match error {
        LogError::Proto(_) => Abort::Threw,
        LogError::NeverCompletes => Abort::NeverReturns,
        LogError::Unmodeled(what) => Abort::Unmodeled(what),
    }
}

/// The same write inside a `try`: a `Proto` throw is caught and the decode falls through.
fn caught(result: Result<(), LogError>) -> Result<bool, Abort> {
    match result {
        Ok(()) => Ok(true),
        Err(LogError::Proto(_)) => Ok(false),
        Err(error) => Err(thrown(error)),
    }
}

/// `Number.isInteger(byte) && byte >= 0 && byte <= 255`.
fn is_byte(value: f64) -> bool {
    value.is_finite() && value.trunc() == value && (0.0..=255.0).contains(&value)
}

/// The `try` block of the multi-byte raw branch; `Err` is the thrown `"Invalid byte"` string.
fn parse_byte_list(value_raw: &JsString) -> Result<Vec<u8>, ()> {
    let mut parts = value_raw.split(Some(&js("-")), None);
    if parts.last() == Some(&js("...")) {
        parts.pop();
    }
    let mut out = Vec::with_capacity(parts.len());
    for part in &parts {
        if part.len() != 2 {
            return Err(());
        }
        let byte = number::parse_int(part.units(), 16.0);
        if !is_byte(byte) {
            return Err(());
        }
        out.push(byte as u8);
    }
    Ok(out)
}

/// The `try` block of the array branch; `Err` is any element's `JSON.parse` failure.
fn parse_array(value_raw: &JsString) -> Result<Vec<JsValue>, ()> {
    let mut parts = value_raw
        .slice(1.0, Some(-1.0))
        .split(Some(&js("; ")), None);
    if parts.len() == 1 && parts[0].is_empty() {
        parts.clear();
    }
    if parts.last() == Some(&js("...")) {
        parts.pop();
    }
    parts
        .iter()
        .map(|part| json::parse(part).map_err(|_| ()))
        .collect()
}

/// `CSVDecoder.parseEntry`. `Err(Abort::Threw)` is a JS throw out of the call, which the
/// worker's `catch` reports as `failed`; the other aborts end the worker.
fn parse_entry(
    log: &mut Log,
    timestamp: f64,
    key: &JsString,
    value_raw: &JsString,
) -> Result<(), Abort> {
    if *value_raw == js("(empty)") {
        return log.put_raw(key, timestamp, Vec::new()).map_err(thrown);
    }

    if is_csv_bytes(value_raw) {
        let done = match parse_byte_list(value_raw) {
            Ok(bytes) => caught(log.put_raw(key, timestamp, Arc::<[u8]>::from(bytes)))?,
            Err(()) => false,
        };
        if done {
            return Ok(());
        }
    }

    if value_raw.starts_with(&js("["), None) && value_raw.ends_with(&js("]"), None) {
        let done = match parse_array(value_raw) {
            Ok(values) => {
                caught(log.put_unknown_struct(key, timestamp, &JsValue::Array(values), true))?
            }
            Err(()) => false,
        };
        if done {
            return Ok(());
        }
    }

    // The final `try`/`catch`: a `JSON.parse` failure or a JS-throwable `putUnknownStruct`
    // error is caught, then the single-byte fallback runs before the error is rethrown.
    let done = match json::parse(value_raw) {
        Ok(value) => caught(log.put_unknown_struct(key, timestamp, &value, true))?,
        Err(_) => false,
    };
    if done {
        return Ok(());
    }
    if is_csv_byte(value_raw) {
        let value = number::parse_int(value_raw.units(), 16.0);
        if is_byte(value) {
            return log
                .put_raw(key, timestamp, Arc::<[u8]>::from([value as u8]))
                .map_err(thrown);
        }
    }
    Err(Abort::Threw)
}

/// `CSVDecoder.decode`.
pub fn decode(log: &mut Log, data: &JsString, post: &mut dyn FnMut(Response)) -> Result<(), Abort> {
    let rows = data.split(Some(&js("\n")), None);
    if rows[0] == js("Timestamp,Key,Value") {
        for (row_index, row) in rows.iter().enumerate().skip(1) {
            if row.is_empty() {
                continue;
            }
            let columns = row.split(Some(&js(",")), None);
            if columns.len() != 3 {
                return Err(Abort::Threw);
            }
            let timestamp = number::parse_float(columns[0].units());
            parse_entry(log, timestamp, &columns[1], &columns[2])?;
            post(Response::Progress(row_index as f64 / rows.len() as f64));
        }
    } else if rows[0].starts_with(&js("Timestamp"), None) {
        let keys = rows[0].split(Some(&js(",")), None);
        for (row_index, row) in rows.iter().enumerate().skip(1) {
            if row.is_empty() {
                continue;
            }
            let columns = row.split(Some(&js(",")), None);
            if columns.len() != keys.len() {
                return Err(Abort::Threw);
            }
            let timestamp = number::parse_float(columns[0].units());
            for (column_index, column) in columns.iter().enumerate().skip(1) {
                parse_entry(log, timestamp, &keys[column_index], column)?;
            }
            post(Response::Progress(row_index as f64 / rows.len() as f64));
        }
    } else {
        return Err(Abort::Threw);
    }
    Ok(())
}

/// csvWorker's `onmessage` for its `start` request. `request.data[0]` is the only file: `null`
/// makes `TextDecoder.decode` throw a TypeError and a missing slot decodes an empty string;
/// both land in the worker's `catch` and answer `failed`.
pub fn run_worker(files: &[Option<&[u8]>], post: &mut impl FnMut(Response)) -> Result<(), Abort> {
    let text_decoder = TextDecoder::new(None, false, false).expect("UTF-8 always resolves");
    let decoded = match files.first().copied() {
        Some(Some(bytes)) => text_decoder
            .decode(bytes)
            .expect("nonfatal UTF-8 never errors"),
        Some(None) => JsString::new(),
        None => JsString::new(),
    };
    let mut log = Log::new(false);
    match decode(&mut log, &decoded, &mut |response| post(response)) {
        Ok(()) => {}
        Err(Abort::Threw) => {
            post(Response::Failed);
            return Ok(());
        }
        Err(abort) => return Err(abort),
    }
    log.get_changed_fields();
    post(Response::Progress(1.0));
    post(Response::Initial {
        log: Box::new(log.to_serialized()),
        is_partial: false,
    });
    Ok(())
}

/// csvWorker: answers `start` and ignores every other request.
pub struct CsvWorker;

impl DecodeWorker for CsvWorker {
    fn handle(&mut self, request: Request, post: &mut dyn FnMut(Response)) -> Result<(), Abort> {
        let Request::Start(files) = request else {
            return Ok(());
        };
        let files: Vec<Option<&[u8]>> = files.iter().map(Option::as_deref).collect();
        run_worker(&files, &mut |response| post(response))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcomes(bytes: Option<&[u8]>) -> Vec<&'static str> {
        let mut kinds = Vec::new();
        let end = run_worker(&[bytes], &mut |response| {
            kinds.push(match response {
                Response::Progress(_) => "progress",
                Response::Initial { .. } => "initial",
                Response::Failed => "failed",
                Response::Fields(_) => "fields",
            });
        });
        assert_eq!(end, Ok(()));
        kinds
    }

    #[test]
    fn a_table_and_a_list_decode_then_post_initial() {
        assert_eq!(
            outcomes(Some(b"Timestamp,/A\n0,1")),
            ["progress", "progress", "initial"]
        );
        assert_eq!(
            outcomes(Some(b"Timestamp,Key,Value\n0,/A,1")),
            ["progress", "progress", "initial"]
        );
    }

    #[test]
    fn a_decode_throw_posts_failed() {
        assert_eq!(outcomes(Some(b"time,/A\n0,1")), ["failed"]);
        assert_eq!(
            outcomes(Some(b"Timestamp,/A\n0,1\n1,abc")),
            ["progress", "failed"]
        );
    }

    #[test]
    fn a_null_or_missing_file_posts_failed() {
        assert_eq!(outcomes(None), ["failed"]);
        let mut kinds = Vec::new();
        let end = run_worker(&[], &mut |response| {
            kinds.push(matches!(response, Response::Failed));
        });
        assert_eq!(end, Ok(()));
        assert_eq!(kinds, [true]);
    }
}
