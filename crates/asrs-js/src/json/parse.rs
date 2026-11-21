// Ported from V8 14.2 (BSD-3-Clause, Copyright the V8 project authors). See THIRD_PARTY_NOTICES.md.

//! A port of V8's `JsonParser` (src/json/json-parser.cc): the same grammar checks in the same
//! order, so every malformed input fails with V8's message and position.

use super::JsonError;
use crate::string::JsString;
use crate::value::{JsObject, JsValue};

/// `JSON.parse(text)` without a reviver.
pub fn parse(text: &JsString) -> Result<JsValue> {
    parse_units(text.units())
}

pub fn parse_str(text: &str) -> Result<JsValue> {
    parse_units(&text.encode_utf16().collect::<Vec<u16>>())
}

pub fn parse_units(text: &[u16]) -> Result<JsValue> {
    Parser { src: text, pos: 0 }.parse()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Token {
    Eos,
    String,
    Number,
    TrueLiteral,
    FalseLiteral,
    NullLiteral,
    Whitespace,
    Colon,
    Comma,
    LBrace,
    RBrace,
    LBrack,
    RBrack,
    Illegal,
}

fn token_of(c: u16) -> Token {
    match c {
        0x22 => Token::String,
        0x2d | 0x30..=0x39 => Token::Number,
        0x74 => Token::TrueLiteral,
        0x66 => Token::FalseLiteral,
        0x6e => Token::NullLiteral,
        0x20 | 0x09 | 0x0a | 0x0d => Token::Whitespace,
        0x3a => Token::Colon,
        0x2c => Token::Comma,
        0x7b => Token::LBrace,
        0x7d => Token::RBrace,
        0x5b => Token::LBrack,
        0x5d => Token::RBrack,
        _ => Token::Illegal,
    }
}

#[derive(Clone, Copy)]
enum Message {
    ExpectedPropNameOrRBrace,
    ExpectedCommaOrRBrack,
    ExpectedCommaOrRBrace,
    UnterminatedString,
    ExpectedColonAfterPropertyName,
    ExpectedDoubleQuotedPropertyName,
    ExponentPartMissingNumber,
    UnterminatedFractionalNumber,
    NoNumberAfterMinusSign,
    BadControlCharacter,
    BadUnicodeEscape,
    BadEscapedCharacter,
    UnexpectedNonWhiteSpaceCharacter,
}

impl Message {
    fn text(self) -> &'static str {
        match self {
            Message::ExpectedPropNameOrRBrace => "Expected property name or '}' in JSON",
            Message::ExpectedCommaOrRBrack => "Expected ',' or ']' after array element in JSON",
            Message::ExpectedCommaOrRBrace => "Expected ',' or '}' after property value in JSON",
            Message::UnterminatedString => "Unterminated string in JSON",
            Message::ExpectedColonAfterPropertyName => "Expected ':' after property name in JSON",
            Message::ExpectedDoubleQuotedPropertyName => {
                "Expected double-quoted property name in JSON"
            }
            Message::ExponentPartMissingNumber => "Exponent part is missing a number in JSON",
            Message::UnterminatedFractionalNumber => "Unterminated fractional number in JSON",
            Message::NoNumberAfterMinusSign => "No number after minus sign in JSON",
            Message::BadControlCharacter => "Bad control character in string literal in JSON",
            Message::BadUnicodeEscape => "Bad Unicode escape in JSON",
            Message::BadEscapedCharacter => "Bad escaped character in JSON",
            Message::UnexpectedNonWhiteSpaceCharacter => {
                "Unexpected non-whitespace character after JSON"
            }
        }
    }
}

const MAX_CONTEXT_CHARACTERS: usize = 10;
const MIN_ORIGINAL_SOURCE_LENGTH_FOR_CONTEXT: usize = MAX_CONTEXT_CHARACTERS * 2 + 1;

enum Frame {
    Array(Vec<JsValue>),
    Object(JsObject, JsString),
}

struct Parser<'a> {
    src: &'a [u16],
    pos: usize,
}

type Result<T> = std::result::Result<T, JsonError>;

fn units(s: &str) -> impl Iterator<Item = u16> + '_ {
    s.encode_utf16()
}

