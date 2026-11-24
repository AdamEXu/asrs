//! `oracle:call` ports for the JS operators in `asrs_js::ops`, driving the `js/ops` vector
//! set. The member-access helpers here run on CVE-1 `Value`s, which carry the kinds `JsValue`
//! cannot (symbols, maps, sets, typed arrays, wrappers), so they port `value[key]`, `key in
//! value`, `Object.keys`, `Object.hasOwn` and the spread/write/delete operators on `Value`
//! directly. The asrs-js vector tests include this file too, so the committed vectors and
//! `cargo xtask parity fn` exercise the same dispatch.

use asrs_js::math::{self, BigInt};
use asrs_js::number;
use asrs_js::ops::{self, Hint, JsOps, Prim};
use asrs_js::string::{JsError, JsString};
use asrs_js::time::{self, NamedZone};
use asrs_js::value::array_index;
use asrs_parity::cve1::js_property_order;
use asrs_parity::{Response, Utf16String, Value};

use crate::js_time;

pub type Function = fn(&[Value]) -> Response;

/// `oracle:call` sources this module answers, keyed by the exact source text. `oracle:call`
/// evals sources in sloppy mode; the strict write/delete ports take sources whose bodies
/// carry a `use strict` directive.
pub const CALLS: &[(&str, Function)] = &[
    ("(a, b) => a + b", |a| binop(a, ops::add)),
    ("(a, b) => a - b", |a| binop(a, ops::subtract)),
    ("(a) => -a", |a| unop(a, ops::negate)),
    ("(a) => +a", |a| unop(a, ops::unary_plus)),
    ("(a, b) => a < b", |a| cmp(a, ops::lt)),
    ("(a, b) => a <= b", |a| cmp(a, ops::le)),
    ("(a, b) => a > b", |a| cmp(a, ops::gt)),
    ("(a, b) => a >= b", |a| cmp(a, ops::ge)),
    ("(a, b) => a === b", |a| {
        Response::Ok(Value::Bool(ops::strict_equals(
            &Cve(arg(a, 0)),
            &Cve(arg(a, 1)),
        )))
    }),
    ("(a, b) => a !== b", |a| {
        Response::Ok(Value::Bool(!ops::strict_equals(
            &Cve(arg(a, 0)),
            &Cve(arg(a, 1)),
        )))
    }),
    ("(a, b) => a == b", |a| {
        Response::Ok(Value::Bool(ops::loose_equals(
            &Cve(arg(a, 0)),
            &Cve(arg(a, 1)),
        )))
    }),
    ("(a, b) => a != b", |a| {
        Response::Ok(Value::Bool(!ops::loose_equals(
            &Cve(arg(a, 0)),
            &Cve(arg(a, 1)),
        )))
    }),
    ("(k, v) => k in v", key_in_call),
    ("(o, k) => o[k]", member_get_call),
    ("(v) => Object.keys(v)", |a| names_call(a, object_keys)),
    ("(v) => Object.getOwnPropertyNames(v)", |a| {
        names_call(a, own_property_names)
    }),
    ("(o, k) => Object.hasOwn(o, k)", has_own_call),
    ("(v) => ({ ...v })", spread_obj_call),
    ("(v) => [...v]", spread_arr_call),
    ("(o, k, v) => { o[k] = v; return o; }", |a| {
        member_set(a, false)
    }),
    ("(o, k, v) => { 'use strict'; o[k] = v; return o; }", |a| {
        member_set(a, true)
    }),
    ("(o, k) => { delete o[k]; return o; }", |a| {
        member_del(a, false)
    }),
    ("(o, k) => { 'use strict'; delete o[k]; return o; }", |a| {
        member_del(a, true)
    }),
    ("(v) => String(v)", string_call),
    ("(v) => Number(v)", number_call),
    ("(v) => Boolean(v)", |a| {
        let c = Cve(arg(a, 0));
        let truthy = !c.is_primitive() || c.to_primitive(Hint::Default).to_boolean();
        Response::Ok(Value::Bool(truthy))
    }),
    ("(v) => typeof v", |a| {
        Response::Ok(Value::string(js_typeof(&arg(a, 0))))
    }),
    ("(v) => Object(v)", |a| {
        Response::Ok(object_ctor(&arg(a, 0)))
    }),
];

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Undefined)
}

pub(crate) fn throw(e: JsError) -> Response {
    Response::Throw(Value::Error(e.name.into(), e.message.into()))
}

fn type_error(message: impl Into<String>) -> Response {
    Response::Throw(Value::Error("TypeError".into(), message.into().into()))
}

pub(crate) fn zone() -> NamedZone {
    js_time::zone()
}

fn cve(args: &[Value], i: usize) -> Cve {
    Cve(arg(args, i))
}

fn binop(args: &[Value], f: impl Fn(&Cve, &Cve) -> Result<Cve, JsError>) -> Response {
    match f(&cve(args, 0), &cve(args, 1)) {
        Ok(Cve(v)) => Response::Ok(v),
        Err(e) => throw(e),
    }
}

fn unop(args: &[Value], f: impl Fn(&Cve) -> Result<Cve, JsError>) -> Response {
    match f(&cve(args, 0)) {
        Ok(Cve(v)) => Response::Ok(v),
        Err(e) => throw(e),
    }
}

fn cmp(args: &[Value], f: impl Fn(&Cve, &Cve) -> Result<bool, JsError>) -> Response {
    match f(&cve(args, 0), &cve(args, 1)) {
        Ok(b) => Response::Ok(Value::Bool(b)),
        Err(e) => throw(e),
    }
}

/// A CVE-1 `Value` as an operand of the JS operators in `asrs_js::ops`; it owns the value
/// so an operator's result can come back out through `from_prim`.
pub(crate) struct Cve(pub Value);

