//! `Math.*`, the `%` and `**` operators, int32 conversions and BigInt conversions, as V8 14.2
//! (Electron 39) computes them, down to the last bit and NaN payload.
//!
//! Transcendentals are ports of V8's `src/base/ieee754.cc` (fdlibm) and, for `sin` and `cos`,
//! of the glibc routines V8 bundles under `third_party/glibc` (`v8_use_libm_trig_functions`).
//! `pow` is the exception: V8 defaults to `--use-std-math-pow`, so it calls the platform C
//! library's `pow`, and so does [`pow`] here.

pub mod bigint;
mod bits;
mod fdlibm;
mod glibc;
pub mod int;
mod sincostab;

pub use bigint::{BigInt, BigIntRangeError, BigIntSyntaxError};
pub use int::{
    clz32, imul, shift_left, shift_right, shift_right_unsigned, to_int8, to_int16, to_int32,
    to_uint8, to_uint8_clamp, to_uint16, to_uint32,
};

pub use fdlibm::{
    acos, acosh, asin, asinh, atan, atan2, atanh, cbrt, cosh, exp, expm1, log, log1p, log2, log10,
    sinh, tan, tanh,
};

pub const NAN: f64 = f64::from_bits(0x7ff8_0000_0000_0000);

pub const E: f64 = std::f64::consts::E;
pub const LN10: f64 = std::f64::consts::LN_10;
pub const LN2: f64 = std::f64::consts::LN_2;
pub const LOG10E: f64 = std::f64::consts::LOG10_E;
pub const LOG2E: f64 = std::f64::consts::LOG2_E;
pub const PI: f64 = std::f64::consts::PI;
pub const SQRT1_2: f64 = std::f64::consts::FRAC_1_SQRT_2;
pub const SQRT2: f64 = std::f64::consts::SQRT_2;

pub fn sin(x: f64) -> f64 {
    glibc::sin(x)
}

pub fn cos(x: f64) -> f64 {
    glibc::cos(x)
}

/// `Math.pow` and `**`. The result for finite non-special operands comes from the platform
/// C library, exactly as in V8 with `--use-std-math-pow` (its default outside AIX).
#[allow(clippy::float_cmp)]
pub fn pow(x: f64, y: f64) -> f64 {
    if y.is_nan() {
        return NAN;
    }
    if y.is_infinite() && (x == 1.0 || x == -1.0) {
        return NAN;
    }
    let x = if x.is_nan() { NAN } else { x };
    if y == 2.0 {
        return x * x;
    }
    if y == 0.5 {
        return if x.is_infinite() {
            f64::INFINITY
        } else {
            (x + 0.0).sqrt()
        };
    }
    x.powf(y)
}

/// The `%` operator on numbers (C `fmod`, which is exact).
pub fn rem(x: f64, y: f64) -> f64 {
    x % y
}

pub fn abs(x: f64) -> f64 {
    x.abs()
}

pub fn sqrt(x: f64) -> f64 {
    x.sqrt()
}

pub fn ceil(x: f64) -> f64 {
    x.ceil()
}

pub fn floor(x: f64) -> f64 {
    x.floor()
}

pub fn trunc(x: f64) -> f64 {
    x.trunc()
}

pub fn fround(x: f64) -> f64 {
    f64::from(x as f32)
}

/// `Math.round`: halves round toward +Infinity and `-0` stays `-0`, computed the way V8's
/// `Float64Round` does (ceil, then step back when the ceiling is more than half above).
pub fn round(x: f64) -> f64 {
    let up = x.ceil();
    if up - 0.5 <= x { up } else { up - 1.0 }
}

pub fn sign(x: f64) -> f64 {
    if x < 0.0 {
        -1.0
    } else if x > 0.0 {
        1.0
    } else {
        x
    }
}

fn is_signaling(x: f64) -> bool {
    x.is_nan() && x.to_bits() & (1 << 51) == 0
}

fn quiet(x: f64) -> f64 {
    f64::from_bits(x.to_bits() | (1 << 51))
}

fn propagate_nan(a: f64, b: f64) -> Option<f64> {
    if is_signaling(a) {
        Some(quiet(a))
    } else if is_signaling(b) {
        Some(quiet(b))
    } else if a.is_nan() {
        Some(a)
    } else if b.is_nan() {
        Some(b)
    } else {
        None
    }
}

/// `Float64Max`, as the arm64 `fmax` instruction computes it: the first NaN operand wins and
/// `+0` is greater than `-0`.
pub fn max2(a: f64, b: f64) -> f64 {
    if let Some(nan) = propagate_nan(a, b) {
        return nan;
    }
    if a == 0.0 && b == 0.0 {
        return if a.is_sign_negative() { b } else { a };
    }
    if a > b { a } else { b }
}

/// `Float64Min`, mirroring [`max2`].
pub fn min2(a: f64, b: f64) -> f64 {
    if let Some(nan) = propagate_nan(a, b) {
        return nan;
    }
    if a == 0.0 && b == 0.0 {
        return if a.is_sign_negative() { a } else { b };
    }
    if a < b { a } else { b }
}

/// `Math.max(...values)`.
pub fn max(values: &[f64]) -> f64 {
    values
        .iter()
        .fold(f64::NEG_INFINITY, |acc, &v| max2(acc, v))
}

/// `Math.min(...values)`.
pub fn min(values: &[f64]) -> f64 {
    values.iter().fold(f64::INFINITY, |acc, &v| min2(acc, v))
}

/// `Math.hypot(...values)`, with V8's separate one, two and three argument fast paths and its
/// Kahan-summed general case.
pub fn hypot(values: &[f64]) -> f64 {
    match *values {
        [] => 0.0,
        [a] => a.abs(),
        [a, b] => {
            let (a, b) = (a.abs(), b.abs());
            if a == f64::INFINITY || b == f64::INFINITY {
                return f64::INFINITY;
            }
            let max = max2(a, b);
            if max.is_nan() {
                return NAN;
            }
            if max == 0.0 {
                return 0.0;
            }
            ((a / max) * (a / max) + (b / max) * (b / max)).sqrt() * max
        }
        [a, b, c] => {
            let (a, b, c) = (a.abs(), b.abs(), c.abs());
            if a == f64::INFINITY || b == f64::INFINITY || c == f64::INFINITY {
                return f64::INFINITY;
            }
            let max = max2(max2(a, b), c);
            if max.is_nan() {
                return NAN;
            }
            if max == 0.0 {
                return 0.0;
            }
            let power_a = (a / max) * (a / max);
            let power_b = (b / max) * (b / max);
            let compensation = (power_a + power_b) - power_a - power_b;
            let power_c = (c / max) * (c / max) - compensation;
            (power_a + power_b + power_c).sqrt() * max
        }
        _ => {
            let mut one_is_nan = false;
            let mut max = 0.0;
            for &v in values {
                if v.is_nan() {
                    one_is_nan = true;
                } else if v.abs() > max {
                    max = v.abs();
                }
            }
            if max == f64::INFINITY {
                return f64::INFINITY;
            }
            if one_is_nan {
                return NAN;
            }
            if max == 0.0 {
                return 0.0;
            }
            let mut sum = 0.0;
            let mut compensation = 0.0;
            for &v in values {
                let n = if v.is_nan() { 0.0 } else { v.abs() / max };
                let summand = n * n - compensation;
                let preliminary = sum + summand;
                compensation = (preliminary - sum) - summand;
                sum = preliminary;
            }
            sum.sqrt() * max
        }
    }
}
