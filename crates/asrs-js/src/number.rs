// Ported from V8 14.2 (BSD-3-Clause, Copyright the V8 project authors). See THIRD_PARTY_NOTICES.md.

//! ECMAScript number formatting and parsing, matching V8 (Electron 39.2.7).
//!
//! Formatting for `toFixed`, `toPrecision`, `toExponential` and `Intl.NumberFormat`
//! rounds an exact or shortest decimal expansion, which is how V8's
//! `base::DoubleToAscii` behaves. `parseInt` and `Number` port V8's
//! `DetectRadixInternal`, `InternalStringToIntDouble` and `HandleGenericCase`
//! paths exactly, including the implementation-defined accumulator for
//! non-power-of-two radixes.

mod bigint;

use bigint::BigUint;

/// V8's radix digits with a trailing NUL, which `DoubleToRadixStringView` can
/// write when `fraction * radix` rounds up to exactly `radix`.
const RADIX_CHARS: &[u8; 37] = b"0123456789abcdefghijklmnopqrstuvwxyz\0";

/// `Number.prototype.toString` / `String(x)` for radix 10.
pub fn to_string(x: f64) -> String {
    ryu_js::Buffer::new().format(x).to_owned()
}

/// `Number.prototype.toString` with an already ToInteger-converted radix.
/// `Err` carries the RangeError message.
pub fn to_string_radix(value: f64, radix: f64) -> Result<String, &'static str> {
    if !(2.0..=36.0).contains(&radix) {
        return Err("toString() radix argument must be between 2 and 36");
    }
    if radix == 10.0 {
        return Ok(to_string(value));
    }
    if value == 0.0 {
        return Ok("0".to_owned());
    }
    if value.is_nan() {
        return Ok("NaN".to_owned());
    }
    if value.is_infinite() {
        return Ok(if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned());
    }
    Ok(radix_string(value, radix as i32))
}

/// `Number.prototype.toFixed` with an already ToInteger-converted digit count.
pub fn to_fixed(value: f64, fraction_digits: f64) -> Result<String, &'static str> {
    if !(0.0..=100.0).contains(&fraction_digits) {
        return Err("toFixed() digits argument must be between 0 and 100");
    }
    if value.is_nan() {
        return Ok("NaN".to_owned());
    }
    let negative = value < 0.0;
    let abs = value.abs();
    if abs >= 1e21 {
        return Ok(to_string(value));
    }
    let f = fraction_digits as i32;
    let (digits, point) = exact_digits(abs);
    let (digits, point) = round_at(&digits, point, point + f);
    Ok(format_fixed(&digits, point, f, negative))
}

/// `Number.prototype.toExponential`. `None` is an undefined argument.
pub fn to_exponential(value: f64, fraction_digits: Option<f64>) -> Result<String, &'static str> {
    if value.is_nan() {
        return Ok("NaN".to_owned());
    }
    let negative = value < 0.0;
    let abs = value.abs();
    if abs.is_infinite() {
        return Ok(if negative { "-Infinity" } else { "Infinity" }.to_owned());
    }
    if let Some(f) = fraction_digits
        && !(0.0..=100.0).contains(&f)
    {
        return Err("toExponential() argument must be between 0 and 100");
    }
    let (digits, point, significant) = match fraction_digits {
        None => {
            let (d, p) = shortest_digits(abs);
            let significant = d.len() as i32;
            (d, p, significant)
        }
        Some(f) => {
            let keep = f as i32 + 1;
            let (d, p) = if abs == 0.0 {
                (vec![b'0'], 1)
            } else {
                let (d, p) = exact_digits(abs);
                round_at(&d, p, keep)
            };
            (d, p, keep)
        }
    };
    Ok(format_exponential(
        &digits,
        point - 1,
        significant,
        negative,
    ))
}

