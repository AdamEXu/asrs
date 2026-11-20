use std::sync::Arc;

use asrs_js::string::JsString;
use asrs_log::log::{
    LogField, LogValue, LogValueSet, LogValueSetAny, LogValues, LoggableType, SerializedLogField,
};
use asrs_parity::{Response, Utf16String, Value};

use crate::functions;

const TYPES: [LoggableType; 8] = [
    LoggableType::Raw,
    LoggableType::Boolean,
    LoggableType::Number,
    LoggableType::String,
    LoggableType::BooleanArray,
    LoggableType::NumberArray,
    LoggableType::StringArray,
    LoggableType::Empty,
];

pub fn number(v: &Value) -> Result<f64, String> {
    v.as_f64()
        .ok_or_else(|| format!("expected a number, got {}", v.kind()))
}

pub fn index(v: &Value) -> Result<i64, String> {
    let x = number(v)?;
    if x.fract() != 0.0 || !x.is_finite() {
        return Err(format!("expected an integer, got {x}"));
    }
    Ok(x as i64)
}

pub fn loggable_type(v: &Value) -> Result<LoggableType, String> {
    let i = index(v)?;
    loggable_type_opt(v).ok_or_else(|| format!("no LoggableType {i}"))
}

/// `Some` when the value is a valid `LoggableType` index; `None` for any other JS value.
pub fn loggable_type_opt(v: &Value) -> Option<LoggableType> {
    index(v)
        .ok()
        .and_then(|i| usize::try_from(i).ok())
        .and_then(|i| TYPES.get(i).copied())
}

pub fn js_string(v: &Value) -> Result<JsString, String> {
    match v {
        Value::String(s) => Ok(JsString::from_units(s.units().to_vec())),
        other => Err(format!("expected a string, got {}", other.kind())),
    }
}

pub fn type_string(v: &Value) -> Result<Option<JsString>, String> {
    match v {
        Value::Null => Ok(None),
        other => js_string(other).map(Some),
    }
}

pub fn array<T>(v: &Value, item: impl Fn(&Value) -> Result<T, String>) -> Result<Vec<T>, String> {
    v.as_array()
        .ok_or_else(|| format!("expected an array, got {}", v.kind()))?
        .iter()
        .map(item)
        .collect()
}

pub fn boolean(v: &Value) -> Result<bool, String> {
    v.as_bool()
        .ok_or_else(|| format!("expected a boolean, got {}", v.kind()))
}

fn put(field: &mut LogField, ty: LoggableType, timestamp: f64, v: &Value) -> Result<(), String> {
    match ty {
        LoggableType::Raw => match v {
            Value::Bytes(b) => field.put_raw(timestamp, b.as_slice()),
            other => return Err(format!("expected bytes, got {}", other.kind())),
        },
        LoggableType::Boolean => field.put_boolean(timestamp, boolean(v)?),
        LoggableType::Number => field.put_number(timestamp, number(v)?),
        LoggableType::String => field.put_string(timestamp, js_string(v)?),
        LoggableType::BooleanArray => field.put_boolean_array(timestamp, array(v, boolean)?),
        LoggableType::NumberArray => field.put_number_array(timestamp, array(v, number)?),
        LoggableType::StringArray => field.put_string_array(timestamp, array(v, js_string)?),
        LoggableType::Empty => return Err("there is no put for Empty".to_owned()),
    }
    Ok(())
}

pub fn string_value(s: &JsString) -> Value {
    Value::String(Utf16String::from_units(s.units().to_vec()))
}

pub trait ToValue {
    fn to_value(&self) -> Value;
}

impl ToValue for Arc<[u8]> {
    fn to_value(&self) -> Value {
        Value::Bytes(self.to_vec())
    }
}

impl ToValue for bool {
    fn to_value(&self) -> Value {
        Value::Bool(*self)
    }
}

impl ToValue for f64 {
    fn to_value(&self) -> Value {
        Value::number(*self)
    }
}

impl ToValue for JsString {
    fn to_value(&self) -> Value {
        string_value(self)
    }
}

impl ToValue for () {
    fn to_value(&self) -> Value {
        Value::Undefined
    }
}

impl<T: ToValue> ToValue for Vec<T> {
    fn to_value(&self) -> Value {
        Value::Array(self.iter().map(ToValue::to_value).collect())
    }
}

pub fn numbers(xs: &[f64]) -> Value {
    Value::Array(xs.iter().map(|&x| Value::number(x)).collect())
}

pub fn set_value<T: ToValue>(set: &LogValueSet<T>) -> Value {
    Value::object([
        ("timestamps", numbers(&set.timestamps)),
        (
            "values",
            Value::Array(set.values.iter().map(ToValue::to_value).collect()),
        ),
    ])
}

pub fn any_set_value(set: &LogValueSetAny) -> Value {
    match set {
        LogValueSetAny::Raw(s) => set_value(s),
        LogValueSetAny::Boolean(s) => set_value(s),
        LogValueSetAny::Number(s) => set_value(s),
        LogValueSetAny::String(s) => set_value(s),
        LogValueSetAny::BooleanArray(s) => set_value(s),
        LogValueSetAny::NumberArray(s) => set_value(s),
        LogValueSetAny::StringArray(s) => set_value(s),
        LogValueSetAny::Empty(s) => set_value(s),
    }
}