impl JsOps for Cve {
    fn to_primitive(&self, hint: Hint) -> Prim {
        let string = |text: &str| Prim::String(JsString::from(text));
        match &self.0 {
            Value::Undefined | Value::Hole => Prim::Undefined,
            Value::Null => Prim::Null,
            Value::Bool(b) => Prim::Bool(*b),
            Value::Number(bits) => Prim::Number(f64::from_bits(*bits)),
            Value::String(s) => Prim::String(JsString::from_units(s.units().to_vec())),
            Value::BigInt(digits) => Prim::BigInt(BigInt::parse(digits).unwrap_or_default()),
            Value::Symbol(_) => Prim::Symbol,
            Value::Boxed(inner) => Cve((**inner).clone()).to_primitive(hint),
            Value::Date(bits) => match hint {
                Hint::Number => Prim::Number(f64::from_bits(*bits)),
                _ => string(&time::date_to_string(f64::from_bits(*bits), &zone())),
            },
            Value::Array(items) | Value::TypedArray(_, items) => {
                Prim::String(JsString::from_units(array_join(items)))
            }
            Value::Bytes(bytes) => Prim::String(JsString::from_units(array_join(
                &bytes
                    .iter()
                    .map(|&b| Value::number(f64::from(b)))
                    .collect::<Vec<_>>(),
            ))),
            Value::Map(_) => string("[object Map]"),
            Value::Set(_) => string("[object Set]"),
            Value::ArrayBuffer(_) => string("[object ArrayBuffer]"),
            Value::DataView(_) => string("[object DataView]"),
            Value::Error(name, message) => {
                let mut text = name.to_string_lossy();
                let message = message.to_string_lossy();
                if !message.is_empty() {
                    if !text.is_empty() {
                        text.push_str(": ");
                    }
                    text.push_str(&message);
                }
                string(&text)
            }
            Value::Function(name) => string(&format!(
                "function {}() {{ [native code] }}",
                name.to_string_lossy()
            )),
            Value::Object(_) | Value::Instance(..) => string("[object Object]"),
        }
    }

    fn is_primitive(&self) -> bool {
        matches!(
            self.0,
            Value::Undefined
                | Value::Hole
                | Value::Null
                | Value::Bool(_)
                | Value::Number(_)
                | Value::String(_)
                | Value::BigInt(_)
                | Value::Symbol(_)
        )
    }

    fn from_prim(p: Prim) -> Self {
        Cve(match p {
            Prim::Undefined => Value::Undefined,
            Prim::Null => Value::Null,
            Prim::Bool(b) => Value::Bool(b),
            Prim::Number(x) => Value::number(x),
            Prim::String(s) => Value::String(Utf16String::from_units(s.units().to_vec())),
            Prim::BigInt(b) => Value::BigInt(format!("{b}")),
            Prim::Symbol => unreachable!("operators never produce a Symbol"),
        })
    }
}

/// `ToString` on a CVE-1 value, with the JS special case that `String(symbol)` is allowed
/// while other `ToString` uses throw.
pub(crate) fn to_units(value: &Value) -> Result<Vec<u16>, Response> {
    Cve(value.clone())
        .to_primitive(Hint::String)
        .to_js_string()
        .map(|s| s.units().to_vec())
        .map_err(throw)
}

/// The comma-separated text `Array.prototype.join()` produces for a CVE-1 array's
/// elements: holes and nullish elements contribute nothing.
pub(crate) fn array_join(items: &[Value]) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(b',' as u16);
        }
        match item {
            Value::Undefined | Value::Null | Value::Hole => {}
            other => {
                if let Ok(units) = to_units(other) {
                    out.extend_from_slice(&units);
                }
            }
        }
    }
    out
}

/// `ToPropertyKey` for a CVE-1 value: symbols become a private marker that can never
/// collide with a string key.
pub(crate) fn prop_key(v: &Value) -> Vec<u16> {
    match Cve(v.clone()).to_primitive(Hint::String) {
        Prim::Symbol => vec![0xd800, 0xdc00],
        Prim::String(s) => s.units().to_vec(),
        Prim::Number(x) => number::to_string(x).encode_utf16().collect(),
        Prim::Bool(b) => (if b { "true" } else { "false" }).encode_utf16().collect(),
        Prim::Null => "null".encode_utf16().collect(),
        Prim::Undefined => "undefined".encode_utf16().collect(),
        Prim::BigInt(b) => format!("{b}").encode_utf16().collect(),
    }
}

/// `Cannot read properties of <what> (reading '<key>')`.
pub(crate) fn cannot_read(v: &Value, key: &str) -> Response {
    let what = match v {
        Value::Null => "null",
        _ => "undefined",
    };
    type_error(format!(
        "Cannot read properties of {what} (reading '{key}')"
    ))
}

/// `container[name]` for the own properties the callers of this harness read: arrays and
/// typed arrays by canonical index, strings by index, objects by name. Inherited members
/// read as `undefined` here; [`member_get`] models the full prototype chain for the `o[k]`
/// port.
pub(crate) fn get_prop(container: &Value, name: &[u16]) -> Result<Value, Response> {
    let at_index = |len: usize, f: &dyn Fn(usize) -> Value| -> Value {
        match array_index(name) {
            Some(i) if (i as usize) < len => f(i as usize),
            _ => Value::Undefined,
        }
    };
    Ok(match container {
        Value::Null | Value::Undefined => {
            return Err(cannot_read(container, &String::from_utf16_lossy(name)));
        }
        Value::Array(items) => at_index(items.len(), &|i| match &items[i] {
            Value::Hole => Value::Undefined,
            v => v.clone(),
        }),
        Value::TypedArray(_, items) => at_index(items.len(), &|i| items[i].clone()),
        Value::Bytes(bytes) => at_index(bytes.len(), &|i| Value::number(f64::from(bytes[i]))),
        Value::String(s) => at_index(s.units().len(), &|i| {
            Value::String(Utf16String::from_units(vec![s.units()[i]]))
        }),
        Value::Object(pairs) | Value::Instance(_, pairs) => pairs
            .iter()
            .find(|(k, _)| k.units() == name)
            .map(|(_, v)| v.clone())
            .unwrap_or(Value::Undefined),
        Value::Boxed(inner) => return get_prop(inner, name),
        _ => Value::Undefined,
    })
}