/// `Number.prototype.toPrecision`. `None` is an undefined argument.
pub fn to_precision(value: f64, precision: Option<f64>) -> Result<String, &'static str> {
    let Some(p) = precision else {
        return Ok(to_string(value));
    };
    if value.is_nan() {
        return Ok("NaN".to_owned());
    }
    let negative = value < 0.0;
    let abs = value.abs();
    if abs.is_infinite() {
        return Ok(if negative { "-Infinity" } else { "Infinity" }.to_owned());
    }
    if !(1.0..=100.0).contains(&p) {
        return Err("toPrecision() argument must be between 1 and 100");
    }
    let p = p as i32;
    let (digits, point) = if abs == 0.0 {
        (vec![b'0'], 1)
    } else {
        let (d, pt) = exact_digits(abs);
        round_at(&d, pt, p)
    };
    Ok(format_precision(&digits, point, p, negative))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SignDisplay {
    Auto,
    Never,
    Always,
    ExceptZero,
}

/// `Intl.NumberFormat` decimal formatting for the "en-US" configuration space
/// upstream uses: `maximumFractionDigits`, `useGrouping`, fraction/integer
/// minimums and `signDisplay`. Values are formatted from the shortest
/// round-trip digits, matching ICU.
#[derive(Clone, Copy)]
pub struct IntlOptions {
    pub min_integer: u32,
    pub min_fraction: u32,
    pub max_fraction: u32,
    pub grouping: bool,
    pub sign_display: SignDisplay,
}

impl Default for IntlOptions {
    fn default() -> Self {
        Self {
            min_integer: 1,
            min_fraction: 0,
            max_fraction: 3,
            grouping: true,
            sign_display: SignDisplay::Auto,
        }
    }
}

fn sign_prefix(negative: bool, zero_result: bool, display: SignDisplay) -> &'static str {
    match display {
        SignDisplay::Never => "",
        SignDisplay::Auto => {
            if negative {
                "-"
            } else {
                ""
            }
        }
        SignDisplay::Always => {
            if negative {
                "-"
            } else {
                "+"
            }
        }
        SignDisplay::ExceptZero => {
            if zero_result {
                ""
            } else if negative {
                "-"
            } else {
                "+"
            }
        }
    }
}

pub fn intl_format(value: f64, options: IntlOptions) -> String {
    if value.is_nan() {
        return match options.sign_display {
            SignDisplay::Always => {
                format!("{}NaN", if value.is_sign_negative() { "-" } else { "+" })
            }
            _ => "NaN".to_owned(),
        };
    }
    let negative = value.is_sign_negative();
    let abs = value.abs();
    if abs.is_infinite() {
        return format!("{}∞", sign_prefix(negative, false, options.sign_display));
    }
    let (digits, point) = shortest_digits(abs);
    let (digits, point) = round_at(&digits, point, point + options.max_fraction as i32);
    let mut out = String::new();
    let zero_result = digits.iter().all(|&d| d == b'0');
    out.push_str(sign_prefix(negative, zero_result, options.sign_display));
    let int_len = point.max(0) as usize;
    let mut written = String::with_capacity(int_len.max(1) + int_len / 3);
    for i in 0..int_len {
        written.push(if i < digits.len() {
            digits[i] as char
        } else {
            '0'
        });
    }
    if written.is_empty() {
        written.push('0');
    }
    while written.len() < options.min_integer as usize {
        written.insert(0, '0');
    }
    if options.grouping {
        for (i, c) in written.chars().enumerate() {
            if i > 0 && (written.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
        }
    } else {
        out.push_str(&written);
    }
    let max_f = options.max_fraction as i32;
    if max_f > 0 {
        let mut frac = String::with_capacity(max_f as usize);
        for j in 0..max_f {
            let i = point + j;
            frac.push(if i < 0 || i as usize >= digits.len() {
                '0'
            } else {
                digits[i as usize] as char
            });
        }
        let mut frac_len = frac.len();
        while frac_len > options.min_fraction as usize && frac.as_bytes()[frac_len - 1] == b'0' {
            frac_len -= 1;
        }
        if frac_len > 0 {
            out.push('.');
            out.push_str(&frac[..frac_len]);
        }
    }
    out
}

/// `Intl.NumberFormat` for "en-US" with `notation: "scientific"` and exactly `fraction` digits.
pub fn intl_format_scientific(value: f64, fraction: u32) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    let sign = if value.is_sign_negative() { "-" } else { "" };
    let abs = value.abs();
    if abs.is_infinite() {
        return format!("{sign}∞");
    }
    let (digits, point) = shortest_digits(abs);
    let (digits, point) = round_at(&digits, point, fraction as i32 + 1);
    let mut out = format!("{sign}{}", digit_or_zero(&digits, 0));
    if fraction > 0 {
        out.push('.');
        out.extend((1..=fraction as i32).map(|i| digit_or_zero(&digits, i)));
    }
    let exponent = if abs == 0.0 { 0 } else { point - 1 };
    out.push_str(&format!("E{exponent}"));
    out
}

