//! The bignumber.js 9.1.2 subset `can-spec-util.ts` uses. A finite value is a sign, a
//! significant-decimal-digit string and a base-10 exponent for the first digit, matching
//! BigNumber's `s`/`c`/`e` triple: `value = 0.digits * 10^(exp + 1)`. Arithmetic is exact
//! (`times`, `plus`, `minus`), `div` is 20 significant digits ROUND_HALF_UP (the library's
//! `DECIMAL_PLACES`/`ROUNDING_MODE` defaults), and `toNumber` is `Number(valueOf())`, the
//! correctly-rounded f64 of the decimal expansion. NaN, +/-Infinity and signed zero follow
//! the library: comparisons with NaN return false, x - x is +0 under the default
//! ROUNDING_MODE (4), and -0 keeps its sign through `toNumber`.

#[derive(Clone, Debug, PartialEq)]
pub enum BigNum {
    Nan,
    Infinity { neg: bool },
    Finite(Finite),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Finite {
    pub neg: bool,
    /// Significant digits, most significant first; empty for zero.
    pub digits: Vec<u8>,
    /// Base-10 exponent of the first digit (bignumber.js `e`).
    pub exp: i64,
}

const ROUND_UP: u8 = 0;
pub const ROUND_DOWN: u8 = 1;
const ROUND_CEIL: u8 = 2;
pub(crate) const ROUND_FLOOR: u8 = 3;
pub const ROUND_HALF_UP: u8 = 4;
pub const ROUND_HALF_DOWN: u8 = 5;
const ROUND_HALF_EVEN: u8 = 6;
const ROUND_HALF_CEIL: u8 = 7;
const ROUND_HALF_FLOOR: u8 = 8;

const ROUNDING_MODE: u8 = ROUND_HALF_UP;
const DECIMAL_PLACES: i64 = 20;
const MAX_EXP: i64 = 10_000_000;
const MIN_EXP: i64 = -10_000_000;

impl BigNum {
    fn zero(neg: bool) -> BigNum {
        BigNum::Finite(Finite {
            neg,
            digits: Vec::new(),
            exp: 0,
        })
    }

    fn is_zero(&self) -> bool {
        matches!(self, BigNum::Finite(f) if f.digits.is_empty())
    }

    fn sign(&self) -> Option<bool> {
        match self {
            BigNum::Nan => None,
            BigNum::Infinity { neg } => Some(*neg),
            BigNum::Finite(f) => Some(f.neg),
        }
    }

    fn sign_bit(&self) -> bool {
        self.sign().unwrap_or(false)
    }