pub(crate) fn js_typeof(v: &Value) -> &'static str {
    match v {
        Value::Undefined | Value::Hole => "undefined",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::BigInt(_) => "bigint",
        Value::Symbol(_) => "symbol",
        Value::Function(_) => "function",
        _ => "object",
    }
}

// ---------- prototype chains ----------

const MAP_PROTO: &[&str] = &[
    "constructor",
    "get",
    "set",
    "has",
    "delete",
    "clear",
    "entries",
    "forEach",
    "keys",
    "size",
    "values",
];

const SET_PROTO: &[&str] = &[
    "constructor",
    "has",
    "add",
    "delete",
    "difference",
    "clear",
    "entries",
    "forEach",
    "intersection",
    "isSubsetOf",
    "isSupersetOf",
    "isDisjointFrom",
    "size",
    "symmetricDifference",
    "union",
    "values",
    "keys",
];

const NUMBER_PROTO: &[&str] = &[
    "constructor",
    "toExponential",
    "toFixed",
    "toPrecision",
    "toString",
    "valueOf",
    "toLocaleString",
];

const STRING_PROTO: &[&str] = &[
    "length",
    "constructor",
    "anchor",
    "at",
    "big",
    "blink",
    "bold",
    "charAt",
    "charCodeAt",
    "codePointAt",
    "concat",
    "endsWith",
    "fontcolor",
    "fontsize",
    "fixed",
    "includes",
    "indexOf",
    "isWellFormed",
    "italics",
    "lastIndexOf",
    "link",
    "localeCompare",
    "match",
    "matchAll",
    "normalize",
    "padEnd",
    "padStart",
    "repeat",
    "replace",
    "replaceAll",
    "search",
    "slice",
    "small",
    "split",
    "strike",
    "sub",
    "substr",
    "substring",
    "sup",
    "startsWith",
    "toString",
    "toWellFormed",
    "trim",
    "trimStart",
    "trimLeft",
    "trimEnd",
    "trimRight",
    "toLocaleLowerCase",
    "toLocaleUpperCase",
    "toLowerCase",
    "toUpperCase",
    "valueOf",
];

const BOOLEAN_PROTO: &[&str] = &["constructor", "toString", "valueOf"];

const BIGINT_PROTO: &[&str] = &["constructor", "toLocaleString", "toString", "valueOf"];

const SYMBOL_PROTO: &[&str] = &["constructor", "toString", "valueOf", "description"];

const ERROR_PROTO: &[&str] = &["constructor", "name", "message", "toString"];

const FUNCTION_PROTO: &[&str] = &[
    "length",
    "name",
    "constructor",
    "apply",
    "bind",
    "call",
    "toString",
    "arguments",
    "caller",
];

const ARRAY_BUFFER_PROTO: &[&str] = &[
    "constructor",
    "byteLength",
    "slice",
    "maxByteLength",
    "resizable",
    "resize",
    "transfer",
    "transferToFixedLength",
    "detached",
];

const DATA_VIEW_PROTO: &[&str] = &[
    "constructor",
    "buffer",
    "byteLength",
    "byteOffset",
    "getInt8",
    "setInt8",
    "getUint8",
    "setUint8",
    "getInt16",
    "setInt16",
    "getUint16",
    "setUint16",
    "getInt32",
    "setInt32",
    "getUint32",
    "setUint32",
    "getFloat32",
    "setFloat32",
    "getFloat64",
    "setFloat64",
    "getBigInt64",
    "setBigInt64",
    "getBigUint64",
    "setBigUint64",
    "getFloat16",
    "setFloat16",
];

/// The own names of a typed array class's prototype: every class contributes `constructor`
/// and `BYTES_PER_ELEMENT`; `Uint8Array` adds the base64/hex converters.
fn ta_class_proto(name: &str) -> &'static [&'static str] {
    match name {
        "Uint8Array" => ops::UINT8_ARRAY_PROTO,
        _ => TA_CLASS_PROTO,
    }
}

const TA_CLASS_PROTO: &[&str] = &["constructor", "BYTES_PER_ELEMENT"];

/// The prototype tables `key in value` and `o[k]` consult, in lookup order: own
/// properties are checked separately before these. `Boxed` values take the wrapped
/// primitive's prototype (`Object("ab")` is a `String` wrapper), and primitives box the
/// same way for member reads (the `in` operator throws on them before it gets here).
fn proto_chain(v: &Value) -> Vec<&'static [&'static str]> {
    match v {
        Value::Object(_) | Value::Instance(..) => vec![ops::OBJECT_PROTO],
        Value::Array(_) => vec![ops::ARRAY_PROTO, ops::OBJECT_PROTO],
        Value::TypedArray(name, _) => vec![
            ta_class_proto(name),
            ops::TYPED_ARRAY_PROTO,
            ops::OBJECT_PROTO,
        ],
        Value::Bytes(_) => vec![
            ops::UINT8_ARRAY_PROTO,
            ops::TYPED_ARRAY_PROTO,
            ops::OBJECT_PROTO,
        ],
        Value::ArrayBuffer(_) => vec![ARRAY_BUFFER_PROTO, ops::OBJECT_PROTO],
        Value::DataView(_) => vec![DATA_VIEW_PROTO, ops::OBJECT_PROTO],
        Value::Date(_) => vec![ops::DATE_PROTO, ops::OBJECT_PROTO],
        Value::Map(_) => vec![MAP_PROTO, ops::OBJECT_PROTO],
        Value::Set(_) => vec![SET_PROTO, ops::OBJECT_PROTO],
        Value::Error(..) => vec![ERROR_PROTO, ops::OBJECT_PROTO],
        Value::Function(_) => vec![FUNCTION_PROTO, ops::OBJECT_PROTO],
        Value::Boxed(inner) => proto_chain(inner.as_ref()),
        Value::String(_) => vec![STRING_PROTO, ops::OBJECT_PROTO],
        Value::Number(_) => vec![NUMBER_PROTO, ops::OBJECT_PROTO],
        Value::Bool(_) => vec![BOOLEAN_PROTO, ops::OBJECT_PROTO],
        Value::BigInt(_) => vec![BIGINT_PROTO, ops::OBJECT_PROTO],
        Value::Symbol(_) => vec![SYMBOL_PROTO, ops::OBJECT_PROTO],
        _ => vec![],
    }
}