/// `String.prototype` whitespace and line terminators accepted by
/// `AdvanceToNonspace`.
fn is_space(u: u16) -> bool {
    matches!(
        u,
        0x0009..=0x000d | 0x0020 | 0x00a0 | 0x1680 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f
        | 0x205f | 0x3000 | 0xfeff
    )
}

fn skip_space(u: &[u16], mut i: usize) -> usize {
    while i < u.len() && is_space(u[i]) {
        i += 1;
    }
    i
}

fn digit(c: u16, radix: i32) -> Option<u32> {
    let d = match c {
        c if (b'0' as u16..=b'9' as u16).contains(&c) => (c - b'0' as u16) as i32,
        c if (b'a' as u16..=b'z' as u16).contains(&c) => (c - b'a' as u16) as i32 + 10,
        c if (b'A' as u16..=b'Z' as u16).contains(&c) => (c - b'A' as u16) as i32 + 10,
        _ => return None,
    };
    (d < radix).then_some(d as u32)
}

/// V8 `InternalStringToIntDouble` for radix 2, 4, 8, 16, 32.
fn int_double(u: &[u16], radix_log2: u32, allow_junk: bool) -> f64 {
    let radix = 1u32 << radix_log2;
    let end = u.len();
    let mut cur = 0usize;
    while cur < end && u[cur] == b'0' as u16 {
        cur += 1;
        if cur == end {
            return 0.0;
        }
    }
    let mut number: i64 = 0;
    let mut exponent: i64 = 0;
    loop {
        let Some(d) = digit(u[cur], radix as i32) else {
            if cur == 0 {
                return f64::NAN;
            }
            if allow_junk || skip_space(u, cur) == end {
                break;
            }
            return f64::NAN;
        };
        number = number * radix as i64 + d as i64;
        let mut overflow = number >> 53;
        if overflow != 0 {
            let mut count = 1;
            while overflow > 1 {
                count += 1;
                overflow >>= 1;
            }
            let mask = (1i64 << count) - 1;
            let dropped = number & mask;
            number >>= count;
            exponent = count as i64;
            let mut zero_tail = true;
            loop {
                cur += 1;
                if cur == end || digit(u[cur], radix as i32).is_none() {
                    break;
                }
                zero_tail &= u[cur] == b'0' as u16;
                exponent += radix_log2 as i64;
            }
            if !allow_junk && skip_space(u, cur) != end {
                return f64::NAN;
            }
            let middle = 1i64 << (count - 1);
            if dropped > middle || (dropped == middle && ((number & 1) != 0 || !zero_tail)) {
                number += 1;
            }
            if number & (1i64 << 53) != 0 {
                exponent += 1;
                number >>= 1;
            }
            break;
        }
        cur += 1;
        if cur == end {
            break;
        }
    }
    if exponent == 0 {
        return number as f64;
    }
    scale2(number as f64, exponent)
}

fn scale2(mantissa: f64, e: i64) -> f64 {
    if e > 1023 {
        return f64::INFINITY;
    }
    if e < -1074 {
        return 0.0;
    }
    if e >= -1022 {
        mantissa * f64::from_bits(((e + 1023) as u64) << 52)
    } else {
        mantissa * f64::from_bits(((e + 2097) as u64) << 52) * f64::from_bits(1)
    }
}

/// `HandleBaseTenCase`: strtod over the collected digit run. Runs of 310 or
/// more digits always overflow to infinity.
fn base_ten(u: &[u16], start: usize) -> f64 {
    let mut end = start;
    while end < u.len() && (b'0' as u16..=b'9' as u16).contains(&u[end]) {
        end += 1;
    }
    let count = end - start;
    if count >= 310 {
        return f64::INFINITY;
    }
    let s: String = u[start..end].iter().map(|&c| c as u8 as char).collect();
    s.parse().unwrap_or(f64::NAN)
}

