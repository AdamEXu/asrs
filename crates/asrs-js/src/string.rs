//! JS string semantics: `JsString` is a sequence of UTF-16 code units, so lone
//! surrogates behave exactly as they do in JS. Case mapping comes from tables
//! generated from the oracle (ICU 74.2), not from Rust's Unicode tables.

pub mod case_table;
pub mod encoding;
pub mod uri;

pub use crate::math::int::{to_uint16, to_uint32};
pub use crate::number::to_integer_or_infinity;
use crate::number::to_string as number_to_string;

use case_table::{CASE_IGNORABLE, CASED, LOWER, UPPER};

pub use encoding::{DecodeError, TextDecoder, text_encode};
pub use uri::{UriError, decode_uri_component, encode_uri_component};

use std::fmt;

/// V8's `String::kMaxLength` on 64-bit (2^29 - 24): producing a longer string is a
/// RangeError "Invalid string length".
pub const MAX_STRING_LENGTH: usize = 536870888;

/// A JS string: a buffer of UTF-16 code units. Lone surrogates are preserved.
#[derive(Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JsString(Vec<u16>);

/// A thrown JS error value (the `name` and `message` that CVE-1 encodes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsError {
    pub name: &'static str,
    pub message: String,
}

impl JsError {
    pub fn new(name: &'static str, message: impl Into<String>) -> Self {
        JsError {
            name,
            message: message.into(),
        }
    }

    pub fn range(message: impl Into<String>) -> Self {
        JsError::new("RangeError", message)
    }

    pub fn type_error(message: impl Into<String>) -> Self {
        JsError::new("TypeError", message)
    }
}

impl JsString {
    pub fn new() -> Self {
        JsString(Vec::new())
    }

    pub fn from_units(units: Vec<u16>) -> Self {
        JsString(units)
    }

    /// The UTF-16 code units. This is `length` in JS terms.
    pub fn units(&self) -> &[u16] {
        &self.0
    }

    pub fn into_units(self) -> Vec<u16> {
        self.0
    }

