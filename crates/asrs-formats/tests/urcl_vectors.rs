//! Replays the `genUrcl` oracle vectors. Each `case` line holds the wpilog worker's
//! index-time ops plus the per-key raw records the worker decodes through
//! `CustomSchemas`; the following `dump` line is the canonical serialized dump of the
//! real upstream session's hub-side Log. The replay mirrors the worker/hub boundary:
//! the range and zero timestamp are frozen at the worker's initial response, and
//! generated parents accumulate in per-response changed-field order.

mod functions {
    use asrs_parity::{Response, Value};

    pub type Function = fn(&[Value]) -> Response;

    pub fn arg(args: &[Value], i: usize) -> Value {
        args.get(i).cloned().unwrap_or(Value::Undefined)
    }
}

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code, clippy::disallowed_methods)]
mod js_json;
#[path = "../../asrs-cli/src/log_field.rs"]
#[allow(dead_code)]
mod log_field;
#[path = "../../asrs-cli/src/log_run.rs"]
#[allow(dead_code)]
mod log_run;

use asrs_formats::urcl;
use asrs_js::string::JsString;
use asrs_log::log::Log;
use asrs_parity::cve1::Value;
use asrs_parity::diff;
use asrs_parity::json::Json;
use asrs_parity::vectors::{self, Record};

fn bits(json: &Json) -> Result<f64, String> {
    let text = json
        .as_str()
        .ok_or("expected bits string")?
        .to_string_lossy();
    u64::from_str_radix(&text, 16)
        .map(f64::from_bits)
        .map_err(|e| format!("bad bits {text}: {e}"))
}

fn js(json: &Json) -> Result<JsString, String> {
    json.as_str()
        .map(|s| JsString::from_units(s.units().to_vec()))
        .ok_or_else(|| "expected string".to_owned())
}

fn apply_ops(log: &mut Log, ops: &Json) -> Result<(), String> {
    for op in ops.as_array().ok_or("ops must be an array")? {
        let op = op.as_array().ok_or("op must be an array")?;
        match op
            .first()
            .and_then(Json::as_str)
            .map(|s| s.to_string_lossy())
            .as_deref()
        {
            Some("range") => {
                log.update_range_with_timestamp(bits(&op[1])?);
                log.update_range_with_timestamp(bits(&op[2])?);
            }
            Some("field") => {
                let name = js(&op[1])?;
                let blank = op[2].as_f64().ok_or("blank type must be a number")?;
                log.create_blank_field(&name, log_field::loggable_type(&Value::number(blank))?);
                if let Json::String(_) = &op[5] {
                    log.set_structured_type(&name, Some(js(&op[5])?));
                }
                log.set_wpilib_type(&name, js(&op[3])?);
                log.set_metadata_string(&name, js(&op[4])?);
            }
            Some("meta") => log.set_metadata_string(&js(&op[1])?, js(&op[2])?),
            Some("del") => log.delete_field(&js(&op[1])?),
            other => return Err(format!("unknown op {other:?}")),
        }
    }
    Ok(())
}

/// The `parseField` type switch for one record, including the CustomSchemas hook.
/// Errors are reported to the caller, which swallows them like the worker's catch.
fn parse_record(
    log: &mut Log,
    key: &JsString,
    type_: &str,
    timestamp: f64,
    v: &Value,
) -> Result<(), String> {
    match type_ {
        "boolean" => log.put_boolean(key, timestamp, log_field::boolean(v)?),
        "int" | "int64" | "float" | "double" => {
            log.put_number(key, timestamp, log_field::number(v)?)
        }
        "string" => log.put_string(key, timestamp, log_field::js_string(v)?),
        "boolean[]" => {
            log.put_boolean_array(key, timestamp, log_field::array(v, log_field::boolean)?)
        }
        "int[]" | "int64[]" | "float[]" | "double[]" => {
            log.put_number_array(key, timestamp, log_field::array(v, log_field::number)?)
        }
        "string[]" => {
            log.put_string_array(key, timestamp, log_field::array(v, log_field::js_string)?)
        }
        "json" => log.put_json(key, timestamp, log_field::js_string(v)?),
        _ => {
            if type_.starts_with("struct:")
                || type_.starts_with("photon:")
                || type_.starts_with("proto:")
            {
                return Err(format!("unsupported type {type_}"));
            }
            let bytes = log_run::bytes(v)?;
            log.put_raw(key, timestamp, bytes.as_ref());
            if let Some(parser) = urcl::custom_schema(type_) {
                let _ = parser(log, key, timestamp, &bytes);
                log.set_generated_parent(key);
            }
        }
    }
    Ok(())
}

