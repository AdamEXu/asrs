//! `fn` names for `asrs_js::time` and `asrs_js::color`. Zone, clock and random
//! streams are ambient state so the oracle's pinned environment and the
//! scripted-vector tests can inject theirs.

use std::cell::RefCell;

use asrs_js::number;
use asrs_js::time::{self, Clock, NamedZone, Rng, SetRng, SystemClock, V8MathRandom};
use asrs_parity::{Response, Value};

use crate::functions::{arg, ok_number, ok_string, to_number, type_error};
use crate::js_color;

pub type Function = fn(&[Value]) -> Response;

thread_local! {
    static ZONE: RefCell<Option<NamedZone>> = const { RefCell::new(None) };
    static RNG: RefCell<Box<dyn Rng>> = RefCell::new(Box::new(V8MathRandom::from_entropy()));
    static CLOCK: RefCell<Box<dyn Clock>> = RefCell::new(Box::new(SystemClock));
}

/// Runs `f` with `zone` as the ambient zone; the default is the system zone,
/// matching the oracle's `TZ`. Used by the vector tests and future scripted
/// zones rather than the binary itself.
#[allow(dead_code)]
pub fn with_zone<T>(zone: NamedZone, f: impl FnOnce() -> T) -> T {
    ZONE.with(|z| {
        let previous = z.replace(Some(zone));
        let out = f();
        z.replace(previous);
        out
    })
}

/// Runs `f` with `rng` as the ambient `Math.random` stream.
pub fn with_rng<T>(rng: Box<dyn Rng>, f: impl FnOnce() -> T) -> T {
    RNG.with(|r| {
        let previous = std::mem::replace(&mut *r.borrow_mut(), rng);
        let out = f();
        *r.borrow_mut() = previous;
        out
    })
}

/// Runs `f` on the ambient `Math.random` stream.
pub fn with_ambient_rng<T>(f: impl FnOnce(&mut dyn Rng) -> T) -> T {
    RNG.with(|r| f(&mut **r.borrow_mut()))
}

/// Runs `f` with `clock` as the ambient `Date.now()`.
#[allow(dead_code)]
pub fn with_clock<T>(clock: Box<dyn Clock>, f: impl FnOnce() -> T) -> T {
    CLOCK.with(|c| {
        let previous = std::mem::replace(&mut *c.borrow_mut(), clock);
        let out = f();
        *c.borrow_mut() = previous;
        out
    })
}

fn zone() -> NamedZone {
    ZONE.with(|z| z.borrow().clone()).unwrap_or_else(|| {
        std::env::var("ASRS_ORACLE_ZONE")
            .ok()
            .filter(|n| !n.is_empty())
            .or_else(|| std::env::var("TZ").ok().filter(|n| !n.is_empty()))
            .and_then(|name| NamedZone::get(&name).ok())
            .unwrap_or_else(NamedZone::system)
    })
}

fn now() -> f64 {
    CLOCK.with(|c| c.borrow().now())
}

/// The port behind an oracle `fn` name, covering this module's table plus the
/// color table; `oracle:call` sources go through `CALLS`. Returns `None` for
/// names this unit does not port.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Response> {
    if name == "oracle:call" {
        let source = args
            .first()
            .and_then(Value::as_str)
            .map(|s| s.to_string_lossy())?;
        return CALLS
            .iter()
            .find(|(s, _)| *s == source)
            .map(|(_, f)| f(&args[1..]));
    }
    FUNCTIONS
        .iter()
        .chain(js_color::FUNCTIONS)
        .find(|(n, _)| *n == name)
        .map(|(_, f)| f(args))
}

