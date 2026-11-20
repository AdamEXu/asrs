//! `fn` names for `asrs_js::math`. The asrs-js vector tests include this file too, so the
//! committed vectors and `cargo xtask parity fn` exercise the same dispatch.

use asrs_js::math::{self, BigInt};
use asrs_parity::{Response, Value};

pub type Function = fn(&[Value]) -> Response;

pub const FUNCTIONS: &[(&str, Function)] = &[
    ("Math.abs", |a| unary(a, math::abs)),
    ("Math.acos", |a| unary(a, math::acos)),
    ("Math.acosh", |a| unary(a, math::acosh)),
    ("Math.asin", |a| unary(a, math::asin)),
    ("Math.asinh", |a| unary(a, math::asinh)),
    ("Math.atan", |a| unary(a, math::atan)),
    ("Math.atanh", |a| unary(a, math::atanh)),
    ("Math.atan2", |a| binary(a, math::atan2)),
    ("Math.cbrt", |a| unary(a, math::cbrt)),
    ("Math.ceil", |a| unary(a, math::ceil)),
    ("Math.clz32", |a| unary(a, |x| f64::from(math::clz32(x)))),
    ("Math.cos", |a| unary(a, math::cos)),
    ("Math.cosh", |a| unary(a, math::cosh)),
    ("Math.exp", |a| unary(a, math::exp)),
    ("Math.expm1", |a| unary(a, math::expm1)),
    ("Math.floor", |a| unary(a, math::floor)),
    ("Math.fround", |a| unary(a, math::fround)),
    ("Math.hypot", |a| variadic(a, math::hypot)),
    ("Math.imul", |a| {
        binary(a, |x, y| f64::from(math::imul(x, y)))
    }),
    ("Math.log", |a| unary(a, math::log)),
    ("Math.log1p", |a| unary(a, math::log1p)),
    ("Math.log10", |a| unary(a, math::log10)),
    ("Math.log2", |a| unary(a, math::log2)),
    ("Math.max", |a| variadic(a, math::max)),
    ("Math.min", |a| variadic(a, math::min)),
    ("Math.pow", |a| binary(a, math::pow)),
    ("Math.round", |a| unary(a, math::round)),
    ("Math.sign", |a| unary(a, math::sign)),
    ("Math.sin", |a| unary(a, math::sin)),
    ("Math.sinh", |a| unary(a, math::sinh)),
    ("Math.sqrt", |a| unary(a, math::sqrt)),
    ("Math.tan", |a| unary(a, math::tan)),
    ("Math.tanh", |a| unary(a, math::tanh)),
    ("Math.trunc", |a| unary(a, math::trunc)),
    ("BigInt", bigint_ctor),
    ("BigInt.asIntN", |a| {
        as_n(a, |b| BigInt::from_i64(b.as_int64()))
    }),
    ("BigInt.asUintN", |a| {
        as_n(a, |b| BigInt::from_u64(b.as_uint64()))
    }),
    ("Number", number_ctor),
];

/// `oracle:call` sources this module answers, keyed by the exact source text.
pub const CALLS: &[(&str, Function)] = &[
    (
        "() => [Math.E, Math.LN10, Math.LN2, Math.LOG10E, Math.LOG2E, Math.PI, Math.SQRT1_2, Math.SQRT2]",
        |_| {
            Response::Ok(Value::Array(
                [
                    math::E,
                    math::LN10,
                    math::LN2,
                    math::LOG10E,
                    math::LOG2E,
                    math::PI,
                    math::SQRT1_2,
                    math::SQRT2,
                ]
                .map(Value::number)
                .to_vec(),
            ))
        },
    ),
    ("(a, b) => a % b", |a| binary(a, math::rem)),
    ("(a, b) => a ** b", |a| binary(a, math::pow)),
    ("(a) => a | 0", |a| {
        unary(a, |x| f64::from(math::to_int32(x)))
    }),
    ("(a) => a >>> 0", |a| {
        unary(a, |x| f64::from(math::to_uint32(x)))
    }),
    ("(a) => new Int8Array([a])[0]", |a| {
        unary(a, |x| f64::from(math::to_int8(x)))
    }),
    ("(a) => new Uint8Array([a])[0]", |a| {
        unary(a, |x| f64::from(math::to_uint8(x)))
    }),
    ("(a) => new Uint8ClampedArray([a])[0]", |a| {
        unary(a, |x| f64::from(math::to_uint8_clamp(x)))
    }),
    ("(a) => new Int16Array([a])[0]", |a| {
        unary(a, |x| f64::from(math::to_int16(x)))
    }),
    ("(a) => new Uint16Array([a])[0]", |a| {
        unary(a, |x| f64::from(math::to_uint16(x)))
    }),
    ("(a, b) => a << b", |a| {
        int_or_bigint(
            a,
            |x, y| f64::from(math::shift_left(x, y)),
            |x, y| Ok(shift(x, y, false)),
        )
    }),
    ("(a, b) => a >> b", |a| {
        int_or_bigint(
            a,
            |x, y| f64::from(math::shift_right(x, y)),
            |x, y| Ok(shift(x, y, true)),
        )
    }),
    ("(a, b) => a >>> b", |a| {
        binary(a, |x, y| f64::from(math::shift_right_unsigned(x, y)))
    }),
    ("(a, b) => a & b", |a| {
        int_or_bigint(
            a,
            |x, y| f64::from(math::to_int32(x) & math::to_int32(y)),
            |x, y| Ok(x.bitand(y)),
        )
    }),
    ("(a, b) => a | b", |a| {
        int_or_bigint(
            a,
            |x, y| f64::from(math::to_int32(x) | math::to_int32(y)),
            |x, y| Ok(x.bitor(y)),
        )
    }),
    ("(a, b) => a ^ b", |a| {
        int_or_bigint(
            a,
            |x, y| f64::from(math::to_int32(x) ^ math::to_int32(y)),
            |x, y| Ok(x.bitxor(y)),
        )
    }),
    ("(a, b) => a + b", |a| {
        int_or_bigint(a, |x, y| x + y, |x, y| Ok(x.add(y)))
    }),
    ("(a, b) => a - b", |a| {
        int_or_bigint(a, |x, y| x - y, |x, y| Ok(x.sub(y)))
    }),
    ("(a, b) => a < b", |a| {
        match (bigint_arg(a, 0), bigint_arg(a, 1)) {
            (Some(x), Some(y)) => Response::Ok(Value::Bool(x < y)),
            _ => unsupported(a),
        }
    }),
];

