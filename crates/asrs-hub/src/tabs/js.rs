//! The JS operators `Tabs.ts` applies to values a restored layout can set to anything
//! (`selected`, `type`, `title`, `controlsHeight`). Inputs come from JSON, so only undefined,
//! null, booleans, numbers, strings, arrays and plain objects are modelled; other variants are
//! treated as plain objects.

use std::cmp::Ordering;

use asrs_js::string::JsString;
use asrs_js::value::{JsValue, array_index};
use asrs_js::{math, number};

enum Prim {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(JsString),
}

fn join(items: &[JsValue]) -> JsString {
    let mut out = JsString::new();
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
            other => out.push_str(&to_string(other)),
        }
    }
    out
}

fn to_primitive(v: &JsValue) -> Prim {
    match v {
        JsValue::Undefined => Prim::Undefined,
        JsValue::Null => Prim::Null,
        JsValue::Bool(b) => Prim::Bool(*b),
        JsValue::Number(n) => Prim::Number(*n),
        JsValue::String(s) => Prim::String(s.clone()),
        JsValue::Array(items) => Prim::String(join(items)),
        _ => Prim::String(JsString::from("[object Object]")),
    }
}

fn prim_to_number(p: &Prim) -> f64 {
    match p {
        Prim::Undefined => f64::NAN,
        Prim::Null => 0.0,
        Prim::Bool(b) => f64::from(u8::from(*b)),
        Prim::Number(n) => *n,
        Prim::String(s) => number::to_number(s.units()),
    }
}

fn prim_to_string(p: &Prim) -> JsString {
    match p {
        Prim::Undefined => JsString::from("undefined"),
        Prim::Null => JsString::from("null"),
        Prim::Bool(b) => JsString::from(if *b { "true" } else { "false" }),
        Prim::Number(n) => JsString::from(number::to_string(*n)),
        Prim::String(s) => s.clone(),
    }
}

/// `ToNumber`.
pub fn to_number(v: &JsValue) -> f64 {
    prim_to_number(&to_primitive(v))
}

/// `ToString`, as string concatenation applies it.
pub fn to_string(v: &JsValue) -> JsString {
    prim_to_string(&to_primitive(v))
}

/// `ToBoolean`.
pub fn truthy(v: &JsValue) -> bool {
    match v {
        JsValue::Undefined | JsValue::Null => false,
        JsValue::Bool(b) => *b,
        JsValue::Number(n) => *n != 0.0 && !n.is_nan(),
        JsValue::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// `===`. Two distinct arrays or objects are never identical, and every array or object value
/// `Tabs` compares came from a separate message or parse.
pub fn strict_eq(a: &JsValue, b: &JsValue) -> bool {
    match (a, b) {
        (JsValue::Undefined, JsValue::Undefined) | (JsValue::Null, JsValue::Null) => true,
        (JsValue::Bool(x), JsValue::Bool(y)) => x == y,
        (JsValue::Number(x), JsValue::Number(y)) => x == y,
        (JsValue::String(x), JsValue::String(y)) => x == y,
        _ => false,
    }
}

/// `SameValueZero`, as `Array.prototype.includes` and `Map.get` compare.
pub fn same_value_zero(a: &JsValue, b: &JsValue) -> bool {
    match (a, b) {
        (JsValue::Number(x), JsValue::Number(y)) => x == y || (x.is_nan() && y.is_nan()),
        _ => strict_eq(a, b),
    }
}

/// `a + b`.
pub fn add(a: &JsValue, b: &JsValue) -> JsValue {
    let (pa, pb) = (to_primitive(a), to_primitive(b));
    if matches!(pa, Prim::String(_)) || matches!(pb, Prim::String(_)) {
        let mut s = prim_to_string(&pa);
        s.push_str(&prim_to_string(&pb));
        JsValue::String(s)
    } else {
        JsValue::Number(prim_to_number(&pa) + prim_to_number(&pb))
    }
}

/// The abstract relational comparison; `None` is its `undefined` (a NaN operand).
fn compare(a: &JsValue, b: &JsValue) -> Option<Ordering> {
    match (to_primitive(a), to_primitive(b)) {
        (Prim::String(x), Prim::String(y)) => Some(x.units().cmp(y.units())),
        (pa, pb) => prim_to_number(&pa).partial_cmp(&prim_to_number(&pb)),
    }
}

pub fn lt(a: &JsValue, b: &JsValue) -> bool {
    compare(a, b) == Some(Ordering::Less)
}

pub fn gt(a: &JsValue, b: &JsValue) -> bool {
    compare(a, b) == Some(Ordering::Greater)
}

pub fn le(a: &JsValue, b: &JsValue) -> bool {
    matches!(compare(a, b), Some(Ordering::Less | Ordering::Equal))
}

pub fn ge(a: &JsValue, b: &JsValue) -> bool {
    matches!(compare(a, b), Some(Ordering::Greater | Ordering::Equal))
}

/// The element `list[key]` reads, for a list of `len` elements. Keys that are not a canonical
/// array index (including `"length"`) read as missing.
pub fn index(key: &JsValue, len: usize) -> Option<usize> {
    let key = to_string(key);
    array_index(key.units())
        .map(|i| i as usize)
        .filter(|&i| i < len)
}

/// The start index `Array.prototype.splice(start, ...)` uses on a list of `len` elements.
pub fn splice_start(start: &JsValue, len: usize) -> usize {
    let relative = number::to_integer_or_infinity(to_number(start));
    let len_f = len as f64;
    let start = if relative < 0.0 {
        math::max2(len_f + relative, 0.0)
    } else {
        math::min2(relative, len_f)
    };
    start as usize
}

pub fn num(n: f64) -> JsValue {
    JsValue::Number(n)
}

pub fn len(n: usize) -> JsValue {
    JsValue::Number(n as f64)
}

/// V8's message for reading `property` from `undefined` or `null`.
pub fn read_error(base: &JsValue, property: &str) -> String {
    format!(
        "Cannot read properties of {} (reading '{property}')",
        if base.is_null() { "null" } else { "undefined" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> JsValue {
        JsValue::string(v)
    }

    #[test]
    fn operators_follow_js() {
        assert_eq!(add(&s("1"), &num(1.0)), s("11"));
        assert_eq!(add(&num(1.0), &num(2.0)), num(3.0));
        assert_eq!(add(&JsValue::Array(vec![num(2.0)]), &num(1.0)), s("21"));
        assert!(!ge(&JsValue::Undefined, &num(1.0)));
        assert!(!lt(&JsValue::Undefined, &num(1.0)));
        assert!(ge(&JsValue::Null, &num(0.0)));
        assert!(gt(&s("51"), &num(3.0)));
        assert!(lt(&s("10"), &s("9")));
        assert!(le(&num(-0.0), &num(0.0)));
        assert_eq!(index(&s("1"), 3), Some(1));
        assert_eq!(index(&num(1.5), 3), None);
        assert_eq!(index(&num(-0.0), 3), Some(0));
        assert_eq!(index(&s("01"), 3), None);
        assert_eq!(splice_start(&num(-1.0), 4), 3);
        assert_eq!(splice_start(&num(f64::NAN), 4), 0);
        assert_eq!(splice_start(&s("9"), 4), 4);
        assert_eq!(
            to_string(&JsValue::Array(vec![JsValue::Null, num(1.0)])),
            JsString::from(",1")
        );
        assert!(same_value_zero(&num(f64::NAN), &num(f64::NAN)));
        assert!(!strict_eq(&num(f64::NAN), &num(f64::NAN)));
    }
}
