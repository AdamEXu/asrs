#![allow(clippy::disallowed_methods)]

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use asrs_js::json;
use asrs_js::value::JsValue;
use asrs_parity::vectors::check_records;
use asrs_parity::{FnCase, Json, Record, Response, Utf16String, Value, diff, fn_vectors};

fn dispatch(case: &FnCase<'_>) -> Response {
    match js_json::FUNCTIONS.iter().find(|(n, _)| *n == case.name()) {
        Some((_, f)) => f(&case.request.args),
        None => Response::Error(format!("no port of {}", case.name())),
    }
}

fn_vectors!(json_values, "json/values", dispatch);

#[test]
fn json_docs() {
    check_records("json/docs", check_doc);
}

#[test]
fn json_invalid() {
    check_records("json/invalid", check_doc);
}

#[test]
fn json_deep() {
    check_records("json/deep", check_deep);
}

/// Deep records carry `shape` and `depth` rather than a doc (which would be huge); the port
/// rebuilds the input and checks the recorded outcome kind: `ok: true` or a `throw` value.
fn check_deep(record: &Record) -> Result<(), String> {
    let str_field = |key: &str| {
        record
            .field(key)
            .and_then(Json::as_str)
            .ok_or_else(|| format!("record has no {key}"))
    };
    let depth = record
        .field("depth")
        .and_then(Json::as_f64)
        .ok_or("record has no depth")? as usize;
    let doc = match str_field("shape")?.to_string_lossy().as_str() {
        "array" => "[".repeat(depth) + &"]".repeat(depth),
        "object" => "{\"a\":".repeat(depth) + "1" + &"}".repeat(depth),
        other => return Err(format!("unknown shape {other}")),
    };
    let outcome = |result: Result<(), json::JsonError>| match result {
        Ok(()) if record.field("ok").is_some() => Ok(()),
        Ok(()) => Err("expected a throw, got ok".to_owned()),
        Err(e) => expect_value(record, "throw", &error_value(&e)),
    };
    let parsed = json::parse_str(&doc);
    match str_field("deep")?.to_string_lossy().as_str() {
        "parse" => outcome(parsed.map(|_| ())),
        "stringify" => {
            let parsed = parsed.map_err(|e| e.to_string())?;
            if record.field("gap").is_some() {
                outcome(json::stringify_gap(&parsed, "  ").map(|_| ()))
            } else {
                outcome(json::stringify(&parsed).map(|_| ()))
            }
        }
        "jsonCopy" => outcome(parsed.and_then(|v| json::json_copy(&v)).map(|_| ())),
        other => Err(format!("unknown deep op {other}")),
    }
}

fn expect_value(record: &Record, key: &str, actual: &Value) -> Result<(), String> {
    let json = record
        .field(key)
        .ok_or_else(|| format!("record has no {key:?}"))?;
    let expected = Value::from_json(json).map_err(|e| e.to_string())?;
    if expected.encode() == actual.encode() {
        Ok(())
    } else {
        Err(format!(
            "{key}: {}",
            diff::report(&expected, actual).unwrap_or_else(|| "encodings differ".to_owned())
        ))
    }
}

fn expect_text(record: &Record, key: &str, actual: Option<String>) -> Result<(), String> {
    let actual = actual.ok_or_else(|| format!("{key}: stringify returned undefined"))?;
    expect_value(record, key, &Value::String(Utf16String::from(actual)))
}

fn error_value(e: &json::JsonError) -> Value {
    Value::Error(
        e.name.into(),
        Utf16String::from_units(e.message.units().to_vec()),
    )
}

fn check_doc(record: &Record) -> Result<(), String> {
    let doc = record
        .field("doc")
        .and_then(Json::as_str)
        .ok_or("record has no doc")?;
    let parsed = match json::parse_units(doc.units()) {
        Ok(v) => v,
        Err(e) => return expect_value(record, "throw", &error_value(&e)),
    };
    expect_value(record, "ok", &js_json::from_js(&parsed))?;
    let text = |r: Result<Option<String>, json::JsonError>| r.map_err(|e| e.to_string());
    expect_text(record, "s", text(json::stringify(&parsed))?)?;
    expect_text(record, "p", text(json::stringify_pretty(&parsed))?)?;
    let copied: JsValue = json::json_copy(&parsed).map_err(|e| e.to_string())?;
    let key = if record.field("c").is_some() {
        "c"
    } else {
        "ok"
    };
    expect_value(record, key, &js_json::from_js(&copied))
}
