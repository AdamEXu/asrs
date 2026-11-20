//! `fn` names for `asrs_js::string`. The string vector tests include this file too, so the
//! committed vectors and `cargo xtask parity fn` exercise the same dispatch.

use asrs_js::number;
use asrs_js::string::{self, JsError, JsString, TextDecoder};
use asrs_parity::{Response, Utf16String, Value};

pub type Function = fn(&[Value]) -> Response;

pub const FUNCTIONS: &[(&str, Function)] = &[
    ("String.prototype.charAt", |a| method(a, "charAt", char_at)),
    ("String.prototype.charCodeAt", |a| {
        method(a, "charCodeAt", char_code_at)
    }),
    ("String.prototype.at", |a| method(a, "at", at)),
    ("String.prototype.codePointAt", |a| {
        method(a, "codePointAt", code_point_at)
    }),
    ("String.prototype.slice", |a| method(a, "slice", slice)),
    ("String.prototype.substring", |a| {
        method(a, "substring", substring)
    }),
    ("String.prototype.indexOf", |a| {
        method(a, "indexOf", index_of)
    }),
    ("String.prototype.lastIndexOf", |a| {
        method(a, "lastIndexOf", last_index_of)
    }),
    ("String.prototype.includes", |a| {
        method(a, "includes", includes)
    }),
    ("String.prototype.startsWith", |a| {
        method(a, "startsWith", starts_with)
    }),
    ("String.prototype.endsWith", |a| {
        method(a, "endsWith", ends_with)
    }),
    ("String.prototype.split", |a| method(a, "split", split)),
    ("String.prototype.padStart", |a| {
        method(a, "padStart", pad_start)
    }),
    ("String.prototype.padEnd", |a| method(a, "padEnd", pad_end)),
    ("String.prototype.repeat", |a| method(a, "repeat", repeat)),
    ("String.prototype.trim", |a| {
        method(a, "trim", |s, _| ok(s.trim()))
    }),
    ("String.prototype.trimStart", |a| {
        method(a, "trimStart", |s, _| ok(s.trim_start()))
    }),
    ("String.prototype.trimEnd", |a| {
        method(a, "trimEnd", |s, _| ok(s.trim_end()))
    }),
    ("String.prototype.toLowerCase", |a| {
        method(a, "toLowerCase", |s, _| ok(s.to_lower_case()))
    }),
    ("String.prototype.toUpperCase", |a| {
        method(a, "toUpperCase", |s, _| ok(s.to_upper_case()))
    }),
    ("String.fromCharCode", from_char_code),
    ("String.fromCodePoint", from_code_point),
    ("encodeURIComponent", encode_uri_component),
    ("decodeURIComponent", decode_uri_component),
    ("oracle:textDecode", text_decode),
    ("oracle:textEncode", text_encode),
    ("shared/util:htmlEncode", html_encode),
];

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Undefined)
}

fn ok(s: JsString) -> Response {
    Response::Ok(Value::String(Utf16String::from_units(s.into_units())))
}

fn ok_number(x: f64) -> Response {
    Response::Ok(Value::number(x))
}

fn throw(e: JsError) -> Response {
    Response::Throw(Value::Error(
        Utf16String::from(e.name),
        Utf16String::from(e.message),
    ))
}

fn type_error(message: impl Into<String>) -> Response {
    throw(JsError::type_error(message))
}

/// `ToString` on a CVE-1 value, minus the null/undefined receiver check.
fn to_string(value: &Value) -> Result<JsString, Response> {
    let units: Vec<u16> = match value {
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
        Value::Array(items) => {
            let mut out = Vec::new();
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(u16::from(b','));
                }
                match item {
                    Value::Undefined | Value::Null | Value::Hole => {}
                    other => out.extend_from_slice(&to_string(other)?.into_units()),
                }
            }
            out
        }
        _ => "[object Object]".encode_utf16().collect(),
    };
    Ok(JsString::from_units(units))
}

/// `ToNumber` on a CVE-1 value.
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
        Value::BigInt(_) => {
            return Err(type_error("Cannot convert a BigInt value to a number"));
        }
        Value::Symbol(_) => {
            return Err(type_error("Cannot convert a Symbol value to a number"));
        }
        Value::Array(items) => {
            let joined = to_string(&Value::Array(items.clone()))?;
            number::to_number(joined.units())
        }
        _ => f64::NAN,
    })
}