/// V8 `HandleGenericCase`: the 32-bit multiply-accumulate approximation the
/// spec allows for non-power-of-two radixes other than 10.
fn generic_int(u: &[u16], start: usize, radix: i32) -> f64 {
    let radix = radix as u32;
    let end = u.len();
    let mut cur = start;
    let mut result = 0.0f64;
    let mut done = false;
    while !done {
        let mut part: u32 = 0;
        let mut multiplier: u32 = 1;
        loop {
            let Some(d) = digit(u[cur], radix as i32) else {
                done = true;
                break;
            };
            let m = multiplier.wrapping_mul(radix);
            if m > u32::MAX / 36 {
                break;
            }
            part = part * radix + d;
            multiplier = m;
            cur += 1;
            if cur == end {
                done = true;
                break;
            }
        }
        result = result * multiplier as f64 + part as f64;
    }
    result
}

fn signed_zero(negative: bool) -> f64 {
    if negative { -0.0 } else { 0.0 }
}

/// `Number` applied to a string (StringToDouble with
/// `ALLOW_NON_DECIMAL_PREFIX`): empty or whitespace-only input gives +0.
pub fn to_number(u: &[u16]) -> f64 {
    let end = u.len();
    let cur = skip_space(u, 0);
    if cur == end {
        return 0.0;
    }
    if u[cur] == b'0' as u16 {
        let p = cur + 1;
        if p == end {
            return 0.0;
        }
        let (log2, rest) = match u[p] {
            c if c == b'x' as u16 || c == b'X' as u16 => (4, p + 1),
            c if c == b'o' as u16 || c == b'O' as u16 => (3, p + 1),
            c if c == b'b' as u16 || c == b'B' as u16 => (1, p + 1),
            _ => (0, 0),
        };
        if log2 != 0 {
            return if rest == end {
                f64::NAN
            } else {
                int_double(&u[rest..], log2, false)
            };
        }
    }
    match scan_decimal(u, cur) {
        Some((parsed, s)) => {
            if skip_space(u, parsed) == end {
                s.parse().unwrap_or(f64::NAN)
            } else {
                f64::NAN
            }
        }
        None => infinity_or_nan(u, cur, end, false),
    }
}

/// `parseFloat`: the StrDecimalLiteral prefix is parsed; anything else yields
/// NaN, with `±Infinity` accepted.
pub fn parse_float(u: &[u16]) -> f64 {
    let cur = skip_space(u, 0);
    if cur == u.len() {
        return f64::NAN;
    }
    if let Some((_, s)) = scan_decimal(u, cur) {
        return s.parse().unwrap_or(f64::NAN);
    }
    infinity_or_nan(u, cur, u.len(), true)
}

fn infinity_or_nan(u: &[u16], cur: usize, end: usize, allow_junk: bool) -> f64 {
    let mut cur = cur;
    let negative = match u.get(cur) {
        Some(&c) if c == b'+' as u16 => {
            cur += 1;
            false
        }
        Some(&c) if c == b'-' as u16 => {
            cur += 1;
            true
        }
        Some(&c) if c == b'I' as u16 => false,
        _ => return f64::NAN,
    };
    if cur == end || u[cur] != b'I' as u16 {
        return f64::NAN;
    }
    if !matches_substr(u, cur, b"Infinity") {
        return f64::NAN;
    }
    cur += 8;
    if !allow_junk && skip_space(u, cur) != end {
        return f64::NAN;
    }
    if negative {
        f64::NEG_INFINITY
    } else {
        f64::INFINITY
    }
}

fn matches_substr(u: &[u16], start: usize, lit: &[u8]) -> bool {
    start + lit.len() <= u.len()
        && u[start..start + lit.len()]
            .iter()
            .zip(lit)
            .all(|(&c, &l)| c == l as u16)
}