fn unsupported(args: &[Value]) -> Response {
    Response::Error(format!("asrs_js::math: unsupported arguments {args:?}"))
}

fn number_arg(args: &[Value], i: usize) -> Option<f64> {
    match args.get(i) {
        None | Some(Value::Undefined) => Some(math::NAN),
        Some(v) => v.as_f64(),
    }
}

fn bigint_arg(args: &[Value], i: usize) -> Option<BigInt> {
    match args.get(i) {
        Some(Value::BigInt(digits)) => BigInt::parse(digits).ok(),
        _ => None,
    }
}

fn unary(args: &[Value], f: impl Fn(f64) -> f64) -> Response {
    match number_arg(args, 0) {
        Some(x) => Response::Ok(Value::number(f(x))),
        None => unsupported(args),
    }
}

fn binary(args: &[Value], f: impl Fn(f64, f64) -> f64) -> Response {
    match (number_arg(args, 0), number_arg(args, 1)) {
        (Some(x), Some(y)) => Response::Ok(Value::number(f(x, y))),
        _ => unsupported(args),
    }
}

fn variadic(args: &[Value], f: impl Fn(&[f64]) -> f64) -> Response {
    let values: Option<Vec<f64>> = args.iter().map(Value::as_f64).collect();
    match values {
        Some(values) => Response::Ok(Value::number(f(&values))),
        None => unsupported(args),
    }
}

fn int_or_bigint(
    args: &[Value],
    number: impl Fn(f64, f64) -> f64,
    bigint: impl Fn(&BigInt, &BigInt) -> Result<BigInt, Response>,
) -> Response {
    if let (Some(x), Some(y)) = (bigint_arg(args, 0), bigint_arg(args, 1)) {
        return match bigint(&x, &y) {
            Ok(v) => Response::Ok(Value::BigInt(format!("{v}"))),
            Err(e) => e,
        };
    }
    binary(args, number)
}

fn shift(x: &BigInt, count: &BigInt, right: bool) -> BigInt {
    let magnitude = if count.is_negative() {
        count.neg().as_uint64()
    } else {
        count.as_uint64()
    };
    if right != count.is_negative() {
        x.shr(magnitude)
    } else {
        x.shl(magnitude)
    }
}

fn throw(name: &str, message: String) -> Response {
    Response::Throw(Value::Error(name.into(), message.into()))
}

/// Number::toString for the non-integral doubles `BigInt(x)` rejects: shortest round-trip
/// digits, in exponent form below 1e-6. Integral values never reach it.
fn non_integral_to_string(x: f64) -> String {
    if x.is_nan() {
        "NaN".to_owned()
    } else if x.is_infinite() {
        if x > 0.0 { "Infinity" } else { "-Infinity" }.to_owned()
    } else if x.abs() < 1e-6 {
        format!("{x:e}")
    } else {
        format!("{x}")
    }
}

fn bigint_ctor(args: &[Value]) -> Response {
    match args.first() {
        Some(Value::Number(bits)) => match BigInt::from_f64(f64::from_bits(*bits)) {
            Ok(v) => Response::Ok(Value::BigInt(format!("{v}"))),
            Err(e) => throw(
                "RangeError",
                math::BigIntRangeError::message(&non_integral_to_string(e.0)),
            ),
        },
        Some(Value::String(s)) => match BigInt::parse(&s.to_string_lossy()) {
            Ok(v) => Response::Ok(Value::BigInt(format!("{v}"))),
            Err(e) => throw("SyntaxError", format!("{e}")),
        },
        Some(Value::BigInt(_)) => Response::Ok(args[0].clone()),
        _ => unsupported(args),
    }
}

fn as_n(args: &[Value], f: impl Fn(&BigInt) -> BigInt) -> Response {
    match (number_arg(args, 0), bigint_arg(args, 1)) {
        (Some(64.0), Some(x)) => Response::Ok(Value::BigInt(format!("{}", f(&x)))),
        _ => unsupported(args),
    }
}

fn number_ctor(args: &[Value]) -> Response {
    match bigint_arg(args, 0) {
        Some(x) => Response::Ok(Value::number(x.to_f64())),
        None => unsupported(args),
    }
}
