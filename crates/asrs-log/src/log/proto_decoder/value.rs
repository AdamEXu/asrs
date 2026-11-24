// Ported from protobufjs 8.3.0 (BSD-3-Clause, Copyright (c) 2016 Daniel Wirtz). See THIRD_PARTY_NOTICES.md.

//! The JS values protobufjs works with: field defaults, `Long` instances and decoded
//! message instances.

use asrs_js::math;
use asrs_js::number::{self, to_int32};
use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue};

use super::reflect::{NodeId, js};

/// A field's `typeDefault` or `defaultValue`.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum JsDefault {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(JsString),
    Bytes(Vec<u8>),
    /// `util.emptyArray`, a frozen plain array.
    EmptyArray,
    /// `util.emptyObject`.
    EmptyObject,
    Long(Long),
    /// A function reached through `Object.prototype` (an enum default named `toString`), with
    /// its `name`.
    Function(JsString),
    /// An enum's `valuesById`, reached as `values.__proto__`.
    Object(JsObject<JsString>),
}

impl JsDefault {
    pub fn is_nullish(&self) -> bool {
        matches!(self, JsDefault::Undefined | JsDefault::Null)
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            JsDefault::Undefined | JsDefault::Null => false,
            JsDefault::Bool(b) => *b,
            JsDefault::Number(x) => *x != 0.0 && !x.is_nan(),
            JsDefault::String(s) => !s.is_empty(),
            _ => true,
        }
    }

    fn to_number(&self) -> f64 {
        match self {
            JsDefault::Undefined => f64::NAN,
            JsDefault::Null => 0.0,
            JsDefault::Bool(b) => f64::from(u8::from(*b)),
            JsDefault::Number(x) => *x,
            JsDefault::String(s) => number::to_number(s.units()),
            _ => f64::NAN,
        }
    }

    /// The value `d.x = <JSON.stringify(typeDefault)>` assigns in generated `toObject` code.
    pub fn json_literal(&self) -> JsValue {
        match self {
            JsDefault::Undefined | JsDefault::Function(_) => JsValue::Undefined,
            JsDefault::Null => JsValue::Null,
            JsDefault::Bool(b) => JsValue::Bool(*b),
            JsDefault::Number(x) if !x.is_finite() => JsValue::Null,
            JsDefault::Number(x) => JsValue::Number(if *x == 0.0 { 0.0 } else { *x }),
            JsDefault::String(s) => JsValue::String(s.clone()),
            JsDefault::Bytes(b) => JsValue::Bytes(b.clone()),
            JsDefault::EmptyArray => JsValue::Array(Vec::new()),
            JsDefault::EmptyObject => JsValue::Object(JsObject::new()),
            JsDefault::Long(l) => l.to_js(),
            JsDefault::Object(o) => JsValue::Object(
                o.iter()
                    .map(|(k, v)| (k.clone(), JsValue::String(v.clone())))
                    .collect(),
            ),
        }
    }
}

/// A `long` 5.2.3 `Long` instance: its own properties are exactly `low`, `high`, `unsigned`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Long {
    pub low: i32,
    pub high: i32,
    pub unsigned: bool,
}

const TWO_PWR_32: f64 = 4294967296.0;
const TWO_PWR_63: f64 = 9223372036854775808.0;
const TWO_PWR_64: f64 = 18446744073709551616.0;

impl Long {
    pub fn from_bits(lo: u32, hi: u32, unsigned: bool) -> Self {
        Long {
            low: lo as i32,
            high: hi as i32,
            unsigned,
        }
    }

    /// `Long.fromNumber(value, unsigned)` on the field default, with JS's coercions (including
    /// `value + 1` concatenating when the default is a string).
    pub fn from_default(value: &JsDefault, unsigned: bool) -> Self {
        let n = value.to_number();
        let plus_one = match value {
            JsDefault::String(s) => {
                let mut t = s.clone();
                t.push_unit(u16::from(b'1'));
                number::to_number(t.units())
            }
            _ => n + 1.0,
        };
        Self::from_number(n, plus_one, unsigned)
    }

