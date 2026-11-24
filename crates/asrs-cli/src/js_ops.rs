//! `oracle:call` ports for the operators of `asrs_js::ops`, driving the `js/ops` vectors. A
//! request whose operands `JsValue` can hold runs the `JsValue` functions the app uses. The
//! operators that take any value also run over the CVE-1 wrapper [`Cve`], and the two must
//! agree; operands only CVE-1 can hold run over the wrapper alone.

use std::borrow::Cow;

use asrs_js::math::BigInt;
use asrs_js::number;
use asrs_js::ops::{self, Hint, JsOps, Prim};
use asrs_js::string::{JsError, JsString};
use asrs_js::time;
use asrs_js::value::{JsValue, array_index};
use asrs_parity::{Response, Utf16String, Value};

use crate::js_json::{from_js, to_js};
use crate::js_time;

pub type Function = fn(&[Value]) -> Response;

/// `oracle:call` sources this module answers, keyed by the exact source text.
pub const CALLS: &[(&str, Function)] = &[
    ("(a, b) => a + b", |a| {
        binary(a, Hint::Default, value2(ops::add), value2(ops::add))
    }),
    ("(a, b) => a - b", |a| {
        binary(
            a,
            Hint::Number,
            value2(ops::subtract),
            value2(ops::subtract),
        )
    }),
    ("(a, b) => a < b", |a| {
        binary(a, Hint::Number, bool2(ops::lt), bool2(ops::lt))
    }),
    ("(a, b) => a <= b", |a| {
        binary(a, Hint::Number, bool2(ops::le), bool2(ops::le))
    }),
    ("(a, b) => a > b", |a| {
        binary(a, Hint::Number, bool2(ops::gt), bool2(ops::gt))
    }),
    ("(a, b) => a >= b", |a| {
        binary(a, Hint::Number, bool2(ops::ge), bool2(ops::ge))
    }),
    ("(a, b) => a === b", |a| {
        binary(
            a,
            Hint::Number,
            test2(ops::strict_equals),
            test2(ops::strict_equals),
        )
    }),
    ("(a, b) => a == b", |a| {
        binary(
            a,
            Hint::Default,
            test2(ops::loose_equals),
            test2(ops::loose_equals),
        )
    }),
    ("(a, b) => [a].includes(b)", |a| {
        binary(
            a,
            Hint::Number,
            test2(ops::same_value_zero),
            test2(ops::same_value_zero),
        )
    }),
    ("(a) => -a", |a| {
        unary(a, Hint::Number, value1(ops::negate), value1(ops::negate))
    }),
    ("(a) => +a", |a| {
        unary(
            a,
            Hint::Number,
            |v| Ok(Value::number(ops::to_number(v)?)),
            value1(ops::unary_plus),
        )
    }),
    ("(a) => a - 0", |a| {
        unary(
            a,
            Hint::Number,
            |v| Ok(Value::number(ops::value_to_number(v)?)),
            |v| value2(ops::subtract)(v, &Cve(Value::number(0.0))),
        )
    }),
    ("(v) => `${v}`", |a| {
        unary(
            a,
            Hint::String,
            |v| Ok(string(ops::to_string(v))),
            |v| Ok(string(v.to_primitive(Hint::String).to_js_string()?)),
        )
    }),
    ("(v) => Boolean(v)", |a| {
        unary(
            a,
            Hint::Number,
            |v| Ok(Value::Bool(ops::to_boolean(v))),
            |v| {
                Ok(Value::Bool(
                    !v.is_primitive() || v.to_primitive(Hint::Default).to_boolean(),
                ))
            },
        )
    }),
    ("(k, v) => k in v", |a| {
        on_js(a, |v| Ok(Value::Bool(ops::has_key(&v[1], &key(&v[0]))?)))
    }),
    ("(o, k) => o[k]", |a| {
        on_js(a, |v| Ok(from_js(&ops::get_key(&v[0], &key(&v[1]))?)))
    }),
    ("(o, k) => Object.hasOwn(o, k)", |a| {
        on_js(a, |v| Ok(Value::Bool(ops::has_own(&v[0], &key(&v[1]))?)))
    }),
    ("(v) => Object.keys(v)", |a| {
        on_js(a, |v| Ok(names(ops::object_keys(&v[0])?)))
    }),
    ("(v) => Object.getOwnPropertyNames(v)", |a| {
        on_js(a, |v| Ok(names(ops::own_property_names(&v[0])?)))
    }),
    ("(v) => ({ ...v })", |a| {
        on_js(a, |v| Ok(from_js(&JsValue::Object(ops::spread(&v[0])))))
    }),
    ("(o, k, v) => { 'use strict'; o[k] = v; return o; }", |a| {
        on_js(a, |v| {
            let mut target = v[0].clone();
            ops::set_key(&mut target, &key(&v[1]), v[2].clone())?;
            Ok(from_js(&target))
        })
    }),
    ("(o, k) => { 'use strict'; delete o[k]; return o; }", |a| {
        on_js(a, |v| {
            let mut target = v[0].clone();
            ops::delete_key(&mut target, &key(&v[1]))?;
            Ok(from_js(&target))
        })
    }),
    ("(k, n) => Array.from({ length: n }, (_, i) => i)[k]", |a| {
        on_js(a, |v| {
            Ok(ops::list_index(&v[0], ops::to_number(&v[1])? as usize)
                .map_or(Value::Undefined, |i| Value::number(i as f64)))
        })
    }),
    (
        "(s, n) => { const l = Array.from({ length: n }, () => 0); l.splice(s, 0, 1); return l.indexOf(1); }",
        |a| {
            on_js(a, |v| {
                let start = ops::splice_start(&v[0], ops::to_number(&v[1])? as usize);
                Ok(Value::number(start as f64))
            })
        },
    ),
    ("() => Object.getOwnPropertyNames(Object.prototype)", |_| {
        table(ops::OBJECT_PROTO)
    }),
    ("() => Object.getOwnPropertyNames(Array.prototype)", |_| {
        table(ops::ARRAY_PROTO)
    }),
    (
        "() => Object.getOwnPropertyNames(Object.getPrototypeOf(Uint8Array.prototype))",
        |_| table(ops::TYPED_ARRAY_PROTO),
    ),
    (
        "() => Object.getOwnPropertyNames(Uint8Array.prototype)",
        |_| table(ops::UINT8_ARRAY_PROTO),
    ),
    ("() => Object.getOwnPropertyNames(Date.prototype)", |_| {
        table(ops::DATE_PROTO)
    }),
];

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Undefined)
}