    /// `BigNumber(v)` for a decimal string (the `b == null` path): the `isNumeric`
    /// grammar, then `parseNumeric` for `Infinity`/`-Infinity`/`NaN` and everything else
    /// (DEBUG off, so unrecognized input is NaN, not an error).
    pub fn from_decimal(s: &str) -> BigNum {
        if !is_numeric(s) {
            return match s {
                "Infinity" => BigNum::Infinity { neg: false },
                "-Infinity" => BigNum::Infinity { neg: true },
                _ => BigNum::Nan,
            };
        }
        let (neg, body) = match s.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, s),
        };
        let body = body.strip_suffix('.').unwrap_or(body);
        let (mantissa, exp_add) = match body.find(['e', 'E']) {
            Some(i) => {
                let exp_str = &body[i + 1..];
                let (esign, edigits) = match exp_str.strip_prefix('-') {
                    Some(rest) => (-1i64, rest),
                    None => (1, exp_str.strip_prefix('+').unwrap_or(exp_str)),
                };
                let exp = edigits.bytes().fold(0i64, |a, b| {
                    a.saturating_mul(10).saturating_add((b - b'0') as i64)
                }) * esign;
                (&body[..i], exp)
            }
            None => (body, 0),
        };
        let (int, frac) = match mantissa.split_once('.') {
            Some((i, f)) => (i, f),
            None => (mantissa, ""),
        };
        let mut digits: Vec<u8> = int.bytes().chain(frac.bytes()).map(|b| b - b'0').collect();
        let leading = digits.iter().take_while(|d| **d == 0).count();
        digits.drain(..leading);
        while digits.last() == Some(&0) {
            digits.pop();
        }
        if digits.is_empty() {
            return BigNum::zero(neg);
        }
        let exp = int.len() as i64 + exp_add - leading as i64 - 1;
        if exp > MAX_EXP {
            return BigNum::Infinity { neg };
        }
        if exp < MIN_EXP {
            return BigNum::zero(neg);
        }
        BigNum::Finite(Finite { neg, digits, exp })
    }

    /// `BigNumber(v)` for a JS number: `String(v)` decimal conversion, non-finite values
    /// through `parseNumeric`, `1/v < 0` for the sign so -0 stays negative.
    pub fn from_f64(v: f64) -> BigNum {
        if v.is_nan() {
            return BigNum::Nan;
        }
        if v.is_infinite() {
            return BigNum::Infinity { neg: v < 0.0 };
        }
        let neg = v.is_sign_negative();
        match BigNum::from_decimal(&asrs_js::number::to_string(v.abs())) {
            BigNum::Finite(mut f) => {
                f.neg = neg;
                BigNum::Finite(f)
            }
            other => other,
        }
    }

    /// `BigNumber(v.toString())` for a bigint; integers only.
    pub fn from_int(v: i128) -> BigNum {
        let mut digits = Vec::new();
        let mut n = v.unsigned_abs();
        loop {
            digits.push((n % 10) as u8);
            n /= 10;
            if n == 0 {
                break;
            }
        }
        digits.reverse();
        while digits.last() == Some(&0) {
            digits.pop();
        }
        if digits.is_empty() {
            return BigNum::zero(false);
        }
        let exp = (v.unsigned_abs().ilog10()) as i64;
        BigNum::Finite(Finite {
            neg: v < 0,
            digits,
            exp: exp.max(0),
        })
    }

    /// The `(integer digits, trailing exponent)` form: `value = digits * 10^t`.
    fn scaled(&self) -> (Vec<u8>, i64) {
        match self {
            BigNum::Finite(f) => (f.digits.clone(), f.exp - f.digits.len() as i64 + 1),
            _ => unreachable!("scaled on non-finite"),
        }
    }

    fn finish(neg: bool, mut digits: Vec<u8>, trailing_exp: i64) -> BigNum {
        let mut dropped = 0i64;
        while digits.last() == Some(&0) {
            digits.pop();
            dropped += 1;
        }
        let trailing_exp = trailing_exp + dropped;
        if digits.is_empty() {
            return BigNum::zero(neg);
        }
        let exp = trailing_exp + digits.len() as i64 - 1;
        if exp > MAX_EXP {
            return BigNum::Infinity { neg };
        }
        if exp < MIN_EXP {
            return BigNum::zero(neg);
        }
        BigNum::Finite(Finite { neg, digits, exp })
    }

    /// `x.times(y)`: exact product.
    pub fn times(&self, y: &BigNum) -> BigNum {
        let (x_inf, y_inf) = (
            matches!(self, BigNum::Infinity { .. }),
            matches!(y, BigNum::Infinity { .. }),
        );
        if matches!(self, BigNum::Nan)
            || matches!(y, BigNum::Nan)
            || (self.is_zero() && y_inf)
            || (y.is_zero() && x_inf)
        {
            return BigNum::Nan;
        }
        let neg = self.sign_bit() ^ y.sign_bit();
        if x_inf || y_inf {
            return BigNum::Infinity { neg };
        }
        if self.is_zero() || y.is_zero() {
            return BigNum::zero(neg);
        }
        let (a, ta) = self.scaled();
        let (b, tb) = y.scaled();
        BigNum::finish(neg, mul_digits(&a, &b), ta + tb)
    }

    /// `x.plus(y)`: exact. Structure mirrors the library, including its delegation to
    /// `minus` on differing signs so zero results take the same signs upstream gives.
    pub fn plus(&self, y: &BigNum) -> BigNum {
        let (a, b) = (self.sign(), y.sign());
        if a.is_none() || b.is_none() {
            return BigNum::Nan;
        }
        let (a, b) = (a.unwrap(), b.unwrap());
        if a != b {
            return self.minus(&y.negated());
        }
        if matches!(self, BigNum::Infinity { .. }) || matches!(y, BigNum::Infinity { .. }) {
            return BigNum::Infinity { neg: a };
        }
        if self.is_zero() || y.is_zero() {
            if !y.is_zero() {
                return y.clone();
            }
            if !self.is_zero() {
                return self.clone();
            }
            return BigNum::zero(a);
        }
        self.magnitude_add(y, a, false)
    }

    /// `x.minus(y)`: exact; delegates to `plus` on differing signs like the library.
    pub fn minus(&self, y: &BigNum) -> BigNum {
        let (a, b) = (self.sign(), y.sign());
        if a.is_none() || b.is_none() {
            return BigNum::Nan;
        }
        let (a, b) = (a.unwrap(), b.unwrap());
        if a != b {
            return self.plus(&y.negated());
        }
        let (x_inf, y_inf) = (
            matches!(self, BigNum::Infinity { .. }),
            matches!(y, BigNum::Infinity { .. }),
        );
        if x_inf || y_inf {
            // x Inf -> x (y finite or NaN for both Inf); y Inf -> -y.
            if x_inf {
                return if y_inf { BigNum::Nan } else { self.clone() };
            }
            return BigNum::Infinity { neg: !b };
        }
        if self.is_zero() || y.is_zero() {
            if !y.is_zero() {
                return BigNum::Finite(Finite {
                    neg: !b,
                    ..y.finite().clone()
                });
            }
            if !self.is_zero() {
                return self.clone();
            }
            return BigNum::zero(ROUNDING_MODE == ROUND_FLOOR);
        }
        self.magnitude_add(y, a, true)
    }

    /// Finite, non-zero, same-sign add (subtract=false) or subtract (subtract=true).
    /// `a` is the shared sign.
    fn magnitude_add(&self, y: &BigNum, a: bool, subtract: bool) -> BigNum {
        let (xa, xt) = self.scaled();
        let (ya, yt) = y.scaled();
        let t = xt.min(yt);
        let xa = shift(&xa, (xt - t) as usize);
        let ya = shift(&ya, (yt - t) as usize);
        if !subtract {
            BigNum::finish(a, add_digits(&xa, &ya), t)
        } else {
            match cmp_digits(&xa, &ya) {
                std::cmp::Ordering::Greater => BigNum::finish(a, sub_digits(&xa, &ya), t),
                std::cmp::Ordering::Less => BigNum::finish(!a, sub_digits(&ya, &xa), t),
                std::cmp::Ordering::Equal => BigNum::zero(ROUNDING_MODE == ROUND_FLOOR),
            }
        }
    }

    /// `x.div(y)`: the quotient rounded to `DECIMAL_PLACES` (20) decimal places with
    /// `ROUNDING_MODE` (half-up), i.e. to `20 + q.e + 1` significant digits.
    pub fn div(&self, y: &BigNum) -> BigNum {
        let s_neg = self.sign_bit() ^ y.sign_bit();
        let (x_inf, y_inf) = (
            matches!(self, BigNum::Infinity { .. }),
            matches!(y, BigNum::Infinity { .. }),
        );
        if matches!(self, BigNum::Nan)
            || matches!(y, BigNum::Nan)
            || (x_inf && y_inf)
            || (self.is_zero() && y.is_zero())
        {
            return BigNum::Nan;
        }
        if self.is_zero() || y_inf {
            return BigNum::zero(s_neg);
        }
        if x_inf || y.is_zero() {
            return BigNum::Infinity { neg: s_neg };
        }
        let (xf, yf) = (self.finite(), y.finite());
        let (xd, yd) = (&xf.digits, &yf.digits);
        // Quotient exponent: (0.xdigits / 0.ydigits) * 10^(x.e - y.e); when the
        // numerator's digit string (padded) sorts below the denominator's, one more
        // power of ten.
        let mut exp = xf.exp - yf.exp;
        if cmp_digits(
            &shift(xd, yd.len().saturating_sub(xd.len())),
            &shift(yd, xd.len().saturating_sub(yd.len())),
        ) == std::cmp::Ordering::Less
        {
            exp -= 1;
        }
        // Significant digits wanted, plus one for the rounding digit.
        let sd = DECIMAL_PLACES + exp + 1;
        let want = (sd + 1).max(2) as usize;
        // q = (xd / yd) * 10^(yd.len - xd.len + x.e - y.e); the first quotient digit is
        // floor(rem / den) with the scaling below so it lands at exponent `exp`.
        let m0 = yd.len() as i64 - xd.len() as i64 + xf.exp - yf.exp - exp;
        let den = shift(yd, (-m0).max(0) as usize);
        let mut rem = shift(xd, m0.max(0) as usize);
        let mut digits: Vec<u8> = Vec::new();
        let mut more = false;
        loop {
            let mut d = 0u8;
            while cmp_digits(&rem, &den) != std::cmp::Ordering::Less {
                rem = sub_digits(&rem, &den);
                d += 1;
            }
            digits.push(d);
            let rem_zero = rem.iter().all(|r| *r == 0);
            if digits.len() >= want {
                more = !rem_zero;
                break;
            }
            if rem_zero {
                break;
            }
            rem.push(0);
        }
        let finite = Finite {
            neg: s_neg,
            digits,
            exp,
        };
        let (rounded, _) = round_finite(&finite, sd, ROUNDING_MODE, more);
        match rounded {
            BigNum::Finite(f) => {
                let t = f.exp - f.digits.len() as i64 + 1;
                BigNum::finish(f.neg, f.digits, t)
            }
            other => other,
        }
    }

    /// The operand with its sign flipped; `y.s = -b` in the library's delegation.
    fn negated(&self) -> BigNum {
        match self {
            BigNum::Nan => BigNum::Nan,
            BigNum::Infinity { neg } => BigNum::Infinity { neg: !*neg },
            BigNum::Finite(f) => BigNum::Finite(Finite {
                neg: !f.neg,
                ..f.clone()
            }),
        }
    }

    fn finite(&self) -> &Finite {
        match self {
            BigNum::Finite(f) => f,
            _ => unreachable!("finite on non-finite BigNum"),
        }
    }

    /// `x.comparedTo(y)`: -1, 0, 1, or None when either is NaN (the library's `null`,
    /// which makes `lt`/`gt`/`eq` all return false).
    fn compare(&self, y: &BigNum) -> Option<i8> {
        let (i, j) = (self.sign()?, y.sign()?);
        let (x_zero, y_zero) = (self.is_zero(), y.is_zero());
        if x_zero || y_zero {
            return Some(if x_zero && y_zero {
                0
            } else if x_zero {
                if j { 1 } else { -1 }
            } else {
                if i { -1 } else { 1 }
            });
        }
        if i != j {
            return Some(if i { -1 } else { 1 });
        }
        let neg = i;
        let (x_inf, y_inf) = (
            matches!(self, BigNum::Infinity { .. }),
            matches!(y, BigNum::Infinity { .. }),
        );
        if x_inf || y_inf {
            return Some(if x_inf && y_inf {
                0
            } else if x_inf ^ neg {
                1
            } else {
                -1
            });
        }
        let (xf, yf) = (self.finite(), y.finite());
        if xf.exp != yf.exp {
            return Some(if (xf.exp > yf.exp) ^ neg { 1 } else { -1 });
        }
        for k in 0..xf.digits.len().min(yf.digits.len()) {
            if xf.digits[k] != yf.digits[k] {
                return Some(if (xf.digits[k] > yf.digits[k]) ^ neg {
                    1
                } else {
                    -1
                });
            }
        }
        Some(if xf.digits.len() == yf.digits.len() {
            0
        } else if (xf.digits.len() > yf.digits.len()) ^ neg {
            1
        } else {
            -1
        })
    }

    pub fn lt(&self, y: &BigNum) -> bool {
        self.compare(y).is_some_and(|c| c < 0)
    }

    pub fn gt(&self, y: &BigNum) -> bool {
        self.compare(y).is_some_and(|c| c > 0)
    }

    /// `x.integerValue(rm)`: `round(x, x.e + 1, rm)` — round to an integer; Infinity and
    /// NaN pass through.
    pub fn integer_value(&self, rm: u8) -> BigNum {
        match self {
            BigNum::Nan | BigNum::Infinity { .. } => self.clone(),
            BigNum::Finite(f) => {
                let (rounded, _) = round_finite(f, f.exp + 1, rm, false);
                rounded
            }
        }
    }

    /// `x.toNumber()`: `+valueOf(this)`, the correctly-rounded f64 of the decimal value.
    /// -0 survives because `valueOf` writes `-0`.
    pub fn to_number(&self) -> f64 {
        match self {
            BigNum::Nan => f64::NAN,
            BigNum::Infinity { neg } => {
                if *neg {
                    f64::NEG_INFINITY
                } else {
                    f64::INFINITY
                }
            }
            BigNum::Finite(f) => {
                let mut s = String::new();
                if f.neg {
                    s.push('-');
                }
                if f.digits.is_empty() {
                    s.push('0');
                } else {
                    for d in &f.digits {
                        s.push((b'0' + d) as char);
                    }
                    // value = digits * 10^t; Number() parses the exact decimal either way.
                    s.push('e');
                    let t = f.exp - f.digits.len() as i64 + 1;
                    use std::fmt::Write as _;
                    let _ = write!(s, "{t}");
                }
                let units: Vec<u16> = s.bytes().map(|b| b as u16).collect();
                asrs_js::number::to_number(&units)
            }
        }
    }

    /// `x.toFixed()`: the plain decimal expansion (no rounding when `dp` is omitted),
    /// `-` only for a non-zero negative.
    pub fn to_fixed(&self) -> String {
        match self {
            BigNum::Nan => "NaN".to_owned(),
            BigNum::Infinity { neg } => {
                if *neg {
                    "-Infinity".to_owned()
                } else {
                    "Infinity".to_owned()
                }
            }
            BigNum::Finite(f) => {
                let digits: String = if f.digits.is_empty() {
                    "0".to_owned()
                } else {
                    f.digits.iter().map(|d| (b'0' + d) as char).collect()
                };
                let e = f.exp;
                let mut s = String::new();
                if e < 0 {
                    s.push_str("0.");
                    for _ in 0..(-e - 1) {
                        s.push('0');
                    }
                    s.push_str(&digits);
                } else {
                    let len = digits.len() as i64;
                    if e + 1 > len {
                        s.push_str(&digits);
                        for _ in 0..(e + 1 - len) {
                            s.push('0');
                        }
                    } else if e + 1 == len {
                        s.push_str(&digits);
                    } else {
                        s.push_str(&digits[..e as usize + 1]);
                        s.push('.');
                        s.push_str(&digits[e as usize + 1..]);
                    }
                }
                if f.neg && !f.digits.is_empty() {
                    s.insert(0, '-');
                }
                s
            }
        }
    }
}

