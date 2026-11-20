use asrs_js::string::JsError;
use asrs_js::value::JsValue;
use asrs_log::msgpack;
use asrs_parity::{Response, Utf16String, Value};

use crate::{functions, js_json};

fn respond(result: Result<JsValue, JsError>) -> Response {
    match result {
        Ok(v) => Response::Ok(js_json::from_js(&v)),
        Err(e) => throw(e),
    }
}

fn throw(e: JsError) -> Response {
    Response::Throw(Value::Error(
        Utf16String::from(e.name),
        Utf16String::from(e.message),
    ))
}

fn bytes_arg(args: &[Value]) -> Result<Vec<u8>, Response> {
    match functions::arg(args, 0) {
        Value::Bytes(b) | Value::ArrayBuffer(b) | Value::DataView(b) => Ok(b),
        Value::TypedArray(kind, items) => typed_array_bytes(&kind, &items)
            .ok_or_else(|| Response::Error("bad typed array kind".to_owned())),
        _ => Err(Response::Error("expected bytes".to_owned())),
    }
}

/// Rebuilds the little-endian buffer bytes a typed array's elements represent.
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

/// The CVE-1 values an encoder call can receive that `JsValue` does not model. What the
/// encoder sees is `Object.keys`/view bytes, so Maps, Sets, boxed primitives and bare
/// ArrayBuffers all encode like an empty object (a boxed String enumerates its chars),
/// and every view encodes its bytes.
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

fn decode(args: &[Value]) -> Response {
    match bytes_arg(args) {
        Ok(bytes) => respond(msgpack::decode(&bytes)),
        Err(r) => r,
    }
}

fn encode(args: &[Value]) -> Response {
    match to_js(&functions::arg(args, 0)) {
        Ok(v) => match msgpack::encode(&v) {
            Ok(bytes) => Response::Ok(Value::Bytes(bytes)),
            Err(e) => throw(e),
        },
        Err(e) => Response::Error(e),
    }
}

fn decode_multi(args: &[Value]) -> Response {
    match bytes_arg(args) {
        Ok(bytes) => {
            let (values, err) = msgpack::decode_multi(&bytes);
            match err {
                Some(e) => throw(e),
                None => Response::Ok(Value::Array(
                    values.iter().map(js_json::from_js).collect(),
                )),
            }
        }
        Err(r) => r,
    }
}

pub const FUNCTIONS: &[(&str, functions::Function)] = &[
    ("npm:@msgpack/msgpack:decode", decode),
    ("npm:@msgpack/msgpack:decodeMulti", decode_multi),
    ("npm:@msgpack/msgpack:encode", encode),
];
