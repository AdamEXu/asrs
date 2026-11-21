//! `JSON.parse` and `JSON.stringify` as V8 14.2 implements them, including its SyntaxError
//! messages, plus upstream's `jsonCopy`.

mod iso_date;
mod parse;

use std::fmt;

use crate::number;
use crate::string::JsString;
use crate::value::JsValue;

pub use iso_date::to_iso_string;
pub use parse::{parse, parse_str, parse_units};

/// A thrown `SyntaxError` (from parsing) or `TypeError` (from stringifying a BigInt). The
/// message is a JS string because V8 quotes source code units, lone surrogates included.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonError {
    pub name: &'static str,
    pub message: JsString,
}

impl JsonError {
    fn syntax(message: JsString) -> Self {
        JsonError {
            name: "SyntaxError",
            message,
        }
    }
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.name, self.message)
    }
}

impl std::error::Error for JsonError {}

/// `JSON.stringify(value)`. `Ok(None)` is the `undefined` result for a top-level `undefined`.
/// The output is always well-formed: lone surrogates are written as `\uXXXX` escapes.
/// With no `replacer` or `space` argument V8 takes an iterative serializer, so any depth
/// succeeds.
pub fn stringify(value: &JsValue) -> Result<Option<String>, JsonError> {
    serialize(value, "", usize::MAX)
}

/// `JSON.stringify(value, null, 2)`. A defined argument puts V8 on its recursive
/// serializer, which throws `RangeError` past a depth limit.
pub fn stringify_pretty(value: &JsValue) -> Result<Option<String>, JsonError> {
    stringify_gap(value, "  ")
}

/// `JSON.stringify` after argument normalization. Callers pass `""` for a `space` that
/// reduced to nothing; V8 still takes its recursive serializer whenever `replacer` or
/// `space` was a defined value, so this entry stays depth-limited either way.
pub fn stringify_gap(value: &JsValue, gap: &str) -> Result<Option<String>, JsonError> {
    serialize(value, gap, MAX_SERIALIZE_DEPTH)
}

fn serialize(value: &JsValue, gap: &str, max_depth: usize) -> Result<Option<String>, JsonError> {
    let mut writer = Writer {
        out: String::new(),
        gap,
        indent: String::new(),
    };
    Ok(writer.serialize(value, max_depth)?.then_some(writer.out))
}

/// V8's general `JSON.stringify` path recurses per level and throws `RangeError` at a
/// stack-size-dependent depth (~6183 on the oracle). The port serializes iteratively but
/// reproduces the throw; the bound sits under the oracle's so vector input deep enough to
/// fail upstream fails here too.
const MAX_SERIALIZE_DEPTH: usize = 6000;

fn range_error() -> JsonError {
    JsonError {
        name: "RangeError",
        message: JsString::from("Maximum call stack size exceeded"),
    }
}

/// The gap for a numeric `space` argument: `min(10, ToIntegerOrInfinity(space))` spaces.
pub fn gap_from_number(space: f64) -> String {
    let n = number::to_integer_or_infinity(space);
    if n < 1.0 {
        String::new()
    } else if n >= 10.0 {
        " ".repeat(10)
    } else {
        " ".repeat(n as usize)
    }
}

/// The gap for a string `space` argument: its first 10 code units.
pub fn gap_from_string(space: &JsString) -> JsString {
    JsString::from_units(space.units()[..space.len().min(10)].to_vec())
}

/// Upstream's `jsonCopy`: `JSON.parse(JSON.stringify(value))`.
pub fn json_copy(value: &JsValue) -> Result<JsValue, JsonError> {
    match stringify(value)? {
        Some(text) => parse_str(&text),
        None => parse_str("undefined"),
    }
}

/// Appends `units` as a JSON string literal, escaped as well-formed `JSON.stringify` does.
pub fn quote_into(units: &[u16], out: &mut String) {
    out.push('"');
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        i += 1;
        match u {
            0x22 => out.push_str("\\\""),
            0x5c => out.push_str("\\\\"),
            0x08 => out.push_str("\\b"),
            0x0c => out.push_str("\\f"),
            0x0a => out.push_str("\\n"),
            0x0d => out.push_str("\\r"),
            0x09 => out.push_str("\\t"),
            0x00..0x20 => push_unit_escape(u, out),
            0xd800..0xdc00 if i < units.len() && (0xdc00..0xe000).contains(&units[i]) => {
                let cp = 0x10000 + ((u32::from(u) - 0xd800) << 10) + (u32::from(units[i]) - 0xdc00);
                i += 1;
                out.push(char::from_u32(cp).unwrap_or(char::REPLACEMENT_CHARACTER));
            }
            0xd800..0xe000 => push_unit_escape(u, out),
            _ => out.push(char::from_u32(u32::from(u)).unwrap_or(char::REPLACEMENT_CHARACTER)),
        }
    }
    out.push('"');
}

