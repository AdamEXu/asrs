use asrs_js::number::{self, IntlOptions, SignDisplay};
use asrs_parity::{Response, Utf16String, Value};

use crate::{js_json, js_math, js_string};

pub type Function = fn(&[Value]) -> Response;

/// Rust counterparts of oracle `fn` names. Each port unit registers the functions it ports
/// under the upstream name the oracle uses.
pub const FUNCTIONS: &[(&str, Function)] = &[
    ("oracle:identity", identity),
    ("oracle:call", oracle_call),
    ("Number", number_fn),
    ("String", string_fn),
    ("parseFloat", parse_float_fn),
    ("parseInt", parse_int_fn),
    ("Number.prototype.toString", number_to_string),
    ("Number.prototype.toFixed", number_to_fixed),
    ("Number.prototype.toPrecision", number_to_precision),
    ("Number.prototype.toExponential", number_to_exponential),
    ("oracle:numberFormat", number_format),
];

pub fn lookup(name: &str) -> Option<Function> {
    FUNCTIONS
        .iter()
        .chain(js_math::FUNCTIONS)
        .chain(js_string::FUNCTIONS)
        .chain(js_json::FUNCTIONS)
        .find(|(n, _)| *n == name)
        .map(|(_, f)| *f)
}

/// `oracle:call` runs a JS function source in the oracle; here each supported source text maps
/// to its port.
fn oracle_call(args: &[Value]) -> Response {
    let source = match args.first().and_then(Value::as_str) {
        Some(s) => s.to_string_lossy(),
        None => return Response::Error("oracle:call needs a source string".to_owned()),
    };
    match js_math::CALLS.iter().find(|(s, _)| *s == source) {
        Some((_, f)) => f(&args[1..]),
        None => Response::Error(format!("oracle:call: no port of {source:?}")),
    }
}

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Undefined)
}

fn identity(args: &[Value]) -> Response {
    Response::Ok(arg(args, 0))
}

fn ok_number(x: f64) -> Response {
    Response::Ok(Value::number(x))
}

fn ok_string(s: String) -> Response {
    Response::Ok(Value::String(Utf16String::from(s)))
}

fn type_error(message: impl Into<String>) -> Response {
    Response::Throw(Value::Error("TypeError".into(), message.into().into()))
}

fn range_error(message: &str) -> Response {
    Response::Throw(Value::Error("RangeError".into(), message.into()))
}