// ---------- member access on Value ----------

/// Own-property read, `None` when `name` is not own. `"stack"` on an error is own but its
/// text is machine-dependent, so it is modelled as absent for reads; the own checks
/// (`hasOwn`, `getOwnPropertyNames`) report it separately.
fn own_get(v: &Value, name: &[u16]) -> Option<Value> {
    let key = String::from_utf16_lossy(name);
    match v {
        Value::Object(pairs) | Value::Instance(_, pairs) => pairs
            .iter()
            .find(|(k, _)| k.units() == name)
            .map(|(_, v)| v.clone()),
        Value::Array(items) => {
            if key == "length" {
                return Some(Value::number(items.len() as f64));
            }
            match array_index(name) {
                Some(i) if (i as usize) < items.len() => match &items[i as usize] {
                    Value::Hole => None,
                    v => Some(v.clone()),
                },
                _ => None,
            }
        }
        Value::TypedArray(_, items) => match array_index(name) {
            Some(i) if (i as usize) < items.len() => Some(items[i as usize].clone()),
            _ => None,
        },
        Value::Bytes(bytes) => match array_index(name) {
            Some(i) if (i as usize) < bytes.len() => {
                Some(Value::number(f64::from(bytes[i as usize])))
            }
            _ => None,
        },
        Value::String(s) => string_own(s, name),
        Value::Error(name2, message) => match key.as_str() {
            "message" => Some(Value::String(message.clone())),
            "name" => Some(Value::String(name2.clone())),
            _ => None,
        },
        Value::Boxed(inner) => match inner.as_ref() {
            Value::String(s) => string_own(s, name),
            _ => None,
        },
        _ => None,
    }
}

fn string_own(s: &Utf16String, name: &[u16]) -> Option<Value> {
    if name == JsString::from("length").units() {
        return Some(Value::number(s.units().len() as f64));
    }
    match array_index(name) {
        Some(i) if (i as usize) < s.units().len() => {
            Some(Value::String(Utf16String::from_units(vec![
                s.units()[i as usize],
            ])))
        }
        _ => None,
    }
}

/// The byte width of a typed array element kind.
fn ta_width(name: &str) -> usize {
    match name {
        "Int16Array" | "Uint16Array" => 2,
        "Int32Array" | "Uint32Array" | "Float32Array" => 4,
        "Float64Array" | "BigInt64Array" | "BigUint64Array" => 8,
        _ => 1,
    }
}

/// The value of a typed array's `buffer` slot: the elements' little-endian bytes, which
/// is what the harness's `new T(items)` produces.
fn ta_buffer(name: &str, items: &[Value]) -> Vec<u8> {
    let mut out = Vec::with_capacity(items.len() * ta_width(name));
    for item in items {
        match (name, item) {
            ("Float64Array", Value::Number(bits)) => out.extend_from_slice(&bits.to_le_bytes()),
            ("Float32Array", Value::Number(bits)) => {
                out.extend_from_slice(&f32::to_bits(f64::from_bits(*bits) as f32).to_le_bytes())
            }
            ("Int32Array" | "Uint32Array", Value::Number(bits)) => {
                out.extend_from_slice(&math::to_int32(f64::from_bits(*bits)).to_le_bytes())
            }
            ("Int16Array" | "Uint16Array", Value::Number(bits)) => {
                out.extend_from_slice(&math::to_int16(f64::from_bits(*bits)).to_le_bytes())
            }
            ("BigInt64Array" | "BigUint64Array", Value::BigInt(d)) => {
                let b = BigInt::parse(d).unwrap_or_default();
                out.extend_from_slice(&b.as_uint64().to_le_bytes())
            }
            (_, Value::Number(bits)) => out.push(math::to_uint8(f64::from_bits(*bits))),
            _ => out.extend(std::iter::repeat_n(0, ta_width(name))),
        }
    }
    out
}

/// The value a prototype member returns when read: methods encode as `{fn: <name>}`;
/// accessor and data properties produce real values.
fn member_get(v: &Value, name: &[u16], key: &str) -> Result<Value, Response> {
    if matches!(v, Value::Null | Value::Undefined | Value::Hole) {
        return Err(cannot_read(v, key));
    }
    if let Some(value) = own_get(v, name) {
        return Ok(value);
    }
    if let Some(value) = accessor_get(v, key) {
        return Ok(value);
    }
    if key == "__proto__" {
        return Ok(match v {
            Value::Array(_) => Value::Array(vec![]),
            Value::Bytes(_) => Value::Instance(Utf16String::from("Uint8Array"), vec![]),
            Value::TypedArray(name, _) => Value::Instance(Utf16String::from(name.clone()), vec![]),
            _ => Value::Object(vec![]),
        });
    }
    if ops::inherited(&proto_chain(v), key) {
        return Ok(proto_value(v, key));
    }
    Ok(Value::Undefined)
}

/// `o[k]` where `k` names an inherited member: `constructor` gives the kind's class
/// function, getters give their data, everything else is the method named `key`.
fn proto_value(v: &Value, key: &str) -> Value {
    if key == "constructor" {
        return Value::Function(Utf16String::from(ctor_name(v)));
    }
    Value::Function(Utf16String::from(key))
}