pub const FUNCTIONS: &[(&str, Function)] = &[
    ("shared/util:formatDate", format_date_fn),
    ("shared/util:formatTimeWithMS", format_time_fn),
    ("shared/util:createUUID", create_uuid_fn),
    ("Date.prototype.toLocaleTimeString", |a| {
        date_method(a, "toLocaleTimeString", time::to_locale_time_string)
    }),
    ("Date.prototype.toString", |a| {
        date_method(a, "toString", time::date_to_string)
    }),
    ("Date.prototype.getTimezoneOffset", |a| {
        match date_arg(a, 0) {
            Ok(t) => ok_number(time::get_timezone_offset(t, &zone())),
            Err(r) => r,
        }
    }),
    ("new Date", new_date_fn),
    ("oracle:withRandom", with_random_fn),
];

/// `oracle:call` sources this module answers, keyed by the exact source text.
pub const CALLS: &[(&str, Function)] = &[
    (
        "() => Intl.DateTimeFormat().resolvedOptions().timeZone",
        |_| ok_string(zone().resolved_name()),
    ),
    ("() => new Date(0).toString()", |_| {
        ok_string(time::date_to_string(0.0, &zone()))
    }),
];

/// `ToNumber` for `new Date` arguments and similar: `BigInt` throws here,
/// unlike `Number(bigint)`.
fn to_number_arg(value: &Value) -> Result<f64, Response> {
    match value {
        Value::BigInt(_) => Err(type_error("Cannot convert a BigInt value to a number")),
        Value::Date(bits) => Ok(f64::from_bits(*bits)),
        _ => to_number(value),
    }
}

/// `arg(i)` unwrapped as a `Date` receiver: a `Date` (or boxed `Date`) gives
/// its milliseconds; anything else throws the getter's `TypeError`.
fn date_arg(args: &[Value], i: usize) -> Result<f64, Response> {
    match arg(args, i) {
        Value::Date(bits) => Ok(f64::from_bits(bits)),
        Value::Boxed(inner) => match *inner {
            Value::Date(bits) => Ok(f64::from_bits(bits)),
            _ => Err(type_error("this is not a Date object.")),
        },
        _ => Err(type_error("this is not a Date object.")),
    }
}

/// The `incompatible receiver <v>` text V8 builds for a `Date.prototype`
/// method on a non-Date receiver.
fn receiver_text(value: &Value) -> String {
    match value {
        Value::Undefined => "undefined".to_owned(),
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(bits) => number::to_string(f64::from_bits(*bits)),
        Value::BigInt(digits) => digits.clone(),
        Value::String(s) => s.to_string_lossy(),
        Value::Symbol(d) => match d {
            Some(d) => format!("Symbol({})", d.to_string_lossy()),
            None => "Symbol()".to_owned(),
        },
        Value::Array(_) => "[object Array]".to_owned(),
        Value::Bytes(_) => "[object Uint8Array]".to_owned(),
        Value::TypedArray(name, _) => format!("[object {name}]"),
        Value::ArrayBuffer(_) => "#<ArrayBuffer>".to_owned(),
        Value::DataView(_) => "#<DataView>".to_owned(),
        Value::Date(_) => "#<Date>".to_owned(),
        Value::Map(_) => "#<Map>".to_owned(),
        Value::Set(_) => "#<Set>".to_owned(),
        Value::Object(_) => "#<Object>".to_owned(),
        Value::Instance(name, _) => format!("#<{}>", name.to_string_lossy()),
        Value::Error(name, message) => {
            let name = name.to_string_lossy();
            let message = message.to_string_lossy();
            if message.is_empty() {
                name
            } else {
                format!("{name}: {message}")
            }
        }
        Value::Boxed(inner) => {
            let class = match inner.as_ref() {
                Value::Number(_) => "Number",
                Value::String(_) => "String",
                Value::Bool(_) => "Boolean",
                Value::BigInt(_) => "BigInt",
                Value::Symbol(_) => "Symbol",
                _ => "Object",
            };
            format!("[object {class}]")
        }
        Value::Function(name) => format!("#<{}>", name.to_string_lossy()),
        Value::Hole => String::new(),
    }
}