/// Scans a `StrDecimalLiteral` prefix the way V8's fast_float usage does:
/// `[+-]?(digits[.digits*]?|.digits+)([eE][+-]?digits+)?`, backtracking an
/// exponent without digits. Returns the end index and a normalized string
/// suitable for a correctly-rounded decimal-to-binary parse.
fn scan_decimal(u: &[u16], start: usize) -> Option<(usize, String)> {
    let mut i = start;
    let mut s = String::new();
    match u.get(i) {
        Some(&c) if c == b'-' as u16 => {
            s.push('-');
            i += 1;
        }
        Some(&c) if c == b'+' as u16 => {
            i += 1;
        }
        _ => {}
    }
    let int_start = i;
    while u
        .get(i)
        .is_some_and(|&c| (b'0' as u16..=b'9' as u16).contains(&c))
    {
        i += 1;
    }
    let int = &u[int_start..i];
    let mut frac: &[u16] = &[];
    if u.get(i) == Some(&(b'.' as u16)) {
        i += 1;
        let fs = i;
        while u
            .get(i)
            .is_some_and(|&c| (b'0' as u16..=b'9' as u16).contains(&c))
        {
            i += 1;
        }
        if int.is_empty() && i == fs {
            return None;
        }
        frac = &u[fs..i];
    } else if int.is_empty() {
        return None;
    }
    if int.is_empty() {
        s.push('0');
    } else {
        s.extend(int.iter().map(|&c| c as u8 as char));
    }
    s.push('.');
    if frac.is_empty() {
        s.push('0');
    } else {
        s.extend(frac.iter().map(|&c| c as u8 as char));
    }
    if matches!(u.get(i), Some(&c) if c == b'e' as u16 || c == b'E' as u16) {
        let mut j = i + 1;
        let exp_sign = match u.get(j) {
            Some(&c) if c == b'-' as u16 => {
                j += 1;
                "-"
            }
            Some(&c) if c == b'+' as u16 => {
                j += 1;
                ""
            }
            _ => "",
        };
        let ds = j;
        while u
            .get(j)
            .is_some_and(|&c| (b'0' as u16..=b'9' as u16).contains(&c))
        {
            j += 1;
        }
        if j > ds {
            s.push('e');
            s.push_str(exp_sign);
            s.extend(u[ds..j].iter().map(|&c| c as u8 as char));
            i = j;
        }
    }
    Some((i, s))
}

/// `parseInt`. `radix_arg` is the radix after ToNumber; the conversion to
/// int32 happens here as in `Runtime_StringParseInt`.
pub fn parse_int(u: &[u16], radix_arg: f64) -> f64 {
    let radix32 = to_int32(radix_arg);
    if radix32 != 0 && !(2..=36).contains(&radix32) {
        return f64::NAN;
    }
    let end = u.len();
    let mut cur = skip_space(u, 0);
    if cur == end {
        return f64::NAN;
    }
    let mut negative = false;
    match u[cur] {
        c if c == b'+' as u16 => {
            cur += 1;
            if cur == end {
                return f64::NAN;
            }
        }
        c if c == b'-' as u16 => {
            cur += 1;
            if cur == end {
                return f64::NAN;
            }
            negative = true;
        }
        _ => {}
    }
    let mut radix = radix32;
    let mut leading_zero = false;
    if radix == 0 {
        radix = 10;
        if u[cur] == b'0' as u16 {
            cur += 1;
            if cur == end {
                return signed_zero(negative);
            }
            if u[cur] == b'x' as u16 || u[cur] == b'X' as u16 {
                radix = 16;
                cur += 1;
                if cur == end {
                    return f64::NAN;
                }
            } else {
                leading_zero = true;
            }
        }
    } else if radix == 16 && u[cur] == b'0' as u16 {
        cur += 1;
        if cur == end {
            return signed_zero(negative);
        }
        if u[cur] == b'x' as u16 || u[cur] == b'X' as u16 {
            cur += 1;
            if cur == end {
                return f64::NAN;
            }
        } else {
            leading_zero = true;
        }
    }
    while u[cur] == b'0' as u16 {
        leading_zero = true;
        cur += 1;
        if cur == end {
            return signed_zero(negative);
        }
    }
    if leading_zero && digit(u[cur], radix).is_none() {
        return signed_zero(negative);
    }
    if !leading_zero && digit(u[cur], radix).is_none() {
        return f64::NAN;
    }
    let result = match radix {
        10 => base_ten(u, cur),
        2 | 4 | 8 | 16 | 32 => int_double(&u[cur..], radix.trailing_zeros(), true),
        _ => generic_int(u, cur, radix),
    };
    if negative { -result } else { result }
}

/// `ToIntegerOrInfinity`.
pub fn to_integer_or_infinity(x: f64) -> f64 {
    if x.is_nan() { 0.0 } else { x.trunc() }
}

