//! The JS operations `NT4.ts` applies to values decoded from the wire, which can be any JSON or
//! msgpack value when the server misbehaves. Module code is strict, so writes to primitives
//! throw.

use asrs_js::number;
use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue, array_index};

use crate::thrown::Thrown;

fn describe(value: &JsValue) -> String {
    match value {
        JsValue::Undefined => "undefined".to_owned(),
        _ => "null".to_owned(),
    }
}

/// `value[key]` for a string key.
pub fn get(value: &JsValue, key: &str) -> Result<JsValue, Thrown> {
    match value {
        JsValue::Undefined | JsValue::Null => Err(Thrown::type_error(format!(
            "Cannot read properties of {} (reading '{key}')",
            describe(value)
        ))),
        JsValue::Object(o) => Ok(o.get(key).cloned().unwrap_or_default()),
        JsValue::Array(items) => Ok(match array_index(&key.encode_utf16().collect::<Vec<_>>()) {
            Some(i) => items.get(i as usize).cloned().unwrap_or_default(),
            None if key == "length" => JsValue::Number(items.len() as f64),
            None => JsValue::Undefined,
        }),
        JsValue::Bytes(bytes) => Ok(match array_index(&key.encode_utf16().collect::<Vec<_>>()) {
            Some(i) => bytes
                .get(i as usize)
                .map_or(JsValue::Undefined, |b| JsValue::Number(f64::from(*b))),
            None if key == "length" => JsValue::Number(bytes.len() as f64),
            None => JsValue::Undefined,
        }),
        JsValue::String(s) => Ok(match array_index(&key.encode_utf16().collect::<Vec<_>>()) {
            Some(i) if (i as usize) < s.len() => {
                JsValue::String(JsString::from_units(vec![s.units()[i as usize]]))
            }
            None if key == "length" => JsValue::Number(s.len() as f64),
            _ => JsValue::Undefined,
        }),
        JsValue::ExtData(ty, data) => Ok(match key {
            "type" => JsValue::Number(f64::from(*ty)),
            "data" => JsValue::Bytes(data.clone()),
            _ => JsValue::Undefined,
        }),
        _ => Ok(JsValue::Undefined),
    }
}

/// `value[i]`.
pub fn index(value: &JsValue, i: usize) -> Result<JsValue, Thrown> {
    get(value, &format!("{i}"))
}

/// `key in value` for an object operand; a primitive operand throws.
pub fn has(value: &JsValue, key: &str) -> Result<bool, Thrown> {
    match value {
        JsValue::Object(o) => Ok(o.contains_key(key)),
        JsValue::Array(items) => Ok(match array_index(&key.encode_utf16().collect::<Vec<_>>()) {
            Some(i) => (i as usize) < items.len(),
            None => key == "length",
        }),
        JsValue::Bytes(bytes) => Ok(match array_index(&key.encode_utf16().collect::<Vec<_>>()) {
            Some(i) => (i as usize) < bytes.len(),
            None => false,
        }),
        JsValue::ExtData(..) => Ok(key == "type" || key == "data"),
        JsValue::Date(_) => Ok(false),
        other => Err(Thrown::type_error(format!(
            "Cannot use 'in' operator to search for '{key}' in {}",
            to_display(other)
        ))),
    }
}

fn to_display(value: &JsValue) -> String {
    match value {
        JsValue::String(s) => s.to_string_lossy(),
        other => to_string(other).to_string_lossy(),
    }
}

/// `Object.keys(value)`.
pub fn object_keys(value: &JsValue) -> Result<Vec<JsString>, Thrown> {
    match value {
        JsValue::Undefined | JsValue::Null => Err(Thrown::type_error(
            "Cannot convert undefined or null to object",
        )),
        JsValue::Object(o) => Ok(o.keys().cloned().collect()),
        JsValue::Array(items) => Ok((0..items.len())
            .map(|i| JsString::from(format!("{i}")))
            .collect()),
        JsValue::Bytes(bytes) => Ok((0..bytes.len())
            .map(|i| JsString::from(format!("{i}")))
            .collect()),
        JsValue::String(s) => Ok((0..s.len())
            .map(|i| JsString::from(format!("{i}")))
            .collect()),
        JsValue::ExtData(..) => Ok(vec![JsString::from("type"), JsString::from("data")]),
        _ => Ok(Vec::new()),
    }
}

/// `value[key]` for a key from [`object_keys`].
pub fn get_key(value: &JsValue, key: &JsString) -> Result<JsValue, Thrown> {
    match value {
        JsValue::Object(o) => Ok(o.get_key(key).cloned().unwrap_or_default()),
        other => get(other, &key.to_string_lossy()),
    }
}

