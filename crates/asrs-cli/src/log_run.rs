use std::sync::Arc;

use asrs_js::string::JsString;
use std::sync::atomic::{AtomicUsize, Ordering};

use asrs_js::time::Rng;
use asrs_js::value::{JsObject, ToJs};
use asrs_log::log::{
    Log, LogError, LogFieldIterator, LogFieldTree, LogValue, LogValueSet, LogValueSetAny,
    LogValues, LoggableType, QueuedStructure, SerializedLog, SerializedLogField, TimestampDisplay,
    log_key_utils, log_util,
};
use asrs_log::realm::JsRealm;
use asrs_parity::{Response, Utf16String, Value};

use crate::functions::{self, arg};
use crate::js_json;
use crate::log_field::{
    ToValue, any_set_value, array, boolean, index, js_string, loggable_type, loggable_type_opt,
    number, numbers, serialized_value, set_value, string_value, structured_type,
    structured_type_value, type_string,
};

pub(crate) fn bytes(v: &Value) -> Result<Arc<[u8]>, String> {
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

pub(crate) fn serialized_field(v: &Value) -> Result<SerializedLogField, String> {
    let ty = loggable_type(&member(v, "type"))?;
    Ok(SerializedLogField {
        timestamps: Arc::new(array(&member(v, "timestamps"), number)?),
        values: values(ty, &member(v, "values"))?,
        structured_type: structured_type(&member(v, "structuredType"))?,
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

fn queue_value(items: &[QueuedStructure]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|item| {
                Value::object([
                    ("key", string_value(&item.key)),
                    ("timestamp", Value::number(item.timestamp)),
                    ("value", Value::Bytes(item.value.to_vec())),
                    ("schemaType", string_value(&item.schema_type)),
                ])
            })
            .collect(),
    )
}

/// The parts of `toSerialized()` beyond the fields: struct schema strings and schemas, proto
/// descriptors and the three retry queues.
pub(crate) fn decoder_state(serialized: &SerializedLog) -> [(&'static str, Value); 6] {
    [
        (
            "structSchemaStrings",
            js_json::from_js(&serialized.struct_decoder.schema_strings.to_js()),
        ),
        (
            "structSchemas",
            js_json::from_js(&serialized.struct_decoder.schemas.to_js()),
        ),
        (
            "protoDescriptors",
            js_json::from_js(&serialized.proto_decoder.to_js()),
        ),
        ("queuedStructs", queue_value(&serialized.queued_structs)),
        (
            "queuedStructArrays",
            queue_value(&serialized.queued_struct_arrays),
        ),
        ("queuedProtos", queue_value(&serialized.queued_protos)),
    ]
}

fn dump_all(log: &Log) -> Value {
    let Value::Object(mut pairs) = dump(log) else {
        unreachable!("dump always returns an object")
    };
    for (name, value) in decoder_state(&log.to_serialized()) {
        pairs.push((name.into(), value));
    }
    Value::Object(pairs)
}

fn threw(results: &mut Vec<Value>, result: Result<(), LogError>) -> Result<(), String> {
    match result {
        Ok(()) => Ok(()),
        Err(LogError::Proto(e)) => {
            results.push(Value::object([(
                "threw",
                Value::Error(
                    Utf16String::from(e.js_name()),
                    Utf16String::from_units(e.js_message().units().to_vec()),
                ),
            )]));
            Ok(())
        }
        Err(LogError::StackOverflow) => {
            results.push(Value::object([(
                "threw",
                Value::Error(
                    Utf16String::from("RangeError"),
                    Utf16String::from("Maximum call stack size exceeded"),
                ),
            )]));
            Ok(())
        }
        Err(LogError::NeverCompletes) => Err("upstream never completes".to_owned()),
        Err(LogError::Unmodeled(what)) => Err(format!("unmodeled: {what}")),
    }
}

fn reload(log: &Log, realm: &mut JsRealm, results: &mut Vec<Value>) -> Result<Log, String> {
    match Log::from_serialized(log.to_serialized(), realm) {
        Ok(log) => Ok(log),
        Err(e) => {
            threw(results, Err(e))?;
            Ok(Log::new(true))
        }
    }
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
        (
            "structuredType",
            structured_type_value(log.structured_type(key)),
        ),
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

pub(crate) fn apply(
    log: &mut Log,
    realm: &mut JsRealm,
    op: &[Value],
    results: &mut Vec<Value>,
) -> Result<(), String> {
    let name = js_string(&arg(op, 0))?.to_string_lossy();
    let key = || js_string(&arg(op, 1));
    let timestamp = || number(&arg(op, 2));
    match name.as_str() {
        "putRaw" => {
            let result = log.put_raw(&key()?, timestamp()?, bytes(&arg(op, 3))?);
            threw(results, result)?;
        }
        "putStruct" => {
            let result = log.put_struct(
                &key()?,
                timestamp()?,
                bytes(&arg(op, 3))?,
                &js_string(&arg(op, 4))?,
                truthy(&arg(op, 5)),
            );
            threw(results, result)?;
        }
        "putPhotonStruct" => {
            let result = log.put_photon_struct(
                &key()?,
                timestamp()?,
                bytes(&arg(op, 3))?,
                &js_string(&arg(op, 4))?,
            );
            threw(results, result)?;
        }
        "putProto" => {
            let result = log.put_proto(
                realm,
                &key()?,
                timestamp()?,
                bytes(&arg(op, 3))?,
                &js_string(&arg(op, 4))?,
            );
            threw(results, result)?;
        }
        "putMsgpack" => {
            let result = log.put_msgpack(realm, &key()?, timestamp()?, bytes(&arg(op, 3))?);
            threw(results, result)?;
        }
        "getUnit" => results.push(log.get_unit(&key()?).map_or(Value::Null, Value::string)),
        "units" => {
            let units = log
                .get_field_keys()
                .iter()
                .map(|key| log.get_unit(key).map_or(Value::Null, Value::string))
                .collect();
            results.push(Value::Array(units));
        }
        "dumpAll" => results.push(dump_all(log)),
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
        "putJSON" => {
            let result = log.put_json(&key()?, timestamp()?, js_string(&arg(op, 3))?);
            threw(results, result)?;
        }
        "putUnknownStruct" => {
            let result = log.put_unknown_struct(
                &key()?,
                timestamp()?,
                &js_json::to_js(&arg(op, 3))?,
                truthy(&arg(op, 4)),
            );
            threw(results, result)?;
        }
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
        "setStructuredType" => log.set_structured_type(&key()?, structured_type(&arg(op, 2))?),
        "setWpilibType" => log.set_wpilib_type(&key()?, js_string(&arg(op, 2))?),
        "setMetadataString" => log.set_metadata_string(&key()?, js_string(&arg(op, 2))?),
        "setGeneratedParent" => log.set_generated_parent(&key()?),
        "reload" => *log = reload(log, realm, results)?,
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
        "findKey" => {
            let search = array(&arg(op, 1), js_string)?;
            results.push(
                log_key_utils::find_key(log, &search)
                    .map_or(Value::Undefined, |key| string_value(&key)),
            );
        }
        "getOrDefault" => {
            let key = js_string(&arg(op, 1))?;
            let timestamp = number(&arg(op, 3))?;
            let uuid = optional_uuid(&arg(op, 5))?;
            let found = match loggable_type_opt(&arg(op, 2)) {
                Some(ty) => typed_value(log, &key, ty, timestamp, uuid.as_deref()),
                None => None,
            };
            results.push(found.unwrap_or_else(|| arg(op, 4)));
        }
        "urclKeys" => results.push(strings(log_util::get_urcl_keys(log).iter())),
        "iterator" => {
            let key = js_string(&arg(op, 1))?;
            let times = array(&arg(op, 2), number)?;
            let set = log.get_range(&key, f64::NEG_INFINITY, f64::INFINITY, None);
            results.push(Value::Array(iterator_values(&set, &times)));
        }
        "valueText" => {
            let key = js_string(&arg(op, 1))?;
            let set = log.get_range(&key, f64::NEG_INFINITY, f64::INFINITY, None);
            let text = set
                .as_ref()
                .and_then(|set| log_util::get_log_value_text_at(set, set.len().checked_sub(1)?));
            results.push(text.map_or(Value::Null, |text| Value::String(text.into())));
        }
        "dump" => results.push(dump(log)),
        other => return Err(format!("logRun: unknown op {other:?}")),
    }
    Ok(())
}

fn typed_value(
    log: &Log,
    key: &JsString,
    ty: LoggableType,
    timestamp: f64,
    uuid: Option<&str>,
) -> Option<Value> {
    fn get<T: LogValue + ToValue>(
        log: &Log,
        key: &JsString,
        timestamp: f64,
        uuid: Option<&str>,
    ) -> Option<Value> {
        log_util::get_value_at::<T>(log, key, timestamp, uuid).map(|v| v.to_value())
    }
    match ty {
        LoggableType::Raw => get::<Arc<[u8]>>(log, key, timestamp, uuid),
        LoggableType::Boolean => get::<bool>(log, key, timestamp, uuid),
        LoggableType::Number => get::<f64>(log, key, timestamp, uuid),
        LoggableType::String => get::<JsString>(log, key, timestamp, uuid),
        LoggableType::BooleanArray => get::<Vec<bool>>(log, key, timestamp, uuid),
        LoggableType::NumberArray => get::<Vec<f64>>(log, key, timestamp, uuid),
        LoggableType::StringArray => get::<Vec<JsString>>(log, key, timestamp, uuid),
        LoggableType::Empty => get::<()>(log, key, timestamp, uuid),
    }
}

fn iterator_values(set: &Option<LogValueSetAny>, times: &[f64]) -> Vec<Value> {
    fn run<T: ToValue>(set: Option<&LogValueSet<T>>, times: &[f64]) -> Vec<Value> {
        let mut iter = LogFieldIterator::new(set);
        times
            .iter()
            .map(|&t| {
                iter.get_at_time(t)
                    .map_or(Value::Undefined, ToValue::to_value)
            })
            .collect()
    }
    match set {
        Some(LogValueSetAny::Raw(s)) => run(Some(s), times),
        Some(LogValueSetAny::Boolean(s)) => run(Some(s), times),
        Some(LogValueSetAny::Number(s)) => run(Some(s), times),
        Some(LogValueSetAny::String(s)) => run(Some(s), times),
        Some(LogValueSetAny::BooleanArray(s)) => run(Some(s), times),
        Some(LogValueSetAny::NumberArray(s)) => run(Some(s), times),
        Some(LogValueSetAny::StringArray(s)) => run(Some(s), times),
        Some(LogValueSetAny::Empty(s)) => run(Some(s), times),
        None => run::<()>(None, times),
    }
}

/// Replays the logRun op list and returns the resulting log alongside the
/// query results `run` reports.
pub(crate) fn build_log(cache: bool, ops: &[Value]) -> Result<(Log, Vec<Value>), String> {
    let mut log = Log::new(cache);
    let (mut realm, drawn, _) = script_realm(Vec::new());
    let mut results = Vec::new();
    for op in ops {
        let op = op.as_array().ok_or("an op must be an array")?;
        apply(&mut log, &mut realm, op, &mut results)?;
    }
    if drawn.load(Ordering::Relaxed) != 0 {
        return Err("logRun scripts draw no Math.random values".to_owned());
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

/// A realm's `Math.random`: the values upstream drew, in order. Drawing past them is an error the
/// script reports, as is leaving any undrawn.
struct ScriptRng {
    values: Vec<f64>,
    drawn: Arc<AtomicUsize>,
}

impl Rng for ScriptRng {
    fn next_f64(&mut self) -> f64 {
        let i = self.drawn.fetch_add(1, Ordering::Relaxed);
        self.values.get(i).copied().unwrap_or(f64::NAN)
    }
}

pub(crate) fn script_realm(values: Vec<f64>) -> (JsRealm, Arc<AtomicUsize>, usize) {
    let drawn = Arc::new(AtomicUsize::new(0));
    let expected = values.len();
    let rng = ScriptRng {
        values,
        drawn: Arc::clone(&drawn),
    };
    (JsRealm::new(Box::new(rng)), drawn, expected)
}

struct ScriptLog {
    log: Log,
    realm: String,
}

/// `oracle:logScript`: several logs across JS realms. `args[0]` lists each realm's
/// `Math.random` values as `[name, [values...]]`; `args[1]` is the op list:
///   ["new", id, realm, enableTimestampSetCache]
///   ["transfer", from, to, toRealm]   to = Log.fromSerialized(from.toSerialized()) in toRealm
///   ["merge", target, source, prefix]   pushes the offset; the source is dropped
///   ["op", id, <logRun op>...]
fn script(args: &[Value]) -> Result<Value, String> {
    let mut realms: Vec<(String, JsRealm)> = Vec::new();
    let mut draws = Vec::new();
    for entry in arg(args, 0).as_array().ok_or("realms must be an array")? {
        let name = js_string(&arg(entry.as_array().ok_or("a realm is a pair")?, 0))?;
        let values = array(
            &arg(entry.as_array().ok_or("a realm is a pair")?, 1),
            number,
        )?;
        let (realm, drawn, expected) = script_realm(values);
        draws.push((name.to_string_lossy(), drawn, expected));
        realms.push((name.to_string_lossy(), realm));
    }
    let realm_index = |realms: &[(String, JsRealm)], name: &str| {
        realms
            .iter()
            .position(|(n, _)| n == name)
            .ok_or_else(|| format!("no realm {name:?}"))
    };
    let mut logs: Vec<(String, ScriptLog)> = Vec::new();
    let log_index = |logs: &[(String, ScriptLog)], id: &str| {
        logs.iter()
            .position(|(n, _)| n == id)
            .ok_or_else(|| format!("no log {id:?}"))
    };
    let mut results = Vec::new();
    for op in arg(args, 1).as_array().ok_or("ops must be an array")? {
        let op = op.as_array().ok_or("an op must be an array")?;
        let name = js_string(&arg(op, 0))?.to_string_lossy();
        let text = |i: usize| js_string(&arg(op, i)).map(|s| s.to_string_lossy());
        match name.as_str() {
            "new" => {
                let realm = text(2)?;
                realm_index(&realms, &realm)?;
                let log = Log::new(boolean(&arg(op, 3))?);
                let id = text(1)?;
                logs.retain(|(n, _)| *n != id);
                logs.push((id, ScriptLog { log, realm }));
            }
            "transfer" => {
                let from = log_index(&logs, &text(1)?)?;
                let serialized = logs[from].1.log.to_serialized().structured_clone();
                let realm = text(3)?;
                let r = realm_index(&realms, &realm)?;
                let log = match Log::from_serialized(serialized, &mut realms[r].1) {
                    Ok(log) => log,
                    Err(e) => {
                        threw(&mut results, Err(e))?;
                        Log::new(true)
                    }
                };
                let id = text(2)?;
                logs.retain(|(n, _)| *n != id);
                logs.push((id, ScriptLog { log, realm }));
            }
            "merge" => {
                let source_index = log_index(&logs, &text(2)?)?;
                let (_, source) = logs.remove(source_index);
                let target = log_index(&logs, &text(1)?)?;
                if logs[target].1.realm != source.realm {
                    return Err("merge across realms".to_owned());
                }
                let r = realm_index(&realms, &source.realm)?;
                let prefix = js_string(&arg(op, 3))?;
                let result = logs[target]
                    .1
                    .log
                    .merge_with(&source.log, &prefix, &mut realms[r].1);
                match result {
                    Ok(offset) => results.push(Value::number(offset)),
                    Err(e) => threw(&mut results, Err(e))?,
                }
            }
            "op" => {
                let i = log_index(&logs, &text(1)?)?;
                let r = realm_index(&realms, &logs[i].1.realm)?;
                apply(&mut logs[i].1.log, &mut realms[r].1, &op[2..], &mut results)?;
            }
            other => return Err(format!("logScript: unknown op {other:?}")),
        }
    }
    for (name, drawn, expected) in draws {
        let drawn = drawn.load(Ordering::Relaxed);
        if drawn != expected {
            return Err(format!(
                "realm {name} drew {drawn} Math.random values, upstream drew {expected}"
            ));
        }
    }
    Ok(Value::Array(results))
}

fn log_script(args: &[Value]) -> Response {
    match script(args) {
        Ok(v) => Response::Ok(v),
        Err(e) => Response::Error(e),
    }
}

pub const FUNCTIONS: &[(&str, functions::Function)] =
    &[("oracle:logRun", log_run), ("oracle:logScript", log_script)];
