use std::cmp::Ordering;
use std::fmt;

/// An arbitrary-precision JS BigInt: sign and magnitude, magnitude in little-endian `u32` limbs
/// with no trailing zero limbs. Zero is never negative.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct BigInt {
    negative: bool,
    mag: Vec<u32>,
}

/// `BigInt(x)` on a number that is not an integer (including NaN and the infinities). JS throws
/// a RangeError whose message is [`BigIntRangeError::message`] applied to `ToString(x)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BigIntRangeError(pub f64);

impl BigIntRangeError {
    pub fn message(number_to_string: &str) -> String {
        format!(
            "The number {number_to_string} cannot be converted to a BigInt because it is not an integer"
        )
    }
}

/// `BigInt(string)` on text that is not a StringIntegerLiteral. JS throws a SyntaxError with
/// the message `Cannot convert <text> to a BigInt`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BigIntSyntaxError(pub String);

impl fmt::Display for BigIntSyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Cannot convert {} to a BigInt", self.0)
    }
}

fn trim_mag(mag: &mut Vec<u32>) {
    while mag.last() == Some(&0) {
        mag.pop();
    }
}

fn cmp_mag(a: &[u32], b: &[u32]) -> Ordering {
    a.len()
        .cmp(&b.len())
        .then_with(|| a.iter().rev().cmp(b.iter().rev()))
}

fn add_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (long, short) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    let mut out = Vec::with_capacity(long.len() + 1);
    let mut carry = 0u64;
    for (i, &l) in long.iter().enumerate() {
        let sum = u64::from(l) + u64::from(short.get(i).copied().unwrap_or(0)) + carry;
        out.push(sum as u32);
        carry = sum >> 32;
    }
    if carry != 0 {
        out.push(carry as u32);
    }
    out
}

fn sub_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(a.len());
    let mut borrow = 0i64;
    for (i, &l) in a.iter().enumerate() {
        let mut d = i64::from(l) - i64::from(b.get(i).copied().unwrap_or(0)) - borrow;
        borrow = 0;
        if d < 0 {
            d += 1 << 32;
            borrow = 1;
        }
        out.push(d as u32);
    }
    trim_mag(&mut out);
    out
}

fn shl_mag(a: &[u32], n: u64) -> Vec<u32> {
    if a.is_empty() {
        return Vec::new();
    }
    let limbs = (n / 32) as usize;
    let bits = (n % 32) as u32;
    let mut out = vec![0u32; limbs];
    out.reserve(a.len() + 1);
    let mut carry = 0u32;
    for &l in a {
        if bits == 0 {
            out.push(l);
        } else {
            out.push((l << bits) | carry);
            carry = l >> (32 - bits);
        }
    }
    if carry != 0 {
        out.push(carry);
    }
    out
}

/// `a >> n` on a magnitude, and whether any one bit was shifted out.
fn shr_mag(a: &[u32], n: u64) -> (Vec<u32>, bool) {
    let limbs = (n / 32) as usize;
    if limbs >= a.len() {
        return (Vec::new(), !a.is_empty());
    }
    let bits = (n % 32) as u32;
    let mut lost = a[..limbs].iter().any(|&l| l != 0);
    let rest = &a[limbs..];
    let mut out = Vec::with_capacity(rest.len());
    if bits == 0 {
        out.extend_from_slice(rest);
    } else {
        lost |= rest[0] & ((1 << bits) - 1) != 0;
        for i in 0..rest.len() {
            let high = rest.get(i + 1).copied().unwrap_or(0);
            out.push((rest[i] >> bits) | (high << (32 - bits)));
        }
    }
    trim_mag(&mut out);
    (out, lost)
}

fn mul_small_add(mag: &mut Vec<u32>, mul: u32, add: u32) {
    let mut carry = u64::from(add);
    for l in mag.iter_mut() {
        let v = u64::from(*l) * u64::from(mul) + carry;
        *l = v as u32;
        carry = v >> 32;
    }
    if carry != 0 {
        mag.push(carry as u32);
    }
}

fn div_small(mag: &mut [u32], div: u32) -> u32 {
    let mut rem = 0u64;
    for l in mag.iter_mut().rev() {
        let v = (rem << 32) | u64::from(*l);
        *l = (v / u64::from(div)) as u32;
        rem = v % u64::from(div);
    }
    rem as u32
}

fn bit_length(mag: &[u32]) -> u64 {
    match mag.last() {
        None => 0,
        Some(&top) => (mag.len() as u64 - 1) * 32 + u64::from(32 - top.leading_zeros()),
    }
}

fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{9}' | '\u{a}' | '\u{b}' | '\u{c}' | '\u{d}' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

impl BigInt {
    pub fn zero() -> Self {
        Self::default()
    }

    fn from_parts(negative: bool, mut mag: Vec<u32>) -> Self {
        trim_mag(&mut mag);
        let negative = negative && !mag.is_empty();
        Self { negative, mag }
    }

    pub fn from_u64(v: u64) -> Self {
        Self::from_parts(false, vec![v as u32, (v >> 32) as u32])
    }

    pub fn from_i64(v: i64) -> Self {
        let mut out = Self::from_u64(v.unsigned_abs());
        out.negative = v < 0;
        out
    }

    pub fn is_negative(&self) -> bool {
        self.negative
    }

    pub fn is_zero(&self) -> bool {
        self.mag.is_empty()
    }

    /// `BigInt(x)` for a number: NumberToBigInt, exact for every integral double.
    pub fn from_f64(x: f64) -> Result<Self, BigIntRangeError> {
        if !x.is_finite() || x.trunc() != x {
            return Err(BigIntRangeError(x));
        }
        let bits = x.to_bits();
        let exponent = ((bits >> 52) & 0x7ff) as i64;
        if exponent == 0 {
            return Ok(Self::zero());
        }
        let mantissa = (bits & ((1 << 52) - 1)) | (1 << 52);
        let shift = exponent - 1075;
        let base = Self::from_u64(if shift < 0 {
            mantissa >> -shift
        } else {
            mantissa
        });
        let mag = if shift > 0 {
            shl_mag(&base.mag, shift as u64)
        } else {
            base.mag
        };
        Ok(Self::from_parts(bits >> 63 != 0, mag))
    }

    /// `Number(bigint)`: round to nearest, ties to even, overflowing to an infinity.
    pub fn to_f64(&self) -> f64 {
        let len = bit_length(&self.mag);
        if len == 0 {
            return 0.0;
        }
        let magnitude = if len <= 64 {
            let v = self.low_u64();
            v as f64
        } else if len > 1025 {
            f64::INFINITY
        } else {
            let (top, lost) = shr_mag(&self.mag, len - 64);
            let top = u64::from(top[0]) | (u64::from(top.get(1).copied().unwrap_or(0)) << 32);
            let mut mantissa = top >> 11;
            let rest = top & 0x7ff;
            let half = 0x400;
            if rest > half || (rest == half && (lost || mantissa & 1 == 1)) {
                mantissa += 1;
            }
            let mut exponent = len as i64 - 1;
            if mantissa == 1 << 53 {
                mantissa >>= 1;
                exponent += 1;
            }
            if exponent > 1023 {
                f64::INFINITY
            } else {
                f64::from_bits(((exponent + 1023) as u64) << 52 | (mantissa & ((1 << 52) - 1)))
            }
        };
        if self.negative { -magnitude } else { magnitude }
    }

    fn low_u64(&self) -> u64 {
        u64::from(self.mag.first().copied().unwrap_or(0))
            | (u64::from(self.mag.get(1).copied().unwrap_or(0)) << 32)
    }

    /// `BigInt.asUintN(64, x)` as a `u64`, which is what `DataView.setBigUint64` stores.
    pub fn as_uint64(&self) -> u64 {
        let low = self.low_u64();
        if self.negative {
            low.wrapping_neg()
        } else {
            low
        }
    }

    /// `BigInt.asIntN(64, x)` as an `i64`, which is what `DataView.setBigInt64` stores.
    pub fn as_int64(&self) -> i64 {
        self.as_uint64() as i64
    }

    /// `BigInt(string)`: StringToBigInt. Accepts surrounding JS whitespace, an optionally
    /// signed decimal literal, or an unsigned `0b`, `0o` or `0x` literal; the empty string is 0.
    pub fn parse(text: &str) -> Result<Self, BigIntSyntaxError> {
        let error = || BigIntSyntaxError(text.to_owned());
        let body = text.trim_matches(is_js_whitespace);
        if body.is_empty() {
            return Ok(Self::zero());
        }
        let bytes = body.as_bytes();
        let (negative, radix, digits) = if bytes.len() > 2 && bytes[0] == b'0' {
            match bytes[1] {
                b'b' | b'B' => (false, 2, &body[2..]),
                b'o' | b'O' => (false, 8, &body[2..]),
                b'x' | b'X' => (false, 16, &body[2..]),
                _ => (false, 10, body),
            }
        } else if let Some(rest) = body.strip_prefix('-') {
            (true, 10, rest)
        } else if let Some(rest) = body.strip_prefix('+') {
            (false, 10, rest)
        } else {
            (false, 10, body)
        };
        if digits.is_empty() {
            return Err(error());
        }
        let mut mag = Vec::new();
        for c in digits.chars() {
            let d = c.to_digit(radix).ok_or_else(error)?;
            mul_small_add(&mut mag, radix, d);
        }
        Ok(Self::from_parts(negative, mag))
    }

