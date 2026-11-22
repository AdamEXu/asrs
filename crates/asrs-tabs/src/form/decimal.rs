//! Blink's `Decimal` (`third_party/blink/renderer/platform/wtf/decimal.cc`, Chromium 142): an
//! 18-digit decimal coefficient with a base-10 exponent. Range inputs parse, clamp and step
//! their values in it, so its truncations and roundings are part of what `value` reads back.

use std::cmp::Ordering;

use asrs_js::number;

const EXPONENT_MAX: i32 = 1023;
const EXPONENT_MIN: i32 = -1023;
const PRECISION: i32 = 18;
const MAX_COEFFICIENT: u64 = 999_999_999_999_999_999;
const DBL_DIG: i32 = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Infinity,
    Normal,
    NaN,
    Zero,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decimal {
    negative: bool,
    class: Class,
    exponent: i32,
    coefficient: u64,
}

fn count_digits(x: u64) -> i32 {
    let mut digits = 0;
    let mut power_of_ten: u64 = 1;
    while x >= power_of_ten {
        digits += 1;
        if power_of_ten >= u64::MAX / 10 {
            break;
        }
        power_of_ten *= 10;
    }
    digits
}

fn scale_down(mut x: u64, mut n: i32) -> u64 {
    while n > 0 && x != 0 {
        x /= 10;
        n -= 1;
    }
    x
}

fn scale_up(x: u64, mut n: i32) -> u64 {
    let mut y: u64 = 1;
    let mut z: u64 = 10;
    loop {
        if n & 1 != 0 {
            y = y.wrapping_mul(z);
        }
        n >>= 1;
        if n == 0 {
            return x.wrapping_mul(y);
        }
        z = z.wrapping_mul(z);
    }
}

fn is_multiple_of_power_of_ten(coefficient: u64, n: i32) -> bool {
    coefficient == 0 || coefficient.is_multiple_of(scale_up(1, n))
}

impl From<i32> for Decimal {
    fn from(i: i32) -> Decimal {
        Decimal::new(i < 0, 0, i64::from(i).unsigned_abs())
    }
}

impl Decimal {
    pub fn new(negative: bool, mut exponent: i32, mut coefficient: u64) -> Decimal {
        let class = if coefficient != 0 {
            Class::Normal
        } else {
            Class::Zero
        };
        if (EXPONENT_MIN..=EXPONENT_MAX).contains(&exponent) {
            while coefficient > MAX_COEFFICIENT {
                coefficient /= 10;
                exponent += 1;
            }
        }
        if exponent > EXPONENT_MAX {
            return Decimal::special(negative, Class::Infinity);
        }
        if exponent < EXPONENT_MIN {
            return Decimal::special(negative, Class::Zero);
        }
        Decimal {
            negative,
            class,
            exponent,
            coefficient,
        }
    }

    fn special(negative: bool, class: Class) -> Decimal {
        Decimal {
            negative,
            class,
            exponent: 0,
            coefficient: 0,
        }
    }

    pub fn nan() -> Decimal {
        Decimal::special(false, Class::NaN)
    }

    pub fn infinity(negative: bool) -> Decimal {
        Decimal::special(negative, Class::Infinity)
    }

    pub fn zero(negative: bool) -> Decimal {
        Decimal::special(negative, Class::Zero)
    }

    pub fn is_finite(&self) -> bool {
        !self.is_special()
    }

    pub fn is_nan(&self) -> bool {
        self.class == Class::NaN
    }

    pub fn is_infinity(&self) -> bool {
        self.class == Class::Infinity
    }

    fn is_special(&self) -> bool {
        matches!(self.class, Class::Infinity | Class::NaN)
    }

    pub fn is_zero(&self) -> bool {
        self.class == Class::Zero
    }

    pub fn is_negative(&self) -> bool {
        self.negative
    }

    pub fn abs(&self) -> Decimal {
        Decimal {
            negative: false,
            ..*self
        }
    }