fn ctor_name(v: &Value) -> String {
    match v {
        Value::Array(_) => "Array".to_owned(),
        Value::Bytes(_) => "Uint8Array".to_owned(),
        Value::TypedArray(name, _) => name.clone(),
        Value::ArrayBuffer(_) => "ArrayBuffer".to_owned(),
        Value::DataView(_) => "DataView".to_owned(),
        Value::Date(_) => "Date".to_owned(),
        Value::Map(_) => "Map".to_owned(),
        Value::Set(_) => "Set".to_owned(),
        Value::Error(name, _) => name.to_string_lossy(),
        Value::Instance(name, _) => name.to_string_lossy(),
        Value::Function(_) => "Function".to_owned(),
        Value::Boxed(inner) => ctor_name(inner),
        Value::String(_) => "String".to_owned(),
        Value::Number(_) => "Number".to_owned(),
        Value::Bool(_) => "Boolean".to_owned(),
        Value::BigInt(_) => "BigInt".to_owned(),
        Value::Symbol(_) => "Symbol".to_owned(),
        _ => "Object".to_owned(),
    }
}

/// The getters (and the prototype data properties that behave like them) a member read
/// produces instead of a function.
fn accessor_get(v: &Value, key: &str) -> Option<Value> {
    let number = |x: f64| Some(Value::number(x));
    match v {
        Value::Bytes(b) => match key {
            "length" | "byteLength" => number(b.len() as f64),
            "byteOffset" => number(0.0),
            "buffer" => Some(Value::ArrayBuffer(b.clone())),
            "BYTES_PER_ELEMENT" => number(1.0),
            _ => None,
        },
        Value::TypedArray(name, items) => match key {
            "length" => number(items.len() as f64),
            "byteLength" => number(items.len() as f64 * ta_width(name) as f64),
            "byteOffset" => number(0.0),
            "buffer" => Some(Value::ArrayBuffer(ta_buffer(name, items))),
            "BYTES_PER_ELEMENT" => number(ta_width(name) as f64),
            _ => None,
        },
        Value::Map(pairs) if key == "size" => number(pairs.len() as f64),
        Value::Set(items) if key == "size" => number(items.len() as f64),
        Value::ArrayBuffer(b) => match key {
            "byteLength" | "maxByteLength" => number(b.len() as f64),
            "resizable" | "detached" => Some(Value::Bool(false)),
            _ => None,
        },
        Value::DataView(b) => match key {
            "buffer" => Some(Value::ArrayBuffer(b.clone())),
            "byteLength" => number(b.len() as f64),
            "byteOffset" => number(0.0),
            _ => None,
        },
        Value::Symbol(d) if key == "description" => Some(match d {
            Some(d) => Value::String(d.clone()),
            None => Value::Undefined,
        }),
        Value::Boxed(inner) => accessor_get(inner, key),
        _ => None,
    }
}