    pub fn neg(&self) -> Self {
        Self::from_parts(!self.negative, self.mag.clone())
    }

    pub fn add(&self, other: &Self) -> Self {
        if self.negative == other.negative {
            return Self::from_parts(self.negative, add_mag(&self.mag, &other.mag));
        }
        match cmp_mag(&self.mag, &other.mag) {
            Ordering::Equal => Self::zero(),
            Ordering::Greater => Self::from_parts(self.negative, sub_mag(&self.mag, &other.mag)),
            Ordering::Less => Self::from_parts(other.negative, sub_mag(&other.mag, &self.mag)),
        }
    }

    pub fn sub(&self, other: &Self) -> Self {
        self.add(&other.neg())
    }

    /// `x << n` for a non-negative shift count.
    pub fn shl(&self, n: u64) -> Self {
        Self::from_parts(self.negative, shl_mag(&self.mag, n))
    }

    /// `x >> n` for a non-negative shift count: an arithmetic shift, rounding toward -Infinity.
    pub fn shr(&self, n: u64) -> Self {
        let (mag, lost) = shr_mag(&self.mag, n);
        if self.negative && lost {
            Self::from_parts(true, add_mag(&mag, &[1]))
        } else {
            Self::from_parts(self.negative, mag)
        }
    }

    fn twos_complement(&self, limbs: usize) -> Vec<u32> {
        let mut out = self.mag.clone();
        out.resize(limbs, 0);
        if self.negative {
            let mut carry = true;
            for l in out.iter_mut() {
                *l = !*l;
                if carry {
                    let (v, c) = l.overflowing_add(1);
                    *l = v;
                    carry = c;
                }
            }
        }
        out
    }

    fn from_twos_complement(mut limbs: Vec<u32>) -> Self {
        let negative = limbs.last().is_some_and(|&l| l >> 31 == 1);
        if negative {
            let mut carry = true;
            for l in limbs.iter_mut() {
                *l = !*l;
                if carry {
                    let (v, c) = l.overflowing_add(1);
                    *l = v;
                    carry = c;
                }
            }
        }
        Self::from_parts(negative, limbs)
    }

    fn bitwise(&self, other: &Self, op: impl Fn(u32, u32) -> u32) -> Self {
        let limbs = self.mag.len().max(other.mag.len()) + 1;
        let a = self.twos_complement(limbs);
        let b = other.twos_complement(limbs);
        Self::from_twos_complement(a.iter().zip(&b).map(|(&x, &y)| op(x, y)).collect())
    }

    pub fn bitand(&self, other: &Self) -> Self {
        self.bitwise(other, |a, b| a & b)
    }

    pub fn bitor(&self, other: &Self) -> Self {
        self.bitwise(other, |a, b| a | b)
    }

    pub fn bitxor(&self, other: &Self) -> Self {
        self.bitwise(other, |a, b| a ^ b)
    }
}

impl Ord for BigInt {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => cmp_mag(&self.mag, &other.mag),
            (true, true) => cmp_mag(&other.mag, &self.mag),
        }
    }
}

impl PartialOrd for BigInt {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl From<i64> for BigInt {
    fn from(v: i64) -> Self {
        Self::from_i64(v)
    }
}

impl From<u64> for BigInt {
    fn from(v: u64) -> Self {
        Self::from_u64(v)
    }
}

/// The decimal form `BigInt.prototype.toString()` produces.
impl fmt::Display for BigInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mag.is_empty() {
            return f.write_str("0");
        }
        let mut mag = self.mag.clone();
        let mut chunks = Vec::new();
        while !mag.is_empty() {
            chunks.push(div_small(&mut mag, 1_000_000_000));
            trim_mag(&mut mag);
        }
        let mut out = String::new();
        if self.negative {
            out.push('-');
        }
        let mut iter = chunks.iter().rev();
        if let Some(first) = iter.next() {
            out.push_str(&format!("{first}"));
        }
        for chunk in iter {
            out.push_str(&format!("{chunk:09}"));
        }
        f.write_str(&out)
    }
}