pub(crate) fn throw(e: JsError) -> Response {
    Response::Throw(Value::Error(e.name.into(), e.message.into()))
}

fn type_error(message: impl Into<String>) -> Response {
    throw(JsError::type_error(message))
}

type Outcome = Result<Value, JsError>;

fn respond(outcome: Outcome) -> Response {
    match outcome {
        Ok(v) => Response::Ok(v),
        Err(e) => throw(e),
    }
}

fn string(s: JsString) -> Value {
    Value::String(Utf16String::from_units(s.into_units()))
}

fn key(v: &JsValue) -> JsString {
    ops::to_string(v)
}

fn names(keys: Vec<JsString>) -> Value {
    Value::Array(keys.into_iter().map(string).collect())
}

fn table(members: &[&str]) -> Response {
    Response::Ok(Value::Array(
        members.iter().map(|m| Value::string(m)).collect(),
    ))
}

trait Operand: JsOps {
    fn value(self) -> Value;
}

impl Operand for JsValue {
    fn value(self) -> Value {
        from_js(&self)
    }
}

impl Operand for Cve {
    fn value(self) -> Value {
        self.0
    }
}

fn value1<V: Operand>(f: fn(&V) -> Result<V, JsError>) -> impl Fn(&V) -> Outcome {
    move |a| f(a).map(V::value)
}

fn value2<V: Operand>(f: fn(&V, &V) -> Result<V, JsError>) -> impl Fn(&V, &V) -> Outcome {
    move |a, b| f(a, b).map(V::value)
}

fn bool2<V: Operand>(f: fn(&V, &V) -> Result<bool, JsError>) -> impl Fn(&V, &V) -> Outcome {
    move |a, b| f(a, b).map(Value::Bool)
}

fn test2<V: Operand>(f: fn(&V, &V) -> bool) -> impl Fn(&V, &V) -> Outcome {
    move |a, b| Ok(Value::Bool(f(a, b)))
}