    pub fn negated(&self) -> Decimal {
        if self.is_nan() {
            return *self;
        }
        Decimal {
            negative: !self.negative,
            ..*self
        }
    }

    pub fn from_double(x: f64) -> Decimal {
        if x.is_finite() {
            Decimal::from_units(&number::to_string(x).encode_utf16().collect::<Vec<_>>())
        } else if x.is_infinite() {
            Decimal::infinity(x < 0.0)
        } else {
            Decimal::nan()
        }
    }

    /// `Decimal::FromString`.
    pub fn from_units(s: &[u16]) -> Decimal {
        #[derive(PartialEq)]
        enum State {
            Digit,
            Dot,
            DotDigit,
            E,
            EDigit,
            ESign,
            Sign,
            Start,
            Zero,
        }
        let digit = |ch: u16| (u16::from(b'0')..=u16::from(b'9')).contains(&ch);
        let value = |ch: u16| u64::from(ch - u16::from(b'0'));
        let mut exponent: i32 = 0;
        let mut exponent_negative = false;
        let mut digits: i32 = 0;
        let mut digits_after_dot: i32 = 0;
        let mut extra_digits: i32 = 0;
        let mut negative = false;
        let mut state = State::Start;
        let mut accumulator: u64 = 0;
        for &ch in s {
            match state {
                State::Digit => {
                    if digit(ch) {
                        if digits < PRECISION {
                            digits += 1;
                            accumulator = accumulator * 10 + value(ch);
                        } else {
                            extra_digits += 1;
                        }
                    } else if ch == u16::from(b'.') {
                        state = State::Dot;
                    } else if ch == u16::from(b'E') || ch == u16::from(b'e') {
                        state = State::E;
                    } else {
                        return Decimal::nan();
                    }
                }
                State::Dot | State::DotDigit => {
                    if digit(ch) {
                        if digits < PRECISION {
                            digits += 1;
                            digits_after_dot += 1;
                            accumulator = accumulator * 10 + value(ch);
                        }
                        state = State::DotDigit;
                    } else if ch == u16::from(b'E') || ch == u16::from(b'e') {
                        state = State::E;
                    } else {
                        return Decimal::nan();
                    }
                }
                State::E => {
                    if ch == u16::from(b'+') {
                        exponent_negative = false;
                        state = State::ESign;
                    } else if ch == u16::from(b'-') {
                        exponent_negative = true;
                        state = State::ESign;
                    } else if digit(ch) {
                        exponent = value(ch) as i32;
                        state = State::EDigit;
                    } else {
                        return Decimal::nan();
                    }
                }
                State::EDigit => {
                    if !digit(ch) {
                        return Decimal::nan();
                    }
                    exponent = exponent * 10 + value(ch) as i32;
                    if exponent > EXPONENT_MAX + PRECISION {
                        if accumulator != 0 {
                            return if exponent_negative {
                                Decimal::zero(false)
                            } else {
                                Decimal::infinity(negative)
                            };
                        }
                        return Decimal::zero(negative);
                    }
                }
                State::ESign => {
                    if !digit(ch) {
                        return Decimal::nan();
                    }
                    exponent = value(ch) as i32;
                    state = State::EDigit;
                }
                State::Sign | State::Start => {
                    if (u16::from(b'1')..=u16::from(b'9')).contains(&ch) {
                        accumulator = value(ch);
                        digits = 1;
                        state = State::Digit;
                    } else if state == State::Start && ch == u16::from(b'-') {
                        negative = true;
                        state = State::Sign;
                    } else if state == State::Start && ch == u16::from(b'+') {
                        negative = false;
                        state = State::Sign;
                    } else if ch == u16::from(b'0') {
                        state = State::Zero;
                    } else if ch == u16::from(b'.') {
                        state = State::Dot;
                    } else {
                        return Decimal::nan();
                    }
                }
                State::Zero => {
                    if ch == u16::from(b'0') {
                        continue;
                    }
                    if (u16::from(b'1')..=u16::from(b'9')).contains(&ch) {
                        accumulator = value(ch);
                        digits = 1;
                        state = State::Digit;
                    } else if ch == u16::from(b'.') {
                        state = State::Dot;
                    } else if ch == u16::from(b'E') || ch == u16::from(b'e') {
                        state = State::E;
                    } else {
                        return Decimal::nan();
                    }
                }
            }
        }
        match state {
            State::Zero => Decimal::zero(negative),
            State::Digit | State::EDigit | State::DotDigit => {
                let mut result_exponent = if exponent_negative {
                    -exponent
                } else {
                    exponent
                } - digits_after_dot
                    + extra_digits;
                if result_exponent < EXPONENT_MIN {
                    return Decimal::zero(false);
                }
                let overflow = result_exponent - EXPONENT_MAX + 1;
                if overflow > 0 {
                    if overflow + digits - digits_after_dot > PRECISION {
                        return Decimal::infinity(negative);
                    }
                    accumulator = scale_up(accumulator, overflow);
                    result_exponent -= overflow;
                }
                Decimal::new(negative, result_exponent, accumulator)
            }
            _ => Decimal::nan(),
        }
    }