/// `ToNumber` on a CVE-1 value. Symbols throw; objects reduce to NaN except
/// arrays, which convert through their ToString join.
fn to_number(value: &Value) -> Result<f64, Response> {
    Ok(match value {
        Value::Undefined => f64::NAN,
        Value::Null => 0.0,
        Value::Bool(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        Value::Number(bits) => f64::from_bits(*bits),
        Value::String(s) => number::to_number(s.units()),
        Value::BigInt(digits) => number::to_number(&digits.encode_utf16().collect::<Vec<u16>>()),
        Value::Symbol(_) => {
            return Err(type_error("Cannot convert a Symbol value to a number"));
        }
        Value::Array(_) => number::to_number(&to_units(value)?),
        _ => f64::NAN,
    })
}

fn to_integer(value: &Value) -> Result<f64, Response> {
    Ok(number::to_integer_or_infinity(to_number(value)?))
}

/// `ToString` on a CVE-1 value, with the JS special case that `String(symbol)`
/// is allowed while other ToString uses throw.
fn to_units(value: &Value) -> Result<Vec<u16>, Response> {
    Ok(match value {
        Value::Undefined => "undefined".encode_utf16().collect(),
        Value::Null => "null".encode_utf16().collect(),
        Value::Bool(b) => if *b { "true" } else { "false" }.encode_utf16().collect(),
        Value::Number(bits) => number::to_string(f64::from_bits(*bits))
            .encode_utf16()
            .collect(),
        Value::String(s) => s.units().to_vec(),
        Value::BigInt(digits) => digits.encode_utf16().collect(),
        Value::Symbol(_) => {
            return Err(type_error("Cannot convert a Symbol value to a string"));
        }
        Value::Array(items) => array_join(items),
        _ => "[object Object]".encode_utf16().collect(),
    })
}

fn array_join(items: &[Value]) -> Vec<u16> {
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

/// `thisNumberValue`: the receiver of `Number.prototype` methods.
fn this_number(args: &[Value], method: &str) -> Result<f64, Response> {
    match &arg(args, 0) {
        Value::Number(bits) => Ok(f64::from_bits(*bits)),
        Value::Boxed(inner) => match inner.as_ref() {
            Value::Number(bits) => Ok(f64::from_bits(*bits)),
            _ => Err(type_error(format!(
                "Number.prototype.{method} requires that 'this' be a Number"
            ))),
        },
        _ => Err(type_error(format!(
            "Number.prototype.{method} requires that 'this' be a Number"
        ))),
    }
}

fn number_fn(args: &[Value]) -> Response {
    match to_number(&arg(args, 0)) {
        Ok(x) => ok_number(x),
        Err(r) => r,
    }
}

fn string_fn(args: &[Value]) -> Response {
    match &arg(args, 0) {
        Value::Symbol(description) => {
            let mut out = Utf16String::from("Symbol(");
            if let Some(d) = description {
                out.push_str(&d.to_string_lossy());
            }
            out.push_str(")");
            Response::Ok(Value::String(out))
        }
        value => match to_units(value) {
            Ok(units) => Response::Ok(Value::String(Utf16String::from_units(units))),
            Err(r) => r,
        },
    }
}

fn parse_float_fn(args: &[Value]) -> Response {
    match to_units(&arg(args, 0)) {
        Ok(units) => ok_number(number::parse_float(&units)),
        Err(r) => r,
    }
}

fn parse_int_fn(args: &[Value]) -> Response {
    let units = match to_units(&arg(args, 0)) {
        Ok(units) => units,
        Err(r) => return r,
    };
    let radix = match to_number(&arg(args, 1)) {
        Ok(x) => x,
        Err(r) => return r,
    };
    ok_number(number::parse_int(&units, radix))
}

fn number_to_string(args: &[Value]) -> Response {
    let x = match this_number(args, "toString") {
        Ok(x) => x,
        Err(r) => return r,
    };
    let radix = match &arg(args, 1) {
        Value::Undefined => 10.0,
        value => match to_integer(value) {
            Ok(x) => x,
            Err(r) => return r,
        },
    };
    match number::to_string_radix(x, radix) {
        Ok(s) => ok_string(s),
        Err(m) => range_error(m),
    }
}

fn number_to_fixed(args: &[Value]) -> Response {
    let x = match this_number(args, "toFixed") {
        Ok(x) => x,
        Err(r) => return r,
    };
    let f = match to_integer(&arg(args, 1)) {
        Ok(x) => x,
        Err(r) => return r,
    };
    match number::to_fixed(x, f) {
        Ok(s) => ok_string(s),
        Err(m) => range_error(m),
    }
}

fn number_to_precision(args: &[Value]) -> Response {
    let x = match this_number(args, "toPrecision") {
        Ok(x) => x,
        Err(r) => return r,
    };
    let p = match &arg(args, 1) {
        Value::Undefined => None,
        value => match to_integer(value) {
            Ok(x) => Some(x),
            Err(r) => return r,
        },
    };
    match number::to_precision(x, p) {
        Ok(s) => ok_string(s),
        Err(m) => range_error(m),
    }
}

fn number_to_exponential(args: &[Value]) -> Response {
    let x = match this_number(args, "toExponential") {
        Ok(x) => x,
        Err(r) => return r,
    };
    let f = match &arg(args, 1) {
        Value::Undefined => None,
        value => match to_integer(value) {
            Ok(x) => Some(x),
            Err(r) => return r,
        },
    };
    match number::to_exponential(x, f) {
        Ok(s) => ok_string(s),
        Err(m) => range_error(m),
    }
}

fn number_format(args: &[Value]) -> Response {
    let options = &arg(args, 1);
    if matches!(options, Value::Null) {
        return type_error("Cannot convert undefined or null to object");
    }
    let numeric = |key: &str, default: f64| -> Result<f64, Response> {
        match options.get(key) {
            None => Ok(default),
            Some(v) => to_number(v),
        }
    };
    let (min_integer, min_fraction, max_fraction) = match (
        numeric("minimumIntegerDigits", 1.0),
        numeric("minimumFractionDigits", 0.0),
        numeric("maximumFractionDigits", 3.0),
    ) {
        (Ok(a), Ok(b), Ok(c)) => (a, b, c),
        (Err(r), _, _) | (_, Err(r), _) | (_, _, Err(r)) => return r,
    };
    let grouping = match options.get("useGrouping") {
        None => true,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(bits)) => {
            f64::from_bits(*bits) != 0.0 && !f64::from_bits(*bits).is_nan()
        }
        Some(Value::Null) | Some(Value::Undefined) => false,
        Some(_) => true,
    };
    let sign_display = match options.get("signDisplay") {
        Some(Value::String(s)) if s.eq_str("never") => SignDisplay::Never,
        Some(Value::String(s)) if s.eq_str("always") => SignDisplay::Always,
        Some(Value::String(s)) if s.eq_str("exceptZero") => SignDisplay::ExceptZero,
        _ => SignDisplay::Auto,
    };
    let value = match to_number(&arg(args, 2)) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let opts = IntlOptions {
        min_integer: min_integer.max(0.0) as u32,
        min_fraction: min_fraction.max(0.0) as u32,
        max_fraction: max_fraction.max(0.0) as u32,
        grouping,
        sign_display,
    };
    ok_string(number::intl_format(value, opts))
}
