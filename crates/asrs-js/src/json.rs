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
pub fn stringify(value: &JsValue) -> Result<Option<String>, JsonError> {
    stringify_gap(value, "")
}

/// `JSON.stringify(value, null, 2)`.
pub fn stringify_pretty(value: &JsValue) -> Result<Option<String>, JsonError> {
    stringify_gap(value, "  ")
}

/// `JSON.stringify(value, null, gap)` with `gap` already reduced to the string the spec uses:
/// see [`gap_from_number`] and [`gap_from_string`].
pub fn stringify_gap(value: &JsValue, gap: &str) -> Result<Option<String>, JsonError> {
    let mut writer = Writer {
        out: String::new(),
        gap,
        indent: String::new(),
    };
    Ok(writer.value(value)?.then_some(writer.out))
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

struct Writer<'a> {
    out: String,
    gap: &'a str,
    indent: String,
}

impl Writer<'_> {
    /// SerializeJSONProperty. Returns false (writing nothing) when the value serializes to
    /// `undefined`.
    fn value(&mut self, value: &JsValue) -> Result<bool, JsonError> {
        match value {
            JsValue::Undefined => return Ok(false),
            JsValue::Null => self.out.push_str("null"),
            JsValue::Bool(b) => self.out.push_str(if *b { "true" } else { "false" }),
            JsValue::Number(x) => self.number(*x),
            JsValue::String(s) => quote_into(s.units(), &mut self.out),
            JsValue::Array(items) => self.array(items)?,
            JsValue::Object(object) => self.object(
                object
                    .iter()
                    .map(|(k, v)| (k.units(), MemberValue::Value(v))),
            )?,
            JsValue::Bytes(bytes) => {
                let keys: Vec<Vec<u16>> = (0..bytes.len())
                    .map(|i| number::to_string(i as f64).encode_utf16().collect())
                    .collect();
                self.object(
                    keys.iter()
                        .zip(bytes)
                        .map(|(k, b)| (k.as_slice(), MemberValue::Number(f64::from(*b)))),
                )?
            }
            JsValue::BigInt(_) => {
                return Err(JsonError {
                    name: "TypeError",
                    message: JsString::from("Do not know how to serialize a BigInt"),
                });
            }
            JsValue::Date(t) => {
                if t.is_finite() {
                    quote_into(
                        &to_iso_string(*t).encode_utf16().collect::<Vec<u16>>(),
                        &mut self.out,
                    )
                } else {
                    self.out.push_str("null")
                }
            }
        }
        Ok(true)
    }

    fn number(&mut self, x: f64) {
        if x.is_finite() {
            self.out.push_str(&number::to_string(x));
        } else {
            self.out.push_str("null");
        }
    }

    fn newline_indent(&mut self) {
        self.out.push('\n');
        self.out.push_str(&self.indent);
    }

    fn array(&mut self, items: &[JsValue]) -> Result<(), JsonError> {
        if items.is_empty() {
            self.out.push_str("[]");
            return Ok(());
        }
        let stepback = self.indent.len();
        self.indent.push_str(self.gap);
        self.out.push('[');
        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                self.out.push(',');
            }
            if !self.gap.is_empty() {
                self.newline_indent();
            }
            if !self.value(item)? {
                self.out.push_str("null");
            }
        }
        self.indent.truncate(stepback);
        if !self.gap.is_empty() {
            self.newline_indent();
        }
        self.out.push(']');
        Ok(())
    }

    fn object<'v>(
        &mut self,
        members: impl Iterator<Item = (&'v [u16], MemberValue<'v>)>,
    ) -> Result<(), JsonError> {
        let start = self.out.len();
        let stepback = self.indent.len();
        self.indent.push_str(self.gap);
        self.out.push('{');
        let mut any = false;
        for (key, value) in members {
            let member_start = self.out.len();
            if any {
                self.out.push(',');
            }
            if !self.gap.is_empty() {
                self.newline_indent();
            }
            quote_into(key, &mut self.out);
            self.out.push(':');
            if !self.gap.is_empty() {
                self.out.push(' ');
            }
            let written = match value {
                MemberValue::Value(v) => self.value(v)?,
                MemberValue::Number(x) => {
                    self.number(x);
                    true
                }
            };
            if written {
                any = true;
            } else {
                self.out.truncate(member_start);
            }
        }
        self.indent.truncate(stepback);
        if !any {
            self.out.truncate(start);
            self.out.push_str("{}");
            return Ok(());
        }
        if !self.gap.is_empty() {
            self.newline_indent();
        }
        self.out.push('}');
        Ok(())
    }
}

enum MemberValue<'v> {
    Value(&'v JsValue),
    Number(f64),
}