fn key_in_call(args: &[Value]) -> Response {
    let v = arg(args, 1);
    let key_arg = arg(args, 0);
    let name = prop_key(&key_arg);
    match v {
        Value::Undefined
        | Value::Hole
        | Value::Null
        | Value::Bool(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::BigInt(_)
        | Value::Symbol(_) => type_error(format!(
            "Cannot use 'in' operator to search for '{}' in {}",
            key_display(&key_arg),
            in_display(&v)
        )),
        _ => {
            let has = has_own(&v, &name)
                || ops::inherited(&proto_chain(&v), &String::from_utf16_lossy(&name));
            Response::Ok(Value::Bool(has))
        }
    }
}

/// How `in` spells a primitive right operand in its TypeError: the `ToString` text
/// (`Symbol(x)` spells itself, as `String(symbol)` is legal).
fn in_display(v: &Value) -> String {
    match v {
        Value::Number(bits) => number::to_string(f64::from_bits(*bits)),
        Value::String(s) => s.to_string_lossy(),
        Value::Bool(b) => if *b { "true" } else { "false" }.to_owned(),
        Value::Null => "null".to_owned(),
        Value::BigInt(d) => d.clone(),
        Value::Symbol(d) => match d {
            Some(d) => format!("Symbol({})", d.to_string_lossy()),
            None => "Symbol()".to_owned(),
        },
        _ => "undefined".to_owned(),
    }
}

/// How an error message spells a property key: a symbol spells `Symbol(desc)`.
fn key_display(v: &Value) -> String {
    match v {
        Value::Symbol(d) => match d {
            Some(d) => format!("Symbol({})", d.to_string_lossy()),
            None => "Symbol()".to_owned(),
        },
        _ => String::from_utf16_lossy(&prop_key(v)),
    }
}

fn member_get_call(args: &[Value]) -> Response {
    let key_arg = arg(args, 1);
    let name = prop_key(&key_arg);
    match member_get(&arg(args, 0), &name, &key_display(&key_arg)) {
        Ok(v) => Response::Ok(v),
        Err(r) => r,
    }
}

fn names_call(
    args: &[Value],
    f: impl Fn(&Value) -> Result<Vec<Utf16String>, Response>,
) -> Response {
    match f(&arg(args, 0)) {
        Ok(names) => Response::Ok(Value::Array(names.into_iter().map(Value::String).collect())),
        Err(r) => r,
    }
}

fn has_own_call(args: &[Value]) -> Response {
    let v = arg(args, 0);
    if matches!(v, Value::Null | Value::Undefined | Value::Hole) {
        return type_error("Cannot convert undefined or null to object");
    }
    let name = prop_key(&arg(args, 1));
    Response::Ok(Value::Bool(has_own(&v, &name)))
}

/// `Object.hasOwn`: own properties only, including `stack`/`message` on errors and
/// `length` on strings and arrays, but not the typed array accessors.
fn has_own(v: &Value, name: &[u16]) -> bool {
    match v {
        Value::Error(..) => matches!(String::from_utf16_lossy(name).as_str(), "stack" | "message"),
        _ => own_get(v, name).is_some(),
    }
}

fn object_keys(v: &Value) -> Result<Vec<Utf16String>, Response> {
    if matches!(v, Value::Null | Value::Undefined | Value::Hole) {
        return Err(type_error("Cannot convert undefined or null to object"));
    }
    let index = |i: usize| Utf16String::from(format!("{i}"));
    Ok(match v {
        Value::Object(pairs) | Value::Instance(_, pairs) => {
            pairs.iter().map(|(k, _)| k.clone()).collect()
        }
        Value::Array(items) => items
            .iter()
            .enumerate()
            .filter(|(_, v)| !matches!(v, Value::Hole))
            .map(|(i, _)| index(i))
            .collect(),
        Value::TypedArray(_, items) => (0..items.len()).map(index).collect(),
        Value::Bytes(b) => (0..b.len()).map(index).collect(),
        Value::String(s) => (0..s.units().len()).map(index).collect(),
        Value::Boxed(inner) => match inner.as_ref() {
            Value::String(s) => (0..s.units().len()).map(index).collect(),
            _ => vec![],
        },
        _ => vec![],
    })
}

/// `Object.getOwnPropertyNames`: enumerable own keys plus the non-enumerable `length`
/// on strings and arrays, and `stack`/`message` on errors.
fn own_property_names(v: &Value) -> Result<Vec<Utf16String>, Response> {
    if matches!(v, Value::Error(..)) {
        return Ok(vec![
            Utf16String::from("stack"),
            Utf16String::from("message"),
        ]);
    }
    let mut names = object_keys(v)?;
    let has_length = matches!(v, Value::Array(_) | Value::String(_))
        || matches!(v, Value::Boxed(inner) if matches!(inner.as_ref(), Value::String(_)));
    if has_length {
        names.push(Utf16String::from("length"));
    }
    Ok(names)
}

fn spread_obj_call(args: &[Value]) -> Response {
    let v = arg(args, 0);
    let index = |i: usize| Utf16String::from(format!("{i}"));
    let pairs: Vec<(Utf16String, Value)> = match &v {
        Value::Object(pairs) | Value::Instance(_, pairs) => pairs.clone(),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .filter(|(_, v)| !matches!(v, Value::Hole))
            .map(|(i, v)| (index(i), v.clone()))
            .collect(),
        Value::TypedArray(_, items) => items
            .iter()
            .enumerate()
            .map(|(i, v)| (index(i), v.clone()))
            .collect(),
        Value::Bytes(b) => b
            .iter()
            .enumerate()
            .map(|(i, &b)| (index(i), Value::number(f64::from(b))))
            .collect(),
        Value::String(s) => s
            .units()
            .iter()
            .enumerate()
            .map(|(i, &u)| (index(i), Value::String(Utf16String::from_units(vec![u]))))
            .collect(),
        Value::Boxed(inner) => match inner.as_ref() {
            Value::String(s) => s
                .units()
                .iter()
                .enumerate()
                .map(|(i, &u)| (index(i), Value::String(Utf16String::from_units(vec![u]))))
                .collect(),
            _ => vec![],
        },
        _ => vec![],
    };
    Response::Ok(Value::Object(pairs))
}

/// `[...string]` iterates code points: a surrogate pair yields one two-unit string.
fn code_point_strings(units: &[u16]) -> Vec<Value> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < units.len() {
        let pair = (0xD800..0xDC00).contains(&units[i])
            && units
                .get(i + 1)
                .is_some_and(|lo| (0xDC00..0xE000).contains(lo));
        let width = if pair { 2 } else { 1 };
        out.push(Value::String(Utf16String::from_units(
            units[i..i + width].to_vec(),
        )));
        i += width;
    }
    out
}

fn spread_arr_call(args: &[Value]) -> Response {
    match &arg(args, 0) {
        Value::Array(items) => Response::Ok(Value::Array(
            items
                .iter()
                .map(|v| match v {
                    Value::Hole => Value::Undefined,
                    v => v.clone(),
                })
                .collect(),
        )),
        Value::TypedArray(_, items) => Response::Ok(Value::Array(items.clone())),
        Value::Bytes(b) => Response::Ok(Value::Array(
            b.iter().map(|&b| Value::number(f64::from(b))).collect(),
        )),
        Value::Set(items) => Response::Ok(Value::Array(items.clone())),
        Value::Map(pairs) => Response::Ok(Value::Array(
            pairs
                .iter()
                .map(|(k, v)| Value::Array(vec![k.clone(), v.clone()]))
                .collect(),
        )),
        Value::String(s) => Response::Ok(Value::Array(code_point_strings(s.units()))),
        Value::Boxed(inner) => match inner.as_ref() {
            Value::String(s) => Response::Ok(Value::Array(code_point_strings(s.units()))),
            _ => type_error("v is not iterable"),
        },
        _ => type_error("v is not iterable"),
    }
}

// ---------- writes and deletes on Value ----------

/// `Cannot set property <key> of <this> which has only a getter`.
fn getter_only(key: &str, of: &str) -> Response {
    type_error(format!(
        "Cannot set property {key} of {of} which has only a getter"
    ))
}

/// `Cannot assign to read only property '<key>' of <of>`.
fn read_only(key: &str, of: &str) -> Response {
    type_error(format!(
        "Cannot assign to read only property '{key}' of {of}"
    ))
}

/// `Cannot create property '<key>' on <kind> '<display>'`.
fn cannot_create(key: &str, kind: &str, display: &str) -> Response {
    type_error(format!(
        "Cannot create property '{key}' on {kind} '{display}'"
    ))
}

/// `Cannot set properties of <what> (setting '<key>')`.
fn cannot_set(key: &str, what: &str) -> Response {
    type_error(format!("Cannot set properties of {what} (setting '{key}')"))
}

/// `Cannot delete property '<key>' of [object <kind>]`.
fn cannot_delete(key: &str, kind: &str) -> Response {
    type_error(format!("Cannot delete property '{key}' of [object {kind}]"))
}

