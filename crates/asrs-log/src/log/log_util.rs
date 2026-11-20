//! `shared/log/LogUtil.ts`, except the Fuse.js `searchFields` (that is unit L10).

use asrs_js::json::{self, JsonError};
use asrs_js::math::{BigInt, int::to_int32};
use asrs_js::number;
use asrs_js::string::{JsError, JsString};
use asrs_js::time::{NamedZone, date_to_string};
use asrs_js::value::JsValue;

use super::{Log, LogValue, LogValueSet, LogValueSetAny, LoggableType};

pub const ARRAY_TEXT_SIZE_LIMIT: usize = 500;
pub const TYPE_KEY: &str = ".type";
pub const MAX_SEARCH_RESULTS: usize = 128;

fn js_err(e: JsonError) -> JsError {
    JsError::new(e.name, e.message.to_string_lossy())
}

/// `===` on the JS values the log can hold. Objects are compared by reference upstream, which
/// two `JsValue` arguments can never share, so they are never equal here.
pub(crate) fn js_strict_eq(a: &JsValue, b: &JsValue) -> bool {
    match (a, b) {
        (JsValue::Undefined, JsValue::Undefined) | (JsValue::Null, JsValue::Null) => true,
        (JsValue::Bool(x), JsValue::Bool(y)) => x == y,
        (JsValue::Number(x), JsValue::Number(y)) => x == y,
        (JsValue::String(x), JsValue::String(y)) => x == y,
        (JsValue::BigInt(x), JsValue::BigInt(y)) => x == y,
        _ => false,
    }
}

/// The `length` property read, which throws on `null`/`undefined` receivers. Arrays,
/// strings and typed arrays have one, plain objects may carry it, everything else reads
/// `undefined`.
pub(crate) fn length_prop(v: &JsValue) -> Result<JsValue, JsError> {
    Ok(match v {
        JsValue::Null => {
            return Err(JsError::type_error(
                "Cannot read properties of null (reading 'length')",
            ));
        }
        JsValue::Undefined => {
            return Err(JsError::type_error(
                "Cannot read properties of undefined (reading 'length')",
            ));
        }
        JsValue::Array(items) => JsValue::Number(items.len() as f64),
        JsValue::String(s) => JsValue::Number(s.len() as f64),
        JsValue::Bytes(bytes) => JsValue::Number(bytes.len() as f64),
        JsValue::Object(object) => object.get("length").cloned().unwrap_or(JsValue::Undefined),
        _ => JsValue::Undefined,
    })
}

/// The `byteLength` property read: `Uint8Array`s have one, plain objects may carry it.
fn byte_length_prop(v: &JsValue) -> Result<JsValue, JsError> {
    Ok(match v {
        JsValue::Null => {
            return Err(JsError::type_error(
                "Cannot read properties of null (reading 'byteLength')",
            ));
        }
        JsValue::Undefined => {
            return Err(JsError::type_error(
                "Cannot read properties of undefined (reading 'byteLength')",
            ));
        }
        JsValue::Bytes(bytes) => JsValue::Number(bytes.len() as f64),
        JsValue::Object(object) => object
            .get("byteLength")
            .cloned()
            .unwrap_or(JsValue::Undefined),
        _ => JsValue::Undefined,
    })
}

/// The `value[index]` property read: an array element, a code unit of a string, a byte, an
/// object's own `"i"` member, or `undefined`.
pub(crate) fn index_get(v: &JsValue, index: usize) -> JsValue {
    match v {
        JsValue::Array(items) => items.get(index).cloned().unwrap_or(JsValue::Undefined),
        JsValue::String(s) => s
            .units()
            .get(index)
            .map(|&u| JsValue::String(JsString::from_units(vec![u])))
            .unwrap_or(JsValue::Undefined),
        JsValue::Bytes(bytes) => bytes
            .get(index)
            .map(|&b| JsValue::Number(f64::from(b)))
            .unwrap_or(JsValue::Undefined),
        JsValue::Object(object) => object
            .get(format!("{index}").as_str())
            .cloned()
            .unwrap_or(JsValue::Undefined),
        _ => JsValue::Undefined,
    }
}

