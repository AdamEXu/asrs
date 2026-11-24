//! JS operators and property access on [`JsValue`], for upstream code that handles values decoded
//! from untyped JSON or msgpack. Module code is strict, so writes to primitives throw.
//!
//! The operators that need no property storage (`===`, `==`, `+`, `-`, the relational
//! comparisons and the primitive conversions behind them) are generic over [`JsOps`]: `JsValue`
//! implements it here and the CVE-1 `Value` implements it in the `fn` harness, so both share
//! the one coercion table.

use std::cmp::Ordering;

use crate::math::{self, BigInt};
use crate::number;
use crate::string::{JsError, JsString};
use crate::value::{JsObject, JsValue, array_index};

fn describe(value: &JsValue) -> String {
    match value {
        JsValue::Undefined => "undefined".to_owned(),
        _ => "null".to_owned(),
    }
}

/// V8's `Cannot read properties of <describe> (reading '<property>')` TypeError.
pub fn cannot_read(value: &JsValue, property: &str) -> JsError {
    JsError::type_error(format!(
        "Cannot read properties of {} (reading '{property}')",
        describe(value)
    ))
}

/// The preferred type `ToPrimitive` gets from its context: `+` and `==` pass `Default`,
/// the relational operators `Number`, and `String()` a `String`. The hint only changes
/// what a `Date` produces; a `Symbol.toPrimitive` member is not modelled.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    Default,
    Number,
    String,
}

/// A value after `ToPrimitive`: every primitive `JsValue` can produce plus `Symbol`, which
/// only CVE-1 values carry. `Symbol` holds no identity; `===`/`==` treat any two as
/// different, which is right for operands decoded separately.
#[derive(Clone)]
pub enum Prim {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(JsString),
    BigInt(BigInt),
    Symbol,
}

/// A JS value the operators run on. `JsValue` implements it here; the `fn` harness wraps
/// CVE-1 `Value`s in a newtype that implements it too, so the operator code below is the
/// only copy.
pub trait JsOps: Sized {
    /// `ToPrimitive`; primitive receivers map to themselves.
    fn to_primitive(&self, hint: Hint) -> Prim;
    /// Whether the value is already primitive (objects reduce through `to_primitive`).
    fn is_primitive(&self) -> bool;
    /// Wraps the primitive an operator produces back into the value type.
    fn from_prim(p: Prim) -> Self;
}

impl JsOps for JsValue {
    fn to_primitive(&self, hint: Hint) -> Prim {
        match self {
            JsValue::Undefined => Prim::Undefined,
            JsValue::Null => Prim::Null,
            JsValue::Bool(b) => Prim::Bool(*b),
            JsValue::Number(x) => Prim::Number(*x),
            JsValue::String(s) => Prim::String(s.clone()),
            JsValue::BigInt(b) => Prim::BigInt(b.clone()),
            JsValue::Array(_) | JsValue::Bytes(_) => Prim::String(to_string(self)),
            // `ToPrimitive` without a number hint goes through `Date.prototype.toString`,
            // whose local-zone text `to_string` does not model (see its docs).
            JsValue::Date(t) => match hint {
                Hint::Number => Prim::Number(*t),
                _ => Prim::String(JsString::from("Invalid Date")),
            },
            JsValue::Object(_) | JsValue::ExtData(..) => {
                Prim::String(JsString::from("[object Object]"))
            }
        }
    }

    fn is_primitive(&self) -> bool {
        matches!(
            self,
            JsValue::Undefined
                | JsValue::Null
                | JsValue::Bool(_)
                | JsValue::Number(_)
                | JsValue::String(_)
                | JsValue::BigInt(_)
        )
    }

    fn from_prim(p: Prim) -> Self {
        match p {
            Prim::Undefined => JsValue::Undefined,
            Prim::Null => JsValue::Null,
            Prim::Bool(b) => JsValue::Bool(b),
            Prim::Number(x) => JsValue::Number(x),
            Prim::String(s) => JsValue::String(s),
            Prim::BigInt(b) => JsValue::BigInt(b),
            Prim::Symbol => unreachable!("JsValue cannot hold a Symbol"),
        }
    }
}