    fn special_values(&self, rhs: &Decimal) -> Special {
        if self.is_finite() && rhs.is_finite() {
            Special::BothFinite
        } else if self.is_nan() {
            Special::EitherNaN(*self)
        } else if rhs.is_nan() {
            Special::EitherNaN(*rhs)
        } else if self.is_infinity() {
            if rhs.is_infinity() {
                Special::BothInfinity
            } else {
                Special::LhsInfinity
            }
        } else {
            Special::RhsInfinity
        }
    }

    fn align(lhs: &Decimal, rhs: &Decimal) -> (u64, u64, i32) {
        let (le, re) = (lhs.exponent, rhs.exponent);
        let mut exponent = le.min(re);
        let mut lc = lhs.coefficient;
        let mut rc = rhs.coefficient;
        if le > re {
            let lhs_digits = count_digits(lc);
            if lhs_digits != 0 {
                let shift = le - re;
                let overflow = lhs_digits + shift - PRECISION;
                if overflow <= 0 {
                    lc = scale_up(lc, shift);
                } else {
                    lc = scale_up(lc, shift - overflow);
                    rc = scale_down(rc, overflow);
                    exponent += overflow;
                }
            }
        } else if le < re {
            let rhs_digits = count_digits(rc);
            if rhs_digits != 0 {
                let shift = re - le;
                let overflow = rhs_digits + shift - PRECISION;
                if overflow <= 0 {
                    rc = scale_up(rc, shift);
                } else {
                    rc = scale_up(rc, shift - overflow);
                    lc = scale_down(lc, overflow);
                    exponent += overflow;
                }
            }
        }
        (lc, rc, exponent)
    }

    fn signed_result(negative: bool, exponent: i32, result: u64) -> Decimal {
        if result as i64 >= 0 {
            Decimal::new(negative, exponent, result)
        } else {
            Decimal::new(!negative, exponent, (result as i64).wrapping_neg() as u64)
        }
    }

    pub fn plus(&self, rhs: &Decimal) -> Decimal {
        match self.special_values(rhs) {
            Special::BothFinite => {}
            Special::BothInfinity => {
                return if self.negative == rhs.negative {
                    *self
                } else {
                    Decimal::nan()
                };
            }
            Special::EitherNaN(v) => return v,
            Special::LhsInfinity => return *self,
            Special::RhsInfinity => return *rhs,
        }
        let (lc, rc, exponent) = Decimal::align(self, rhs);
        let result = if self.negative == rhs.negative {
            lc.wrapping_add(rc)
        } else {
            lc.wrapping_sub(rc)
        };
        if self.negative && !rhs.negative && result == 0 {
            return Decimal::new(false, exponent, 0);
        }
        Decimal::signed_result(self.negative, exponent, result)
    }

