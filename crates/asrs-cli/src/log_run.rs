use std::sync::Arc;

use asrs_js::string::JsString;
use asrs_js::value::JsObject;
use asrs_log::log::{
    Log, LogFieldTree, LogValue, LogValues, LoggableType, SerializedLogField, TimestampDisplay,
};
use asrs_parity::{Response, Utf16String, Value};

use crate::functions::{self, arg};
use crate::js_json;
use crate::log_field::{
    ToValue, any_set_value, array, boolean, index, js_string, loggable_type, number, numbers,
    serialized_value, set_value, string_value, type_string,
};

fn bytes(v: &Value) -> Result<Arc<[u8]>, String> {
    match v {
        Value::Bytes(b) => Ok(Arc::from(b.as_slice())),
        other => Err(format!("expected bytes, got {}", other.kind())),
    }
}

fn pair(v: &Value) -> Result<[f64; 2], String> {
    let items = array(v, number)?;
    match items.as_slice() {
        [x, y] => Ok([*x, *y]),
        _ => Err("expected [x, y]".to_owned()),
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Undefined | Value::Null | Value::Hole => false,
        Value::Bool(b) => *b,
        Value::Number(bits) => {
            let x = f64::from_bits(*bits);
            x != 0.0 && !x.is_nan()
        }
        Value::String(s) => !s.units().is_empty(),
        Value::BigInt(d) => d != "0" && d != "-0",
        _ => true,
    }
}

fn optional_uuid(v: &Value) -> Result<Option<String>, String> {
    match v {
        Value::Undefined | Value::Null => Ok(None),
        other => Ok(Some(js_string(other)?.to_string_lossy())),
    }
}

fn values(ty: LoggableType, v: &Value) -> Result<LogValues, String> {
    fn column<T>(
        v: &Value,
        item: impl Fn(&Value) -> Result<T, String>,
    ) -> Result<Arc<Vec<T>>, String> {
        array(v, item).map(Arc::new)
    }
    Ok(match ty {
        LoggableType::Raw => LogValues::Raw(column(v, bytes)?),
        LoggableType::Boolean => LogValues::Boolean(column(v, boolean)?),
        LoggableType::Number => LogValues::Number(column(v, number)?),
        LoggableType::String => LogValues::String(column(v, js_string)?),
        LoggableType::BooleanArray => {
            LogValues::BooleanArray(column(v, |item| array(item, boolean))?)
        }
        LoggableType::NumberArray => LogValues::NumberArray(column(v, |item| array(item, number))?),
        LoggableType::StringArray => {
            LogValues::StringArray(column(v, |item| array(item, js_string))?)
        }
        LoggableType::Empty => LogValues::Empty(column(v, |_| Ok(()))?),
    })
}

fn member(v: &Value, name: &str) -> Value {
    v.get(name).cloned().unwrap_or(Value::Undefined)
}

fn serialized_field(v: &Value) -> Result<SerializedLogField, String> {
    let ty = loggable_type(&member(v, "type"))?;
    Ok(SerializedLogField {
        timestamps: Arc::new(array(&member(v, "timestamps"), number)?),
        values: values(ty, &member(v, "values"))?,
        structured_type: type_string(&member(v, "structuredType"))?,
        wpilib_type: type_string(&member(v, "wpilibType"))?,
        metadata_string: js_string(&member(v, "metadataString"))?,
        striping_reference: boolean(&member(v, "stripingReference"))?,
        type_warning: boolean(&member(v, "typeWarning"))?,
    })
}

fn optional_js(s: Option<&JsString>) -> Value {
    s.map_or(Value::Null, string_value)
}

fn strings<'a>(items: impl Iterator<Item = &'a JsString>) -> Value {
    Value::Array(items.map(string_value).collect())
}

fn tree_value(children: &JsObject<LogFieldTree>) -> Value {
    Value::Object(
        children
            .iter()
            .map(|(name, node)| {
                (
                    Utf16String::from_units(name.units().to_vec()),
                    Value::object([
                        ("fullKey", optional_js(node.full_key.as_ref())),
                        ("children", tree_value(&node.children)),
                    ]),
                )
            })
            .collect(),
    )
}