/// Strict-mode `target[key] = value`. Named properties on arrays are invisible to everything
/// upstream does with them (`JSON.stringify`), so they are not kept.
pub fn set_key(target: &mut JsValue, key: &JsString, value: JsValue) -> Result<(), Thrown> {
    let name = key.to_string_lossy();
    match target {
        JsValue::Object(o) => {
            o.insert(key.clone(), value);
            Ok(())
        }
        JsValue::Array(items) => {
            if let Some(i) = array_index(key.units()) {
                let i = i as usize;
                if i >= items.len() {
                    items.resize(i + 1, JsValue::Undefined);
                }
                items[i] = value;
            }
            Ok(())
        }
        JsValue::Undefined | JsValue::Null => Err(Thrown::type_error(format!(
            "Cannot set properties of {} (setting '{name}')",
            describe(target)
        ))),
        JsValue::Bool(_) | JsValue::Number(_) | JsValue::String(_) | JsValue::BigInt(_) => {
            Err(Thrown::type_error(format!(
                "Cannot create property '{name}' on {} '{}'",
                target.type_of(),
                to_display(target)
            )))
        }
        JsValue::Bytes(_) | JsValue::Date(_) | JsValue::ExtData(..) => Ok(()),
    }
}

/// Strict-mode `delete target[key]`.
pub fn delete_key(target: &mut JsValue, key: &JsString) -> Result<(), Thrown> {
    match target {
        JsValue::Object(o) => {
            o.remove_key(key);
            Ok(())
        }
        JsValue::Array(items) => {
            if let Some(i) = array_index(key.units())
                && let Some(slot) = items.get_mut(i as usize)
            {
                *slot = JsValue::Undefined;
            }
            Ok(())
        }
        JsValue::Undefined | JsValue::Null => Err(Thrown::type_error(
            "Cannot convert undefined or null to object",
        )),
        JsValue::String(s) => match array_index(key.units()) {
            Some(i) if (i as usize) < s.len() => Err(Thrown::type_error(format!(
                "Cannot delete property '{}' of [object String]",
                key.to_string_lossy()
            ))),
            _ => Ok(()),
        },
        JsValue::Bytes(bytes) => match array_index(key.units()) {
            Some(i) if (i as usize) < bytes.len() => Err(Thrown::type_error(format!(
                "Cannot delete property '{}' of [object Uint8Array]",
                key.to_string_lossy()
            ))),
            _ => Ok(()),
        },
        _ => Ok(()),
    }
}

/// `SameValueZero`, the `Map` key comparison. Objects never compare equal: every one upstream
/// compares comes from a separate parse.
pub fn same_value_zero(a: &JsValue, b: &JsValue) -> bool {
    match (a, b) {
        (JsValue::Number(x), JsValue::Number(y)) => x == y || (x.is_nan() && y.is_nan()),
        _ => strict_equals(a, b),
    }
}

/// `===`.
pub fn strict_equals(a: &JsValue, b: &JsValue) -> bool {
    match (a, b) {
        (JsValue::Undefined, JsValue::Undefined) | (JsValue::Null, JsValue::Null) => true,
        (JsValue::Bool(x), JsValue::Bool(y)) => x == y,
        (JsValue::Number(x), JsValue::Number(y)) => x == y,
        (JsValue::String(x), JsValue::String(y)) => x == y,
        (JsValue::BigInt(x), JsValue::BigInt(y)) => x == y,
        _ => false,
    }
}

/// `ToString` for the values a decoder produces. Dates and objects never reach a numeric
/// context upstream as anything but `NaN`, so their exact text is not modelled.
pub fn to_string(value: &JsValue) -> JsString {
    match value {
        JsValue::Undefined => JsString::from("undefined"),
        JsValue::Null => JsString::from("null"),
        JsValue::Bool(b) => JsString::from(if *b { "true" } else { "false" }),
        JsValue::Number(x) => JsString::from(number::to_string(*x)),
        JsValue::String(s) => s.clone(),
        JsValue::BigInt(b) => JsString::from(format!("{b}")),
        JsValue::Array(items) => {
            let mut out = JsString::new();
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_unit(u16::from(b','));
                }
                if !matches!(item, JsValue::Undefined | JsValue::Null) {
                    out.push_str(&to_string(item));
                }
            }
            out
        }
        JsValue::Bytes(bytes) => JsString::from(
            bytes
                .iter()
                .map(|b| format!("{b}"))
                .collect::<Vec<_>>()
                .join(","),
        ),
        JsValue::Date(_) => JsString::from("Invalid Date"),
        JsValue::Object(_) | JsValue::ExtData(..) => JsString::from("[object Object]"),
    }
}

/// `ToNumber`, as a binary `-` applies it.
pub fn value_to_number(value: &JsValue) -> Result<f64, Thrown> {
    Ok(match value {
        JsValue::Undefined => f64::NAN,
        JsValue::Null => 0.0,
        JsValue::Bool(b) => f64::from(u8::from(*b)),
        JsValue::Number(x) => *x,
        JsValue::String(s) => number::to_number(s.units()),
        JsValue::BigInt(_) => {
            return Err(Thrown::type_error(
                "Cannot mix BigInt and other types, use explicit conversions",
            ));
        }
        JsValue::Date(t) => *t,
        JsValue::Array(_) | JsValue::Bytes(_) => number::to_number(to_string(value).units()),
        JsValue::Object(_) | JsValue::ExtData(..) => f64::NAN,
    })
}

pub fn object(pairs: impl IntoIterator<Item = (&'static str, JsValue)>) -> JsValue {
    let mut o = JsObject::new();
    for (k, v) in pairs {
        o.insert(k, v);
    }
    JsValue::Object(o)
}