impl Prim {
    /// `ToNumber` on a primitive.
    pub fn to_number(&self) -> Result<f64, JsError> {
        Ok(match self {
            Prim::Undefined => f64::NAN,
            Prim::Null => 0.0,
            Prim::Bool(b) => f64::from(u8::from(*b)),
            Prim::Number(x) => *x,
            Prim::String(s) => number::to_number(s.units()),
            Prim::BigInt(_) => {
                return Err(JsError::type_error(
                    "Cannot convert a BigInt value to a number",
                ));
            }
            Prim::Symbol => {
                return Err(JsError::type_error(
                    "Cannot convert a Symbol value to a number",
                ));
            }
        })
    }

    /// `ToString` on a primitive: `+` and `String()` go through it. A symbol throws even
    /// though `String(symbol)` itself is legal, because the operator path cannot call it.
    pub fn to_js_string(&self) -> Result<JsString, JsError> {
        Ok(match self {
            Prim::Undefined => JsString::from("undefined"),
            Prim::Null => JsString::from("null"),
            Prim::Bool(b) => JsString::from(if *b { "true" } else { "false" }),
            Prim::Number(x) => JsString::from(number::to_string(*x)),
            Prim::String(s) => s.clone(),
            Prim::BigInt(b) => JsString::from(format!("{b}")),
            Prim::Symbol => {
                return Err(JsError::type_error(
                    "Cannot convert a Symbol value to a string",
                ));
            }
        })
    }

    /// `ToBoolean` on a primitive: every object is truthy, so callers only reduce
    /// through `ToPrimitive` once they know the value is one.
    pub fn to_boolean(&self) -> bool {
        match self {
            Prim::Undefined | Prim::Null => false,
            Prim::Bool(b) => *b,
            Prim::Number(x) => *x != 0.0 && !x.is_nan(),
            Prim::String(s) => !s.is_empty(),
            Prim::BigInt(b) => !b.is_zero(),
            Prim::Symbol => true,
        }
    }
}

/// `ToNumeric`: a BigInt stays a BigInt, everything else becomes a number.
enum Numeric {
    Num(f64),
    Big(BigInt),
}

fn to_numeric(p: &Prim) -> Result<Numeric, JsError> {
    match p {
        Prim::Symbol => Err(JsError::type_error(
            "Cannot convert a Symbol value to a number",
        )),
        Prim::BigInt(b) => Ok(Numeric::Big(b.clone())),
        other => Ok(Numeric::Num(other.to_number()?)),
    }
}

fn cannot_mix() -> JsError {
    JsError::type_error("Cannot mix BigInt and other types, use explicit conversions")
}

/// `+` on two primitives: string concatenation wins when either side is a string, then a
/// BigInt pair adds, a lone BigInt is the mix TypeError, and the rest is numeric.
fn add_prims(a: &Prim, b: &Prim) -> Result<Prim, JsError> {
    if matches!(a, Prim::String(_)) || matches!(b, Prim::String(_)) {
        let mut s = a.to_js_string()?;
        s.push_str(&b.to_js_string()?);
        return Ok(Prim::String(s));
    }
    match (to_numeric(a)?, to_numeric(b)?) {
        (Numeric::Big(x), Numeric::Big(y)) => Ok(Prim::BigInt(x.add(&y))),
        (Numeric::Big(_), _) | (_, Numeric::Big(_)) => Err(cannot_mix()),
        (Numeric::Num(x), Numeric::Num(y)) => Ok(Prim::Number(x + y)),
    }
}

/// `-` on two primitives: the numeric half of `add_prims`.
fn sub_prims(a: &Prim, b: &Prim) -> Result<Prim, JsError> {
    match (to_numeric(a)?, to_numeric(b)?) {
        (Numeric::Big(x), Numeric::Big(y)) => Ok(Prim::BigInt(x.sub(&y))),
        (Numeric::Big(_), _) | (_, Numeric::Big(_)) => Err(cannot_mix()),
        (Numeric::Num(x), Numeric::Num(y)) => Ok(Prim::Number(x - y)),
    }
}

