//! `fn` names for `asrs_js::color` (the `color-convert` 2.0.1 subset) and the
//! `shared/Colors.ts` composition that uses it. The `wrapRaw`/`wrapRounded`
//! argument and result handling from `index.js` is reproduced here.

use asrs_js::color;
use asrs_js::math;
use asrs_js::number;
use asrs_js::string::JsString;
use asrs_parity::{Response, Utf16String, Value};

use crate::functions::{arg, array_join, ok_string, to_number, type_error};

pub type Function = fn(&[Value]) -> Response;

pub const FUNCTIONS: &[(&str, Function)] = &[
    ("npm:color-convert:hex.rgb", |a| hex_to(a, |t| {
        rgb_response(color::hex_rgb(t))
    })),
    ("npm:color-convert:hex.rgb.raw", |a| hex_to(a, |t| {
        rgb_response(color::hex_rgb(t))
    })),
    ("npm:color-convert:hex.hsl", |a| hex_to(a, |t| {
        arr_response(color::hex_hsl(t))
    })),
    ("npm:color-convert:hex.hsl.raw", |a| hex_to(a, |t| {
        arr_response(color::hex_hsl_raw(t))
    })),
    ("npm:color-convert:rgb.hsl", |a| channels_to(a, |c| {
        arr_response(color::rgb_hsl(c).map(math::round))
    })),
    ("npm:color-convert:rgb.hsl.raw", |a| channels_to(a, |c| {
        arr_response(color::rgb_hsl(c))
    })),
    ("npm:color-convert:rgb.hex", |a| channels_to(a, |c| {
        ok_string(color::rgb_hex(c))
    })),
    ("npm:color-convert:rgb.hex.raw", |a| channels_to(a, |c| {
        ok_string(color::rgb_hex(c))
    })),
    ("npm:color-convert:hsl.rgb", |a| channels_to(a, |c| {
        arr_response(color::hsl_rgb(c).map(math::round))
    })),
    ("npm:color-convert:hsl.rgb.raw", |a| channels_to(a, |c| {
        arr_response(color::hsl_rgb(c))
    })),
    ("npm:color-convert:hsl.hex", |a| channels_to(a, |c| {
        ok_string(color::hsl_hex(c))
    })),
    ("npm:color-convert:hsl.hex.raw", |a| channels_to(a, |c| {
        ok_string(color::hsl_hex(c))
    })),
    ("shared/Colors:ensureThemeContrast", ensure_theme_contrast_fn),
];

/// `wrapRaw`'s effective `args`: `arg0` itself when its `length` exceeds 1
/// (string, array or typed array), otherwise the whole argument list.
/// `undefined`/`null` arg0 returns `arg0` before any of that.
enum Wrapped {
    Nullish(Value),
    Text(Vec<u16>),
    Items(Vec<Value>),
}

fn wrapped(args: &[Value]) -> Wrapped {
    let arg0 = args.first().cloned().unwrap_or(Value::Undefined);
    match arg0 {
        Value::Undefined | Value::Null => Wrapped::Nullish(arg0),
        Value::String(s) if s.units().len() > 1 => Wrapped::Text(s.units().to_vec()),
        Value::Array(items) if items.len() > 1 => Wrapped::Items(items),
        Value::Bytes(bytes) if bytes.len() > 1 => Wrapped::Items(
            bytes
                .iter()
                .map(|&b| Value::number(f64::from(b)))
                .collect(),
        ),
        Value::TypedArray(_, items) if items.len() > 1 => Wrapped::Items(items),
        _ => Wrapped::Items(args.to_vec()),
    }
}

/// `args.toString(16)` under the wrap: a string stays itself, anything else is
/// an array join (which ignores the radix).
fn hex_text(args: &[Value]) -> Result<Option<String>, Response> {
    Ok(match wrapped(args) {
        Wrapped::Nullish(_) => None,
        Wrapped::Text(units) => Some(JsString::from_units(units).to_string_lossy()),
        Wrapped::Items(items) => Some(
            JsString::from_units(array_join(&items)).to_string_lossy(),
        ),
    })
}

