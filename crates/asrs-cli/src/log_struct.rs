//! `oracle:struct`: the op-list driver the oracle's `struct` helper performs
//! against upstream's StructDecoder. Every op delegates to
//! `asrs_log::log::struct_decoder::StructDecoder`; only CVE-1 conversion
//! happens here.

use asrs_js::string::{JsError, JsString};
use asrs_js::value::JsValue;
use asrs_log::log::struct_decoder::StructDecoder;
use asrs_parity::{Response, Utf16String, Value};

use crate::{functions, js_json};

pub const FUNCTIONS: &[(&str, functions::Function)] = &[("oracle:struct", struct_fn)];

fn struct_fn(args: &[Value]) -> Response {
    let Value::Array(ops) = functions::arg(args, 0) else {
        return Response::Error("oracle:struct: ops must be an array".to_owned());
    };
    let mut decoders = vec![StructDecoder::new()];
    let mut results = Vec::with_capacity(ops.len());
    for op in ops {
        results.push(run_op(&mut decoders, &op));
    }
    // Upstream hands back `decoder.toSerialized()` by reference: results are
    // only serialized once the whole op list has run, so each of them shows
    // that decoder instance's final state.
    Response::Ok(Value::Array(
        results
            .into_iter()
            .map(|result| match result {
                Ok(OpOut::Live(i)) => outcome(Ok(decoders[i].to_serialized())),
                Ok(OpOut::Now(value)) => outcome(Ok(value)),
                Err(e) => outcome(Err(e)),
            })
            .collect(),
    ))
}

enum OpOut {
    /// The `toSerialized()` reference of the decoder at the given index.
    Live(usize),
    Now(JsValue),
}

fn run_op(decoders: &mut Vec<StructDecoder>, op: &Value) -> Result<OpOut, JsError> {
    let Value::Array(parts) = op else {
        return Err(JsError::new("Error", "oracle:struct: op is not an array"));
    };
    let kind = match functions::arg(parts, 0) {
        Value::String(s) => s.to_string_lossy(),
        _ => String::new(),
    };
    let live = decoders.len() - 1;
    let decoder = decoders.last_mut().expect("nonempty");
    match kind.as_str() {
        "addSchema" => match (name_arg(parts, 1), bytes_arg(parts, 2)) {
            (Ok(name), Ok(bytes)) => decoder
                .add_schema(&name, &bytes)
                .map(|()| OpOut::Live(live)),
            (Err(e), _) | (_, Err(e)) => Err(e),
        },
        "decode" => match (name_arg(parts, 1), bytes_arg(parts, 2)) {
            (Ok(name), Ok(bytes)) => decoder
                .decode(&name, &bytes)
                .map(|d| OpOut::Now(decoded_to_js(d))),
            (Err(e), _) | (_, Err(e)) => Err(e),
        },
        "decodeArray" => match (name_arg(parts, 1), bytes_arg(parts, 2)) {
            (Ok(name), Ok(bytes)) => decoder
                .decode_array(&name, &bytes, length_arg(parts, 3))
                .map(|d| OpOut::Now(decoded_to_js(d))),
            (Err(e), _) | (_, Err(e)) => Err(e),
        },
        "toSerialized" => Ok(OpOut::Live(live)),
        "fromSerialized" => match js_json::to_js(&functions::arg(parts, 1)) {
            Ok(value) => match StructDecoder::from_serialized(&value) {
                Ok(restored) => {
                    decoders.push(restored);
                    Ok(OpOut::Live(live + 1))
                }
                Err(e) => Err(e),
            },
            Err(message) => Err(JsError::type_error(message)),
        },
        _ => Err(JsError::new(
            "Error",
            format!("oracle:struct: unknown op {kind}"),
        )),
    }
}

fn decoded_to_js(decoded: asrs_log::log::struct_decoder::Decoded) -> JsValue {
    let mut schema_types = asrs_js::value::JsObject::new();
    for (key, ty) in decoded.schema_types.iter() {
        schema_types.insert(key.clone(), JsValue::String(ty.clone()));
    }
    JsValue::Object(asrs_js::value::JsObject::from_iter([
        (JsString::from("data"), decoded.data),
        (JsString::from("schemaTypes"), JsValue::Object(schema_types)),
    ]))
}

fn name_arg(parts: &[Value], i: usize) -> Result<JsString, JsError> {
    match functions::to_units(&functions::arg(parts, i)) {
        Ok(units) => Ok(JsString::from_units(units)),
        Err(Response::Throw(Value::Error(_, message))) => {
            Err(JsError::type_error(message.to_string_lossy()))
        }
        Err(_) => Err(JsError::type_error("oracle:struct: bad name")),
    }
}

fn bytes_arg(parts: &[Value], i: usize) -> Result<Vec<u8>, JsError> {
    match functions::arg(parts, i) {
        Value::Bytes(b) | Value::ArrayBuffer(b) | Value::DataView(b) => Ok(b),
        Value::TypedArray(kind, items) => js_json::typed_array_bytes(&kind, &items)
            .ok_or_else(|| JsError::type_error("oracle:struct: bad typed array")),
        _ => Err(JsError::type_error(
            "The provided value is not of type '(ArrayBuffer or ArrayBufferView)'",
        )),
    }
}

fn length_arg(parts: &[Value], i: usize) -> Option<f64> {
    match functions::arg(parts, i) {
        Value::Undefined => None,
        value => Some(functions::to_number(&value).unwrap_or(f64::NAN)),
    }
}

fn outcome(result: Result<JsValue, JsError>) -> Value {
    match result {
        Ok(value) => Value::object([("ok", js_json::from_js(&value))]),
        Err(e) => Value::object([(
            "throw",
            Value::Error(Utf16String::from(e.name), Utf16String::from(e.message)),
        )]),
    }
}