/// `/-?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?$/i` — the `isNumeric` test.
fn is_numeric(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    let (mantissa, exp) = match s.find(['e', 'E']) {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, ""),
    };
    if !exp.is_empty() {
        let digits = exp.strip_prefix(['+', '-']).unwrap_or(exp);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
    } else if s.len() != mantissa.len() {
        return false;
    }
    let (int, frac) = match mantissa.split_once('.') {
        Some((i, f)) => (i, f),
        None => (mantissa, ""),
    };
    let int_digits = int.bytes().all(|b| b.is_ascii_digit());
    let frac_digits = frac.bytes().all(|b| b.is_ascii_digit());
    (!int.is_empty() && int_digits && frac_digits)
        || (int.is_empty() && !frac.is_empty() && frac_digits)
}

fn mul_digits(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; a.len() + b.len()];
    for (i, &da) in a.iter().enumerate().rev() {
        let mut carry = 0u16;
        for (j, &db) in b.iter().enumerate().rev() {
            let k = i + j + 1;
            let t = out[k] as u16 + da as u16 * db as u16 + carry;
            out[k] = (t % 10) as u8;
            carry = t / 10;
        }
        out[i] += carry as u8;
    }
    let first_nonzero = out.iter().position(|d| *d != 0).unwrap_or(out.len());
    out.drain(..first_nonzero);
    out
}

