//! `oracle:struct`: the op list the oracle's `struct` helper runs against
//! upstream's StructDecoder, replayed on `asrs_log`'s. Only CVE-1 conversion
//! happens here.

use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::log::struct_decoder::{
    DecodeError, Decoded, Schema, SerializedStructDecoder, StructDecoder, ValueSchema,
};
use asrs_parity::{Response, Utf16String, Value};

use crate::{functions, js_json};

pub const FUNCTIONS: &[(&str, functions::Function)] = &[("oracle:struct", struct_fn)];

fn struct_fn(args: &[Value]) -> Response {
    let Value::Array(ops) = functions::arg(args, 0) else {
        return Response::Error("oracle:struct: ops must be an array".to_owned());
    };
    let mut decoder = StructDecoder::new();
    let mut results = Vec::with_capacity(ops.len());
    for op in &ops {
        match run_op(&mut decoder, op) {
            Ok(result) => results.push(result),
            Err(message) => return Response::Error(format!("oracle:struct: {message}")),
        }
    }
    Response::Ok(Value::Array(results))
}

fn run_op(decoder: &mut StructDecoder, op: &Value) -> Result<Value, String> {
    let Value::Array(parts) = op else {
        return Err("op is not an array".to_owned());
    };
    let kind = match functions::arg(parts, 0) {
        Value::String(s) => s.to_string_lossy(),
        other => return Err(format!("bad op kind {other:?}")),
    };
    let serialized = |decoder: &StructDecoder| ok(&decoder.to_serialized().to_js());
    Ok(match kind.as_str() {
        "addSchema" => {
            decoder.add_schema(&name_arg(parts, 1)?, &bytes_arg(parts, 2)?);
            serialized(decoder)
        }
        "decode" => decoded(decoder.decode(&name_arg(parts, 1)?, &bytes_arg(parts, 2)?)),
        "decodeArray" => decoded(decoder.decode_array(
            &name_arg(parts, 1)?,
            &bytes_arg(parts, 2)?,
            length_arg(parts, 3)?,
        )),
        "toSerialized" => serialized(decoder),
        "fromSerialized" => {
            let data = js_json::to_js(&functions::arg(parts, 1))?;
            *decoder = StructDecoder::from_serialized(parse_serialized(&data)?);
            serialized(decoder)
        }
        _ => return Err(format!("unknown op {kind}")),
    })
}

fn ok(value: &JsValue) -> Value {
    Value::object([("ok", js_json::from_js(value))])
}

fn decoded(result: Result<Decoded, DecodeError>) -> Value {
    match result {
        Ok(decoded) => ok(&decoded_to_js(&decoded)),
        Err(e) => {
            let (name, message) = js_error(e);
            Value::object([(
                "throw",
                Value::Error(Utf16String::from(name), Utf16String::from(message)),
            )])
        }
    }
}

pub fn decoded_to_js(decoded: &Decoded) -> JsValue {
    asrs_js::js_object! {
        "data" => decoded.data,
        "schemaTypes" => decoded.schema_types,
    }
}

/// The exception upstream throws for each error. `NeverCompletes` has none: the
/// oracle cannot record it.
pub fn js_error(error: DecodeError) -> (&'static str, &'static str) {
    match error {
        DecodeError::SchemaNotDefined => ("Error", "Schema not defined"),
        DecodeError::NotASchema => (
            "TypeError",
            "Cannot read properties of undefined (reading 'length')",
        ),
        DecodeError::OffsetOutOfBounds => ("RangeError", "offset is out of bounds"),
        DecodeError::InvalidArrayLength => ("RangeError", "Invalid array length"),
        DecodeError::NeverCompletes => ("asrs", "never completes"),
    }
}

fn name_arg(parts: &[Value], i: usize) -> Result<JsString, String> {
    match functions::arg(parts, i) {
        Value::String(s) => Ok(JsString::from_units(s.units().to_vec())),
        other => Err(format!("name must be a string, got {other:?}")),
    }
}

fn bytes_arg(parts: &[Value], i: usize) -> Result<Vec<u8>, String> {
    match functions::arg(parts, i) {
        Value::Bytes(b) => Ok(b),
        other => Err(format!("value must be a Uint8Array, got {other:?}")),
    }
}

fn length_arg(parts: &[Value], i: usize) -> Result<Option<f64>, String> {
    match functions::arg(parts, i) {
        Value::Undefined => Ok(None),
        Value::Number(bits) => Ok(Some(f64::from_bits(bits))),
        other => Err(format!("arrayLength must be a number, got {other:?}")),
    }
}

/// Reads a `toSerialized()` result back into its typed form.
pub fn parse_serialized(value: &JsValue) -> Result<SerializedStructDecoder, String> {
    let object = as_object(value, "serialized decoder")?;
    Ok(SerializedStructDecoder {
        schema_strings: map_of(object.get("schemaStrings"), |v| {
            as_string(v, "schema string")
        })?,
        schemas: map_of(object.get("schemas"), parse_schema)?,
    })
}

fn parse_schema(value: &JsValue) -> Result<Schema, String> {
    let object = as_object(value, "schema")?;
    let value_schemas = match object.get("valueSchemas") {
        Some(JsValue::Array(items)) => items
            .iter()
            .map(parse_value_schema)
            .collect::<Result<_, _>>()?,
        other => return Err(format!("valueSchemas must be an array, got {other:?}")),
    };
    Ok(Schema {
        length: as_number(object.get("length"), "length")?,
        value_schemas,
    })
}

fn parse_value_schema(value: &JsValue) -> Result<ValueSchema, String> {
    let object = as_object(value, "value schema")?;
    let undefined = JsValue::Undefined;
    let get = |key: &str| object.get(key).unwrap_or(&undefined);
    let nullable = |key: &str| match get(key) {
        JsValue::Null => Ok(None),
        v => as_number(Some(v), key).map(Some),
    };
    let bit_range = match get("bitRange") {
        JsValue::Array(items) if items.len() == 2 => [
            as_number(items.first(), "bitRange")?,
            as_number(items.get(1), "bitRange")?,
        ],
        other => return Err(format!("bad bitRange {other:?}")),
    };
    Ok(ValueSchema {
        name: as_string(get("name"), "name")?,
        ty: as_string(get("type"), "type")?,
        enum_data: match get("enum") {
            JsValue::Null => None,
            v => Some(map_of(Some(v), |name| as_string(name, "enum name"))?),
        },
        bitfield_width: nullable("bitfieldWidth")?,
        array_length: nullable("arrayLength")?,
        bit_range,
    })
}

fn map_of<V>(
    value: Option<&JsValue>,
    parse: impl Fn(&JsValue) -> Result<V, String>,
) -> Result<JsObject<V>, String> {
    let undefined = JsValue::Undefined;
    let object = as_object(value.unwrap_or(&undefined), "map")?;
    let mut out = JsObject::new();
    for (key, v) in object.iter() {
        out.insert(key.clone(), parse(v)?);
    }
    Ok(out)
}

fn as_object<'a>(value: &'a JsValue, what: &str) -> Result<&'a JsObject, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{what} must be an object, got {value:?}"))
}

fn as_string(value: &JsValue, what: &str) -> Result<JsString, String> {
    value
        .as_str()
        .cloned()
        .ok_or_else(|| format!("{what} must be a string, got {value:?}"))
}

fn as_number(value: Option<&JsValue>, what: &str) -> Result<f64, String> {
    value
        .and_then(JsValue::as_f64)
        .ok_or_else(|| format!("{what} must be a number, got {value:?}"))
}