/// One element of `Array.prototype.join`: `null`/`undefined` print as `""`, the rest as
/// `ToString` (for `Date` that is `Date.prototype.toString`, not the default object tag).
pub(crate) fn join_element(v: &JsValue) -> Result<JsString, JsError> {
    Ok(match v {
        JsValue::Undefined | JsValue::Null => JsString::new(),
        JsValue::Bool(b) => JsString::from(if *b { "true" } else { "false" }),
        JsValue::Number(x) => JsString::from(number::to_string(*x)),
        JsValue::String(s) => s.clone(),
        JsValue::BigInt(b) => JsString::from(format!("{b}")),
        JsValue::Array(items) => array_join(items)?,
        JsValue::Bytes(bytes) => bytes_join(bytes),
        JsValue::Date(t) => JsString::from(date_to_string(*t, &NamedZone::system())),
        JsValue::Object(_) | JsValue::ExtData(..) => JsString::from("[object Object]"),
    })
}

fn array_join(items: &[JsValue]) -> Result<JsString, JsError> {
    let mut out = JsString::new();
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push_unit(u16::from(b','));
        }
        out.push_str(&join_element(item)?);
    }
    Ok(out)
}

fn bytes_join(bytes: &[u8]) -> JsString {
    let mut out = JsString::new();
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 {
            out.push_unit(u16::from(b','));
        }
        out.push_str(&JsString::from(number::to_string(f64::from(b))));
    }
    out
}

/// `ToNumber` on a `JsValue`, for the `& 0xff` coercions `getLogValueText` applies. A
/// `BigInt` reaches `&` unconverted, and V8 rejects the mixed operand pair.
fn to_number_js(v: &JsValue) -> Result<f64, JsError> {
    Ok(match v {
        JsValue::Undefined => f64::NAN,
        JsValue::Null => 0.0,
        JsValue::Bool(b) => f64::from(*b as i8),
        JsValue::Number(x) => *x,
        JsValue::String(s) => number::to_number(s.units()),
        JsValue::BigInt(_) => {
            return Err(JsError::type_error(
                "Cannot mix BigInt and other types, use explicit conversions",
            ));
        }
        JsValue::Array(items) => number::to_number(array_join(items)?.units()),
        JsValue::Bytes(bytes) => number::to_number(bytes_join(bytes).units()),
        JsValue::Date(t) => *t,
        JsValue::Object(_) | JsValue::ExtData(..) => f64::NAN,
    })
}

/// `i < bound` where `bound` is an already-read property value: a relational compare, so a
/// `BigInt` bound stays exact and `undefined`/`NaN` is simply false.
fn index_less_than(i: usize, bound: &JsValue) -> Result<bool, JsError> {
    Ok(match bound {
        JsValue::BigInt(b) => BigInt::from_u64(i as u64) < *b,
        other => (i as f64) < to_number_js(other)?,
    })
}

/// The `type === LoggableType.Raw` arm of `getLogValueText`, where `array.slice`/`forEach`
/// and `array.length` act on whatever JS value was passed.
fn raw_value_text(value: &JsValue) -> Result<String, JsError> {
    if js_strict_eq(&length_prop(value)?, &JsValue::Number(0.0)) {
        return Ok("(empty)".to_owned());
    }
    let mut text: Vec<String> = Vec::new();
    let len = match value {
        JsValue::Bytes(bytes) => {
            for &byte in bytes.iter().take(ARRAY_TEXT_SIZE_LIMIT) {
                text.push(format!("{byte:02x}"));
            }
            bytes.len() as f64
        }
        JsValue::Array(items) => {
            for item in items.iter().take(ARRAY_TEXT_SIZE_LIMIT) {
                let byte = to_int32(to_number_js(item)?) & 0xff;
                text.push(format!("{byte:02x}"));
            }
            items.len() as f64
        }
        JsValue::String(_) => {
            return Err(JsError::type_error(
                "array.slice(...).forEach is not a function",
            ));
        }
        _ => return Err(JsError::type_error("array.slice is not a function")),
    };
    if len > ARRAY_TEXT_SIZE_LIMIT as f64 {
        text.push("...".to_owned());
    }
    Ok(text.join("-"))
}

