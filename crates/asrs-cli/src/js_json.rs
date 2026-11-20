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
        other => return Err(format!("no JsValue for CVE-1 {}", other.kind())),
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
    }
}

fn utf16(s: &JsString) -> Utf16String {
    Utf16String::from_units(s.units().to_vec())
}

fn throw(e: JsonError) -> Response {
    Response::Throw(Value::Error(e.name.into(), utf16(&e.message)))
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
        Ok(v) => Response::Ok(from_js(&v)),
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
    match json::stringify_gap(&value, &gap) {
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
        Ok(v) => Response::Ok(from_js(&v)),
        Err(e) => throw(e),
    }
}
