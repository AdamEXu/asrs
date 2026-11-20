use std::fmt;

use crate::utf16::Utf16String;

/// A JSON tree with `JSON.parse` semantics: strings are UTF-16 so escaped lone surrogates
/// survive, and a duplicate key keeps its first position and takes the last value.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(Utf16String),
    Array(Vec<Json>),
    Object(Vec<(Utf16String, Json)>),
}

pub const MAX_DEPTH: usize = 1000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub offset: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JSON: {} at byte {}", self.message, self.offset)
    }
}

impl std::error::Error for ParseError {}

pub fn parse(text: &str) -> Result<Json, ParseError> {
    let mut parser = Parser {
        text,
        bytes: text.as_bytes(),
        pos: 0,
    };
    parser.skip_ws();
    let value = parser.value(0)?;
    parser.skip_ws();
    if parser.pos != parser.bytes.len() {
        return Err(parser.error("unexpected data after the value"));
    }
    Ok(value)
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(pairs) => pairs.iter().find(|(k, _)| k.eq_str(key)).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&Utf16String> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn string(s: &str) -> Json {
        Json::String(Utf16String::from(s))
    }

    pub fn object<'a>(pairs: impl IntoIterator<Item = (&'a str, Json)>) -> Json {
        Json::Object(
            pairs
                .into_iter()
                .map(|(k, v)| (Utf16String::from(k), v))
                .collect(),
        )
    }

    pub fn to_compact(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, None, 0);
        out
    }

    /// Like `JSON.stringify(value, null, 2)` for the values this harness writes.
    pub fn to_pretty(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, Some(2), 0);
        out
    }

    fn write(&self, out: &mut String, indent: Option<usize>, level: usize) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Number(n) => write_number(*n, out),
            Json::String(s) => write_string(s.units(), out),
            Json::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    newline(out, indent, level + 1);
                    item.write(out, indent, level + 1);
                }
                if !items.is_empty() {
                    newline(out, indent, level);
                }
                out.push(']');
            }
            Json::Object(pairs) => {
                out.push('{');
                for (i, (key, value)) in pairs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    newline(out, indent, level + 1);
                    write_string(key.units(), out);
                    out.push(':');
                    if indent.is_some() {
                        out.push(' ');
                    }
                    value.write(out, indent, level + 1);
                }
                if !pairs.is_empty() {
                    newline(out, indent, level);
                }
                out.push('}');
            }
        }
    }
}

fn newline(out: &mut String, indent: Option<usize>, level: usize) {
    if let Some(width) = indent {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', width * level));
    }
}

/// Integers are written as `JSON.stringify` writes them; other finite numbers use Rust's
/// shortest round-trip form, which can differ from JS in exponent formatting.
fn write_number(n: f64, out: &mut String) {
    use fmt::Write;
    if !n.is_finite() {
        out.push_str("null");
    } else if n == 0.0 {
        out.push('0');
    } else if n.fract() == 0.0 && n.abs() < 1e21 {
        let _ = write!(out, "{n:.0}");
    } else {
        let _ = write!(out, "{n}");
    }
}