fn add_digits(a: &[u8], b: &[u8]) -> Vec<u8> {
    let n = a.len().max(b.len());
    let mut out = vec![0u8; n + 1];
    for k in 0..n {
        let da = if k < n - a.len() {
            0
        } else {
            a[k - (n - a.len())]
        };
        let db = if k < n - b.len() {
            0
        } else {
            b[k - (n - b.len())]
        };
        out[k + 1] = da + db;
    }
    for k in (1..=n).rev() {
        out[k - 1] += out[k] / 10;
        out[k] %= 10;
    }
    if out[0] == 0 {
        out.remove(0);
    }
    out
}

/// a - b for digit integers with a >= b.
fn sub_digits(a: &[u8], b: &[u8]) -> Vec<u8> {
    let n = a.len();
    let mut out = a.to_vec();
    for k in (0..n).rev() {
        let db = if k < n - b.len() {
            0
        } else {
            b[k - (n - b.len())]
        };
        if out[k] < db {
            let mut i = k;
            while out[i - 1] == 0 {
                out[i - 1] = 9;
                i -= 1;
            }
            out[i - 1] -= 1;
            out[k] += 10;
        }
        out[k] -= db;
    }
    let first_nonzero = out.iter().position(|d| *d != 0).unwrap_or(out.len());
    out.drain(..first_nonzero);
    out
}