    /// `String.prototype.length`: the number of UTF-16 code units.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn push_unit(&mut self, unit: u16) {
        self.0.push(unit);
    }

    pub fn push_str(&mut self, s: &JsString) {
        self.0.extend_from_slice(&s.0);
    }

    pub fn as_string(&self) -> Option<String> {
        String::from_utf16(&self.0).ok()
    }

    pub fn to_string_lossy(&self) -> String {
        String::from_utf16_lossy(&self.0)
    }

    /// `String.prototype.charCodeAt`: the unit at `pos`, or `None` out of bounds.
    pub fn char_code_at(&self, pos: f64) -> Option<u16> {
        let i = to_integer_or_infinity(pos);
        if i >= 0.0 && i < self.0.len() as f64 {
            Some(self.0[i as usize])
        } else {
            None
        }
    }

    /// `String.prototype.charAt`: the unit at `pos` as a string, or `""`.
    pub fn char_at(&self, pos: f64) -> JsString {
        match self.char_code_at(pos) {
            Some(unit) => JsString::from_units(vec![unit]),
            None => JsString::new(),
        }
    }

    /// `String.prototype.at`: negative `pos` counts from the end; `None` out of bounds.
    pub fn at(&self, pos: f64) -> Option<JsString> {
        let mut i = to_integer_or_infinity(pos);
        let len = self.0.len() as f64;
        if i < 0.0 {
            i += len;
        }
        if i >= 0.0 && i < len {
            Some(JsString::from_units(vec![self.0[i as usize]]))
        } else {
            None
        }
    }

    /// `String.prototype.codePointAt`: the code point at `pos`, or `None`.
    pub fn code_point_at(&self, pos: f64) -> Option<u32> {
        let i = to_integer_or_infinity(pos);
        if i < 0.0 || i >= self.0.len() as f64 {
            return None;
        }
        let i = i as usize;
        let first = self.0[i];
        if is_high_surrogate(first) {
            match self.0.get(i + 1) {
                Some(&second) if is_low_surrogate(second) => {
                    Some(combine_surrogates(first, second))
                }
                _ => Some(u32::from(first)),
            }
        } else {
            Some(u32::from(first))
        }
    }

    /// `String.prototype.slice(start, end)`. Negative indices count from the end.
    pub fn slice(&self, start: f64, end: Option<f64>) -> JsString {
        let len = self.0.len() as f64;
        let clamp = |x: f64| -> usize {
            let i = to_integer_or_infinity(x);
            if i < 0.0 {
                (len + i).max(0.0) as usize
            } else {
                i.min(len) as usize
            }
        };
        let start = clamp(start);
        let end = clamp(end.unwrap_or(f64::INFINITY)).max(start);
        JsString::from_units(self.0[start..end].to_vec())
    }

    /// `String.prototype.substring(start, end)`: clamps to `[0, len]` and swaps.
    pub fn substring(&self, start: f64, end: Option<f64>) -> JsString {
        let len = self.0.len() as f64;
        let clamp = |x: f64| to_integer_or_infinity(x).clamp(0.0, len) as usize;
        let mut start = clamp(start);
        let mut end = end.map_or(self.0.len(), clamp);
        if start > end {
            std::mem::swap(&mut start, &mut end);
        }
        JsString::from_units(self.0[start..end].to_vec())
    }

    /// `String.prototype.indexOf(search, position)`: `-1` when not found.
    pub fn index_of(&self, search: &JsString, position: Option<f64>) -> i64 {
        let len = self.0.len();
        let pos =
            to_integer_or_infinity(position.unwrap_or(f64::NAN)).clamp(0.0, len as f64) as usize;
        if search.0.is_empty() {
            return pos as i64;
        }
        if search.0.len() > len {
            return -1;
        }
        for i in pos..=(len - search.0.len()) {
            if self.0[i..i + search.0.len()] == search.0[..] {
                return i as i64;
            }
        }
        -1
    }

    /// `String.prototype.lastIndexOf(search, position)`: `position` NaN/undefined means
    /// +Infinity; the result is the largest start index `<=` clamped position.
    pub fn last_index_of(&self, search: &JsString, position: Option<f64>) -> i64 {
        let len = self.0.len();
        let raw = position.unwrap_or(f64::NAN);
        let pos = if raw.is_nan() {
            f64::INFINITY
        } else {
            to_integer_or_infinity(raw)
        };
        let pos = pos.clamp(0.0, len as f64) as usize;
        if search.0.is_empty() {
            return pos as i64;
        }
        if search.0.len() > len {
            return -1;
        }
        let first = pos.min(len - search.0.len());
        for i in (0..=first).rev() {
            if self.0[i..i + search.0.len()] == search.0[..] {
                return i as i64;
            }
        }
        -1
    }

    /// `String.prototype.includes(search, position)`.
    pub fn includes(&self, search: &JsString, position: Option<f64>) -> bool {
        self.index_of(search, position) >= 0
    }

    /// `String.prototype.startsWith(search, position)`.
    pub fn starts_with(&self, search: &JsString, position: Option<f64>) -> bool {
        let len = self.0.len();
        let start = to_integer_or_infinity(position.unwrap_or(0.0)).clamp(0.0, len as f64) as usize;
        start + search.0.len() <= len && self.0[start..start + search.0.len()] == search.0[..]
    }

    /// `String.prototype.endsWith(search, position)`: `position` bounds the end index.
    pub fn ends_with(&self, search: &JsString, position: Option<f64>) -> bool {
        let len = self.0.len();
        let end =
            to_integer_or_infinity(position.unwrap_or(len as f64)).clamp(0.0, len as f64) as usize;
        search.0.len() <= end && self.0[end - search.0.len()..end] == search.0[..]
    }

    /// `String.prototype.split(separator, limit)`: `None` separator wraps the whole
    /// string; `None` limit is 2^32 - 1. Splits on code units.
    pub fn split(&self, separator: Option<&JsString>, limit: Option<f64>) -> Vec<JsString> {
        let lim = match limit {
            None => u32::MAX,
            Some(x) => to_uint32(x),
        } as usize;
        if lim == 0 {
            return Vec::new();
        }
        let Some(sep) = separator else {
            return vec![self.clone()];
        };
        if self.0.is_empty() {
            return if sep.0.is_empty() {
                Vec::new()
            } else {
                vec![self.clone()]
            };
        }
        let mut out = Vec::new();
        if sep.0.is_empty() {
            for &u in self.0.iter().take(lim) {
                out.push(JsString::from_units(vec![u]));
            }
            return out;
        }
        let mut p = 0usize;
        while p < self.0.len() {
            let mut q = p;
            let mut found = false;
            while q + sep.0.len() <= self.0.len() {
                if self.0[q..q + sep.0.len()] == sep.0[..] {
                    found = true;
                    break;
                }
                q += 1;
            }
            if !found {
                break;
            }
            out.push(JsString::from_units(self.0[p..q].to_vec()));
            if out.len() == lim {
                return out;
            }
            p = q + sep.0.len();
        }
        out.push(JsString::from_units(self.0[p..].to_vec()));
        out.truncate(lim);
        out
    }

    /// `String.prototype.padStart(maxLength, fillString)`: `None` fill is `" "`.
    /// The fill is repeated and truncated by code units, which can split a
    /// surrogate pair.
    pub fn pad_start(&self, max_len: f64, fill: Option<&JsString>) -> Result<JsString, JsError> {
        self.pad(max_len, fill, true)
    }

    /// `String.prototype.padEnd(maxLength, fillString)`.
    pub fn pad_end(&self, max_len: f64, fill: Option<&JsString>) -> Result<JsString, JsError> {
        self.pad(max_len, fill, false)
    }

    fn pad(
        &self,
        max_len: f64,
        fill: Option<&JsString>,
        at_start: bool,
    ) -> Result<JsString, JsError> {
        let target = to_length(max_len) as usize;
        if target <= self.0.len() {
            return Ok(self.clone());
        }
        let space = JsString::from_units(vec![0x20]);
        let filler = fill.unwrap_or(&space);
        if filler.0.is_empty() {
            return Ok(self.clone());
        }
        if target > MAX_STRING_LENGTH {
            return Err(JsError::range("Invalid string length"));
        }
        let needed = target - self.0.len();
        let mut padding = Vec::with_capacity(needed);
        while padding.len() < needed {
            for &u in &filler.0 {
                if padding.len() < needed {
                    padding.push(u);
                }
            }
        }
        let mut out = Vec::with_capacity(target);
        if at_start {
            out.extend_from_slice(&padding);
            out.extend_from_slice(&self.0);
        } else {
            out.extend_from_slice(&self.0);
            out.extend_from_slice(&padding);
        }
        Ok(JsString::from_units(out))
    }

    /// `String.prototype.repeat(count)`. V8 checks `ToIntegerOrInfinity(count)` for the
    /// range error but formats the original argument in the message.
    pub fn repeat(&self, count: f64) -> Result<JsString, JsError> {
        let n = to_integer_or_infinity(count);
        if n < 0.0 || n == f64::INFINITY {
            return Err(JsError::range(format!(
                "Invalid count value: {}",
                number_to_string(count)
            )));
        }
        let n = n as usize;
        if self.0.is_empty() || n == 0 {
            return Ok(JsString::new());
        }
        if n > MAX_STRING_LENGTH / self.0.len() {
            return Err(JsError::range("Invalid string length"));
        }
        let mut out = Vec::with_capacity(self.0.len() * n);
        for _ in 0..n {
            out.extend_from_slice(&self.0);
        }
        Ok(JsString::from_units(out))
    }

    /// `String.prototype.trim` on the JS whitespace set.
    pub fn trim(&self) -> JsString {
        self.trim_start().trim_end()
    }

    pub fn trim_start(&self) -> JsString {
        let start = self
            .0
            .iter()
            .position(|&u| !is_js_whitespace(u))
            .unwrap_or(self.0.len());
        JsString::from_units(self.0[start..].to_vec())
    }

    pub fn trim_end(&self) -> JsString {
        let end = self
            .0
            .iter()
            .rposition(|&u| !is_js_whitespace(u))
            .map_or(0, |i| i + 1);
        JsString::from_units(self.0[..end].to_vec())
    }

    /// `String.prototype.toLowerCase`: Unicode default case conversion, including the
    /// context-sensitive final sigma (U+03A3 -> U+03C2 at a cased-word end).
    pub fn to_lower_case(&self) -> JsString {
        let cps: Vec<(u32, usize)> = self.code_points().collect();
        let mut out = Vec::with_capacity(self.0.len());
        for (k, &(cp, _)) in cps.iter().enumerate() {
            if cp == 0x3a3 && is_final_sigma(&cps, k) {
                out.push(0x3c2);
            } else {
                push_mapped(cp, LOWER, &mut out);
            }
        }
        JsString::from_units(out)
    }

    /// `String.prototype.toUpperCase`.
    pub fn to_upper_case(&self) -> JsString {
        let mut out = Vec::with_capacity(self.0.len());
        for (cp, _) in self.code_points() {
            push_mapped(cp, UPPER, &mut out);
        }
        JsString::from_units(out)
    }

    /// Code points with their starting unit index. Lone surrogates yield themselves.
    fn code_points(&self) -> impl Iterator<Item = (u32, usize)> + '_ {
        let mut i = 0;
        std::iter::from_fn(move || {
            if i >= self.0.len() {
                return None;
            }
            let start = i;
            let first = self.0[i];
            i += 1;
            if is_high_surrogate(first)
                && let Some(&second) = self.0.get(i)
                && is_low_surrogate(second)
            {
                i += 1;
                return Some((combine_surrogates(first, second), start));
            }
            Some((u32::from(first), start))
        })
    }

    /// `String.fromCharCode`: each argument through `ToUint16`.
    pub fn from_char_code(args: &[f64]) -> JsString {
        JsString::from_units(args.iter().map(|&x| to_uint16(x)).collect())
    }

    /// `String.fromCodePoint`: RangeError "Invalid code point <arg>" for a non-integer
    /// or out-of-range argument.
    pub fn from_code_point(args: &[f64]) -> Result<JsString, JsError> {
        let mut units = Vec::with_capacity(args.len());
        for &x in args {
            if !x.is_finite() || x.fract() != 0.0 || !(0.0..=0x10ffff as f64).contains(&x) {
                return Err(JsError::range(format!(
                    "Invalid code point {}",
                    number_to_string(x)
                )));
            }
            let cp = x as u32;
            if cp <= 0xffff {
                units.push(cp as u16);
            } else {
                let v = cp - 0x10000;
                units.push(0xd800 + (v >> 10) as u16);
                units.push(0xdc00 + (v & 0x3ff) as u16);
            }
        }
        Ok(JsString::from_units(units))
    }
}

