//! The oracle's log dumps. The live session tests include this file too.

use std::sync::Arc;

use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_log::log::{Log, LogValue, LogValues, SerializedLogField, StructuredType};
use asrs_parity::{Utf16String, Value};

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

impl ToValue for JsValue {
    fn to_value(&self) -> Value {
        match self {
            JsValue::Undefined => Value::Undefined,
            JsValue::Null => Value::Null,
            JsValue::Bool(b) => Value::Bool(*b),
            JsValue::Number(x) => Value::number(*x),
            JsValue::String(s) => string_value(s),
            JsValue::Array(items) => Value::Array(items.iter().map(ToValue::to_value).collect()),
            JsValue::Object(object) => Value::Object(
                object
                    .iter()
                    .map(|(k, v)| (Utf16String::from_units(k.units().to_vec()), v.to_value()))
                    .collect(),
            ),
            JsValue::Bytes(bytes) => Value::Bytes(bytes.clone()),
            JsValue::BigInt(b) => Value::BigInt(format!("{b}")),
            JsValue::Date(t) => Value::Date(t.to_bits()),
            JsValue::ExtData(..) => unreachable!("no unchecked source holds ExtData"),
        }
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

pub fn strings<'a>(items: impl Iterator<Item = &'a JsString>) -> Value {
    Value::Array(items.map(string_value).collect())
}

pub fn structured_type_value(t: &StructuredType) -> Value {
    match t {
        StructuredType::Null => Value::Null,
        StructuredType::Undefined => Value::Undefined,
        StructuredType::Named(s) => string_value(s),
        StructuredType::Foreign(value) => value.to_value(),
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
        LogValues::Foreign(_, c) => Value::Array(c.iter().map(ToValue::to_value).collect()),
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
        ("structuredType", structured_type_value(&s.structured_type)),
        ("wpilibType", optional_string(&s.wpilib_type)),
        ("metadataString", string_value(&s.metadata_string)),
        ("stripingReference", Value::Bool(s.striping_reference)),
        ("typeWarning", Value::Bool(s.type_warning)),
    ])
}

/// `dump(log)` from `oracle/src/logRun.ts`.
pub fn dump(log: &Log) -> Value {
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