/// Compare two digit integers of possibly different lengths.
fn cmp_digits(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    a.len().cmp(&b.len()).then_with(|| a.iter().cmp(b.iter()))
}

fn shift(digits: &[u8], zeros: usize) -> Vec<u8> {
    let mut out = digits.to_vec();
    out.extend(std::iter::repeat_n(0, zeros));
    out
}

/// `round(x, sd, rm, r)` on a finite value: round to `sd` significant digits. Returns the
/// rounded value and whether rounding dropped non-zero digits.
fn round_finite(f: &Finite, sd: i64, rm: u8, r_extra: bool) -> (BigNum, bool) {
    if sd >= f.digits.len() as i64 && !r_extra && sd > 0 {
        return (BigNum::Finite(f.clone()), false);
    }
    if sd <= 0 {
        // Digit at the round position: the first digit for sd == 0, zero for sd < 0.
        let rd = if sd == 0 {
            f.digits.first().copied().unwrap_or(0)
        } else {
            0
        };
        let r = r_extra || sd < 0 || f.digits.len() > 1;
        if round_up(rd, r, rm, f.neg, 0) {
            return (
                BigNum::Finite(Finite {
                    neg: f.neg,
                    digits: vec![1],
                    exp: -sd,
                }),
                true,
            );
        }
        return (BigNum::zero(f.neg), true);
    }
    let sd = sd as usize;
    let rd = f.digits.get(sd).copied().unwrap_or(0);
    let r = r_extra
        || f.digits[(sd + 1).min(f.digits.len())..]
            .iter()
            .any(|d| *d != 0);
    let prev = if sd > 0 { f.digits[sd - 1] } else { 0 };
    let mut kept: Vec<u8> = f.digits[..sd.min(f.digits.len())].to_vec();
    let mut exp = f.exp;
    if round_up(rd, r, rm, f.neg, prev) {
        // Increment the kept digits; all-nines carries out the front.
        let mut i = kept.len();
        loop {
            if i == 0 {
                kept.insert(0, 1);
                exp += 1;
                break;
            }
            i -= 1;
            if kept[i] == 9 {
                kept[i] = 0;
            } else {
                kept[i] += 1;
                break;
            }
        }
    }
    while kept.last() == Some(&0) {
        kept.pop();
    }
    if kept.is_empty() {
        return (BigNum::zero(f.neg), true);
    }
    (
        BigNum::Finite(Finite {
            neg: f.neg,
            digits: kept,
            exp,
        }),
        true,
    )
}