impl fmt::Debug for JsString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.to_string_lossy())
    }
}

impl fmt::Display for JsString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_lossy())
    }
}

impl From<&str> for JsString {
    fn from(s: &str) -> Self {
        JsString(s.encode_utf16().collect())
    }
}

impl From<String> for JsString {
    fn from(s: String) -> Self {
        JsString::from(s.as_str())
    }
}

impl From<&String> for JsString {
    fn from(s: &String) -> Self {
        JsString::from(s.as_str())
    }
}

impl From<Vec<u16>> for JsString {
    fn from(units: Vec<u16>) -> Self {
        JsString::from_units(units)
    }
}

fn is_high_surrogate(u: u16) -> bool {
    (0xd800..0xdc00).contains(&u)
}

fn is_low_surrogate(u: u16) -> bool {
    (0xdc00..0xe000).contains(&u)
}

fn combine_surrogates(high: u16, low: u16) -> u32 {
    0x10000 + ((u32::from(high) - 0xd800) << 10) + (u32::from(low) - 0xdc00)
}

/// ECMAScript `ToLength`: `ToIntegerOrInfinity` clamped to `[0, 2^53 - 1]`.
pub fn to_length(x: f64) -> f64 {
    to_integer_or_infinity(x).clamp(0.0, 9007199254740991.0)
}