/// `StringToBigInt` for the decimal case (`StringToNumeric` would also take radix
/// literals, which callers never send).
fn string_to_bigint(s: &JsString) -> Option<BigInt> {
    BigInt::parse(&s.trim().to_string_lossy()).ok()
}

/// What a BigInt comparison converts the other operand to; `None` is the spec's
/// `undefined` (an unparsable string, `undefined` or `NaN`).
fn prim_to_bigint(p: &Prim) -> Option<BigInt> {
    match p {
        Prim::BigInt(b) => Some(b.clone()),
        Prim::Bool(b) => Some(BigInt::from_i64(i64::from(*b))),
        Prim::Null => Some(BigInt::zero()),
        Prim::String(s) => string_to_bigint(s),
        _ => None,
    }
}

/// A BigInt's mathematical order against an f64, without converting the BigInt: compare
/// it to `floor(x)` and use the fraction to break the tie.
fn bigint_cmp_f64(big: &BigInt, x: f64) -> Option<Ordering> {
    if x.is_nan() {
        return None;
    }
    let floor = math::floor(x);
    let Ok(bound) = BigInt::from_f64(floor) else {
        return Some(if x > 0.0 {
            Ordering::Less
        } else {
            Ordering::Greater
        });
    };
    Some(match big.cmp(&bound) {
        Ordering::Equal if floor == x => Ordering::Equal,
        Ordering::Equal => Ordering::Less,
        other => other,
    })
}

/// `SameValue` for primitives, the leaf of `===`.
fn strict_eq_prims(a: &Prim, b: &Prim) -> bool {
    match (a, b) {
        (Prim::Undefined, Prim::Undefined) | (Prim::Null, Prim::Null) => true,
        (Prim::Bool(x), Prim::Bool(y)) => x == y,
        (Prim::Number(x), Prim::Number(y)) => x == y,
        (Prim::String(x), Prim::String(y)) => x == y,
        (Prim::BigInt(x), Prim::BigInt(y)) => x == y,
        _ => false,
    }
}

/// `==` on primitives. Two symbols are never equal here: operands arrive as separate
/// values, so they hold distinct symbols upstream too.
fn loose_eq_prims(a: &Prim, b: &Prim) -> bool {
    match (a, b) {
        (Prim::Undefined | Prim::Null, Prim::Undefined | Prim::Null) => true,
        (Prim::Undefined | Prim::Null, _) | (_, Prim::Undefined | Prim::Null) => false,
        (Prim::Symbol, _) | (_, Prim::Symbol) => false,
        (Prim::Bool(x), other) | (other, Prim::Bool(x)) => {
            loose_eq_prims(&Prim::Number(f64::from(u8::from(*x))), other)
        }
        (Prim::Number(x), Prim::Number(y)) => x == y,
        (Prim::String(x), Prim::String(y)) => x == y,
        (Prim::BigInt(x), Prim::BigInt(y)) => x == y,
        (Prim::Number(x), Prim::String(s)) | (Prim::String(s), Prim::Number(x)) => {
            *x == number::to_number(s.units())
        }
        (Prim::BigInt(x), Prim::String(s)) | (Prim::String(s), Prim::BigInt(x)) => {
            string_to_bigint(s).is_some_and(|b| b == *x)
        }
        (Prim::Number(x), Prim::BigInt(b)) | (Prim::BigInt(b), Prim::Number(x)) => {
            x.is_finite() && x.fract() == 0.0 && BigInt::from_f64(*x).is_ok_and(|v| v == *b)
        }
    }
}

/// `===` strict equality: numbers by IEEE value (`NaN` never equal, `-0` equals `+0`),
/// BigInts by value, and reference equality for objects, which is never true here —
/// every operand upstream compares came from a separate parse.
pub fn strict_equals<V: JsOps>(a: &V, b: &V) -> bool {
    a.is_primitive()
        && b.is_primitive()
        && strict_eq_prims(
            &a.to_primitive(Hint::Default),
            &b.to_primitive(Hint::Default),
        )
}