/// The `r = rm < 4 ? ... : ...` decision of bignumber.js `round`.
fn round_up(rd: u8, r: bool, rm: u8, neg: bool, prev: u8) -> bool {
    if rm < 4 {
        (rd != 0 || r) && (rm == ROUND_UP || rm == if neg { ROUND_FLOOR } else { ROUND_CEIL })
    } else {
        rd > 5
            || (rd == 5
                && (rm == ROUND_HALF_UP
                    || r
                    || (rm == ROUND_HALF_EVEN && prev % 2 == 1)
                    || rm
                        == if neg {
                            ROUND_HALF_FLOOR
                        } else {
                            ROUND_HALF_CEIL
                        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bn(s: &str) -> BigNum {
        BigNum::from_decimal(s)
    }

    fn fixed(b: &BigNum) -> String {
        b.to_fixed()
    }

    #[test]
    fn decimal_parse() {
        assert_eq!(fixed(&bn("0.5")), "0.5");
        assert_eq!(fixed(&bn("-0")), "0");
        assert_eq!(fixed(&bn("18446744073709551615")), "18446744073709551615");
        assert_eq!(
            fixed(&bn("0.00003082369457075716")),
            "0.00003082369457075716"
        );
        assert_eq!(fixed(&bn("1e3")), "1000");
        assert_eq!(bn("5e-324").to_number().to_bits(), 5e-324f64.to_bits());
        assert_eq!(fixed(&bn("garbage")), "NaN");
        assert_eq!(fixed(&bn("Infinity")), "Infinity");
    }

    #[test]
    fn times() {
        let x = bn("18446744073709551615").times(&bn("0.00003082369457075716"));
        assert_eq!(fixed(&x), "568596805152947.9226152601546508134");
        let x = bn("5").times(&bn("0.00001541161211566339"));
        assert_eq!(fixed(&x), "0.00007705806057831695");
    }

    #[test]
    fn plus_minus() {
        assert_eq!(fixed(&bn("1.5").plus(&bn("0.25"))), "1.75");
        assert_eq!(fixed(&bn("1.5").minus(&bn("2"))), "-0.5");
        assert_eq!(fixed(&bn("0").plus(&bn("0"))), "0");
        assert_eq!(fixed(&bn("1").minus(&bn("1"))), "0");
        assert_eq!(
            fixed(&bn("1e-20").plus(&bn("1e20"))),
            "100000000000000000000.00000000000000000001"
        );
    }

    #[test]
    fn compares() {
        assert!(bn("1").lt(&bn("2")));
        assert!(!bn("2").lt(&bn("1")));
        assert!(bn("2").gt(&bn("1")));
        assert!(!BigNum::Nan.lt(&bn("0")));
        assert!(!BigNum::Nan.gt(&bn("0")));
        assert!(bn("-0").lt(&bn("1")));
        assert!(!bn("0").lt(&bn("-0")) && !bn("-0").lt(&bn("0")));
    }

    #[test]
    fn integer_value() {
        assert_eq!(fixed(&bn("2.5").integer_value(ROUND_HALF_DOWN)), "2");
        assert_eq!(fixed(&bn("2.5").integer_value(ROUND_HALF_UP)), "3");
        assert_eq!(fixed(&bn("-2.5").integer_value(ROUND_HALF_DOWN)), "-2");
        assert_eq!(fixed(&bn("0.5").integer_value(ROUND_HALF_UP)), "1");
        assert_eq!(fixed(&bn("0.4").integer_value(ROUND_HALF_UP)), "0");
        assert_eq!(fixed(&bn("-0.4").integer_value(ROUND_DOWN)), "0");
        assert_eq!(
            fixed(&bn("32442.9").integer_value(ROUND_HALF_DOWN)),
            "32443"
        );
    }

    #[test]
    fn to_number() {
        assert_eq!(bn("0.5").to_number(), 0.5);
        assert_eq!(bn("-0").to_number().to_bits(), (-0.0f64).to_bits());
        assert_eq!(
            bn("18446744073709551615").to_number().to_bits(),
            1.8446744073709552e19f64.to_bits()
        );
        assert!(BigNum::Nan.to_number().is_nan());
        assert_eq!(
            BigNum::Infinity { neg: true }.to_number(),
            f64::NEG_INFINITY
        );
        assert_eq!(bn("5e-324").to_number().to_bits(), 5e-324f64.to_bits());
    }

    #[test]
    fn div() {
        assert_eq!(fixed(&bn("1").div(&bn("3"))), "0.33333333333333333333");
        assert_eq!(fixed(&bn("1.5").div(&bn("1"))), "1.5");
        assert_eq!(fixed(&bn("0").div(&bn("0"))), "NaN");
        assert_eq!(fixed(&bn("1").div(&bn("0"))), "Infinity");
        assert_eq!(
            fixed(&bn("32442").div(&bn("0.00003082369457075716"))),
            "1052501994.05940608469304528568"
        );
    }
}