fn is_js_whitespace(u: u16) -> bool {
    matches!(
        u,
        0x0009 | 0x000a | 0x000b | 0x000c | 0x000d | 0x0020 | 0x00a0 | 0x1680 | 0x2000
            ..=0x200a | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000 | 0xfeff
    )
}

fn push_mapped(cp: u32, table: &[(u32, &[u16])], out: &mut Vec<u16>) {
    match table.binary_search_by_key(&cp, |&(k, _)| k) {
        Ok(i) => out.extend_from_slice(table[i].1),
        Err(_) => push_code_point(cp, out),
    }
}

pub(crate) fn push_code_point(cp: u32, out: &mut Vec<u16>) {
    if cp <= 0xffff {
        out.push(cp as u16);
    } else {
        let v = cp - 0x10000;
        out.push(0xd800 + (v >> 10) as u16);
        out.push(0xdc00 + (v & 0x3ff) as u16);
    }
}

fn in_ranges(cp: u32, ranges: &[(u32, u32)]) -> bool {
    ranges
        .binary_search_by(|&(lo, hi)| {
            if cp < lo {
                std::cmp::Ordering::Greater
            } else if cp > hi {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// The Final_Sigma context: a cased code point precedes (skipping case-ignorable) and
/// no cased code point follows (skipping case-ignorable).
fn is_final_sigma(cps: &[(u32, usize)], index: usize) -> bool {
    let mut preceded = false;
    for &(cp, _) in cps[..index].iter().rev() {
        if in_ranges(cp, CASE_IGNORABLE) {
            continue;
        }
        preceded = in_ranges(cp, CASED);
        break;
    }
    if !preceded {
        return false;
    }
    for &(cp, _) in &cps[index + 1..] {
        if in_ranges(cp, CASE_IGNORABLE) {
            continue;
        }
        return !in_ranges(cp, CASED);
    }
    true
}

/// `shared/util.ts` `htmlEncode`: replaces `<`, `>`, `&` and every code unit in
/// U+00A0..=U+9999 with `&#<decimal>;`. Surrogate code units (U+D800+) pass through.
pub fn html_encode(text: &JsString) -> JsString {
    let mut out = Vec::with_capacity(text.0.len());
    for &u in &text.0 {
        if (0xa0..=0x9999).contains(&u) || matches!(u, 0x3c | 0x3e | 0x26) {
            out.extend(format!("&#{u};").encode_utf16());
        } else {
            out.push(u);
        }
    }
    JsString::from_units(out)
}