/// `getLogValueText`: the display string for a log value. `Ok(None)` is the `undefined`
/// upstream returns from `JSON.stringify(undefined)`; `Err` is a thrown `TypeError`.
pub fn get_log_value_text(value: &JsValue, ty: LoggableType) -> Result<Option<String>, JsError> {
    if value.is_null() {
        return Ok(Some("null".to_owned()));
    }
    if ty == LoggableType::Raw {
        return raw_value_text(value).map(Some);
    }
    if let JsValue::Array(items) = value {
        let mut parts: Vec<Option<String>> = Vec::with_capacity(ARRAY_TEXT_SIZE_LIMIT + 1);
        for item in items.iter().take(ARRAY_TEXT_SIZE_LIMIT) {
            parts.push(json::stringify(item).map_err(js_err)?);
        }
        if items.len() > ARRAY_TEXT_SIZE_LIMIT {
            parts.push(Some("\"...\"".to_owned()));
        }
        let text = parts
            .iter()
            .map(|part| part.as_deref().unwrap_or(""))
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(Some(format!("[{text}]")));
    }
    json::stringify(value).map_err(js_err)
}

/// `getOrDefault` for typed callers: the value of `key` in effect at `timestamp`, or
/// `default` when the field is missing, has another type, or has no sample at or before it.
pub fn get_or_default<T: LogValue>(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    default: T,
    uuid: Option<&str>,
) -> T {
    if log.get_type(key) != Some(T::TYPE) {
        return default;
    }
    match log.get::<T>(key, timestamp, timestamp, uuid, None) {
        Some(set)
            if set.timestamps.first().is_some_and(|&t| t <= timestamp)
                && !set.values.is_empty() =>
        {
            set.values.into_iter().next().unwrap_or(default)
        }
        _ => default,
    }
}

fn element_js(set: &LogValueSetAny, index: usize) -> Option<JsValue> {
    fn item<T>(set: &LogValueSet<T>, index: usize, f: impl Fn(&T) -> JsValue) -> Option<JsValue> {
        set.values.get(index).map(f)
    }
    match set {
        LogValueSetAny::Raw(s) => item(s, index, |b| JsValue::Bytes(b.to_vec())),
        LogValueSetAny::Boolean(s) => item(s, index, |&b| JsValue::Bool(b)),
        LogValueSetAny::Number(s) => item(s, index, |&x| JsValue::Number(x)),
        LogValueSetAny::String(s) => item(s, index, |s| JsValue::String(s.clone())),
        LogValueSetAny::BooleanArray(s) => item(s, index, |a| {
            JsValue::Array(a.iter().map(|&b| JsValue::Bool(b)).collect())
        }),
        LogValueSetAny::NumberArray(s) => item(s, index, |a| {
            JsValue::Array(a.iter().map(|&x| JsValue::Number(x)).collect())
        }),
        LogValueSetAny::StringArray(s) => item(s, index, |a| {
            JsValue::Array(a.iter().map(|s| JsValue::String(s.clone())).collect())
        }),
        LogValueSetAny::Empty(_) => None,
    }
}

fn first_js(set: &LogValueSetAny) -> Option<JsValue> {
    element_js(set, 0)
}