impl Parser<'_> {
    fn current(&self) -> Option<u16> {
        self.src.get(self.pos).copied()
    }

    fn peek(&self) -> Token {
        self.current().map_or(Token::Eos, token_of)
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.current(), Some(0x20 | 0x09 | 0x0a | 0x0d)) {
            self.pos += 1;
        }
    }

    fn check(&mut self, token: Token) -> bool {
        self.skip_whitespace();
        if self.peek() == token {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_next(&mut self, token: Token, message: Message) -> Result<()> {
        self.skip_whitespace();
        if self.peek() == token {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.error(Some(message), self.peek()))
        }
    }

    fn parse(mut self) -> Result<JsValue> {
        let value = self.parse_value()?;
        if !self.check(Token::Eos) {
            return Err(self.error(Some(Message::UnexpectedNonWhiteSpaceCharacter), self.peek()));
        }
        Ok(value)
    }

    fn parse_value(&mut self) -> Result<JsValue> {
        let mut stack: Vec<Frame> = Vec::new();
        loop {
            let mut value = loop {
                self.skip_whitespace();
                match self.peek() {
                    Token::String => {
                        self.pos += 1;
                        break JsValue::String(self.scan_string()?);
                    }
                    Token::Number => break JsValue::Number(self.parse_number()?),
                    Token::LBrace => {
                        self.pos += 1;
                        if self.check(Token::RBrace) {
                            break JsValue::Object(JsObject::new());
                        }
                        self.expect_next(Token::String, Message::ExpectedPropNameOrRBrace)?;
                        let key = self.scan_string()?;
                        self.expect_next(Token::Colon, Message::ExpectedColonAfterPropertyName)?;
                        stack.push(Frame::Object(JsObject::new(), key));
                    }
                    Token::LBrack => {
                        self.pos += 1;
                        if self.check(Token::RBrack) {
                            break JsValue::Array(Vec::new());
                        }
                        stack.push(Frame::Array(Vec::new()));
                    }
                    Token::TrueLiteral => {
                        self.scan_literal("true")?;
                        break JsValue::Bool(true);
                    }
                    Token::FalseLiteral => {
                        self.scan_literal("false")?;
                        break JsValue::Bool(false);
                    }
                    Token::NullLiteral => {
                        self.scan_literal("null")?;
                        break JsValue::Null;
                    }
                    token => return Err(self.error(None, token)),
                }
            };

            loop {
                match stack.pop() {
                    None => return Ok(value),
                    Some(Frame::Object(mut object, key)) => {
                        object.insert(key, value);
                        if self.check(Token::Comma) {
                            self.expect_next(
                                Token::String,
                                Message::ExpectedDoubleQuotedPropertyName,
                            )?;
                            let key = self.scan_string()?;
                            self.expect_next(
                                Token::Colon,
                                Message::ExpectedColonAfterPropertyName,
                            )?;
                            stack.push(Frame::Object(object, key));
                            break;
                        }
                        self.expect_next(Token::RBrace, Message::ExpectedCommaOrRBrace)?;
                        value = JsValue::Object(object);
                    }
                    Some(Frame::Array(mut items)) => {
                        items.push(value);
                        if self.check(Token::Comma) {
                            stack.push(Frame::Array(items));
                            break;
                        }
                        self.expect_next(Token::RBrack, Message::ExpectedCommaOrRBrack)?;
                        value = JsValue::Array(items);
                    }
                }
            }
        }
    }

    fn scan_literal(&mut self, literal: &str) -> Result<()> {
        let expected: Vec<u16> = units(literal).collect();
        let remaining = self.src.len() - self.pos;
        if remaining >= expected.len()
            && self.src[self.pos..self.pos + expected.len()] == expected[..]
        {
            self.pos += expected.len();
            return Ok(());
        }
        self.pos += 1;
        for &e in &expected[1..expected.len().min(remaining)] {
            let c = self.src[self.pos];
            if c != e {
                return Err(self.unexpected_character(Some(c)));
            }
            self.pos += 1;
        }
        Err(self.error(None, Token::Eos))
    }

    /// Scans a string body; `pos` is just past the opening quote.
    fn scan_string(&mut self) -> Result<JsString> {
        let mut out: Vec<u16> = Vec::new();
        loop {
            let Some(c) = self.current() else {
                return Err(self.error(Some(Message::UnterminatedString), Token::Illegal));
            };
            match c {
                0x22 => {
                    self.pos += 1;
                    return Ok(JsString::from_units(out));
                }
                0x5c => {
                    self.pos += 1;
                    let Some(c) = self.current() else {
                        return Err(self.unexpected_character(None));
                    };
                    if c > 0xff {
                        return Err(self.unexpected_character(Some(c)));
                    }
                    let unit = match c {
                        0x22 | 0x5c | 0x2f => c,
                        0x62 => 0x08,
                        0x66 => 0x0c,
                        0x6e => 0x0a,
                        0x72 => 0x0d,
                        0x74 => 0x09,
                        0x75 => self.scan_unicode_escape()?,
                        _ => {
                            return Err(
                                self.error(Some(Message::BadEscapedCharacter), Token::Illegal)
                            );
                        }
                    };
                    out.push(unit);
                    self.pos += 1;
                }
                0x00..0x20 => {
                    return Err(self.error(Some(Message::BadControlCharacter), Token::Illegal));
                }
                _ => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    /// `pos` is on the `u`; leaves it on the last hex digit.
    fn scan_unicode_escape(&mut self) -> Result<u16> {
        let mut value: u16 = 0;
        for _ in 0..4 {
            self.pos += 1;
            let digit = self
                .current()
                .and_then(|c| char::from_u32(u32::from(c))?.to_digit(16));
            match digit {
                Some(d) => value = value * 16 + d as u16,
                None => return Err(self.error(Some(Message::BadUnicodeEscape), Token::Illegal)),
            }
        }
        Ok(value)
    }

    fn is_digit(c: Option<u16>) -> bool {
        matches!(c, Some(0x30..=0x39))
    }

    fn is_number_part(c: Option<u16>) -> bool {
        matches!(c, Some(0x2e | 0x65 | 0x45 | 0x2d | 0x2b | 0x30..=0x39))
    }

    fn skip_digits(&mut self) {
        while Self::is_digit(self.current()) {
            self.pos += 1;
        }
    }

    fn parse_number(&mut self) -> Result<f64> {
        let start = self.pos;
        let negative = self.current() == Some(0x2d);
        if negative {
            self.pos += 1;
        }
        if self.current() == Some(0x30) {
            self.pos += 1;
            let c = self.current();
            if Self::is_number_part(c) {
                if Self::is_digit(c) {
                    return Err(self.error(None, Token::Number));
                }
            } else if !negative {
                return Ok(0.0);
            }
        } else {
            let digits_start = self.pos;
            self.skip_digits();
            if self.pos == digits_start {
                return Err(self.error(Some(Message::NoNumberAfterMinusSign), Token::Illegal));
            }
        }
        if self.current() == Some(0x2e) {
            self.pos += 1;
            if !Self::is_digit(self.current()) {
                return Err(self.error(Some(Message::UnterminatedFractionalNumber), Token::Illegal));
            }
            self.skip_digits();
        }
        if matches!(self.current(), Some(0x65 | 0x45)) {
            self.pos += 1;
            if matches!(self.current(), Some(0x2d | 0x2b)) {
                self.pos += 1;
            }
            if !Self::is_digit(self.current()) {
                return Err(self.error(Some(Message::ExponentPartMissingNumber), Token::Illegal));
            }
            self.skip_digits();
        }
        let text: String = self.src[start..self.pos]
            .iter()
            .map(|&u| char::from(u as u8))
            .collect();
        Ok(text.parse().unwrap_or(f64::NAN))
    }

    fn unexpected_character(&self, c: Option<u16>) -> JsonError {
        self.error(
            None,
            c.map_or(Token::Eos, |c| {
                if c <= 0xff {
                    token_of(c)
                } else {
                    Token::Illegal
                }
            }),
        )
    }

    /// V8's `ReportUnexpectedToken`: an explicit message, or one chosen from the token at
    /// `pos`.
    fn error(&self, message: Option<Message>, token: Token) -> JsonError {
        let text = match (message, token) {
            (Some(m), _) => self.at_position(m.text()),
            (None, Token::Eos) => JsString::from("Unexpected end of JSON input"),
            (None, Token::Number) => self.at_position("Unexpected number in JSON"),
            (None, Token::String) => self.at_position("Unexpected string in JSON"),
            (None, _) => self.unexpected_token(),
        };
        JsonError::syntax(text)
    }

    fn at_position(&self, prefix: &str) -> JsString {
        let (line, column) = self.file_location();
        JsString::from(format!(
            "{prefix} at position {} (line {line} column {column})",
            self.pos
        ))
    }

    fn file_location(&self) -> (usize, usize) {
        let mut line = 1;
        let mut last_line_break = 0;
        let mut i = 0;
        let end = self.pos;
        while i < end {
            if self.src[i] == 0x0d && i + 1 < end && self.src[i + 1] == 0x0a {
                i += 1;
            }
            if self.src[i] == 0x0d || self.src[i] == 0x0a {
                line += 1;
                last_line_break = i + 1;
            }
            i += 1;
        }
        (line, 1 + end - last_line_break)
    }

    fn is_special_string(&self) -> bool {
        ["[object Object]", "undefined", "Infinity", "NaN"]
            .iter()
            .any(|s| units(s).eq(self.src.iter().copied()))
    }

    fn unexpected_token(&self) -> JsString {
        let mut out: Vec<u16> = Vec::new();
        let push = |out: &mut Vec<u16>, s: &str| out.extend(units(s));
        if self.is_special_string() {
            push(&mut out, "\"");
            out.extend_from_slice(self.src);
            push(&mut out, "\" is not valid JSON");
            return JsString::from_units(out);
        }
        push(&mut out, "Unexpected token '");
        out.push(self.src[self.pos]);
        push(&mut out, "', ");
        let len = self.src.len();
        let pos = self.pos;
        if len < MIN_ORIGINAL_SOURCE_LENGTH_FOR_CONTEXT {
            push(&mut out, "\"");
            out.extend_from_slice(self.src);
            push(&mut out, "\"");
        } else if pos < MAX_CONTEXT_CHARACTERS {
            push(&mut out, "\"");
            out.extend_from_slice(&self.src[..pos + MAX_CONTEXT_CHARACTERS]);
            push(&mut out, "\"...");
        } else if pos < len - MAX_CONTEXT_CHARACTERS {
            push(&mut out, "...\"");
            out.extend_from_slice(
                &self.src[pos - MAX_CONTEXT_CHARACTERS..pos + MAX_CONTEXT_CHARACTERS],
            );
            push(&mut out, "\"...");
        } else {
            push(&mut out, "...\"");
            out.extend_from_slice(&self.src[pos - MAX_CONTEXT_CHARACTERS..]);
            push(&mut out, "\"");
        }
        push(&mut out, " is not valid JSON");
        JsString::from_units(out)
    }
}