/// `args[i]` for `i` in 0..3, each value as the arithmetic operand the
/// conversions use it as (`undefined` reads as `NaN`, `BigInt` cannot mix).
fn channels(args: &[Value]) -> Result<Option<[f64; 3]>, Response> {
    let items: Vec<f64> = match wrapped(args) {
        Wrapped::Nullish(_) => return Ok(None),
        Wrapped::Text(units) => units.iter().map(|&u| number::to_number(&[u])).collect(),
        Wrapped::Items(items) => items
            .iter()
            .map(|v| match v {
                Value::BigInt(_) => Err(type_error(
                    "Cannot mix BigInt and other values, use explicit conversions",
                )),
                Value::Symbol(_) => {
                    Err(type_error("Cannot convert a Symbol value to a number"))
                }
                Value::Undefined | Value::Hole => Ok(f64::NAN),
                Value::Date(bits) => Ok(f64::from_bits(*bits)),
                other => to_number(other),
            })
            .collect::<Result<_, Response>>()?,
    };
    Ok(Some([
        items.first().copied().unwrap_or(f64::NAN),
        items.get(1).copied().unwrap_or(f64::NAN),
        items.get(2).copied().unwrap_or(f64::NAN),
    ]))
}

fn arr_response(v: [f64; 3]) -> Response {
    Response::Ok(Value::Array(v.map(Value::number).to_vec()))
}

fn rgb_response(v: [u8; 3]) -> Response {
    arr_response(v.map(f64::from))
}

fn hex_to(args: &[Value], f: impl Fn(&str) -> Response) -> Response {
    match hex_text(args) {
        Ok(Some(text)) => f(&text),
        Ok(None) => Response::Ok(args.first().cloned().unwrap_or(Value::Undefined)),
        Err(r) => r,
    }
}

fn channels_to(args: &[Value], f: impl Fn([f64; 3]) -> Response) -> Response {
    match channels(args) {
        Ok(Some(c)) => f(c),
        Ok(None) => Response::Ok(args.first().cloned().unwrap_or(Value::Undefined)),
        Err(r) => r,
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Undefined | Value::Null | Value::Hole => false,
        Value::Bool(b) => *b,
        Value::Number(bits) => {
            let x = f64::from_bits(*bits);
            x != 0.0 && !x.is_nan()
        }
        Value::String(s) => !s.units().is_empty(),
        Value::BigInt(digits) => digits != "0" && digits != "-0",
        _ => true,
    }
}

fn ensure_theme_contrast_fn(args: &[Value]) -> Response {
    let sliced = match arg(args, 0) {
        Value::Null => {
            return type_error("Cannot read properties of null (reading 'slice')");
        }
        Value::Undefined => {
            return type_error("Cannot read properties of undefined (reading 'slice')");
        }
        Value::String(s) => Value::String(Utf16String::from_units(
            JsString::from_units(s.units().to_vec())
                .slice(1.0, None)
                .into_units(),
        )),
        Value::Array(items) => {
            Value::Array(items.get(1..).unwrap_or_default().to_vec())
        }
        _ => return type_error("color2.slice is not a function"),
    };
    let dark = match arg(args, 1) {
        Value::Undefined => {
            return type_error("window.matchMedia is not a function");
        }
        value => truthy(&value),
    };
    let text = match hex_text(&[sliced]) {
        Ok(Some(text)) => text,
        Ok(None) => return Response::Error("asrs_js::color: unexpected null".to_owned()),
        Err(r) => return r,
    };
    let mut hsl = color::hex_hsl(&text);
    hsl[2] = if dark {
        math::max2(hsl[2], 65.0)
    } else {
        math::min2(hsl[2], 45.0)
    };
    ok_string(format!("#{}", color::hsl_hex(hsl)))
}
