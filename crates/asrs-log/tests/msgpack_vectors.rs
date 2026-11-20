#![allow(clippy::disallowed_methods)]

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use asrs_js::string::JsError;
use asrs_js::value::{JsObject, JsValue};
use asrs_log::msgpack;
use asrs_parity::vectors::check_records;
use asrs_parity::{Record, Utf16String, Value};

fn to_js(value: &Value) -> Result<JsValue, String> {
    js_json::to_js(&normalize(value))
}

fn normalize(value: &Value) -> Value {
    match value {
        Value::Map(_) | Value::Set(_) | Value::ArrayBuffer(_) => Value::Object(Vec::new()),
        Value::Boxed(inner) => match inner.as_ref() {
            Value::String(s) => Value::Object(
                s.units()
                    .iter()
                    .enumerate()
                    .map(|(i, u)| {
                        (
                            Utf16String::from(i.to_string()),
                            Value::String(Utf16String::from_units(vec![*u])),
                        )
                    })
                    .collect(),
            ),
            _ => Value::Object(Vec::new()),
        },
        Value::DataView(b) => Value::Bytes(b.clone()),
        Value::TypedArray(kind, items) => typed_array_bytes(kind, items)
            .map(Value::Bytes)
            .unwrap_or_else(|| value.clone()),
        Value::Array(items) => Value::Array(items.iter().map(normalize).collect()),
        Value::Object(pairs) => Value::Object(
            pairs
                .iter()
                .map(|(k, v)| (k.clone(), normalize(v)))
                .collect(),
        ),
        Value::Instance(name, pairs) => Value::Instance(
            name.clone(),
            pairs
                .iter()
                .map(|(k, v)| (k.clone(), normalize(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn typed_array_bytes(kind: &str, items: &[Value]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    for item in items {
        let bits = match item {
            Value::Number(bits) => f64::from_bits(*bits),
            Value::BigInt(digits) => {
                let v: i128 = digits.parse().ok()?;
                out.extend_from_slice(&(v as i64).to_le_bytes());
                continue;
            }
            _ => return None,
        };
        match kind {
            "Int8Array" => out.push(bits as i8 as u8),
            "Uint8Array" | "Uint8ClampedArray" => out.push(bits as u8),
            "Int16Array" => out.extend_from_slice(&(bits as i16).to_le_bytes()),
            "Uint16Array" => out.extend_from_slice(&(bits as u16).to_le_bytes()),
            "Int32Array" => out.extend_from_slice(&(bits as i32).to_le_bytes()),
            "Uint32Array" => out.extend_from_slice(&(bits as u32).to_le_bytes()),
            "Float32Array" => out.extend_from_slice(&(bits as f32).to_le_bytes()),
            "Float64Array" => out.extend_from_slice(&bits.to_le_bytes()),
            _ => return None,
        }
    }
    Some(out)
}

fn thrown(e: JsError) -> Value {
    Value::Error(Utf16String::from(e.name), Utf16String::from(e.message))
}

fn hex_bytes(record: &Record, key: &str) -> Result<Vec<u8>, String> {
    match record.value(key).map_err(|e| e.to_string())? {
        Value::Bytes(b) => Ok(b),
        other => Err(format!("{key} is not bytes: {other:?}")),
    }
}

fn outcome(record: &Record) -> Result<Result<Value, Value>, String> {
    if let Some(ok) = record.field("ok") {
        Ok(Ok(Value::from_json(ok).map_err(|e| e.to_string())?))
    } else if let Some(e) = record.field("throw") {
        Ok(Err(Value::from_json(e).map_err(|e| e.to_string())?))
    } else {
        Err("record has neither ok nor throw".to_owned())
    }
}

#[test]
fn decode_vectors() {
    check_records("msgpack/decode", |record| {
        let bytes = hex_bytes(record, "d")?;
        let actual = msgpack::decode(&bytes).map(|v| js_json::from_js(&v)).map_err(thrown);
        let expected = outcome(record)?;
        if actual != expected {
            return Err(format!("expected {expected:?}\ngot {actual:?}"));
        }
        Ok(())
    });
}

#[test]
fn multi_vectors() {
    check_records("msgpack/multi", |record| {
        let bytes = hex_bytes(record, "m")?;
        let (values, err) = msgpack::decode_multi(&bytes);
        let actual = match err {
            Some(e) => Err(thrown(e)),
            None => Ok(Value::Array(values.iter().map(js_json::from_js).collect())),
        };
        let expected = outcome(record)?;
        if actual != expected {
            return Err(format!("expected {expected:?}\ngot {actual:?}"));
        }
        Ok(())
    });
}

#[test]
fn encode_vectors() {
    check_records("msgpack/encode", |record| {
        let input = record.value("e").map_err(|e| e.to_string())?;
        let value = to_js(&input)?;
        let actual = msgpack::encode(&value).map(Value::Bytes).map_err(thrown);
        let expected = outcome(record)?;
        if actual != expected {
            return Err(format!("input {input:?}\nexpected {expected:?}\ngot {actual:?}"));
        }
        Ok(())
    });
}
