#![allow(clippy::disallowed_methods)]

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::collections::{HashMap, VecDeque};

use asrs_js::string::JsString;
use asrs_js::value::ToJs;
use asrs_log::log::proto_decoder::{ProtoDecoder, ProtoError, ProtoRealm, SerializedProtoDecoder};
use asrs_parity::json::Json;
use asrs_parity::vectors::check_records;
use asrs_parity::{Record, Utf16String, Value};

type Item = (JsString, Vec<u8>);

#[derive(Default)]
struct Session {
    decoder: ProtoDecoder,
    queue: Vec<Item>,
    drain: VecDeque<Item>,
}

fn hex(json: &Json) -> Result<Vec<u8>, String> {
    let s = json
        .as_str()
        .ok_or_else(|| format!("expected hex, got {json:?}"))?
        .to_string_lossy();
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|e| format!("hex: {e}")))
        .collect()
}

fn hex_string(bytes: &[u8]) -> Value {
    Value::string(&bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

fn field<'a>(json: &'a Json, key: &str) -> Result<&'a Json, String> {
    json.get(key)
        .ok_or_else(|| format!("missing {key:?} in {json:?}"))
}

/// A `Long` instance is CVE-1 tagged with its class; the port hands the log the same own
/// properties as a plain object.
fn plain_longs(value: Value) -> Value {
    match value {
        Value::Instance(name, pairs) if name.eq_str("Long") || name.eq_str("Long2") => {
            Value::Object(
                pairs
                    .into_iter()
                    .map(|(k, v)| (k, plain_longs(v)))
                    .collect(),
            )
        }
        Value::Object(pairs) => Value::Object(
            pairs
                .into_iter()
                .map(|(k, v)| (k, plain_longs(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(plain_longs).collect()),
        other => other,
    }
}

fn serialized(decoder: &ProtoDecoder) -> Value {
    Value::Array(
        decoder
            .to_serialized()
            .0
            .iter()
            .map(|d| hex_string(d))
            .collect(),
    )
}

fn thrown(error: &ProtoError) -> Value {
    Value::object([(
        "throw",
        Value::Error(
            Utf16String::from(error.js_name()),
            Utf16String::from_units(error.js_message().units().to_vec()),
        ),
    )])
}

fn compare(record: &Record, actual: Value) -> Result<(), String> {
    let (key, expected) = if let Some(ok) = record.field("ok") {
        ("ok", ok)
    } else if let Some(throw) = record.field("throw") {
        ("throw", throw)
    } else {
        return Err("record has neither ok nor throw".to_owned());
    };
    let expected = plain_longs(Value::from_json(expected).map_err(|e| e.to_string())?);
    let expected = Value::object([(key, expected)]);
    if actual != expected {
        return Err(format!("expected {expected:?}\ngot      {actual:?}"));
    }
    Ok(())
}

fn replay(record: &Record, session: &mut Session, realm: &mut ProtoRealm) -> Result<(), String> {
    if let Some(s) = record.field("s") {
        let bytes = hex(field(s, "b")?)?;
        return match session.decoder.add_descriptor(realm, &bytes) {
            Err(e) => compare(record, thrown(&e)),
            Ok(()) => {
                compare(
                    record,
                    Value::object([("ok", serialized(&session.decoder))]),
                )?;
                session.drain = std::mem::take(&mut session.queue).into();
                Ok(())
            }
        };
    }
    if let Some(Json::Array(items)) = record.field("r") {
        let descriptors = items.iter().map(hex).collect::<Result<Vec<_>, _>>()?;
        return match ProtoDecoder::from_serialized(realm, &SerializedProtoDecoder(descriptors)) {
            Err(e) => compare(record, thrown(&e)),
            Ok(decoder) => {
                session.decoder = decoder;
                compare(
                    record,
                    Value::object([("ok", serialized(&session.decoder))]),
                )
            }
        };
    }
    let d = field(&record.json, "d")?;
    let name = d
        .get("n")
        .and_then(Json::as_str)
        .map(|s| JsString::from_units(s.units().to_vec()))
        .ok_or("missing type name")?;
    let bytes = hex(field(d, "b")?)?;
    if matches!(d.get("q"), Some(Json::Bool(true))) {
        let item = session
            .drain
            .pop_front()
            .ok_or("queued decode but the drain is empty")?;
        if item != (name.clone(), bytes.clone()) {
            return Err(format!("queued decode mismatch: drain has {:?}", item.0));
        }
    }
    match session.decoder.decode(&name, &bytes) {
        Ok(decoded) => compare(
            record,
            Value::object([(
                "ok",
                Value::object([
                    ("data", js_json::from_js(&decoded.data)),
                    (
                        "schemaTypes",
                        js_json::from_js(&decoded.schema_types.to_js()),
                    ),
                ]),
            )]),
        ),
        Err(e) => {
            session.queue.push((name, bytes));
            compare(record, thrown(&e))
        }
    }
}

fn check(id: &str) {
    let mut sessions: HashMap<String, Session> = HashMap::new();
    let mut realm = ProtoRealm::new();
    check_records(id, |record| {
        let case = record.string("case").ok_or("missing case")?;
        replay(record, sessions.entry(case).or_default(), &mut realm)
    });
}

#[test]
fn replay_vectors() {
    check("log/proto-replay");
}

#[test]
fn matrix_vectors() {
    check("log/proto-matrix");
}

#[test]
fn fuzz_vectors() {
    check("log/proto-fuzz");
}