    pub fn minus(&self, rhs: &Decimal) -> Decimal {
        match self.special_values(rhs) {
            Special::BothFinite => {}
            Special::BothInfinity => {
                return if self.negative == rhs.negative {
                    Decimal::nan()
                } else {
                    *self
                };
            }
            Special::EitherNaN(v) => return v,
            Special::LhsInfinity => return *self,
            Special::RhsInfinity => return Decimal::infinity(!rhs.negative),
        }
        let (lc, rc, exponent) = Decimal::align(self, rhs);
        let result = if self.negative == rhs.negative {
            lc.wrapping_sub(rc)
        } else {
            lc.wrapping_add(rc)
        };
        if self.negative && rhs.negative && result == 0 {
            return Decimal::new(false, exponent, 0);
        }
        Decimal::signed_result(self.negative, exponent, result)
    }

    pub fn times(&self, rhs: &Decimal) -> Decimal {
        let negative = self.negative != rhs.negative;
        match self.special_values(rhs) {
            Special::BothFinite => {
                let mut exponent = self.exponent + rhs.exponent;
                let mut work = u128::from(self.coefficient) * u128::from(rhs.coefficient);
                while work >> 64 != 0 {
                    work /= 10;
                    exponent += 1;
                }
                Decimal::new(negative, exponent, work as u64)
            }
            Special::BothInfinity => Decimal::infinity(negative),
            Special::EitherNaN(v) => v,
            Special::LhsInfinity if rhs.is_zero() => Decimal::nan(),
            Special::RhsInfinity if self.is_zero() => Decimal::nan(),
            Special::LhsInfinity | Special::RhsInfinity => Decimal::infinity(negative),
        }
    }

    pub fn divided_by(&self, rhs: &Decimal) -> Decimal {
        let negative = self.negative != rhs.negative;
        match self.special_values(rhs) {
            Special::BothFinite => {}
            Special::BothInfinity => return Decimal::nan(),
            Special::EitherNaN(v) => return v,
            Special::LhsInfinity => return Decimal::infinity(negative),
            Special::RhsInfinity => return Decimal::zero(negative),
        }
        if rhs.is_zero() {
            return if self.is_zero() {
                Decimal::nan()
            } else {
                Decimal::infinity(negative)
            };
        }
        let mut exponent = self.exponent - rhs.exponent;
        if self.is_zero() {
            return Decimal::new(negative, exponent, 0);
        }
        let mut remainder = self.coefficient;
        let divisor = rhs.coefficient;
        let mut result: u64 = 0;
        loop {
            while remainder < divisor && result < MAX_COEFFICIENT / 10 {
                remainder = remainder.wrapping_mul(10);
                result *= 10;
                exponent -= 1;
            }
            if remainder < divisor {
                break;
            }
            let quotient = remainder / divisor;
            if result > MAX_COEFFICIENT - quotient {
                break;
            }
            result += quotient;
            remainder %= divisor;
            if remainder == 0 {
                break;
            }
        }
        if remainder > divisor / 2 {
            result += 1;
        }
        Decimal::new(negative, exponent, result)
    }

    fn compare_to(&self, rhs: &Decimal) -> Decimal {
        let result = self.minus(rhs);
        match result.class {
            Class::Infinity => Decimal::from(if result.negative { -1 } else { 1 }),
            Class::NaN | Class::Normal => result,
            Class::Zero => Decimal::zero(false),
        }
    }

    /// `operator==`, `operator<` and `operator>`: every comparison with a NaN is false.
    pub fn compare(&self, rhs: &Decimal) -> Option<Ordering> {
        if self == rhs {
            return Some(Ordering::Equal);
        }
        let result = self.compare_to(rhs);
        if result.is_nan() {
            None
        } else if result.is_zero() {
            Some(Ordering::Equal)
        } else if result.negative {
            Some(Ordering::Less)
        } else {
            Some(Ordering::Greater)
        }
    }