/// `CanonicalNumericIndexString`: the strings typed array `[[Set]]` treats as element
/// writes. A non-integer like `"1.5"` counts too, so the value conversion runs and can
/// throw even though no element is stored.
fn canonical_numeric(name: &[u16]) -> Option<f64> {
    let s = String::from_utf16_lossy(name);
    let x: f64 = s.parse().ok()?;
    (number::to_string(x) == s).then_some(x)
}

/// The element a typed array write stores: `ToNumber` for the numeric kinds, `ToBigInt`
/// for the `Big*64` kinds.
fn ta_element(name: &str, v: &Value) -> Result<Value, Response> {
    if matches!(name, "BigInt64Array" | "BigUint64Array") {
        return match v {
            Value::BigInt(_) => Ok(v.clone()),
            Value::String(s) => BigInt::parse(&s.to_string_lossy())
                .map(|b| Value::BigInt(format!("{b}")))
                .map_err(|_| {
                    Response::Throw(Value::Error(
                        "SyntaxError".into(),
                        format!("Cannot convert {} to a BigInt", s.to_string_lossy()).into(),
                    ))
                }),
            Value::Bool(b) => Ok(Value::BigInt(if *b { "1" } else { "0" }.to_owned())),
            other => Err(type_error(format!(
                "Cannot convert {} to a BigInt",
                in_display(other)
            ))),
        };
    }
    let x = Cve(v.clone())
        .to_primitive(Hint::Number)
        .to_number()
        .map_err(throw)?;
    Ok(Value::number(match name {
        "Int8Array" => f64::from(math::to_int8(x)),
        "Uint8Array" => f64::from(math::to_uint8(x)),
        "Uint8ClampedArray" => f64::from(math::to_uint8_clamp(x)),
        "Int16Array" => f64::from(math::to_int16(x)),
        "Uint16Array" => f64::from(math::to_uint16(x)),
        "Int32Array" => f64::from(math::to_int32(x)),
        "Uint32Array" => f64::from(math::to_uint32(x)),
        "Float32Array" => x as f32 as f64,
        _ => x,
    }))
}

/// The array length a `length` write accepts (`ToUint32`), or the `RangeError`.
fn array_length(v: &Value) -> Result<usize, Response> {
    let x = Cve(v.clone())
        .to_primitive(Hint::Number)
        .to_number()
        .map_err(throw)?;
    if !(0.0..=4294967295.0).contains(&x) || x.fract() != 0.0 {
        return Err(Response::Throw(Value::Error(
            "RangeError".into(),
            "Invalid array length".into(),
        )));
    }
    Ok(x as usize)
}

fn member_set(args: &[Value], strict: bool) -> Response {
    let mut target = arg(args, 0);
    let name = prop_key(&arg(args, 1));
    let key = String::from_utf16_lossy(&name);
    let value = arg(args, 2);
    if let Err(r) = set_member(&mut target, &name, &key, value, strict) {
        return r;
    }
    Response::Ok(target)
}

