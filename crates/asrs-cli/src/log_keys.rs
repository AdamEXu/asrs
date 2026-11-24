//! `fn` names for `asrs_log::log::log_key_utils` and the `LogUtil` value helpers. Wrappers
//! convert the CVE-1 arguments to the port's `JsString` and typed value inputs; where upstream
//! would hit a string method or `RegExp.test` on a non-string they produce the same
//! `TypeError` V8 throws.

use asrs_js::string::JsString;
use asrs_log::log::log_util::{self, LogValueText};
use asrs_log::log::{LogValue, LoggableType, log_key_utils as keys};
use asrs_parity::{Response, Value};

use crate::functions::{arg, to_units, type_error};
use crate::log_dump::string_value;
use crate::log_field::{self, array, js_string, loggable_type, number};
use crate::log_run::bytes;

fn ok(v: impl FnOnce() -> Value) -> Response {
    Response::Ok(v())
}

fn boolean(b: bool) -> Response {
    Response::Ok(Value::Bool(b))
}

fn optional(s: Option<JsString>) -> Response {
    Response::Ok(s.map_or(Value::Null, |s| string_value(&s)))
}

fn string(s: JsString) -> Response {
    Response::Ok(string_value(&s))
}

/// `key.match(...)`/`key.split(...)`: the receiver must be a string; anything else throws.
fn method_string(v: &Value, var: &str, method: &str) -> Result<JsString, Response> {
    match v {
        Value::String(s) => Ok(JsString::from_units(s.units().to_vec())),
        Value::Null => Err(type_error(format!(
            "Cannot read properties of null (reading '{method}')"
        ))),
        Value::Undefined => Err(type_error(format!(
            "Cannot read properties of undefined (reading '{method}')"
        ))),
        _ => Err(type_error(format!("{var}.{method} is not a function"))),
    }
}

/// `RegExp.test` coerces its argument with `ToString`.
fn coerced_string(v: &Value) -> Result<JsString, Response> {
    to_units(v).map(JsString::from_units)
}

const LOG_PREFIX: [u16; 4] = [b'l' as u16, b'o' as u16, b'g' as u16, b':' as u16];

/// The `name?.startsWith(...)` receiver: `null`/`undefined` short-circuit to `undefined`,
/// strings call through, anything else is the `TypeError` optional chaining still throws.
fn optional_type(v: &Value, name: &str) -> Result<Option<JsString>, Response> {
    match v {
        Value::Null | Value::Undefined => Ok(None),
        Value::String(s) => Ok(Some(JsString::from_units(s.units().to_vec()))),
        _ => Err(type_error(format!("{name}?.startsWith is not a function"))),
    }
}

fn is_schema(a: &[Value]) -> Response {
    match coerced_string(&arg(a, 0)) {
        Ok(k) => boolean(keys::is_schema(&k)),
        Err(r) => r,
    }
}

fn get_schema_type(a: &[Value]) -> Response {
    match method_string(&arg(a, 0), "key", "match") {
        Ok(k) => optional(keys::get_schema_type(&k)),
        Err(r) => r,
    }
}

// `wpilibType?.startsWith("log:") === true` short-circuits, so a bad `structuredType` is
// never read when the first type already says child log.
fn is_child_log(a: &[Value]) -> Response {
    match optional_type(&arg(a, 0), "wpilibType") {
        Err(r) => r,
        Ok(w)
            if w.as_ref()
                .is_some_and(|t| t.units().starts_with(&LOG_PREFIX)) =>
        {
            boolean(true)
        }
        Ok(w) => match optional_type(&arg(a, 1), "structuredType") {
            Err(r) => r,
            Ok(s) => boolean(keys::is_child_log(w.as_ref(), s.as_ref())),
        },
    }
}

fn child_log_extension(a: &[Value]) -> Response {
    match optional_type(&arg(a, 0), "wpilibType") {
        Err(r) => r,
        Ok(w)
            if w.as_ref()
                .is_some_and(|t| t.units().starts_with(&LOG_PREFIX)) =>
        {
            optional(keys::get_child_log_extension(w.as_ref(), None))
        }
        Ok(w) => match optional_type(&arg(a, 1), "structuredType") {
            Err(r) => r,
            Ok(s) => optional(keys::get_child_log_extension(w.as_ref(), s.as_ref())),
        },
    }
}

fn is_merge_prefix(a: &[Value]) -> Response {
    match coerced_string(&arg(a, 0)) {
        Ok(k) => boolean(keys::is_merge_prefix(&k)),
        Err(r) => r,
    }
}

fn extract_merge_prefix(a: &[Value]) -> Response {
    match method_string(&arg(a, 0), "key", "match") {
        Ok(k) => string(keys::extract_merge_prefix(&k)),
        Err(r) => r,
    }
}

fn structured_type_from_raw(a: &[Value]) -> Response {
    match method_string(&arg(a, 0), "rawType", "startsWith") {
        Ok(k) => optional(keys::get_structured_type_from_raw(&k)),
        Err(r) => r,
    }
}