fn push_unit_escape(u: u16, out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push_str("\\u");
    for shift in [12, 8, 4, 0] {
        out.push(char::from(HEX[usize::from((u >> shift) & 0xf)]));
    }
}

type Members<'v> = Box<dyn Iterator<Item = (MemberKey<'v>, MemberValue<'v>)> + 'v>;

enum MemberKey<'v> {
    Ref(&'v [u16]),
    Owned(Vec<u16>),
}

/// An object member's value as the serializer sees it: a tree value, or a `Number`/`Bytes`
/// member `ExtData` and `Bytes` produce without a `JsValue`.
enum MemberValue<'v> {
    Value(&'v JsValue),
    Number(f64),
    Bytes(&'v [u8]),
}

enum Frame<'v> {
    /// Items still to write, and whether a child was dispatched (a `false` result writes
    /// `null`, as `JSON.stringify` does for `undefined` array elements).
    Array(std::iter::Enumerate<std::slice::Iter<'v, JsValue>>, bool),
    Object {
        members: Members<'v>,
        any: bool,
        /// `out` length before the `{`, for the `!any` `{}` rewrite.
        start: usize,
        /// `out` length where the member being serialized began, for `undefined` rollback.
        member_start: Option<usize>,
    },
}

/// The next thing to serialize: a value, or the bytes `{"0": b, ...}` serializes.
enum Target<'v> {
    Value(&'v JsValue),
    Bytes(&'v [u8]),
}

struct Writer<'a> {
    out: String,
    gap: &'a str,
    indent: String,
}

impl Writer<'_> {
    /// SerializeJSONProperty on a worklist. Returns false (writing nothing) when the value
    /// serializes to `undefined`.
    fn serialize(&mut self, root: &JsValue, max_depth: usize) -> Result<bool, JsonError> {
        let mut frames: Vec<Frame<'_>> = Vec::new();
        let mut target = Target::Value(root);
        'descend: loop {
            let mut wrote = loop {
                match target {
                    Target::Value(JsValue::Undefined) => break false,
                    Target::Value(JsValue::Null) => {
                        self.out.push_str("null");
                        break true;
                    }
                    Target::Value(JsValue::Bool(b)) => {
                        self.out.push_str(if *b { "true" } else { "false" });
                        break true;
                    }
                    Target::Value(JsValue::Number(x)) => {
                        let x = *x;
                        self.number(x);
                        break true;
                    }
                    Target::Value(JsValue::String(s)) => {
                        Self::quote_into(s.units(), &mut self.out);
                        break true;
                    }
                    Target::Value(JsValue::Array(items)) => {
                        if items.is_empty() {
                            self.out.push_str("[]");
                            break true;
                        }
                        if frames.len() >= max_depth {
                            return Err(range_error());
                        }
                        self.out.push('[');
                        self.indent.push_str(self.gap);
                        frames.push(Frame::Array(items.iter().enumerate(), false));
                        // `wrote` is ignored below: the fresh frame has nothing pending.
                        break true;
                    }
                    Target::Value(JsValue::Object(object)) => {
                        if frames.len() >= max_depth {
                            return Err(range_error());
                        }
                        let start = self.out.len();
                        self.out.push('{');
                        self.indent.push_str(self.gap);
                        let members: Members<'_> = Box::new(
                            object
                                .iter()
                                .map(|(k, v)| (MemberKey::Ref(k.units()), MemberValue::Value(v))),
                        );
                        frames.push(Frame::Object {
                            members,
                            any: false,
                            start,
                            member_start: None,
                        });
                        break true;
                    }
                    Target::Value(JsValue::Bytes(bytes)) => {
                        target = Target::Bytes(bytes);
                    }
                    Target::Bytes(bytes) => {
                        if bytes.is_empty() {
                            self.out.push_str("{}");
                            break true;
                        }
                        if frames.len() >= max_depth {
                            return Err(range_error());
                        }
                        let start = self.out.len();
                        self.out.push('{');
                        self.indent.push_str(self.gap);
                        let members: Members<'_> =
                            Box::new(bytes.iter().enumerate().map(|(i, b)| {
                                (
                                    MemberKey::Owned(
                                        number::to_string(i as f64).encode_utf16().collect(),
                                    ),
                                    MemberValue::Number(f64::from(*b)),
                                )
                            }));
                        frames.push(Frame::Object {
                            members,
                            any: false,
                            start,
                            member_start: None,
                        });
                        break true;
                    }
                    Target::Value(JsValue::BigInt(_)) => {
                        return Err(JsonError {
                            name: "TypeError",
                            message: JsString::from("Do not know how to serialize a BigInt"),
                        });
                    }
                    Target::Value(JsValue::Date(t)) => {
                        let t = *t;
                        if t.is_finite() {
                            Self::quote_into(
                                &to_iso_string(t).encode_utf16().collect::<Vec<u16>>(),
                                &mut self.out,
                            );
                        } else {
                            self.out.push_str("null");
                        }
                        break true;
                    }
                    Target::Value(JsValue::ExtData(kind, data)) => {
                        if frames.len() >= max_depth {
                            return Err(range_error());
                        }
                        let start = self.out.len();
                        self.out.push('{');
                        self.indent.push_str(self.gap);
                        let members: Members<'_> = Box::new(
                            [
                                (
                                    MemberKey::Owned(b"type".map(u16::from).to_vec()),
                                    MemberValue::Number(f64::from(*kind)),
                                ),
                                (
                                    MemberKey::Owned(b"data".map(u16::from).to_vec()),
                                    MemberValue::Bytes(data),
                                ),
                            ]
                            .into_iter(),
                        );
                        frames.push(Frame::Object {
                            members,
                            any: false,
                            start,
                            member_start: None,
                        });
                        break true;
                    }
                }
            };
            loop {
                match frames.last_mut() {
                    None => return Ok(wrote),
                    Some(Frame::Array(items, pending)) => {
                        if *pending && !wrote {
                            self.out.push_str("null");
                        }
                        match items.next() {
                            Some((i, item)) => {
                                if i > 0 {
                                    self.out.push(',');
                                }
                                if !self.gap.is_empty() {
                                    self.newline_indent();
                                }
                                *pending = true;
                                target = Target::Value(item);
                                continue 'descend;
                            }
                            None => {
                                frames.pop();
                                self.indent.truncate(self.indent.len() - self.gap.len());
                                if !self.gap.is_empty() {
                                    self.newline_indent();
                                }
                                self.out.push(']');
                                wrote = true;
                            }
                        }
                    }
                    Some(Frame::Object {
                        members,
                        any,
                        start,
                        member_start,
                    }) => {
                        if let Some(member_start) = member_start.take() {
                            if wrote {
                                *any = true;
                            } else {
                                self.out.truncate(member_start);
                            }
                        }
                        match members.next() {
                            Some((key, member)) => {
                                let ms = self.out.len();
                                if *any {
                                    self.out.push(',');
                                }
                                if !self.gap.is_empty() {
                                    self.newline_indent();
                                }
                                match key {
                                    MemberKey::Ref(units) => Self::quote_into(units, &mut self.out),
                                    MemberKey::Owned(units) => {
                                        Self::quote_into(&units, &mut self.out)
                                    }
                                }
                                self.out.push(':');
                                if !self.gap.is_empty() {
                                    self.out.push(' ');
                                }
                                match member {
                                    MemberValue::Number(x) => {
                                        self.number(x);
                                        *any = true;
                                        wrote = true;
                                    }
                                    MemberValue::Value(v) => {
                                        *member_start = Some(ms);
                                        target = Target::Value(v);
                                        continue 'descend;
                                    }
                                    MemberValue::Bytes(b) => {
                                        *member_start = Some(ms);
                                        target = Target::Bytes(b);
                                        continue 'descend;
                                    }
                                }
                            }
                            None => {
                                let (start, any) = (*start, *any);
                                frames.pop();
                                self.indent.truncate(self.indent.len() - self.gap.len());
                                if !any {
                                    self.out.truncate(start);
                                    self.out.push_str("{}");
                                } else {
                                    if !self.gap.is_empty() {
                                        self.newline_indent();
                                    }
                                    self.out.push('}');
                                }
                                wrote = true;
                            }
                        }
                    }
                }
            }
        }
    }

    fn newline_indent(&mut self) {
        self.out.push('\n');
        self.out.push_str(&self.indent);
    }

    fn quote_into(units: &[u16], out: &mut String) {
        quote_into(units, out);
    }

    fn number(&mut self, x: f64) {
        if x.is_finite() {
            self.out.push_str(&number::to_string(x));
        } else {
            self.out.push_str("null");
        }
    }
}