/// Structured-clone canonicalizes NaN payloads across the worker boundary.
fn canonicalize(v: &mut Value) {
    match v {
        Value::Number(bits) if f64::from_bits(*bits).is_nan() => *bits = f64::NAN.to_bits(),
        Value::Array(items) => items.iter_mut().for_each(canonicalize),
        Value::Object(pairs) => pairs.iter_mut().for_each(|(_, item)| canonicalize(item)),
        _ => {}
    }
}

fn replay(case: &Json) -> Result<Value, String> {
    let mut log = Log::new(false);
    apply_ops(&mut log, case.get("ops").ok_or("case has no ops")?)?;
    log.get_changed_fields();
    let initial = log.to_serialized();
    let initial_range = initial.timestamp_range;
    let initial_zero = initial.timestamp_zero;
    let mut parents = initial.generated_parents;
    let requests = case
        .get("requests")
        .and_then(Json::as_array)
        .ok_or("case has no requests")?;
    let types = case.get("types").ok_or("case has no types")?;
    let records = case.get("records").ok_or("case has no records")?;
    for key_json in requests {
        let key = js(key_json)?;
        let Some(type_) = types.get(&key.to_string_lossy()) else {
            continue; // the worker warns and skips unknown keys
        };
        let type_ = type_
            .as_str()
            .ok_or("type must be a string")?
            .to_string_lossy();
        let Some(recs) = records.get(&key.to_string_lossy()) else {
            continue;
        };
        for record in recs.as_array().ok_or("records must be an array")? {
            if record.get("null").is_some() || record.get("throw").is_some() {
                continue;
            }
            let timestamp = bits(record.get("t").ok_or("record has no t")?)? / 1e6;
            let v = Value::from_json(record.get("v").ok_or("record has no v")?)
                .map_err(|e| format!("record value: {}", e.0))?;
            let _ = parse_record(&mut log, &key, &type_, timestamp, &v);
        }
        let mut has_root = false;
        for child in log.get_changed_fields() {
            if child == key {
                has_root = true;
            }
            if log.is_generated_parent(&child) && !parents.contains(&child) {
                parents.push(child);
            }
        }
        if !has_root
            && log.get_field(&key).is_some()
            && log.is_generated_parent(&key)
            && !parents.contains(&key)
        {
            parents.push(key.clone());
        }
    }
    let mut dump = log_run::dump(&log);
    if let Value::Object(pairs) = &mut dump {
        for (k, v) in pairs.iter_mut() {
            match k.to_string_lossy().as_str() {
                "generatedParents" => {
                    *v = Value::Array(parents.iter().map(log_field::string_value).collect());
                }
                "timestampRange" => {
                    *v = initial_range.map_or(Value::Null, |r| log_field::numbers(&r));
                }
                "timestampZero" => {
                    *v = initial_zero.map_or(Value::Null, Value::number);
                }
                _ => {}
            }
        }
    }
    canonicalize(&mut dump);
    Ok(dump)
}

#[test]
fn urcl() {
    let records = vectors::load("urcl/urcl").expect("load urcl/urcl vectors");
    let mut failures = Vec::new();
    let mut cases = 0;
    let mut index = 0;
    while index < records.len() {
        let record: &Record = &records[index];
        if record.string("t").as_deref() == Some("meta") {
            index += 1;
            continue;
        }
        assert_eq!(
            record.string("t").as_deref(),
            Some("case"),
            "line {}: expected a case record",
            record.line
        );
        let id = record.string("id").unwrap_or_default();
        let expected = records
            .get(index + 1)
            .filter(|r| r.string("t").as_deref() == Some("dump"))
            .and_then(|r| r.value("log").ok())
            .unwrap_or_else(|| panic!("line {}: case has no dump", record.line));
        match replay(&record.json) {
            Ok(actual) if actual == expected => {}
            Ok(actual) => {
                let detail =
                    diff::report(&expected, &actual).unwrap_or_else(|| "dumps differ".to_owned());
                failures.push(format!("{id}: {detail}"));
            }
            Err(e) => failures.push(format!("{id}: {e}")),
        }
        cases += 1;
        index += 2;
    }
    assert!(
        failures.is_empty(),
        "urcl/urcl: {} of {cases} cases differ\n{}",
        failures.len(),
        failures.join("\n")
    );
    eprintln!("urcl/urcl: {cases} cases, 0 mismatches");
}