/// Writes a JSON string exactly as well-formed `JSON.stringify` does.
pub fn write_string(units: &[u16], out: &mut String) {
    use fmt::Write;
    out.push('"');
    let mut i = 0;
    while i < units.len() {
        let unit = units[i];
        i += 1;
        match unit {
            0x22 => out.push_str("\\\""),
            0x5c => out.push_str("\\\\"),
            0x08 => out.push_str("\\b"),
            0x0c => out.push_str("\\f"),
            0x0a => out.push_str("\\n"),
            0x0d => out.push_str("\\r"),
            0x09 => out.push_str("\\t"),
            0x00..=0x1f => {
                let _ = write!(out, "\\u{unit:04x}");
            }
            0xd800..=0xdbff => match units.get(i) {
                Some(&low @ 0xdc00..=0xdfff) => {
                    i += 1;
                    let code =
                        0x10000 + ((u32::from(unit) - 0xd800) << 10) + (u32::from(low) - 0xdc00);
                    out.push(char::from_u32(code).expect("valid surrogate pair"));
                }
                _ => {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            },
            0xdc00..=0xdfff => {
                let _ = write!(out, "\\u{unit:04x}");
            }
            _ => out.push(char::from_u32(u32::from(unit)).expect("BMP scalar")),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn error(&self, message: &str) -> ParseError {
        ParseError {
            offset: self.pos,
            message: message.to_owned(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), ParseError> {
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.error(&format!("expected '{}'", byte as char)))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, ParseError> {
        if depth > MAX_DEPTH {
            return Err(self.error("nesting too deep"));
        }
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Json::String),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.error("unexpected character")),
            None => Err(self.error("unexpected end of input")),
        }
    }

    fn literal(&mut self, word: &str, value: Json) -> Result<Json, ParseError> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(self.error("invalid literal"))
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, ParseError> {
        self.pos += 1;
        let mut pairs: Vec<(Utf16String, Json)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Object(pairs));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.error("expected a string key"));
            }
            let key = self.string()?;
            self.skip_ws();
            self.expect(b':')?;
            self.skip_ws();
            let value = self.value(depth + 1)?;
            match pairs.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 = value,
                None => pairs.push((key, value)),
            }
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Json::Object(pairs));
                }
                _ => return Err(self.error("expected ',' or '}'")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json, ParseError> {
        self.pos += 1;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Array(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value(depth + 1)?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Json::Array(items));
                }
                _ => return Err(self.error("expected ',' or ']'")),
            }
        }
    }

    fn string(&mut self) -> Result<Utf16String, ParseError> {
        self.pos += 1;
        let mut units = Vec::new();
        loop {
            let Some(byte) = self.peek() else {
                return Err(self.error("unterminated string"));
            };
            match byte {
                b'"' => {
                    self.pos += 1;
                    return Ok(Utf16String::from_units(units));
                }
                b'\\' => {
                    self.pos += 1;
                    let escaped = self
                        .peek()
                        .ok_or_else(|| self.error("unterminated escape"))?;
                    self.pos += 1;
                    let unit = match escaped {
                        b'"' => 0x22,
                        b'\\' => 0x5c,
                        b'/' => 0x2f,
                        b'b' => 0x08,
                        b'f' => 0x0c,
                        b'n' => 0x0a,
                        b'r' => 0x0d,
                        b't' => 0x09,
                        b'u' => self.hex4()?,
                        _ => {
                            self.pos -= 1;
                            return Err(self.error("invalid escape"));
                        }
                    };
                    units.push(unit);
                }
                0x00..=0x1f => return Err(self.error("control character in string")),
                0x20..=0x7f => {
                    units.push(u16::from(byte));
                    self.pos += 1;
                }
                _ => {
                    let ch = self.text[self.pos..]
                        .chars()
                        .next()
                        .expect("pos is on a char boundary");
                    let mut buf = [0u16; 2];
                    units.extend_from_slice(ch.encode_utf16(&mut buf));
                    self.pos += ch.len_utf8();
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u16, ParseError> {
        let digits = self
            .bytes
            .get(self.pos..self.pos + 4)
            .ok_or_else(|| self.error("short \\u escape"))?;
        let mut unit = 0u16;
        for &d in digits {
            let v = (d as char)
                .to_digit(16)
                .ok_or_else(|| self.error("bad \\u escape"))?;
            unit = unit * 16 + v as u16;
        }
        self.pos += 4;
        Ok(unit)
    }

    fn number(&mut self) -> Result<Json, ParseError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => self.digits(),
            _ => return Err(self.error("invalid number")),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.error("invalid number"));
            }
            self.digits();
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.error("invalid number"));
            }
            self.digits();
        }
        let literal = &self.text[start..self.pos];
        literal
            .parse::<f64>()
            .map(Json::Number)
            .map_err(|_| self.error("invalid number"))
    }

    fn digits(&mut self) {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_keys_keep_first_position_and_last_value() {
        let json = parse(r#"{"a":1,"b":2,"a":3}"#).unwrap();
        assert_eq!(json.to_compact(), r#"{"a":3,"b":2}"#);
    }

    #[test]
    fn escapes_and_surrogates() {
        let json = parse(r#""\u0041\/\ud83d\ude00\ud800\u001f\u007f""#).unwrap();
        assert_eq!(json.to_compact(), "\"A/\u{1f600}\\ud800\\u001f\u{7f}\"");
    }

    #[test]
    fn numbers_follow_json_parse() {
        let n = |s: &str| parse(s).unwrap().as_f64().unwrap().to_bits();
        assert_eq!(n("-0"), 0x8000_0000_0000_0000);
        assert_eq!(n("1e400"), f64::INFINITY.to_bits());
        assert_eq!(n("2.4703282292062328e-324"), 1);
        for bad in ["01", "1.", ".5", "+1", "1e", "-", "0x1", "NaN", "Infinity"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn rejects_malformed_text() {
        for bad in [
            "",
            "[1,]",
            "{\"a\":1,}",
            "\"\\x\"",
            "\"a",
            "\"\u{1}\"",
            "nul",
            "null x",
            "{1:2}",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn pretty_matches_json_stringify_indent_two() {
        let json = parse(r#"{"a":[1,{"b":[]}],"c":{}}"#).unwrap();
        assert_eq!(
            json.to_pretty(),
            "{\n  \"a\": [\n    1,\n    {\n      \"b\": []\n    }\n  ],\n  \"c\": {}\n}"
        );
    }
}