fn remove_merge_prefix(a: &[Value]) -> Response {
    match method_string(&arg(a, 0), "key", "match") {
        Ok(k) => string(keys::remove_merge_prefix(&k)),
        Err(r) => r,
    }
}

fn apply_key_prefix(a: &[Value]) -> Response {
    match (js_string(&arg(a, 0)), js_string(&arg(a, 1))) {
        (Ok(p), Ok(k)) => string(keys::apply_key_prefix(&p, &k)),
        (Err(e), _) | (_, Err(e)) => Response::Error(e),
    }
}

fn split_log_key(a: &[Value]) -> Response {
    match method_string(&arg(a, 0), "key", "match") {
        Ok(k) => ok(|| Value::Array(keys::split_log_key(&k).iter().map(string_value).collect())),
        Err(r) => r,
    }
}

fn filter_by_prefixes(a: &[Value]) -> Response {
    let fields = match array(&arg(a, 0), js_string) {
        Ok(f) => f,
        Err(e) => return Response::Error(e),
    };
    match method_string(&arg(a, 1), "prefixes", "split") {
        Err(r) => r,
        Ok(prefixes) => {
            let schemas = matches!(arg(a, 2), Value::Bool(true));
            let nt_only = matches!(arg(a, 3), Value::Bool(true));
            ok(|| {
                Value::Array(
                    keys::filter_field_by_prefixes(&fields, &prefixes, schemas, nt_only)
                        .iter()
                        .map(string_value)
                        .collect(),
                )
            })
        }
    }
}

fn log_value_text(a: &[Value]) -> Response {
    fn text<T: LogValueText>(v: &Value, item: impl Fn(&Value) -> Result<T, String>) -> Response {
        match item(v) {
            Ok(x) => Response::Ok(Value::String(log_util::get_log_value_text(&x).into())),
            Err(e) => Response::Error(e),
        }
    }
    let v = &arg(a, 0);
    match loggable_type(&arg(a, 1)) {
        Ok(LoggableType::Raw) => text(v, bytes),
        Ok(LoggableType::Boolean) => text(v, log_field::boolean),
        Ok(LoggableType::Number) => text(v, number),
        Ok(LoggableType::String) => text(v, js_string),
        Ok(LoggableType::BooleanArray) => text(v, |x| array(x, log_field::boolean)),
        Ok(LoggableType::NumberArray) => text(v, |x| array(x, number)),
        Ok(LoggableType::StringArray) => text(v, |x| array(x, js_string)),
        Ok(LoggableType::Empty) => text(v, |_| Ok(())),
        Err(e) => Response::Error(e),
    }
}

fn log_values_equal(a: &[Value]) -> Response {
    fn equal<T: LogValue>(a: &[Value], item: impl Fn(&Value) -> Result<T, String>) -> Response {
        match (item(&arg(a, 1)), item(&arg(a, 2))) {
            (Ok(x), Ok(y)) => boolean(log_util::log_values_equal(&x, &y)),
            (Err(e), _) | (_, Err(e)) => Response::Error(e),
        }
    }
    match loggable_type(&arg(a, 0)) {
        Ok(LoggableType::Raw) => equal(a, bytes),
        Ok(LoggableType::Boolean) => equal(a, log_field::boolean),
        Ok(LoggableType::Number) => equal(a, number),
        Ok(LoggableType::String) => equal(a, js_string),
        Ok(LoggableType::BooleanArray) => equal(a, |x| array(x, log_field::boolean)),
        Ok(LoggableType::NumberArray) => equal(a, |x| array(x, number)),
        Ok(LoggableType::StringArray) => equal(a, |x| array(x, js_string)),
        Ok(LoggableType::Empty) => equal(a, |_| Ok(())),
        Err(e) => Response::Error(e),
    }
}

pub const FUNCTIONS: &[(&str, crate::functions::Function)] = &[
    ("shared/log/LogKeyUtils:isSchema", is_schema),
    ("shared/log/LogKeyUtils:getSchemaType", get_schema_type),
    ("shared/log/LogKeyUtils:isChildLog", is_child_log),
    (
        "shared/log/LogKeyUtils:getChildLogExtension",
        child_log_extension,
    ),
    ("shared/log/LogKeyUtils:isMergePrefix", is_merge_prefix),
    (
        "shared/log/LogKeyUtils:extractMergePrefix",
        extract_merge_prefix,
    ),
    (
        "shared/log/LogKeyUtils:getStructuredTypeFromRaw",
        structured_type_from_raw,
    ),
    (
        "shared/log/LogKeyUtils:removeMergePrefix",
        remove_merge_prefix,
    ),
    ("shared/log/LogKeyUtils:applyKeyPrefix", apply_key_prefix),
    ("shared/log/LogKeyUtils:splitLogKey", split_log_key),
    (
        "shared/log/LogKeyUtils:filterFieldByPrefixes",
        filter_by_prefixes,
    ),
    ("shared/log/LogUtil:getLogValueText", log_value_text),
    ("shared/log/LogUtil:logValuesEqual", log_values_equal),
];