/// `SameValueZero`, the `Map`/`Set` key comparison: `===` except `NaN` equals `NaN`.
pub fn same_value_zero<V: JsOps>(a: &V, b: &V) -> bool {
    if !a.is_primitive() || !b.is_primitive() {
        return false;
    }
    match (a.to_primitive(Hint::Default), b.to_primitive(Hint::Default)) {
        (Prim::Number(x), Prim::Number(y)) => x == y || (x.is_nan() && y.is_nan()),
        (pa, pb) => strict_eq_prims(&pa, &pb),
    }
}

/// `==` abstract equality. Two objects compare by identity, so two object operands are
/// never equal; one object side reduces through `ToPrimitive` with the default hint.
pub fn loose_equals<V: JsOps>(a: &V, b: &V) -> bool {
    if !a.is_primitive() && !b.is_primitive() {
        return false;
    }
    loose_eq_prims(
        &a.to_primitive(Hint::Default),
        &b.to_primitive(Hint::Default),
    )
}

/// `a + b`.
pub fn add<V: JsOps>(a: &V, b: &V) -> Result<V, JsError> {
    Ok(V::from_prim(add_prims(
        &a.to_primitive(Hint::Default),
        &b.to_primitive(Hint::Default),
    )?))
}

/// `a - b`.
pub fn subtract<V: JsOps>(a: &V, b: &V) -> Result<V, JsError> {
    Ok(V::from_prim(sub_prims(
        &a.to_primitive(Hint::Number),
        &b.to_primitive(Hint::Number),
    )?))
}

/// Unary `-a`: `ToNumeric`, so a BigInt negates instead of throwing.
pub fn negate<V: JsOps>(a: &V) -> Result<V, JsError> {
    Ok(V::from_prim(
        match to_numeric(&a.to_primitive(Hint::Number))? {
            Numeric::Big(b) => Prim::BigInt(b.neg()),
            Numeric::Num(x) => Prim::Number(-x),
        },
    ))
}

/// Unary `+a`: `ToNumber`, so a BigInt throws.
pub fn unary_plus<V: JsOps>(a: &V) -> Result<V, JsError> {
    Ok(V::from_prim(Prim::Number(
        a.to_primitive(Hint::Number).to_number()?,
    )))
}

/// The abstract relational comparison; `None` is its `undefined` (a NaN operand).
pub fn compare<V: JsOps>(a: &V, b: &V) -> Result<Option<Ordering>, JsError> {
    let pa = a.to_primitive(Hint::Number);
    let pb = b.to_primitive(Hint::Number);
    if let (Prim::String(x), Prim::String(y)) = (&pa, &pb) {
        return Ok(Some(x.units().cmp(y.units())));
    }
    if matches!(pa, Prim::Symbol) || matches!(pb, Prim::Symbol) {
        return Err(JsError::type_error(
            "Cannot convert a Symbol value to a number",
        ));
    }
    if matches!(pa, Prim::BigInt(_)) || matches!(pb, Prim::BigInt(_)) {
        let big_vs_f64 = |big: &Prim, x: f64| match prim_to_bigint(big) {
            Some(big) => bigint_cmp_f64(&big, x),
            None => None,
        };
        if let Prim::Number(x) = pa {
            return Ok(big_vs_f64(&pb, x).map(Ordering::reverse));
        }
        if let Prim::Number(y) = pb {
            return Ok(big_vs_f64(&pa, y));
        }
        return Ok(match (prim_to_bigint(&pa), prim_to_bigint(&pb)) {
            (Some(x), Some(y)) => Some(x.cmp(&y)),
            _ => None,
        });
    }
    Ok(pa.to_number()?.partial_cmp(&pb.to_number()?))
}

/// `a < b`.
pub fn lt<V: JsOps>(a: &V, b: &V) -> Result<bool, JsError> {
    Ok(compare(a, b)? == Some(Ordering::Less))
}

/// `a > b`.
pub fn gt<V: JsOps>(a: &V, b: &V) -> Result<bool, JsError> {
    Ok(compare(a, b)? == Some(Ordering::Greater))
}