    fn from_number(n: f64, plus_one: f64, unsigned: bool) -> Self {
        if n.is_nan() {
            return Long::from_bits(0, 0, unsigned);
        }
        if unsigned {
            if n < 0.0 {
                return Long::from_bits(0, 0, true);
            }
            if n >= TWO_PWR_64 {
                return Long::from_bits(u32::MAX, u32::MAX, true);
            }
        } else {
            if n <= -TWO_PWR_63 {
                return Long::from_bits(0, 0x8000_0000, false);
            }
            if plus_one >= TWO_PWR_63 {
                return Long::from_bits(u32::MAX, 0x7fff_ffff, false);
            }
        }
        if n < 0.0 {
            return Self::from_number(-n, -n + 1.0, unsigned).neg();
        }
        Long {
            low: to_int32(math::rem(n, TWO_PWR_32)),
            high: to_int32(n / TWO_PWR_32),
            unsigned,
        }
    }

    fn neg(self) -> Self {
        let v = self.as_i64().wrapping_neg();
        Long::from_bits(v as u32, (v >> 32) as u32, self.unsigned)
    }

    fn as_i64(self) -> i64 {
        (i64::from(self.high) << 32) | i64::from(self.low as u32)
    }

    /// `Long#toString()` in radix 10.
    pub fn to_decimal(self) -> JsString {
        if self.unsigned {
            js(&format!("{}", self.as_i64() as u64))
        } else {
            js(&format!("{}", self.as_i64()))
        }
    }

    pub fn to_js(self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("low", JsValue::Number(f64::from(self.low)));
        o.insert("high", JsValue::Number(f64::from(self.high)));
        o.insert("unsigned", JsValue::Bool(self.unsigned));
        JsValue::Object(o)
    }

    /// `util.longToHash`: the 8-character string protobufjs keys int64 map entries by.
    pub fn to_hash(self) -> JsString {
        let lo = self.low as u32;
        let hi = self.high as u32;
        JsString::from_units(
            [
                lo,
                lo >> 8,
                lo >> 16,
                lo >> 24,
                hi,
                hi >> 8,
                hi >> 16,
                hi >> 24,
            ]
            .iter()
            .map(|b| (b & 255) as u16)
            .collect(),
        )
    }
}

/// `util.longFromKey(key, unsigned).toString()`.
pub(super) fn long_key_to_string(key: &JsString, unsigned: bool) -> JsString {
    let u = key.units();
    let is_hash = u.len() == 8 && u.iter().all(|&c| c <= 0xff);
    if !is_hash || is_decimal_key(u) {
        return key.clone();
    }
    let word = |i: usize| -> u32 {
        u32::from(u[i])
            | u32::from(u[i + 1]) << 8
            | u32::from(u[i + 2]) << 16
            | u32::from(u[i + 3]) << 24
    };
    Long::from_bits(word(0), word(4), unsigned).to_decimal()
}

