//! Replays the `genUrcl` oracle vectors. Each `case` line holds the wpilog worker's
//! index-time ops plus the per-key records it decodes; the following `dump` line is the
//! hub-side Log of a real upstream session. The replay keeps the worker's Log and the hub's
//! Log apart and moves the `initial` and `fields` responses between them as the decode
//! thread does.

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

use asrs_formats::custom_schemas::{self, SchemaError};
use asrs_js::string::JsString;
use asrs_js::time::V8MathRandom;
use asrs_log::log::{Log, LogField};
use asrs_log::realm::JsRealm;
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

/// The `parseField` type switch for one record. F02's wpilog worker replaces this once merged.
fn parse_record(
    log: &mut Log,
    key: &JsString,
    type_: &JsString,
    timestamp: f64,
    v: &Value,
) -> Result<(), String> {
    match type_.to_string_lossy().as_str() {
        "boolean" => log
            .put_boolean(key, timestamp, log_field::boolean(v)?)
            .unwrap(),
        "int" | "int64" | "float" | "double" => log
            .put_number(key, timestamp, log_field::number(v)?)
            .unwrap(),
        "string" => log
            .put_string(key, timestamp, log_field::js_string(v)?)
            .unwrap(),
        "boolean[]" => log
            .put_boolean_array(key, timestamp, log_field::array(v, log_field::boolean)?)
            .unwrap(),
        "int[]" | "int64[]" | "float[]" | "double[]" => log
            .put_number_array(key, timestamp, log_field::array(v, log_field::number)?)
            .unwrap(),
        "string[]" => log
            .put_string_array(key, timestamp, log_field::array(v, log_field::js_string)?)
            .unwrap(),
        "json" => log
            .put_json(key, timestamp, log_field::js_string(v)?)
            .map_err(|e| format!("{e:?}"))?,
        other => {
            if other.starts_with("struct:")
                || other.starts_with("photon:")
                || other.starts_with("proto:")
                || other == "msgpack"
            {
                return Err(format!("unsupported type {other}"));
            }
            let bytes = log_run::bytes(v)?;
            log.put_raw(key, timestamp, bytes.as_ref())
                .map_err(|e| format!("putRaw: {e:?}"))?;
            if let Some(parser) = custom_schemas::get(type_) {
                match parser(log, key, timestamp, &bytes) {
                    Ok(()) | Err(SchemaError::Threw) => {}
                    Err(SchemaError::Log(e)) => return Err(format!("custom schema: {e:?}")),
                }
                log.set_generated_parent(key);
            }
        }
    }
    Ok(())
}

/// The worker's `fields` response for `key`, applied to the hub's Log.
fn post_fields(worker: &mut Log, hub: &mut Log, key: &JsString) {
    let mut fields = Vec::new();
    let mut has_root = false;
    for child in worker.get_changed_fields() {
        has_root |= child == *key;
        fields.push(child);
    }
    if !has_root && worker.get_field(key).is_some() {
        fields.push(key.clone());
    }
    for child in fields {
        let field = worker.get_field(&child).expect("changed field exists");
        hub.set_field(
            &child,
            LogField::from_serialized(field.to_serialized().structured_clone()),
        );
        if worker.is_generated_parent(&child) {
            hub.set_generated_parent(&child);
        }
    }
}

fn replay(case: &Json) -> Result<Value, String> {
    let mut realm = JsRealm::new(Box::new(V8MathRandom::from_entropy()));
    let mut worker = Log::new(false);
    apply_ops(&mut worker, case.get("ops").ok_or("case has no ops")?)?;
    worker.get_changed_fields();
    let initial = Log::from_serialized(worker.to_serialized().structured_clone(), &mut realm)
        .map_err(|e| format!("fromSerialized: {e:?}"))?;
    let mut hub = Log::new(true);
    hub.merge_with(&initial, &JsString::new(), &mut realm)
        .map_err(|e| format!("mergeWith: {e:?}"))?;

    let requests = case
        .get("requests")
        .and_then(Json::as_array)
        .ok_or("case has no requests")?;
    let types = case.get("types").ok_or("case has no types")?;
    let records = case.get("records").ok_or("case has no records")?;
    for key_json in requests {
        let mut key = js(key_json)?;
        if types.get(&key.to_string_lossy()).is_none()
            && key.starts_with(&JsString::from("/"), None)
        {
            key = key.slice(1.0, None);
        }
        let Some(type_) = types.get(&key.to_string_lossy()) else {
            continue;
        };
        let type_ = js(type_)?;
        if let Some(recs) = records.get(&key.to_string_lossy()) {
            for record in recs.as_array().ok_or("records must be an array")? {
                if record.get("null").is_some() || record.get("throw").is_some() {
                    continue;
                }
                let timestamp = bits(record.get("t").ok_or("record has no t")?)? / 1e6;
                let v = Value::from_json(record.get("v").ok_or("record has no v")?)
                    .map_err(|e| format!("record value: {}", e.0))?;
                parse_record(&mut worker, &key, &type_, timestamp, &v)?;
            }
        }
        post_fields(&mut worker, &mut hub, &key);
    }
    Ok(log_run::dump(&hub))
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
