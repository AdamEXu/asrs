//! `oracle:photon`: replays a `PhotonStructDecoder` op list through the `asrs-log` port.
//! Ops are `["addSchema", name, bytes]`, `["decode"|"decodeImpl", name, bytes]` or
//! `["reload", ..]` (`from_serialized(to_serialized())`); each
//! result is wrapped `{"ok": v}` or `{"throw": e}` so a throw can't abort the session.

use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue};
use asrs_log::log::photon_struct_decoder::PhotonStructDecoder;
use asrs_parity::{Response, Utf16String, Value};

use crate::{functions, js_json};

pub const FUNCTIONS: &[(&str, functions::Function)] = &[("oracle:photon", photon)];

fn photon(args: &[Value]) -> Response {
    let Value::Array(ops) = functions::arg(args, 0) else {
        return Response::Error("oracle:photon: first argument must be an op array".to_owned());
    };
    let mut decoder = PhotonStructDecoder::new();
    let mut results = Vec::with_capacity(ops.len());
    for op in &ops {
        match run_op(&mut decoder, op) {
            Ok(ok) => results.push(Value::object([("ok", ok)])),
            Err(thrown) => results.push(Value::object([("throw", thrown)])),
        }
    }
    Response::Ok(Value::Array(results))
}

fn run_op(decoder: &mut PhotonStructDecoder, op: &Value) -> Result<Value, Value> {
    let Value::Array(parts) = op else {
        return Err(bad_op("op is not an array"));
    };
    let kind = match functions::to_units(parts.first().unwrap_or(&Value::Undefined)) {
        Ok(units) => String::from_utf16_lossy(&units),
        Err(_) => return Err(bad_op("op kind is not a string")),
    };
    let arg = |i: usize| parts.get(i).cloned().unwrap_or(Value::Undefined);
    match kind.as_str() {
        "reload" => {
            *decoder = PhotonStructDecoder::from_serialized(&decoder.to_serialized());
            Ok(js_json::from_js(&decoder.to_serialized()))
        }
        "addSchema" => {
            let name = name_arg(&arg(1))?;
            let bytes = bytes_arg(&arg(2))?;
            decoder.add_schema(&name, &bytes);
            Ok(Value::Array(
                decoder
                    .get_schemas()
                    .keys()
                    .map(|k| Value::String(utf16(k)))
                    .collect(),
            ))
        }
        "decode" => {
            let name = name_arg(&arg(1))?;
            let bytes = bytes_arg(&arg(2))?;
            let r = decoder.decode(&name, &bytes).map_err(thrown)?;
            Ok(Value::object([
                ("data", js_json::from_js(&r.data)),
                ("schemaTypes", schema_types(&r.schema_types)),
            ]))
        }
        "decodeImpl" => {
            let name = name_arg(&arg(1))?;
            let bytes = bytes_arg(&arg(2))?;
            let r = decoder.decode_impl(&name, &bytes).map_err(thrown)?;
            Ok(Value::object([
                ("data", js_json::from_js(&r.data)),
                ("schemaTypes", schema_types(&r.schema_types)),
                ("bytesConsumed", Value::number(r.bytes_consumed as f64)),
            ]))
        }
        _ => Err(bad_op(&format!("bad op {kind:?}"))),
    }
}

fn thrown(e: JsError) -> Value {
    Value::Error(Utf16String::from(e.name), Utf16String::from(e.message))
}

fn name_arg(value: &Value) -> Result<JsString, Value> {
    match functions::to_units(value) {
        Ok(units) => Ok(JsString::from_units(units)),
        Err(_) => Err(bad_op("schema name is not coercible")),
    }
}

fn bytes_arg(value: &Value) -> Result<Vec<u8>, Value> {
    match value {
        Value::Bytes(b) | Value::ArrayBuffer(b) | Value::DataView(b) => Ok(b.clone()),
        Value::TypedArray(kind, items) => {
            js_json::typed_array_bytes(kind, items).ok_or_else(|| bad_op("bad typed array kind"))
        }
        _ => Err(Value::Error(
            "TypeError".into(),
            "First argument to DataView constructor must be an ArrayBuffer"
                .to_owned()
                .into(),
        )),
    }
}

fn schema_types(map: &JsObject<JsValue>) -> Value {
    Value::Object(
        map.iter()
            .map(|(k, v)| (utf16(k), js_json::from_js(v)))
            .collect(),
    )
}

fn utf16(s: &JsString) -> Utf16String {
    Utf16String::from_units(s.units().to_vec())
}

fn bad_op(message: &str) -> Value {
    Value::Error(
        Utf16String::from("Error"),
        Utf16String::from(format!("oracle:photon: {message}")),
    )
}