/// `/^-?(?:0|[1-9][0-9]*)$/`.
fn is_decimal_key(u: &[u16]) -> bool {
    let digits = match u.first() {
        Some(&c) if c == u16::from(b'-') => &u[1..],
        _ => u,
    };
    let is_digit = |c: &u16| (u16::from(b'0')..=u16::from(b'9')).contains(c);
    match digits {
        [] => false,
        [c] => is_digit(c),
        [first, rest @ ..] => {
            *first != u16::from(b'0') && is_digit(first) && rest.iter().all(is_digit)
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Val {
    Num(f64),
    Bool(bool),
    Str(JsString),
    Bytes(Vec<u8>),
    Long(Long),
    Msg(Box<Message>),
    Array(Vec<Val>),
    Map(JsObject<Val>),
}

impl Val {
    pub fn is_truthy(&self) -> bool {
        match self {
            Val::Num(x) => *x != 0.0 && !x.is_nan(),
            Val::Bool(b) => *b,
            Val::Str(s) => !s.is_empty(),
            _ => true,
        }
    }
}

/// A decoded message: its type and its own enumerable properties in JS order.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Message {
    pub ty: NodeId,
    pub own: JsObject<Val>,
    pub odd: Option<Box<Odd>>,
}

impl Message {
    pub fn new(ty: NodeId, own: JsObject<Val>) -> Self {
        Message { ty, own, odd: None }
    }

    /// The `m` a decoder handed a truthy non-message merge target writes to.
    pub fn onto(ty: NodeId, own: JsObject<Val>, target: Target) -> Self {
        Message {
            ty,
            own,
            odd: Some(Box::new(Odd {
                unknowns: None,
                target,
            })),
        }
    }

    pub fn target(&self) -> Target {
        self.odd.as_ref().map_or(Target::Message, |o| o.target)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Odd {
    /// The own non-enumerable `$unknowns` property `util.makeProp` defines.
    pub unknowns: Option<Val>,
    pub target: Target,
}

/// What `m` is: a message instance, or the truthy non-message value the decoder merges into.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Target {
    #[default]
    Message,
    /// A `$unknowns` array of this length; `own` holds its elements under their index keys.
    Array(usize),
    /// A string, number or boolean, on which property writes do nothing.
    Primitive,
}

/// A `bytes` field's string default: base64 when it looks like base64, UTF-8 otherwise.
pub(super) fn bytes_default(text: &JsString) -> Vec<u8> {
    let u = text.units();
    if is_base64(u) {
        let mut p = u.len();
        while p > 0 && u[p - 1] == u16::from(b'=') {
            p -= 1;
        }
        let length = p * 3 / 4;
        let mut out = base64_decode(u);
        out.resize(length, 0);
        out
    } else {
        utf8_write(u)
    }
}

fn b64_value(c: u16) -> Option<u8> {
    let c = u8::try_from(c).ok()?;
    Some(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' | b'-' => 62,
        b'/' | b'_' => 63,
        _ => return None,
    })
}

fn is_base64(u: &[u16]) -> bool {
    let std = |c: &u16| b64_value(*c).is_some() && *c != u16::from(b'-') && *c != u16::from(b'_');
    let url = |c: &u16| b64_value(*c).is_some() && *c != u16::from(b'+') && *c != u16::from(b'/');
    let eq = u16::from(b'=');
    let standard = u.len().is_multiple_of(4)
        && (u.is_empty() || {
            let (body, tail) = u.split_at(u.len() - 4);
            body.iter().all(std)
                && (tail.iter().all(std)
                    || (tail[..2].iter().all(std) && tail[2] == eq && tail[3] == eq)
                    || (tail[..3].iter().all(std) && tail[3] == eq))
        });
    if standard {
        return true;
    }
    if !u
        .iter()
        .any(|&c| c == u16::from(b'-') || c == u16::from(b'_'))
    {
        return false;
    }
    let tail_ok =
        |body: &[u16], rem: &[usize]| body.iter().all(url) && rem.contains(&(body.len() % 4));
    tail_ok(u, &[0, 2, 3])
        || (u.len() >= 2 && u.ends_with(&[eq, eq]) && tail_ok(&u[..u.len() - 2], &[2]))
        || (u.ends_with(&[eq]) && tail_ok(&u[..u.len() - 1], &[3]))
}

fn base64_decode(u: &[u16]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut j = 0;
    let mut t = 0u8;
    for &c in u {
        if c == u16::from(b'=') && j > 1 {
            break;
        }
        let Some(v) = b64_value(c) else { break };
        match j {
            0 => {
                t = v;
                j = 1;
            }
            1 => {
                out.push(t << 2 | (v & 48) >> 4);
                t = v;
                j = 2;
            }
            2 => {
                out.push((t & 15) << 4 | (v & 60) >> 2);
                t = v;
                j = 3;
            }
            _ => {
                out.push((t & 3) << 6 | v);
                j = 0;
            }
        }
    }
    out
}

/// `util.utf8.write`: lone surrogates encode as three bytes.
fn utf8_write(u: &[u16]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < u.len() {
        let c1 = u32::from(u[i]);
        if c1 < 128 {
            out.push(c1 as u8);
        } else if c1 < 2048 {
            out.push((c1 >> 6 | 192) as u8);
            out.push((c1 & 63 | 128) as u8);
        } else if c1 & 0xfc00 == 0xd800 && u.get(i + 1).is_some_and(|&c2| c2 & 0xfc00 == 0xdc00) {
            let c2 = u32::from(u[i + 1]);
            let c = 0x10000 + ((c1 & 0x3ff) << 10) + (c2 & 0x3ff);
            i += 1;
            out.push((c >> 18 | 240) as u8);
            out.push((c >> 12 & 63 | 128) as u8);
            out.push((c >> 6 & 63 | 128) as u8);
            out.push((c & 63 | 128) as u8);
        } else {
            out.push((c1 >> 12 | 224) as u8);
            out.push((c1 >> 6 & 63 | 128) as u8);
            out.push((c1 & 63 | 128) as u8);
        }
        i += 1;
    }
    out
}