pub(crate) fn dump(log: &Log) -> Value {
    let serialized = log.to_serialized();
    Value::object([
        ("keys", strings(serialized.fields.keys())),
        (
            "fields",
            Value::Array(serialized.fields.values().map(serialized_value).collect()),
        ),
        (
            "generatedParents",
            strings(serialized.generated_parents.iter()),
        ),
        (
            "timestampRange",
            serialized
                .timestamp_range
                .map_or(Value::Null, |r| numbers(&r)),
        ),
        (
            "timestampZero",
            serialized.timestamp_zero.map_or(Value::Null, Value::number),
        ),
    ])
}

fn typed_get<T: LogValue + ToValue>(log: &Log, op: &[Value]) -> Result<Value, String> {
    let key = js_string(&arg(op, 2))?;
    let uuid = optional_uuid(&arg(op, 5))?;
    let offset = match arg(op, 6) {
        Value::Undefined => None,
        other => Some(index(&other)?),
    };
    let start = number(&arg(op, 3))?;
    let end = number(&arg(op, 4))?;
    Ok(log
        .get::<T>(&key, start, end, uuid.as_deref(), offset)
        .map_or(Value::Undefined, |set| set_value(&set)))
}

fn get(log: &Log, op: &[Value]) -> Result<Value, String> {
    let method = js_string(&arg(op, 1))?.to_string_lossy();
    match method.as_str() {
        "getRange" => {
            let key = js_string(&arg(op, 2))?;
            let uuid = optional_uuid(&arg(op, 5))?;
            Ok(log
                .get_range(
                    &key,
                    number(&arg(op, 3))?,
                    number(&arg(op, 4))?,
                    uuid.as_deref(),
                )
                .map_or(Value::Undefined, |set| any_set_value(&set)))
        }
        "getRaw" => typed_get::<Arc<[u8]>>(log, op),
        "getBoolean" => typed_get::<bool>(log, op),
        "getNumber" => typed_get::<f64>(log, op),
        "getString" => typed_get::<JsString>(log, op),
        "getBooleanArray" => typed_get::<Vec<bool>>(log, op),
        "getNumberArray" => typed_get::<Vec<f64>>(log, op),
        "getStringArray" => typed_get::<Vec<JsString>>(log, op),
        other => Err(format!("logRun: unknown getter {other:?}")),
    }
}

fn query(log: &Log, key: &JsString) -> Value {
    Value::object([
        (
            "type",
            log.get_type(key)
                .map_or(Value::Null, |ty| Value::number(f64::from(ty as u8))),
        ),
        ("structuredType", optional_js(log.get_structured_type(key))),
        ("wpilibType", optional_js(log.get_wpilib_type(key))),
        (
            "metadataString",
            string_value(&log.get_metadata_string(key)),
        ),
        ("typeWarning", Value::Bool(log.get_type_warning(key))),
        (
            "stripingReference",
            Value::Bool(log.get_striping_reference(key)),
        ),
        ("isGenerated", Value::Bool(log.is_generated(key))),
        (
            "generatedParent",
            optional_js(log.get_generated_parent(key)),
        ),
        (
            "isGeneratedParent",
            Value::Bool(log.is_generated_parent(key)),
        ),
        (
            "field",
            log.get_field(key).map_or(Value::Null, |field| {
                serialized_value(&field.to_serialized())
            }),
        ),
    ])
}

fn display(v: &Value) -> Result<Option<TimestampDisplay>, String> {
    match type_string(v)?.map(|s| s.to_string_lossy()).as_deref() {
        None => Ok(None),
        Some("start-zero") => Ok(Some(TimestampDisplay::StartZero)),
        Some("original") => Ok(Some(TimestampDisplay::Original)),
        Some(_) => Ok(None),
    }
}