    pub fn less_than(&self, rhs: &Decimal) -> bool {
        self.compare(rhs) == Some(Ordering::Less)
    }

    pub fn greater_than(&self, rhs: &Decimal) -> bool {
        self.compare(rhs) == Some(Ordering::Greater)
    }

    pub fn at_least(&self, rhs: &Decimal) -> bool {
        matches!(self.compare(rhs), Some(Ordering::Greater | Ordering::Equal))
    }

    pub fn round(&self) -> Decimal {
        if self.is_special() || self.exponent >= 0 {
            return *self;
        }
        let mut result = self.coefficient;
        let digits = count_digits(result);
        let drop_digits = -self.exponent;
        if digits < drop_digits {
            return Decimal::zero(false);
        }
        result = scale_down(result, drop_digits - 1);
        if result % 10 >= 5 {
            result += 10;
        }
        result /= 10;
        Decimal::new(self.negative, 0, result)
    }

    pub fn floor(&self) -> Decimal {
        if self.is_special() || self.exponent >= 0 {
            return *self;
        }
        let mut result = self.coefficient;
        let digits = count_digits(result);
        let drop_digits = -self.exponent;
        if digits < drop_digits {
            return if self.negative {
                Decimal::from(-1)
            } else {
                Decimal::zero(false)
            };
        }
        result = scale_down(result, drop_digits);
        if self.negative && !is_multiple_of_power_of_ten(self.coefficient, drop_digits) {
            result += 1;
        }
        Decimal::new(self.negative, 0, result)
    }

    /// `Decimal::ToString`.
    pub fn to_js_string(&self) -> String {
        match self.class {
            Class::Infinity => {
                return if self.negative {
                    "-Infinity"
                } else {
                    "Infinity"
                }
                .to_owned();
            }
            Class::NaN => return "NaN".to_owned(),
            Class::Normal | Class::Zero => {}
        }
        let mut out = String::new();
        if self.negative {
            out.push('-');
        }
        let mut exponent = self.exponent;
        let mut coefficient = self.coefficient;
        if exponent < 0 {
            let mut last_digit = 0;
            while count_digits(coefficient) > DBL_DIG {
                last_digit = coefficient % 10;
                coefficient /= 10;
                exponent += 1;
            }
            if last_digit >= 5 {
                coefficient += 1;
            }
            while exponent < 0 && coefficient != 0 && coefficient.is_multiple_of(10) {
                coefficient /= 10;
                exponent += 1;
            }
        }
        let digits = format!("{coefficient}");
        let digits = digits.as_bytes();
        let mut length = digits.len() as i32;
        let adjusted = exponent + length - 1;
        if exponent <= 0 && adjusted >= -6 {
            if exponent == 0 {
                out.push_str(std::str::from_utf8(digits).unwrap_or_default());
                return out;
            }
            if adjusted >= 0 {
                for (i, &d) in digits.iter().enumerate() {
                    out.push(char::from(d));
                    if i as i32 == adjusted {
                        out.push('.');
                    }
                }
                return out;
            }
            out.push_str("0.");
            for _ in adjusted + 1..0 {
                out.push('0');
            }
            out.push_str(std::str::from_utf8(digits).unwrap_or_default());
        } else {
            out.push(char::from(digits[0]));
            while length >= 2 && digits[length as usize - 1] == b'0' {
                length -= 1;
            }
            if length >= 2 {
                out.push('.');
                for &d in &digits[1..length as usize] {
                    out.push(char::from(d));
                }
            }
            if adjusted != 0 {
                out.push_str(if adjusted < 0 { "e" } else { "e+" });
                out.push_str(&format!("{adjusted}"));
            }
        }
        out
    }
}

enum Special {
    BothFinite,
    BothInfinity,
    EitherNaN(Decimal),
    LhsInfinity,
    RhsInfinity,
}