fn column_value<T: ToValue + LogValue>(values: &LogValues) -> Value {
    Value::Array(
        values
            .as_slice::<T>()
            .unwrap_or_default()
            .iter()
            .map(ToValue::to_value)
            .collect(),
    )
}

fn values_value(values: &LogValues) -> Value {
    match values {
        LogValues::Raw(_) => column_value::<Arc<[u8]>>(values),
        LogValues::Boolean(_) => column_value::<bool>(values),
        LogValues::Number(_) => column_value::<f64>(values),
        LogValues::String(_) => column_value::<JsString>(values),
        LogValues::BooleanArray(_) => column_value::<Vec<bool>>(values),
        LogValues::NumberArray(_) => column_value::<Vec<f64>>(values),
        LogValues::StringArray(_) => column_value::<Vec<JsString>>(values),
        LogValues::Empty(_) => column_value::<()>(values),
    }
}

pub fn optional_string(s: &Option<JsString>) -> Value {
    s.as_ref().map_or(Value::Null, string_value)
}

/// `LogField.toSerialized()` as the CVE-1 object upstream produces.
pub fn serialized_value(s: &SerializedLogField) -> Value {
    Value::object([
        ("type", Value::number(f64::from(s.get_type() as u8))),
        ("timestamps", numbers(&s.timestamps)),
        ("values", values_value(&s.values)),
        ("structuredType", optional_string(&s.structured_type)),
        ("wpilibType", optional_string(&s.wpilib_type)),
        ("metadataString", string_value(&s.metadata_string)),
        ("stripingReference", Value::Bool(s.striping_reference)),
        ("typeWarning", Value::Bool(s.type_warning)),
    ])
}

fn typed_get<T: LogValue + ToValue>(
    field: &LogField,
    start: f64,
    end: f64,
    uuid: Option<&str>,
    offset: Option<i64>,
) -> Value {
    field
        .get::<T>(start, end, uuid, offset)
        .map_or(Value::Undefined, |set| set_value(&set))
}

fn get(field: &LogField, op: &[Value]) -> Result<Value, String> {
    let start = number(&functions::arg(op, 2))?;
    let end = number(&functions::arg(op, 3))?;
    let uuid = match functions::arg(op, 4) {
        Value::Undefined => None,
        other => Some(js_string(&other)?.to_string_lossy()),
    };
    let uuid = uuid.as_deref();
    let offset = match functions::arg(op, 5) {
        Value::Undefined => None,
        other => Some(index(&other)?),
    };
    let kind = index(&functions::arg(op, 1))?;
    if kind == -1 {
        return Ok(any_set_value(&field.get_range(start, end, uuid, offset)));
    }
    Ok(match loggable_type(&functions::arg(op, 1))? {
        LoggableType::Raw => typed_get::<Arc<[u8]>>(field, start, end, uuid, offset),
        LoggableType::Boolean => typed_get::<bool>(field, start, end, uuid, offset),
        LoggableType::Number => typed_get::<f64>(field, start, end, uuid, offset),
        LoggableType::String => typed_get::<JsString>(field, start, end, uuid, offset),
        LoggableType::BooleanArray => typed_get::<Vec<bool>>(field, start, end, uuid, offset),
        LoggableType::NumberArray => typed_get::<Vec<f64>>(field, start, end, uuid, offset),
        LoggableType::StringArray => typed_get::<Vec<JsString>>(field, start, end, uuid, offset),
        LoggableType::Empty => return Err("there is no typed getter for Empty".to_owned()),
    })
}

fn run(args: &[Value]) -> Result<Value, String> {
    let mut field = LogField::new(loggable_type(&functions::arg(args, 0))?);
    let ops = functions::arg(args, 1);
    let mut results = Vec::new();
    for op in ops.as_array().ok_or("ops must be an array")? {
        let op = op.as_array().ok_or("an op must be an array")?;
        let name = functions::arg(op, 0);
        let name = name
            .as_str()
            .map(Utf16String::to_string_lossy)
            .unwrap_or_default();
        match name.as_str() {
            "put" => {
                let ty = loggable_type(&functions::arg(op, 1))?;
                let timestamp = number(&functions::arg(op, 2))?;
                put(&mut field, ty, timestamp, &functions::arg(op, 3))?;
            }
            "get" => results.push(get(&field, op)?),
            "clear" => field.clear_before_time(number(&functions::arg(op, 1))?),
            "meta" => {
                field.wpilib_type = type_string(&functions::arg(op, 1))?;
                field.structured_type = type_string(&functions::arg(op, 2))?;
                field.metadata_string = js_string(&functions::arg(op, 3))?;
            }
            "reload" => field = LogField::from_serialized(field.to_serialized()),
            "state" => results.push(serialized_value(&field.to_serialized())),
            other => return Err(format!("logFieldRun: unknown op {other:?}")),
        }
    }
    Ok(Value::Array(results))
}

fn log_field_run(args: &[Value]) -> Response {
    match run(args) {
        Ok(v) => Response::Ok(v),
        Err(e) => Response::Error(e),
    }
}

pub const FUNCTIONS: &[(&str, functions::Function)] = &[("oracle:logFieldRun", log_field_run)];