/// `a <= b`.
pub fn le<V: JsOps>(a: &V, b: &V) -> Result<bool, JsError> {
    Ok(matches!(
        compare(a, b)?,
        Some(Ordering::Less | Ordering::Equal)
    ))
}

/// `a >= b`.
pub fn ge<V: JsOps>(a: &V, b: &V) -> Result<bool, JsError> {
    Ok(matches!(
        compare(a, b)?,
        Some(Ordering::Greater | Ordering::Equal)
    ))
}

/// `value[key]` for a string key. Inherited members are not modelled: reading one reads
/// `undefined`, and `has`/`has_own` distinguish inherited from missing (#64).
pub fn get(value: &JsValue, key: &str) -> Result<JsValue, JsError> {
    match value {
        JsValue::Undefined | JsValue::Null => Err(cannot_read(value, key)),
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
            None if key == "length" || key == "byteLength" => JsValue::Number(bytes.len() as f64),
            None if key == "byteOffset" => JsValue::Number(0.0),
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

/// The element `list[key]` reads, for a list of `len` elements: `ToPropertyKey`, then the
/// canonical array index, then the bounds check. Keys that are not a canonical index
/// (including `"length"`) read as missing.
pub fn list_index(key: &JsValue, len: usize) -> Option<usize> {
    array_index(to_string(key).units())
        .map(|i| i as usize)
        .filter(|&i| i < len)
}

/// `key in value` for an object operand; a primitive operand throws.
pub fn has(value: &JsValue, key: &str) -> Result<bool, JsError> {
    has_key(value, &JsString::from(key))
}

/// The string-keyed members each prototype contributes, for `key in value`: the `in`
/// operator sees the whole chain, so `"toString" in {}` and `"length" in new Uint8Array()`
/// hold. `get` still reads inherited members as `undefined` (they are functions the value
/// type cannot carry; see `get`).
pub const OBJECT_PROTO: &[&str] = &[
    "constructor",
    "__defineGetter__",
    "__defineSetter__",
    "hasOwnProperty",
    "__lookupGetter__",
    "__lookupSetter__",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toString",
    "valueOf",
    "__proto__",
    "toLocaleString",
];

pub const ARRAY_PROTO: &[&str] = &[
    "length",
    "constructor",
    "at",
    "concat",
    "copyWithin",
    "fill",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "lastIndexOf",
    "pop",
    "push",
    "reverse",
    "shift",
    "unshift",
    "slice",
    "sort",
    "splice",
    "includes",
    "indexOf",
    "join",
    "keys",
    "entries",
    "values",
    "forEach",
    "filter",
    "flat",
    "flatMap",
    "map",
    "every",
    "some",
    "reduce",
    "reduceRight",
    "toReversed",
    "toSorted",
    "toSpliced",
    "with",
    "toLocaleString",
    "toString",
];

/// `%TypedArray%.prototype`, shared by every typed array class.
pub const TYPED_ARRAY_PROTO: &[&str] = &[
    "constructor",
    "buffer",
    "byteLength",
    "byteOffset",
    "length",
    "entries",
    "keys",
    "values",
    "at",
    "copyWithin",
    "every",
    "fill",
    "filter",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "forEach",
    "includes",
    "indexOf",
    "join",
    "lastIndexOf",
    "map",
    "reverse",
    "reduce",
    "reduceRight",
    "set",
    "slice",
    "some",
    "sort",
    "subarray",
    "toReversed",
    "toSorted",
    "with",
    "toLocaleString",
    "toString",
];

/// `Uint8Array.prototype`, between `%TypedArray%.prototype` and the instance.
pub const UINT8_ARRAY_PROTO: &[&str] = &[
    "constructor",
    "BYTES_PER_ELEMENT",
    "toBase64",
    "setFromBase64",
    "toHex",
    "setFromHex",
];

pub const DATE_PROTO: &[&str] = &[
    "constructor",
    "toString",
    "toDateString",
    "toTimeString",
    "toISOString",
    "toUTCString",
    "toGMTString",
    "getDate",
    "setDate",
    "getDay",
    "getFullYear",
    "setFullYear",
    "getHours",
    "setHours",
    "getMilliseconds",
    "setMilliseconds",
    "getMinutes",
    "setMinutes",
    "getMonth",
    "setMonth",
    "getSeconds",
    "setSeconds",
    "getTime",
    "setTime",
    "getTimezoneOffset",
    "getUTCDate",
    "setUTCDate",
    "getUTCDay",
    "getUTCFullYear",
    "setUTCFullYear",
    "getUTCHours",
    "setUTCHours",
    "getUTCMilliseconds",
    "setUTCMilliseconds",
    "getUTCMinutes",
    "setUTCMinutes",
    "getUTCMonth",
    "setUTCMonth",
    "getUTCSeconds",
    "setUTCSeconds",
    "valueOf",
    "getYear",
    "setYear",
    "toJSON",
    "toLocaleString",
    "toLocaleDateString",
    "toLocaleTimeString",
];

/// Whether `name` appears in one of the prototype tables the `in` operator consults.
pub fn inherited(tables: &[&[&str]], name: &str) -> bool {
    tables
        .iter()
        .flat_map(|t| t.iter())
        .any(|member| *member == name)
}

/// `key in value` with a JS string key, including members inherited through the
/// prototype chain (`"toString" in {}`, `"length" in new Uint8Array()`).
pub fn has_key(value: &JsValue, key: &JsString) -> Result<bool, JsError> {
    let index = array_index(key.units()).map(|i| i as usize);
    match value {
        JsValue::Object(o) => {
            Ok(o.get_key(key).is_some() || inherited(&[OBJECT_PROTO], &key.to_string_lossy()))
        }
        JsValue::Array(items) => Ok(match index {
            Some(i) => i < items.len(),
            None => {
                key.units() == JsString::from("length").units()
                    || inherited(&[ARRAY_PROTO, OBJECT_PROTO], &key.to_string_lossy())
            }
        }),
        JsValue::Bytes(bytes) => Ok(match index {
            Some(i) => i < bytes.len(),
            None => inherited(
                &[UINT8_ARRAY_PROTO, TYPED_ARRAY_PROTO, OBJECT_PROTO],
                &key.to_string_lossy(),
            ),
        }),
        JsValue::ExtData(..) => Ok(*key == JsString::from("type")
            || *key == JsString::from("data")
            || inherited(&[OBJECT_PROTO], &key.to_string_lossy())),
        JsValue::Date(_) => Ok(inherited(
            &[DATE_PROTO, OBJECT_PROTO],
            &key.to_string_lossy(),
        )),
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

/// The kinds where `length` is an own property: arrays and string primitives (a typed
/// array's `length` is an inherited accessor, and `ExtData`'s `type`/`data` pair stands
/// alone).
fn own_length(value: &JsValue) -> Option<usize> {
    match value {
        JsValue::Array(items) => Some(items.len()),
        JsValue::String(s) => Some(s.len()),
        _ => None,
    }
}

/// `Object.getOwnPropertyNames(value)`: index keys plus `length` where it is own, which
/// excludes typed arrays.
pub fn own_property_names(value: &JsValue) -> Result<Vec<JsString>, JsError> {
    let mut names = object_keys(value)?;
    if own_length(value).is_some() {
        names.push(JsString::from("length"));
    }
    Ok(names)
}

/// `Object.hasOwn(value, key)`: own properties only, including `length` on strings and
/// arrays but not on typed arrays.
pub fn has_own(value: &JsValue, key: &JsString) -> Result<bool, JsError> {
    if let Some(len) = own_length(value) {
        return Ok(array_index(key.units()).is_some_and(|i| (i as usize) < len)
            || *key == JsString::from("length"));
    }
    Ok(match value {
        JsValue::Object(o) => o.get_key(key).is_some(),
        other => object_keys(other)?.contains(key),
    })
}

/// `[...value]`: arrays spread elements, strings code units, typed arrays bytes;
/// everything else is not iterable.
pub fn spread_iter(value: &JsValue) -> Result<Vec<JsValue>, JsError> {
    match value {
        JsValue::Array(items) => Ok(items.clone()),
        JsValue::String(s) => Ok(s
            .units()
            .iter()
            .map(|&u| JsValue::String(JsString::from_units(vec![u])))
            .collect()),
        JsValue::Bytes(bytes) => Ok(bytes
            .iter()
            .map(|&b| JsValue::Number(f64::from(b)))
            .collect()),
        other => Err(JsError::type_error(format!(
            "{} is not iterable (cannot read property Symbol(Symbol.iterator))",
            spread_describe(other)
        ))),
    }
}

fn spread_describe(value: &JsValue) -> String {
    match value {
        JsValue::Undefined => "undefined".to_owned(),
        JsValue::Null => "null".to_owned(),
        JsValue::Object(_) | JsValue::ExtData(..) => "{}".to_owned(),
        JsValue::Array(_) => to_string(value).to_string_lossy(),
        JsValue::String(s) => format!("'{}'", s.to_string_lossy()),
        other => to_string(other).to_string_lossy(),
    }
}

/// `{...value}`: own enumerable properties, which for a string primitive or a typed
/// array are the index keys.
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
        JsValue::ExtData(ty, data) => {
            object.insert("type", JsValue::Number(f64::from(*ty)));
            object.insert("data", JsValue::Bytes(data.clone()));
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
                let n = to_number(&value)?;
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
        JsValue::Bytes(bytes) => {
            if let Some(i) = array_index(key.units()) {
                if (i as usize) < bytes.len() {
                    bytes[i as usize] = math::to_uint8(to_number(&value)?);
                }
                Ok(())
            } else if matches!(
                name.as_str(),
                "length" | "byteLength" | "byteOffset" | "buffer"
            ) {
                Err(JsError::type_error(format!(
                    "Cannot set property {name} of [object Object] which has only a getter"
                )))
            } else if name == "BYTES_PER_ELEMENT" {
                Err(JsError::type_error(
                    "Cannot assign to read only property 'BYTES_PER_ELEMENT' of object '[object Uint8Array]'",
                ))
            } else {
                Ok(())
            }
        }
        JsValue::Date(_) | JsValue::ExtData(..) => Ok(()),
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
/// context upstream as anything but `NaN`, so their exact text is not modelled; a valid
/// `Date`'s string is its local-zone text, which [`time::date_to_string`](crate::time)
/// can produce where a zone is known.
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
    !value.is_primitive() || JsOps::to_primitive(value, Hint::Default).to_boolean()
}

/// `ToNumber`, as unary `+` and an `Array.prototype.length` write apply it.
pub fn to_number(value: &JsValue) -> Result<f64, JsError> {
    JsOps::to_primitive(value, Hint::Number).to_number()
}

/// `ToNumber`, as a binary `-` applies it: the BigInt failure is the operator's "mix"
/// message rather than the conversion's.
pub fn value_to_number(value: &JsValue) -> Result<f64, JsError> {
    Ok(match value {
        JsValue::Undefined => f64::NAN,
        JsValue::Null => 0.0,
        JsValue::Bool(b) => f64::from(u8::from(*b)),
        JsValue::Number(x) => *x,
        JsValue::String(s) => number::to_number(s.units()),
        JsValue::BigInt(_) => return Err(cannot_mix()),
        JsValue::Date(t) => *t,
        JsValue::Array(_) | JsValue::Bytes(_) => number::to_number(to_string(value).units()),
        JsValue::Object(_) | JsValue::ExtData(..) => f64::NAN,
    })
}

/// The start index `Array.prototype.splice(start, ...)` uses on a list of `len` elements.
pub fn splice_start(start: &JsValue, len: usize) -> usize {
    let relative = number::to_integer_or_infinity(value_to_number(start).unwrap_or(f64::NAN));
    let len_f = len as f64;
    let start = if relative < 0.0 {
        math::max2(len_f + relative, 0.0)
    } else {
        math::min2(relative, len_f)
    };
    start as usize
}

pub fn object(pairs: impl IntoIterator<Item = (&'static str, JsValue)>) -> JsValue {
    let mut o = JsObject::new();
    for (k, v) in pairs {
        o.insert(k, v);
    }
    JsValue::Object(o)
}
