//! `fn` names for `asrs_js::json`, and the CVE-1 to `JsValue` conversions they need. The JSON
//! vector tests include this file too.

use asrs_js::json::{self, JsonError};
use asrs_js::math::BigInt;
use asrs_js::number;
use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue};
use asrs_parity::{Response, Utf16String, Value};

pub type Function = fn(&[Value]) -> Response;

pub const FUNCTIONS: &[(&str, Function)] = &[
    ("JSON.parse", parse),
    ("JSON.stringify", stringify),
    ("shared/util:jsonCopy", json_copy),
];

pub fn to_js(value: &Value) -> Result<JsValue, String> {
    Ok(match value {
        Value::Undefined | Value::Hole => JsValue::Undefined,
        Value::Null => JsValue::Null,
        Value::Bool(b) => JsValue::Bool(*b),
        Value::Number(bits) => JsValue::Number(f64::from_bits(*bits)),
        Value::String(s) => JsValue::String(JsString::from_units(s.units().to_vec())),
        Value::BigInt(digits) => {
            JsValue::BigInt(BigInt::parse(digits).map_err(|e| format!("bigint: {e}"))?)
        }
        Value::Array(items) => JsValue::Array(items.iter().map(to_js).collect::<Result<_, _>>()?),
        Value::Object(pairs) => {
            let mut object = JsObject::new();
            for (k, v) in pairs {
                object.insert(JsString::from_units(k.units().to_vec()), to_js(v)?);
            }
            JsValue::Object(object)
        }
        Value::Bytes(bytes) => JsValue::Bytes(bytes.clone()),
        Value::Date(bits) => JsValue::Date(f64::from_bits(*bits)),
        Value::Instance(name, pairs) if name.eq_str("ExtData") || name.eq_str("ExtData2") => {
            let kind = match pairs.iter().find(|(k, _)| k.eq_str("type")) {
                Some((_, Value::Number(bits))) => f64::from_bits(*bits) as i8,
                _ => return Err("ExtData needs a numeric type".to_owned()),
            };
            let data = match pairs.iter().find(|(k, _)| k.eq_str("data")) {
                Some((_, Value::Bytes(bytes))) => bytes.clone(),
                _ => return Err("ExtData needs binary data".to_owned()),
            };
            JsValue::ExtData(kind, data)
        }
        other => return Err(format!("no JsValue for CVE-1 {}", other.kind())),
    })
}

/// Upstream's harness converts each fn result with a recursive `tree()`: a `JSON.parse` of
/// very deep input succeeds upstream, then `tree` throws `RangeError`. Parsed values are
/// the only unbounded `JsValue` source here, so this bounds the conversion before the
/// recursion - and the recursive CVE-1 encoder downstream of it - can overflow.
const MAX_TREE_DEPTH: usize = 2800;

pub fn from_js_parsed(value: &JsValue) -> Result<Value, JsonError> {
    from_js_at(value, 0)
}

fn from_js_at(value: &JsValue, depth: usize) -> Result<Value, JsonError> {
    if depth > MAX_TREE_DEPTH {
        return Err(JsonError {
            name: "RangeError",
            message: JsString::from("Maximum call stack size exceeded"),
        });
    }
    Ok(match value {
        JsValue::Undefined => Value::Undefined,
        JsValue::Null => Value::Null,
        JsValue::Bool(b) => Value::Bool(*b),
        JsValue::Number(x) => Value::number(*x),
        JsValue::String(s) => Value::String(utf16(s)),
        JsValue::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(from_js_at(item, depth + 1)?);
            }
            Value::Array(out)
        }
        JsValue::Object(object) => {
            let mut out = Vec::with_capacity(object.len());
            for (k, v) in object.iter() {
                out.push((utf16(k), from_js_at(v, depth + 1)?));
            }
            Value::Object(out)
        }
        JsValue::Bytes(bytes) => Value::Bytes(bytes.clone()),
        JsValue::BigInt(b) => Value::BigInt(format!("{b}")),
        JsValue::Date(t) => Value::Date(t.to_bits()),
        JsValue::ExtData(kind, data) => ext_data_value(*kind, data),
    })
}

pub fn from_js(value: &JsValue) -> Value {
    match value {
        JsValue::Undefined => Value::Undefined,
        JsValue::Null => Value::Null,
        JsValue::Bool(b) => Value::Bool(*b),
        JsValue::Number(x) => Value::number(*x),
        JsValue::String(s) => Value::String(utf16(s)),
        JsValue::Array(items) => Value::Array(items.iter().map(from_js).collect()),
        JsValue::Object(object) => {
            Value::Object(object.iter().map(|(k, v)| (utf16(k), from_js(v))).collect())
        }
        JsValue::Bytes(bytes) => Value::Bytes(bytes.clone()),
        JsValue::BigInt(b) => Value::BigInt(format!("{b}")),
        JsValue::Date(t) => Value::Date(t.to_bits()),
        JsValue::ExtData(kind, data) => ext_data_value(*kind, data),
    }
}