/// The `JsValue` an operator with `hint` can run on. A valid `Date`'s text needs the zone,
/// which `JsValue` operators do not have.
fn modeled(v: &Value, hint: Hint) -> Option<JsValue> {
    fn has_date(v: &Value) -> bool {
        match v {
            Value::Date(bits) => !f64::from_bits(*bits).is_nan(),
            Value::Array(items) => items.iter().any(has_date),
            _ => false,
        }
    }
    let exempt = hint == Hint::Number && matches!(v, Value::Date(_));
    (exempt || !has_date(v)).then(|| to_js(v).ok()).flatten()
}

fn agree(over_js: Option<Outcome>, over_cve: Outcome) -> Response {
    let cve = respond(over_cve);
    match over_js.map(respond) {
        Some(js) if js != cve => {
            Response::Error(format!("JsValue gives {js:?}, CVE-1 gives {cve:?}"))
        }
        _ => cve,
    }
}

fn unary(
    args: &[Value],
    hint: Hint,
    js: impl Fn(&JsValue) -> Outcome,
    cve: impl Fn(&Cve) -> Outcome,
) -> Response {
    let a = arg(args, 0);
    agree(modeled(&a, hint).map(|a| js(&a)), cve(&Cve(a)))
}

fn binary(
    args: &[Value],
    hint: Hint,
    js: impl Fn(&JsValue, &JsValue) -> Outcome,
    cve: impl Fn(&Cve, &Cve) -> Outcome,
) -> Response {
    let (a, b) = (arg(args, 0), arg(args, 1));
    agree(
        modeled(&a, hint)
            .zip(modeled(&b, hint))
            .map(|(a, b)| js(&a, &b)),
        cve(&Cve(a), &Cve(b)),
    )
}

fn on_js(args: &[Value], f: impl Fn(&[JsValue]) -> Outcome) -> Response {
    match args.iter().map(to_js).collect::<Result<Vec<_>, _>>() {
        Ok(values) => respond(f(&values)),
        Err(e) => Response::Error(e),
    }
}

/// A CVE-1 `Value` as an operand of the JS operators in `asrs_js::ops`; it owns the value
/// so an operator's result can come back out through `from_prim`.
pub(crate) struct Cve(pub Value);

fn to_primitive(v: &Value, hint: Hint) -> Prim<'_> {
    let string = |text: &str| Prim::String(Cow::Owned(JsString::from(text)));
    match v {
        Value::Undefined | Value::Hole => Prim::Undefined,
        Value::Null => Prim::Null,
        Value::Bool(b) => Prim::Bool(*b),
        Value::Number(bits) => Prim::Number(f64::from_bits(*bits)),
        Value::String(s) => Prim::String(Cow::Owned(JsString::from_units(s.units().to_vec()))),
        Value::BigInt(digits) => {
            Prim::BigInt(Cow::Owned(BigInt::parse(digits).unwrap_or_default()))
        }
        Value::Symbol(_) => Prim::Symbol,
        Value::Boxed(inner) => to_primitive(inner, hint),
        Value::Date(bits) => match hint {
            Hint::Number => Prim::Number(f64::from_bits(*bits)),
            _ => string(&time::date_to_string(
                f64::from_bits(*bits),
                &js_time::zone(),
            )),
        },
        Value::Array(items) | Value::TypedArray(_, items) => {
            Prim::String(Cow::Owned(JsString::from_units(array_join(items))))
        }
        Value::Bytes(bytes) => Prim::String(Cow::Owned(JsString::from_units(array_join(
            &bytes
                .iter()
                .map(|&b| Value::number(f64::from(b)))
                .collect::<Vec<_>>(),
        )))),
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

impl JsOps for Cve {
    fn to_primitive(&self, hint: Hint) -> Prim<'_> {
        to_primitive(&self.0, hint)
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

    fn as_number(&self) -> Option<f64> {
        match self.0 {
            Value::Number(bits) => Some(f64::from_bits(bits)),
            _ => None,
        }
    }

    fn from_prim(p: Prim<'_>) -> Self {
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

/// `ToString` on a CVE-1 value; a symbol throws.
pub(crate) fn to_units(value: &Value) -> Result<Vec<u16>, Response> {
    to_primitive(value, Hint::String)
        .to_js_string()
        .map(JsString::into_units)
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
    match to_primitive(v, Hint::String) {
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
/// read as `undefined`.
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
