//! JS operators and property access on [`JsValue`], for upstream code that handles values decoded
//! from untyped JSON or msgpack. Module code is strict, so writes to primitives throw.

use crate::number;
use crate::string::{JsError, JsString};
use crate::value::{JsObject, JsValue, array_index};

fn describe(value: &JsValue) -> String {
    match value {
        JsValue::Undefined => "undefined".to_owned(),
        _ => "null".to_owned(),
    }
}

/// `value[key]` for a string key.
pub fn get(value: &JsValue, key: &str) -> Result<JsValue, JsError> {
    match value {
        JsValue::Undefined | JsValue::Null => Err(JsError::type_error(format!(
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
pub fn index(value: &JsValue, i: usize) -> Result<JsValue, JsError> {
    get(value, &format!("{i}"))
}

/// `key in value` for an object operand; a primitive operand throws.
pub fn has(value: &JsValue, key: &str) -> Result<bool, JsError> {
    has_key(value, &JsString::from(key))
}

/// `key in value` with a JS string key. Inherited members (`Object.prototype`'s, a typed
/// array's `length`) are not modelled.
pub fn has_key(value: &JsValue, key: &JsString) -> Result<bool, JsError> {
    let index = array_index(key.units()).map(|i| i as usize);
    match value {
        JsValue::Object(o) => Ok(o.get_key(key).is_some()),
        JsValue::Array(items) => Ok(match index {
            Some(i) => i < items.len(),
            None => key.units() == JsString::from("length").units(),
        }),
        JsValue::Bytes(bytes) => Ok(index.is_some_and(|i| i < bytes.len())),
        JsValue::ExtData(..) => {
            Ok(*key == JsString::from("type") || *key == JsString::from("data"))
        }
        JsValue::Date(_) => Ok(false),
        other => Err(JsError::type_error(format!(
            "Cannot use 'in' operator to search for '{}' in {}",
            key.to_string_lossy(),
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
pub fn object_keys(value: &JsValue) -> Result<Vec<JsString>, JsError> {
    match value {
        JsValue::Undefined | JsValue::Null => Err(JsError::type_error(
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
pub fn get_key(value: &JsValue, key: &JsString) -> Result<JsValue, JsError> {
    match value {
        JsValue::Object(o) => Ok(o.get_key(key).cloned().unwrap_or_default()),
        other => get(other, &key.to_string_lossy()),
    }
}

fn indexed_len(value: &JsValue) -> Option<usize> {
    match value {
        JsValue::Array(items) => Some(items.len()),
        JsValue::Bytes(bytes) => Some(bytes.len()),
        JsValue::String(s) => Some(s.len()),
        _ => None,
    }
}

/// `Object.getOwnPropertyNames(value)`: the wrapper's index keys plus `length` for strings,
/// arrays and typed arrays; nothing for the other primitives.
pub fn own_property_names(value: &JsValue) -> Result<Vec<JsString>, JsError> {
    let mut names = object_keys(value)?;
    if indexed_len(value).is_some() {
        names.push(JsString::from("length"));
    }
    Ok(names)
}

/// `Object.hasOwn(value, key)`: own properties only, including `length` on strings, arrays and
/// typed arrays.
pub fn has_own(value: &JsValue, key: &JsString) -> Result<bool, JsError> {
    if let Some(len) = indexed_len(value) {
        return Ok(array_index(key.units()).is_some_and(|i| (i as usize) < len)
            || *key == JsString::from("length"));
    }
    Ok(match value {
        JsValue::Object(o) => o.get_key(key).is_some(),
        other => object_keys(other)?.contains(key),
    })
}

/// `{...value}`: own enumerable properties, which for a string primitive are its index keys.
pub fn spread(value: &JsValue) -> JsObject<JsValue> {
    let mut object = JsObject::new();
    match value {
        JsValue::Object(o) => {
            for (k, v) in o.iter() {
                object.insert(k.clone(), v.clone());
            }
        }
        JsValue::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                object.insert(JsString::from(format!("{i}")), v.clone());
            }
        }
        JsValue::Bytes(bytes) => {
            for (i, b) in bytes.iter().enumerate() {
                object.insert(
                    JsString::from(format!("{i}")),
                    JsValue::Number(f64::from(*b)),
                );
            }
        }
        JsValue::String(s) => {
            for (i, &u) in s.units().iter().enumerate() {
                object.insert(
                    JsString::from(format!("{i}")),
                    JsValue::String(JsString::from_units(vec![u])),
                );
            }
        }
        _ => {}
    }
    object
}

// JS arrays go sparse; growing a dense Vec past this many holes would exhaust memory on a
// hostile index such as 4294967294, so such writes are dropped instead.
const MAX_HOLES: usize = 1 << 20;

fn grow(items: &mut Vec<JsValue>, len: usize) {
    if len > items.len() && len - items.len() <= MAX_HOLES {
        items.resize(len, JsValue::Undefined);
    }
}

/// Strict-mode `target[key] = value`. Named properties on arrays are invisible to everything
/// upstream does with them (`JSON.stringify`, `Object.keys` aside from indices), so they are not
/// kept; nor is an assignment to an object's inherited `__proto__` accessor.
pub fn set_key(target: &mut JsValue, key: &JsString, value: JsValue) -> Result<(), JsError> {
    let name = key.to_string_lossy();
    match target {
        JsValue::Object(o) => {
            if name != "__proto__" || o.contains_key("__proto__") {
                o.insert(key.clone(), value);
            }
            Ok(())
        }
        JsValue::Array(items) => {
            if let Some(i) = array_index(key.units()) {
                let i = i as usize;
                grow(items, i + 1);
                if let Some(slot) = items.get_mut(i) {
                    *slot = value;
                }
            } else if name == "length" {
                let n = value_to_number(&value)?;
                if !(0.0..=4294967295.0).contains(&n) || n.fract() != 0.0 {
                    return Err(JsError::range("Invalid array length"));
                }
                let n = n as usize;
                if n < items.len() {
                    items.truncate(n);
                } else {
                    grow(items, n);
                }
            }
            Ok(())
        }
        JsValue::Undefined | JsValue::Null => Err(JsError::type_error(format!(
            "Cannot set properties of {} (setting '{name}')",
            describe(target)
        ))),
        JsValue::String(s)
            if name == "length"
                || array_index(key.units()).is_some_and(|i| (i as usize) < s.len()) =>
        {
            Err(JsError::type_error(format!(
                "Cannot assign to read only property '{name}' of string '{}'",
                s.to_string_lossy()
            )))
        }
        JsValue::Bool(_) | JsValue::Number(_) | JsValue::String(_) | JsValue::BigInt(_) => {
            Err(JsError::type_error(format!(
                "Cannot create property '{name}' on {} '{}'",
                target.type_of(),
                to_display(target)
            )))
        }
        JsValue::Bytes(_) | JsValue::Date(_) | JsValue::ExtData(..) => Ok(()),
    }
}

/// Strict-mode `delete target[key]`.
pub fn delete_key(target: &mut JsValue, key: &JsString) -> Result<(), JsError> {
    let name = key.to_string_lossy();
    let undeletable = |kind: &str| {
        Err(JsError::type_error(format!(
            "Cannot delete property '{name}' of [object {kind}]"
        )))
    };
    match target {
        JsValue::Object(o) => {
            o.remove_key(key);
            Ok(())
        }
        JsValue::Array(items) => {
            if name == "length" {
                return undeletable("Array");
            }
            if let Some(i) = array_index(key.units())
                && let Some(slot) = items.get_mut(i as usize)
            {
                *slot = JsValue::Undefined;
            }
            Ok(())
        }
        JsValue::Undefined | JsValue::Null => Err(JsError::type_error(
            "Cannot convert undefined or null to object",
        )),
        JsValue::String(s) => {
            if name == "length" || array_index(key.units()).is_some_and(|i| (i as usize) < s.len())
            {
                undeletable("String")
            } else {
                Ok(())
            }
        }
        JsValue::Bytes(bytes) => {
            if array_index(key.units()).is_some_and(|i| (i as usize) < bytes.len()) {
                undeletable("Uint8Array")
            } else {
                Ok(())
            }
        }
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

/// `==` for the values `JSON.parse` produces. Objects compare by identity, so two object
/// operands are never equal.
pub fn loose_equals(a: &JsValue, b: &JsValue) -> bool {
    match (a, b) {
        (JsValue::Undefined | JsValue::Null, JsValue::Undefined | JsValue::Null) => true,
        (JsValue::Undefined | JsValue::Null, _) | (_, JsValue::Undefined | JsValue::Null) => false,
        (JsValue::Number(x), JsValue::String(s)) | (JsValue::String(s), JsValue::Number(x)) => {
            *x == number::to_number(s.units())
        }
        (JsValue::Bool(x), other) | (other, JsValue::Bool(x)) => {
            loose_equals(&JsValue::Number(f64::from(u8::from(*x))), other)
        }
        (
            JsValue::Array(_) | JsValue::Object(_) | JsValue::Bytes(_),
            JsValue::String(_) | JsValue::Number(_),
        ) => loose_equals(&JsValue::String(to_string(a)), b),
        (
            JsValue::String(_) | JsValue::Number(_),
            JsValue::Array(_) | JsValue::Object(_) | JsValue::Bytes(_),
        ) => loose_equals(a, &JsValue::String(to_string(b))),
        _ => strict_equals(a, b),
    }
}

/// `ToString` of a nested array without a stack frame per level: the comma-separated
/// `Array.prototype.join` text, walking in order.
fn join_into(items: &[JsValue], out: &mut JsString) {
    let mut stack = vec![(items, 0usize)];
    while let Some((items, i)) = stack.last_mut() {
        let Some(item) = items.get(*i) else {
            stack.pop();
            continue;
        };
        if *i > 0 {
            out.push_unit(u16::from(b','));
        }
        *i += 1;
        match item {
            JsValue::Array(items) => stack.push((items, 0)),
            JsValue::Undefined | JsValue::Null => {}
            leaf => out.push_str(&to_string(leaf)),
        }
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
            join_into(items, &mut out);
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

/// `ToBoolean`.
pub fn to_boolean(value: &JsValue) -> bool {
    match value {
        JsValue::Undefined | JsValue::Null => false,
        JsValue::Bool(b) => *b,
        JsValue::Number(n) => *n != 0.0 && !n.is_nan(),
        JsValue::String(s) => !s.is_empty(),
        JsValue::BigInt(b) => !b.is_zero(),
        JsValue::Array(_)
        | JsValue::Object(_)
        | JsValue::Bytes(_)
        | JsValue::Date(_)
        | JsValue::ExtData(..) => true,
    }
}

/// `ToNumber`, as a binary `-` applies it.
pub fn value_to_number(value: &JsValue) -> Result<f64, JsError> {
    Ok(match value {
        JsValue::Undefined => f64::NAN,
        JsValue::Null => 0.0,
        JsValue::Bool(b) => f64::from(u8::from(*b)),
        JsValue::Number(x) => *x,
        JsValue::String(s) => number::to_number(s.units()),
        JsValue::BigInt(_) => {
            return Err(JsError::type_error(
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