/// The last sample of a range as the `JsValue` an upstream caller would hold.
pub fn last_value_js(set: &LogValueSetAny) -> Option<JsValue> {
    element_js(set, set.len().checked_sub(1)?)
}

/// `getOrDefault` on untyped values, as the JS callers upstream have them: the default is
/// returned untouched, whatever its type.
pub fn get_or_default_value(
    log: &Log,
    key: &JsString,
    ty: LoggableType,
    timestamp: f64,
    default: JsValue,
    uuid: Option<&str>,
) -> JsValue {
    if log.get_type(key) == Some(ty)
        && let Some(set) = log.get_range(key, timestamp, timestamp, uuid)
        && let (Some(&t0), Some(value)) = (set.timestamps().first(), first_js(&set))
        && t0 <= timestamp
    {
        return value;
    }
    default
}

/// `logValuesEqual` on untyped values, including `arraysEqual`'s `length`/`every` property
/// reads and the Raw `byteLength` loop. `Err` is the `TypeError` upstream throws.
pub fn log_values_equal(ty: LoggableType, a: &JsValue, b: &JsValue) -> Result<bool, JsError> {
    match ty {
        LoggableType::Boolean | LoggableType::Number | LoggableType::String => {
            Ok(js_strict_eq(a, b))
        }
        LoggableType::BooleanArray | LoggableType::NumberArray | LoggableType::StringArray => {
            let alen = length_prop(a)?;
            let blen = length_prop(b)?;
            if !js_strict_eq(&alen, &blen) {
                return Ok(false);
            }
            // `a.every` exists on arrays and `Uint8Array`s; strings and everything else
            // throw once the lengths are equal.
            let items: Vec<JsValue> = match a {
                JsValue::Array(items) => items.clone(),
                JsValue::Bytes(bytes) => bytes
                    .iter()
                    .map(|&b| JsValue::Number(f64::from(b)))
                    .collect(),
                _ => {
                    return Err(JsError::type_error("a.every is not a function"));
                }
            };
            for (i, item) in items.iter().enumerate() {
                if !js_strict_eq(item, &index_get(b, i)) {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        LoggableType::Raw => {
            let alen = byte_length_prop(a)?;
            if !js_strict_eq(&alen, &byte_length_prop(b)?) {
                return Ok(false);
            }
            let mut i = 0usize;
            while index_less_than(i, &alen)? {
                if !js_strict_eq(&index_get(a, i), &index_get(b, i)) {
                    return Ok(false);
                }
                i += 1;
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// `getURCLKeys`: field keys whose wpilib type starts with `URCL`.
pub fn get_urcl_keys(log: &Log) -> Vec<JsString> {
    let prefix = [b'U' as u16, b'R' as u16, b'C' as u16, b'L' as u16];
    log.field_keys()
        .filter(|key| {
            log.get_wpilib_type(key)
                .is_some_and(|t| t.units().starts_with(&prefix))
        })
        .cloned()
        .collect()
}

/// `LogFieldIterator`: a forward-only cursor into a `{timestamps, values}` set for
/// monotonically advancing time lookups.
pub struct LogFieldIterator<'a, T> {
    data: Option<(&'a [f64], &'a [T])>,
    idx: usize,
}

impl<'a, T> LogFieldIterator<'a, T> {
    pub fn new(data: Option<&'a LogValueSet<T>>) -> Self {
        LogFieldIterator {
            data: data.map(|set| (set.timestamps.as_slice(), set.values.as_slice())),
            idx: 0,
        }
    }

    /// The value in effect at `time`, advancing the cursor but never moving it back.
    pub fn get_at_time(&mut self, time: f64) -> Option<&'a T> {
        let (timestamps, values) = self.data?;
        while self.idx + 1 < timestamps.len() && timestamps[self.idx + 1] <= time {
            self.idx += 1;
        }
        if self.idx < timestamps.len() && timestamps[self.idx] <= time {
            Some(&values[self.idx])
        } else {
            None
        }
    }
}