/// `ToInt32`, used for the `parseInt` radix as `DoubleToInt32` does.
pub fn to_int32(x: f64) -> i32 {
    if !x.is_finite() || x == 0.0 {
        return 0;
    }
    const M: f64 = 4294967296.0;
    let t = x.trunc();
    let m = t - (t / M).floor() * M;
    if m >= 2147483648.0 {
        (m - M) as i32
    } else {
        m as i32
    }
}

/// Port of V8 `DoubleToRadixStringView` for finite nonzero values.
fn radix_string(value: f64, radix: i32) -> String {
    let radix_f = radix as f64;
    let mut buffer = [0u8; 2200];
    let mid = buffer.len() / 2;
    let mut integer_cursor = mid;
    let mut fraction_cursor = mid;

    let negative = value < 0.0;
    let value = value.abs();
    let mut integer = value.floor();
    let mut fraction = value - integer;
    let mut delta = 0.5 * (f64::from_bits(value.to_bits() + 1) - value);
    if delta <= 0.0 {
        delta = f64::from_bits(1);
    }

    if fraction >= delta {
        buffer[fraction_cursor] = b'.';
        fraction_cursor += 1;
        loop {
            fraction *= radix_f;
            delta *= radix_f;
            let digit = fraction as usize;
            buffer[fraction_cursor] = RADIX_CHARS[digit];
            fraction_cursor += 1;
            fraction -= digit as f64;
            if (fraction > 0.5 || (fraction == 0.5 && (digit & 1) != 0)) && fraction + delta > 1.0 {
                loop {
                    fraction_cursor -= 1;
                    if fraction_cursor == mid {
                        integer += 1.0;
                        break;
                    }
                    let c = buffer[fraction_cursor];
                    let d = if c > b'9' { c - b'a' + 10 } else { c - b'0' };
                    if d + 1 < radix as u8 {
                        buffer[fraction_cursor] = RADIX_CHARS[(d + 1) as usize];
                        fraction_cursor += 1;
                        break;
                    }
                }
                break;
            }
            if fraction < delta {
                break;
            }
        }
    }

    while dbl_exponent(integer / radix_f) > 0 {
        integer /= radix_f;
        integer_cursor -= 1;
        buffer[integer_cursor] = b'0';
    }
    loop {
        let remainder = integer % radix_f;
        integer_cursor -= 1;
        buffer[integer_cursor] = RADIX_CHARS[remainder as usize];
        integer = (integer - remainder) / radix_f;
        if integer <= 0.0 {
            break;
        }
    }

    if negative {
        integer_cursor -= 1;
        buffer[integer_cursor] = b'-';
    }
    String::from_utf8_lossy(&buffer[integer_cursor..fraction_cursor]).into_owned()
}

fn dbl_exponent(x: f64) -> i32 {
    let e = ((x.to_bits() >> 52) & 0x7ff) as i32;
    if e == 0 { -1074 } else { e - 1075 }
}

/// Shortest round-trip digits of a finite nonzero |x|, normalized as
/// `0.digits * 10^point` with no leading or trailing zeros.
fn shortest_digits(x: f64) -> (Vec<u8>, i32) {
    if x == 0.0 {
        return (vec![b'0'], 1);
    }
    let mut buf = ryu_js::Buffer::new();
    let s = buf.format_finite(x);
    let b = s.as_bytes();
    let mut i = 0usize;
    if b.first() == Some(&b'-') {
        i = 1;
    }
    let mut digits = Vec::with_capacity(b.len());
    let mut point = 0i32;
    while i < b.len() && b[i].is_ascii_digit() {
        digits.push(b[i]);
        point += 1;
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            digits.push(b[i]);
            i += 1;
        }
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        let neg = if i < b.len() && b[i] == b'-' {
            i += 1;
            true
        } else {
            if i < b.len() && b[i] == b'+' {
                i += 1;
            }
            false
        };
        let mut exp = 0i32;
        while i < b.len() && b[i].is_ascii_digit() {
            exp = exp * 10 + (b[i] - b'0') as i32;
            i += 1;
        }
        point += if neg { -exp } else { exp };
    }
    let leading = digits.iter().take_while(|&&d| d == b'0').count();
    digits.drain(..leading);
    point -= leading as i32;
    while digits.last() == Some(&b'0') {
        digits.pop();
    }
    if digits.is_empty() {
        (vec![b'0'], 1)
    } else {
        (digits, point)
    }
}