/// A `Date.prototype` method returning a string: the receiver is `arg(0)`,
/// errors use the `incompatible receiver` form (`toString`,
/// `toLocaleTimeString` and friends; the getters use the other one).
fn date_method(args: &[Value], name: &str, f: impl Fn(f64, &NamedZone) -> String) -> Response {
    match arg(args, 0) {
        Value::Date(bits) => ok_string(f(f64::from_bits(bits), &zone())),
        Value::Boxed(inner) => match *inner {
            Value::Date(bits) => ok_string(f(f64::from_bits(bits), &zone())),
            other => incompatible(name, &other),
        },
        other => incompatible(name, &other),
    }
}

fn incompatible(name: &str, receiver: &Value) -> Response {
    type_error(format!(
        "Method Date.prototype.{name} called on incompatible receiver {}",
        receiver_text(receiver)
    ))
}

fn format_date_fn(args: &[Value]) -> Response {
    let epoch = match arg(args, 0) {
        Value::Date(bits) => f64::from_bits(bits),
        Value::Boxed(inner) => match *inner {
            Value::Date(bits) => f64::from_bits(bits),
            _ => {
                return type_error("date.getFullYear is not a function");
            }
        },
        Value::Undefined => now(),
        Value::Null => {
            return type_error("Cannot read properties of null (reading 'getFullYear')");
        }
        _ => {
            return type_error("date.getFullYear is not a function");
        }
    };
    ok_string(time::format_date(epoch, &zone()))
}

fn format_time_fn(args: &[Value]) -> Response {
    match to_number_arg(&arg(args, 0)) {
        Ok(t) => ok_string(time::format_time_with_ms(t)),
        Err(r) => r,
    }
}

fn create_uuid_fn(_args: &[Value]) -> Response {
    with_ambient_rng(|rng| ok_string(time::create_uuid(rng)))
}

fn ok_date(t: f64) -> Response {
    Response::Ok(Value::Date(time::time_clip(t).to_bits()))
}

fn new_date_fn(args: &[Value]) -> Response {
    if args.is_empty() {
        return ok_date(now());
    }
    if args.len() == 1 {
        return match &args[0] {
            Value::BigInt(_) => type_error("Cannot convert a BigInt value to a number"),
            Value::Symbol(_) => type_error("Cannot convert a Symbol value to a number"),
            Value::Date(bits) => ok_date(f64::from_bits(*bits)),
            Value::Boxed(inner) => new_date_fn(&[inner.as_ref().clone()]),
            Value::String(_) | Value::Array(_) | Value::Bytes(_) | Value::TypedArray(..) => {
                Response::Error("asrs_js::time: Date.parse is not ported".to_owned())
            }
            value => match to_number(value) {
                Ok(t) => ok_date(t),
                Err(r) => r,
            },
        };
    }
    let mut fields = Vec::with_capacity(args.len().min(7));
    for value in args.iter().take(7) {
        match to_number_arg(value) {
            Ok(x) => fields.push(x),
            Err(r) => return r,
        }
    }
    ok_date(time::local_date(&fields, &zone()))
}

fn with_random_fn(args: &[Value]) -> Response {
    let values = match arg(args, 0) {
        Value::Array(items) => items
            .iter()
            .map(|v| to_number(v).unwrap_or(f64::NAN))
            .collect::<Vec<f64>>(),
        _ => Vec::new(),
    };
    let name = match arg(args, 1).as_str() {
        Some(s) => s.to_string_lossy(),
        None => return Response::Error("oracle:withRandom needs a fn name".to_owned()),
    };
    let fn_args = match arg(args, 2) {
        Value::Array(items) => items,
        _ => Vec::new(),
    };
    with_rng(Box::new(SetRng::new(values)), || {
        dispatch(&name, &fn_args)
            .unwrap_or_else(|| Response::Error(format!("oracle:withRandom: no port of {name:?}")))
    })
}