fn to_boolean(value: &Value) -> bool {
    match value {
        Value::Undefined | Value::Null | Value::Hole => false,
        Value::Bool(b) => *b,
        Value::Number(bits) => {
            let x = f64::from_bits(*bits);
            x != 0.0 && !x.is_nan()
        }
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// The receiver of a `String.prototype` method: RequireObjectCoercible then ToString.
fn receiver(args: &[Value], method: &str) -> Result<JsString, Response> {
    match &arg(args, 0) {
        Value::Undefined | Value::Null => Err(type_error(format!(
            "String.prototype.{method} called on null or undefined"
        ))),
        value => to_string(value),
    }
}

fn str_arg(args: &[Value], i: usize) -> Result<JsString, Response> {
    to_string(&arg(args, i))
}

fn num_arg(args: &[Value], i: usize) -> Result<f64, Response> {
    to_number(&arg(args, i))
}

fn opt_num_arg(args: &[Value], i: usize) -> Result<Option<f64>, Response> {
    match &arg(args, i) {
        Value::Undefined => Ok(None),
        v => to_number(v).map(Some),
    }
}

fn opt_str_arg(args: &[Value], i: usize) -> Result<Option<JsString>, Response> {
    match &arg(args, i) {
        Value::Undefined => Ok(None),
        v => to_string(v).map(Some),
    }
}

fn method(args: &[Value], name: &str, f: impl Fn(&JsString, &[Value]) -> Response) -> Response {
    match receiver(args, name) {
        Ok(s) => f(&s, &args[1.min(args.len())..]),
        Err(r) => r,
    }
}

fn char_at(s: &JsString, args: &[Value]) -> Response {
    match num_arg(args, 0) {
        Ok(pos) => ok(s.char_at(pos)),
        Err(r) => r,
    }
}

fn char_code_at(s: &JsString, args: &[Value]) -> Response {
    match num_arg(args, 0) {
        Ok(pos) => ok_number(s.char_code_at(pos).map_or(f64::NAN, f64::from)),
        Err(r) => r,
    }
}

fn at(s: &JsString, args: &[Value]) -> Response {
    match num_arg(args, 0) {
        Ok(pos) => match s.at(pos) {
            Some(v) => ok(v),
            None => Response::Ok(Value::Undefined),
        },
        Err(r) => r,
    }
}

fn code_point_at(s: &JsString, args: &[Value]) -> Response {
    match num_arg(args, 0) {
        Ok(pos) => match s.code_point_at(pos) {
            Some(cp) => ok_number(f64::from(cp)),
            None => Response::Ok(Value::Undefined),
        },
        Err(r) => r,
    }
}

fn slice(s: &JsString, args: &[Value]) -> Response {
    match (num_arg(args, 0), opt_num_arg(args, 1)) {
        (Ok(start), Ok(end)) => ok(s.slice(start, end)),
        (Err(r), _) | (_, Err(r)) => r,
    }
}

fn substring(s: &JsString, args: &[Value]) -> Response {
    match (num_arg(args, 0), opt_num_arg(args, 1)) {
        (Ok(start), Ok(end)) => ok(s.substring(start, end)),
        (Err(r), _) | (_, Err(r)) => r,
    }
}

fn index_of(s: &JsString, args: &[Value]) -> Response {
    match (str_arg(args, 0), opt_num_arg(args, 1)) {
        (Ok(search), Ok(pos)) => ok_number(s.index_of(&search, pos) as f64),
        (Err(r), _) | (_, Err(r)) => r,
    }
}

fn last_index_of(s: &JsString, args: &[Value]) -> Response {
    match (str_arg(args, 0), opt_num_arg(args, 1)) {
        (Ok(search), Ok(pos)) => ok_number(s.last_index_of(&search, pos) as f64),
        (Err(r), _) | (_, Err(r)) => r,
    }
}

fn includes(s: &JsString, args: &[Value]) -> Response {
    match (str_arg(args, 0), opt_num_arg(args, 1)) {
        (Ok(search), Ok(pos)) => Response::Ok(Value::Bool(s.includes(&search, pos))),
        (Err(r), _) | (_, Err(r)) => r,
    }
}

fn starts_with(s: &JsString, args: &[Value]) -> Response {
    match (str_arg(args, 0), opt_num_arg(args, 1)) {
        (Ok(search), Ok(pos)) => Response::Ok(Value::Bool(s.starts_with(&search, pos))),
        (Err(r), _) | (_, Err(r)) => r,
    }
}

fn ends_with(s: &JsString, args: &[Value]) -> Response {
    match (str_arg(args, 0), opt_num_arg(args, 1)) {
        (Ok(search), Ok(pos)) => Response::Ok(Value::Bool(s.ends_with(&search, pos))),
        (Err(r), _) | (_, Err(r)) => r,
    }
}

fn split(s: &JsString, args: &[Value]) -> Response {
    match (opt_str_arg(args, 0), opt_num_arg(args, 1)) {
        (Ok(sep), Ok(limit)) => Response::Ok(Value::Array(
            s.split(sep.as_ref(), limit)
                .into_iter()
                .map(|part| Value::String(Utf16String::from_units(part.into_units())))
                .collect(),
        )),
        (Err(r), _) | (_, Err(r)) => r,
    }
}

fn pad_start(s: &JsString, args: &[Value]) -> Response {
    match (num_arg(args, 0), opt_str_arg(args, 1)) {
        (Ok(len), Ok(fill)) => match s.pad_start(len, fill.as_ref()) {
            Ok(v) => ok(v),
            Err(e) => throw(e),
        },
        (Err(r), _) | (_, Err(r)) => r,
    }
}

fn pad_end(s: &JsString, args: &[Value]) -> Response {
    match (num_arg(args, 0), opt_str_arg(args, 1)) {
        (Ok(len), Ok(fill)) => match s.pad_end(len, fill.as_ref()) {
            Ok(v) => ok(v),
            Err(e) => throw(e),
        },
        (Err(r), _) | (_, Err(r)) => r,
    }
}

fn repeat(s: &JsString, args: &[Value]) -> Response {
    match num_arg(args, 0) {
        Ok(count) => match s.repeat(count) {
            Ok(v) => ok(v),
            Err(e) => throw(e),
        },
        Err(r) => r,
    }
}

fn from_char_code(args: &[Value]) -> Response {
    let mut units = Vec::with_capacity(args.len());
    for value in args {
        match to_number(value) {
            Ok(x) => units.push(string::to_uint16(x)),
            Err(r) => return r,
        }
    }
    ok(JsString::from_units(units))
}

fn from_code_point(args: &[Value]) -> Response {
    let mut cps = Vec::with_capacity(args.len());
    for value in args {
        match to_number(value) {
            Ok(x) => cps.push(x),
            Err(r) => return r,
        }
    }
    match JsString::from_code_point(&cps) {
        Ok(v) => ok(v),
        Err(e) => throw(e),
    }
}

fn encode_uri_component(args: &[Value]) -> Response {
    match str_arg(args, 0) {
        Ok(s) => match string::encode_uri_component(&s) {
            Ok(v) => ok(v),
            Err(e) => throw(e.into_error()),
        },
        Err(r) => r,
    }
}

fn decode_uri_component(args: &[Value]) -> Response {
    match str_arg(args, 0) {
        Ok(s) => match string::decode_uri_component(&s) {
            Ok(v) => ok(v),
            Err(e) => throw(e.into_error()),
        },
        Err(r) => r,
    }
}

fn text_decode(args: &[Value]) -> Response {
    let bytes = match &arg(args, 0) {
        Value::Undefined => Vec::new(),
        Value::Bytes(b) => b.clone(),
        _ => {
            return type_error(
                "The provided value is not of type '(ArrayBuffer or ArrayBufferView)'",
            );
        }
    };
    let label = match opt_str_arg(args, 1) {
        Ok(l) => l,
        Err(r) => return r,
    };
    let options = arg(args, 2);
    let fatal = options.get("fatal").is_some_and(to_boolean);
    let ignore_bom = options.get("ignoreBOM").is_some_and(to_boolean);
    let decoder = match TextDecoder::new(label.as_ref(), fatal, ignore_bom) {
        Ok(d) => d,
        Err(e) => return throw(e.into_error()),
    };
    match decoder.decode(&bytes) {
        Ok(s) => ok(s),
        Err(e) => throw(e.into_error()),
    }
}

fn text_encode(args: &[Value]) -> Response {
    match str_arg(args, 0) {
        Ok(s) => Response::Ok(Value::Bytes(string::text_encode(&s))),
        Err(r) => r,
    }
}

fn html_encode(args: &[Value]) -> Response {
    match str_arg(args, 0) {
        Ok(s) => ok(string::html_encode(&s)),
        Err(r) => r,
    }
}