/// Exact decimal digits of a finite nonzero |x| as `0.digits * 10^point`,
/// with trailing zeros stripped.
fn exact_digits(x: f64) -> (Vec<u8>, i32) {
    debug_assert!(x.is_finite() && x >= 0.0);
    let bits = x.to_bits();
    let exp_field = ((bits >> 52) & 0x7ff) as i32;
    let (mantissa, e) = if exp_field == 0 {
        (bits & ((1 << 52) - 1), -1074i32)
    } else {
        (bits & ((1 << 52) - 1) | (1 << 52), exp_field - 1075)
    };
    if mantissa == 0 {
        return (vec![b'0'], 1);
    }
    let mut n = BigUint::from_u64(mantissa);
    let (mut digits, point) = if e >= 0 {
        n.shl(e as u32);
        let digits = n.to_decimal();
        let point = digits.len() as i32;
        (digits, point)
    } else {
        n.mul_pow5((-e) as u32);
        let digits = n.to_decimal();
        let point = digits.len() as i32 + e;
        (digits, point)
    };
    while digits.last() == Some(&b'0') {
        digits.pop();
    }
    (digits, point)
}

/// Rounds `0.digits * 10^point` to `keep` digits, picking the larger value on
/// an exact tie. `keep <= 0` yields an empty digit vector (zero).
fn round_at(digits: &[u8], point: i32, keep: i32) -> (Vec<u8>, i32) {
    if keep >= digits.len() as i32 {
        return (digits.to_vec(), point);
    }
    if keep < 0 {
        return (Vec::new(), point);
    }
    let mut out: Vec<u8> = digits[..keep as usize].to_vec();
    let mut point = point;
    if digits[keep as usize] >= b'5' {
        match out.iter().rposition(|&d| d != b'9') {
            Some(i) => {
                out[i] += 1;
                out.truncate(i + 1);
            }
            None => {
                out = vec![b'1'];
                point += 1;
            }
        }
    } else {
        while out.last() == Some(&b'0') {
            out.pop();
        }
    }
    (out, point)
}

fn digit_or_zero(digits: &[u8], i: i32) -> char {
    if i < 0 || i as usize >= digits.len() {
        '0'
    } else {
        digits[i as usize] as char
    }
}

fn format_fixed(digits: &[u8], point: i32, f: i32, negative: bool) -> String {
    let mut s = String::new();
    if negative {
        s.push('-');
    }
    if point <= 0 {
        s.push('0');
    } else {
        for i in 0..point {
            s.push(digit_or_zero(digits, i));
        }
    }
    if f > 0 {
        s.push('.');
        for j in 0..f {
            s.push(digit_or_zero(digits, point + j));
        }
    }
    s
}

fn format_exponential(digits: &[u8], exponent: i32, significant: i32, negative: bool) -> String {
    let mut s = String::new();
    if negative {
        s.push('-');
    }
    s.push(digit_or_zero(digits, 0));
    if significant != 1 {
        s.push('.');
        for i in 1..significant {
            s.push(digit_or_zero(digits, i));
        }
    }
    s.push('e');
    if exponent < 0 {
        s.push('-');
    } else {
        s.push('+');
    }
    s.push_str(&exponent.unsigned_abs().to_string());
    s
}

fn format_precision(digits: &[u8], point: i32, p: i32, negative: bool) -> String {
    let e = point - 1;
    if e < -6 || e >= p {
        return format_exponential(digits, e, p, negative);
    }
    let mut s = String::new();
    if negative {
        s.push('-');
    }
    if e == p - 1 {
        for i in 0..p {
            s.push(digit_or_zero(digits, i));
        }
    } else if e >= 0 {
        for i in 0..=e {
            s.push(digit_or_zero(digits, i));
        }
        s.push('.');
        for i in (e + 1)..p {
            s.push(digit_or_zero(digits, i));
        }
    } else {
        s.push_str("0.");
        for _ in 0..(-e - 1) {
            s.push('0');
        }
        for i in 0..p {
            s.push(digit_or_zero(digits, i));
        }
    }
    s
}