fn ext_data_value(kind: i8, data: &[u8]) -> Value {
    Value::Instance(
        // The bundled oracle mangles the class name to `ExtData2`.
        Utf16String::from("ExtData2"),
        vec![
            (Utf16String::from("type"), Value::number(f64::from(kind))),
            (Utf16String::from("data"), Value::Bytes(data.to_vec())),
        ],
    )
}

fn utf16(s: &JsString) -> Utf16String {
    Utf16String::from_units(s.units().to_vec())
}

fn throw(e: JsonError) -> Response {
    Response::Throw(Value::Error(e.name.into(), utf16(&e.message)))
}

fn tree_result(value: &JsValue) -> Response {
    match from_js_parsed(value) {
        Ok(v) => Response::Ok(v),
        Err(e) => throw(e),
    }
}

fn parse(args: &[Value]) -> Response {
    let text: Vec<u16> = match args.first() {
        Some(Value::String(s)) => s.units().to_vec(),
        Some(Value::Undefined) | None => "undefined".encode_utf16().collect(),
        Some(Value::Null) => "null".encode_utf16().collect(),
        Some(Value::Bool(b)) => b
            .then_some("true")
            .unwrap_or("false")
            .encode_utf16()
            .collect(),
        Some(Value::Number(bits)) => number::to_string(f64::from_bits(*bits))
            .encode_utf16()
            .collect(),
        Some(other) => return Response::Error(format!("JSON.parse: unsupported {}", other.kind())),
    };
    match json::parse_units(&text) {
        Ok(v) => tree_result(&v),
        Err(e) => throw(e),
    }
}

fn gap(space: Option<&Value>) -> Result<String, String> {
    Ok(match space {
        None | Some(Value::Undefined | Value::Null) => String::new(),
        Some(Value::Number(bits)) => json::gap_from_number(f64::from_bits(*bits)),
        Some(Value::String(s)) => {
            let gap = json::gap_from_string(&JsString::from_units(s.units().to_vec()));
            gap.as_string()
                .ok_or("JSON.stringify: gap with a lone surrogate")?
        }
        Some(other) => {
            return Err(format!(
                "JSON.stringify: unsupported space {}",
                other.kind()
            ));
        }
    })
}

fn stringify(args: &[Value]) -> Response {
    if !matches!(args.get(1), None | Some(Value::Undefined | Value::Null)) {
        return Response::Error("JSON.stringify: replacers are not ported".to_owned());
    }
    let value = match args.first().map_or(Ok(JsValue::Undefined), to_js) {
        Ok(v) => v,
        Err(e) => return Response::Error(e),
    };
    let gap = match gap(args.get(2)) {
        Ok(g) => g,
        Err(e) => return Response::Error(e),
    };
    // V8 only takes its unbounded iterative serializer when `replacer` and `space` are
    // both `undefined`; any defined argument recurses and can throw `RangeError`.
    let fast = matches!(args.get(1), None | Some(Value::Undefined))
        && matches!(args.get(2), None | Some(Value::Undefined));
    let result = if fast {
        json::stringify(&value)
    } else {
        json::stringify_gap(&value, &gap)
    };
    match result {
        Ok(Some(text)) => Response::Ok(Value::String(Utf16String::from(text))),
        Ok(None) => Response::Ok(Value::Undefined),
        Err(e) => throw(e),
    }
}

fn json_copy(args: &[Value]) -> Response {
    let value = match args.first().map_or(Ok(JsValue::Undefined), to_js) {
        Ok(v) => v,
        Err(e) => return Response::Error(e),
    };
    match json::json_copy(&value) {
        Ok(v) => tree_result(&v),
        Err(e) => throw(e),
    }
}

/// The CVE-1 values an encoder call can receive that `JsValue` does not model. What the
/// encoder sees is `Object.keys`/view bytes, so Maps, Sets, boxed primitives and bare
/// ArrayBuffers all encode like an empty object (a boxed String enumerates its chars),
/// and every view encodes its bytes.
pub fn to_js_encodable(value: &Value) -> Result<JsValue, String> {
    to_js(&normalize_encodable(value))
}

fn normalize_encodable(value: &Value) -> Value {
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
        Value::Array(items) => Value::Array(items.iter().map(normalize_encodable).collect()),
        Value::Object(pairs) => Value::Object(
            pairs
                .iter()
                .map(|(k, v)| (k.clone(), normalize_encodable(v)))
                .collect(),
        ),
        Value::Instance(name, pairs) => Value::Instance(
            name.clone(),
            pairs
                .iter()
                .map(|(k, v)| (k.clone(), normalize_encodable(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Rebuilds the little-endian buffer bytes a typed array's elements represent.
pub fn typed_array_bytes(kind: &str, items: &[Value]) -> Option<Vec<u8>> {
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