pub(crate) fn apply(log: &mut Log, op: &[Value], results: &mut Vec<Value>) -> Result<(), String> {
    let name = js_string(&arg(op, 0))?.to_string_lossy();
    let key = || js_string(&arg(op, 1));
    let timestamp = || number(&arg(op, 2));
    match name.as_str() {
        "putRaw" => log.put_raw(&key()?, timestamp()?, bytes(&arg(op, 3))?),
        "putBoolean" => log.put_boolean(&key()?, timestamp()?, boolean(&arg(op, 3))?),
        "putNumber" => log.put_number(&key()?, timestamp()?, number(&arg(op, 3))?),
        "putString" => log.put_string(&key()?, timestamp()?, js_string(&arg(op, 3))?),
        "putBooleanArray" => {
            log.put_boolean_array(&key()?, timestamp()?, array(&arg(op, 3), boolean)?)
        }
        "putNumberArray" => {
            log.put_number_array(&key()?, timestamp()?, array(&arg(op, 3), number)?)
        }
        "putStringArray" => {
            log.put_string_array(&key()?, timestamp()?, array(&arg(op, 3), js_string)?)
        }
        "putJSON" => log.put_json(&key()?, timestamp()?, js_string(&arg(op, 3))?),
        "putUnknownStruct" => log.put_unknown_struct(
            &key()?,
            timestamp()?,
            &js_json::to_js(&arg(op, 3))?,
            truthy(&arg(op, 4)),
        ),
        "putPose" => log.put_pose(
            &key()?,
            timestamp()?,
            pair(&arg(op, 3))?,
            number(&arg(op, 4))?,
        ),
        "putTranslationArray" => {
            log.put_translation_array(&key()?, timestamp()?, &array(&arg(op, 3), pair)?)
        }
        "createBlankField" => log.create_blank_field(&key()?, loggable_type(&arg(op, 2))?),
        "setField" => log.set_field(
            &key()?,
            asrs_log::log::LogField::from_serialized(serialized_field(&arg(op, 2))?),
        ),
        "deleteField" => log.delete_field(&key()?),
        "clearBeforeTime" => log.clear_before_time(number(&arg(op, 1))?, boolean(&arg(op, 2))?),
        "updateRangeWithTimestamp" => log.update_range_with_timestamp(number(&arg(op, 1))?),
        "setStructuredType" => log.set_structured_type(&key()?, type_string(&arg(op, 2))?),
        "setWpilibType" => log.set_wpilib_type(&key()?, js_string(&arg(op, 2))?),
        "setMetadataString" => log.set_metadata_string(&key()?, js_string(&arg(op, 2))?),
        "setGeneratedParent" => log.set_generated_parent(&key()?),
        "reload" => *log = Log::from_serialized(log.to_serialized()),
        "get" => results.push(get(log, op)?),
        "getTimestamps" => {
            let keys = array(&arg(op, 1), js_string)?;
            let uuid = optional_uuid(&arg(op, 2))?;
            results.push(numbers(&log.get_timestamps(&keys, uuid.as_deref())));
        }
        "getChangedFields" => results.push(strings(log.get_changed_fields().iter())),
        "query" => results.push(query(log, &key()?)),
        "keys" => results.push(Value::object([
            ("keys", strings(log.field_keys())),
            ("count", Value::number(log.get_field_count() as f64)),
        ])),
        "fieldTree" => results.push(tree_value(
            &log.get_field_tree(boolean(&arg(op, 1))?, &js_string(&arg(op, 2))?),
        )),
        "range" => results.push(Value::object([
            ("range", numbers(&log.get_timestamp_range())),
            (
                "last",
                log.get_last_timestamp()
                    .map_or(Value::Undefined, Value::number),
            ),
        ])),
        "displayOffset" => results.push(Value::number(
            log.get_timestamp_display_offset(display(&arg(op, 1))?),
        )),
        "dump" => results.push(dump(log)),
        other => return Err(format!("logRun: unknown op {other:?}")),
    }
    Ok(())
}

/// Replays the logRun op list and returns the resulting log alongside the
/// query results `run` reports.
pub(crate) fn build_log(cache: bool, ops: &[Value]) -> Result<(Log, Vec<Value>), String> {
    let mut log = Log::new(cache);
    let mut results = Vec::new();
    for op in ops {
        let op = op.as_array().ok_or("an op must be an array")?;
        apply(&mut log, op, &mut results)?;
    }
    Ok((log, results))
}

fn run(args: &[Value]) -> Result<Value, String> {
    let ops = arg(args, 1);
    let (_, results) = build_log(
        boolean(&arg(args, 0))?,
        ops.as_array().ok_or("ops must be an array")?,
    )?;
    Ok(Value::Array(results))
}

fn log_run(args: &[Value]) -> Response {
    match run(args) {
        Ok(v) => Response::Ok(v),
        Err(e) => Response::Error(e),
    }
}

pub const FUNCTIONS: &[(&str, functions::Function)] = &[("oracle:logRun", log_run)];