fn set_member(
    target: &mut Value,
    name: &[u16],
    key: &str,
    value: Value,
    strict: bool,
) -> Result<(), Response> {
    match target {
        Value::Undefined | Value::Hole | Value::Null => {
            let what = if matches!(target, Value::Null) {
                "null"
            } else {
                "undefined"
            };
            Err(cannot_set(key, what))
        }
        Value::Object(pairs) | Value::Instance(_, pairs) => {
            if key != "__proto__" {
                if let Some(slot) = pairs.iter_mut().find(|(k, _)| k.units() == name) {
                    slot.1 = value;
                } else {
                    pairs.push((Utf16String::from_units(name.to_vec()), value));
                    *pairs = js_property_order(std::mem::take(pairs));
                }
            }
            Ok(())
        }
        Value::Array(items) => {
            if key == "length" {
                let n = array_length(&value)?;
                items.resize(n, Value::Hole);
                return Ok(());
            }
            if let Some(i) = array_index(name) {
                let i = i as usize;
                if i >= items.len() {
                    items.resize(i + 1, Value::Hole);
                }
                items[i] = value;
            }
            Ok(())
        }
        Value::Bytes(bytes) => {
            // V8 converts the value before it checks the index, so a conversion TypeError
            // fires on out-of-bounds and non-integer numeric keys in both modes.
            if canonical_numeric(name).is_some() {
                let x = Cve(value)
                    .to_primitive(Hint::Number)
                    .to_number()
                    .map_err(throw)?;
                if let Some(i) = array_index(name)
                    && (i as usize) < bytes.len()
                {
                    bytes[i as usize] = math::to_uint8(x);
                }
                return Ok(());
            }
            ta_named_write(key, "Uint8Array", strict)
        }
        Value::TypedArray(name2, items) => {
            if canonical_numeric(name).is_some() {
                let element = ta_element(name2, &value)?;
                if let Some(i) = array_index(name)
                    && (i as usize) < items.len()
                {
                    items[i as usize] = element;
                }
                return Ok(());
            }
            ta_named_write(key, name2, strict)
        }
        Value::String(s) if strict => {
            let text = s.to_string_lossy();
            let immutable = key == "length"
                || array_index(name).is_some_and(|i| (i as usize) < s.units().len());
            Err(if immutable {
                read_only(key, &format!("string '{text}'"))
            } else {
                cannot_create(key, "string", &text)
            })
        }
        Value::Number(bits) if strict => Err(cannot_create(
            key,
            "number",
            &number::to_string(f64::from_bits(*bits)),
        )),
        Value::Bool(b) if strict => Err(cannot_create(
            key,
            "boolean",
            if *b { "true" } else { "false" },
        )),
        Value::BigInt(d) if strict => Err(cannot_create(key, "bigint", d)),
        Value::Symbol(d) if strict => Err(cannot_create(
            key,
            "symbol",
            &match d {
                Some(d) => format!("Symbol({})", d.to_string_lossy()),
                None => "Symbol()".to_owned(),
            },
        )),
        Value::Map(_) | Value::Set(_) if strict && key == "size" => {
            let of = if matches!(target, Value::Map(_)) {
                "#<Map>"
            } else {
                "#<Set>"
            };
            Err(getter_only(key, of))
        }
        Value::ArrayBuffer(_)
            if strict
                && matches!(
                    key,
                    "byteLength" | "maxByteLength" | "resizable" | "detached"
                ) =>
        {
            Err(getter_only(key, "#<ArrayBuffer>"))
        }
        Value::DataView(_) if strict && matches!(key, "buffer" | "byteLength" | "byteOffset") => {
            Err(getter_only(key, "#<DataView>"))
        }
        Value::Error(_, message) if key == "message" => {
            *message = Utf16String::from_units(to_units(&value)?);
            Ok(())
        }
        Value::Boxed(inner) => {
            if strict
                && let Value::String(s) = inner.as_ref()
                && (key == "length"
                    || array_index(name).is_some_and(|i| (i as usize) < s.units().len()))
            {
                return Err(read_only(key, "object '[object String]'"));
            }
            // BigInt and Symbol wrappers encode as `c` instances, which keep own
            // properties; `box` wrappers drop them, so only these writes are visible.
            if key != "__proto__" && matches!(inner.as_ref(), Value::BigInt(_) | Value::Symbol(_)) {
                *target = Value::Instance(
                    Utf16String::from(ctor_name(inner)),
                    vec![(Utf16String::from_units(name.to_vec()), value)],
                );
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// A named write on a typed array: index writes are handled earlier; `length`,
/// `byteLength`, `byteOffset` and `buffer` are getter-only, `BYTES_PER_ELEMENT` is a
/// read-only data property, and any other name writes an invisible own property.
fn ta_named_write(key: &str, class: &str, strict: bool) -> Result<(), Response> {
    if !strict {
        return Ok(());
    }
    if matches!(key, "length" | "byteLength" | "byteOffset" | "buffer") {
        Err(getter_only(key, "[object Object]"))
    } else if key == "BYTES_PER_ELEMENT" {
        Err(read_only(
            "BYTES_PER_ELEMENT",
            &format!("object '[object {class}]'"),
        ))
    } else {
        Ok(())
    }
}

fn member_del(args: &[Value], strict: bool) -> Response {
    let mut target = arg(args, 0);
    let name = prop_key(&arg(args, 1));
    let key = String::from_utf16_lossy(&name);
    if let Err(r) = del_member(&mut target, &name, &key, strict) {
        return r;
    }
    Response::Ok(target)
}

fn del_member(target: &mut Value, name: &[u16], key: &str, strict: bool) -> Result<(), Response> {
    match target {
        Value::Undefined | Value::Hole | Value::Null => {
            Err(type_error("Cannot convert undefined or null to object"))
        }
        Value::Object(pairs) | Value::Instance(_, pairs) => {
            if let Some(i) = pairs.iter().position(|(k, _)| k.units() == name) {
                pairs.remove(i);
            }
            Ok(())
        }
        Value::Array(items) => {
            if key == "length" {
                return if strict {
                    Err(cannot_delete(key, "Array"))
                } else {
                    Ok(())
                };
            }
            if let Some(i) = array_index(name)
                && let Some(slot) = items.get_mut(i as usize)
            {
                *slot = Value::Hole;
            }
            Ok(())
        }
        Value::Bytes(b) => {
            if strict && array_index(name).is_some_and(|i| (i as usize) < b.len()) {
                Err(cannot_delete(key, "Uint8Array"))
            } else {
                Ok(())
            }
        }
        Value::TypedArray(name2, items) => {
            if strict && array_index(name).is_some_and(|i| (i as usize) < items.len()) {
                Err(cannot_delete(key, name2))
            } else {
                Ok(())
            }
        }
        Value::String(s) => {
            let in_range = key == "length"
                || array_index(name).is_some_and(|i| (i as usize) < s.units().len());
            if strict && in_range {
                Err(cannot_delete(key, "String"))
            } else {
                Ok(())
            }
        }
        Value::Error(_, message) if key == "message" => {
            *message = Utf16String::from("");
            Ok(())
        }
        Value::Boxed(inner) if strict => match inner.as_ref() {
            Value::String(s)
                if key == "length"
                    || array_index(name).is_some_and(|i| (i as usize) < s.units().len()) =>
            {
                Err(cannot_delete(key, "String"))
            }
            _ => Ok(()),
        },
        _ => Ok(()),
    }
}

// ---------- conversions ----------

fn string_call(args: &[Value]) -> Response {
    match &arg(args, 0) {
        Value::Symbol(d) => Response::Ok(Value::string(&match d {
            Some(d) => format!("Symbol({})", d.to_string_lossy()),
            None => "Symbol()".to_owned(),
        })),
        v => match to_units(v) {
            Ok(units) => Response::Ok(Value::String(Utf16String::from_units(units))),
            Err(r) => r,
        },
    }
}

fn number_call(args: &[Value]) -> Response {
    // `Number()` is `ToNumber` except a BigInt converts to a double instead of throwing.
    match Cve(arg(args, 0)).to_primitive(Hint::Number) {
        Prim::BigInt(b) => Response::Ok(Value::number(b.to_f64())),
        prim => match prim.to_number() {
            Ok(x) => Response::Ok(Value::number(x)),
            Err(e) => throw(e),
        },
    }
}

/// `Object(v)`: primitives wrap, objects pass through, nullish becomes `{}`. The encoder
/// spells BigInt and Symbol wrappers as `c` instances.
fn object_ctor(v: &Value) -> Value {
    match v {
        Value::Undefined | Value::Hole | Value::Null => Value::Object(vec![]),
        primitive if Cve(primitive.clone()).is_primitive() => {
            Value::Boxed(Box::new(primitive.clone()))
        }
        object => object.clone(),
    }
}
