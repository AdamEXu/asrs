//! WHATWG `TextDecoder` (utf-8, utf-16le, utf-16be) and `TextEncoder` as V8/Node
//! implement them: BOM stripping, per-error U+FFFD resync, unpaired-surrogate
//! handling in UTF-16, and V8's exact error strings.

use super::{JsError, JsString};

#[derive(Debug)]
pub enum DecodeError {
    /// The label did not resolve to an implemented encoding.
    UnsupportedEncoding(String),
    /// `fatal: true` and the input was malformed.
    InvalidData(String),
}

impl DecodeError {
    pub fn into_error(self) -> JsError {
        match self {
            DecodeError::UnsupportedEncoding(name) => {
                JsError::range(format!("The \"{name}\" encoding is not supported"))
            }
            DecodeError::InvalidData(encoding) => JsError::type_error(format!(
                "The encoded data was not valid for encoding {encoding}"
            )),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
}

impl Encoding {
    fn name(self) -> &'static str {
        match self {
            Encoding::Utf8 => "utf-8",
            Encoding::Utf16Le => "utf-16le",
            Encoding::Utf16Be => "utf-16be",
        }
    }
}

/// `TextDecoder` restricted to the encodings upstream uses.
pub struct TextDecoder {
    encoding: Encoding,
    fatal: bool,
    ignore_bom: bool,
}

/// Labels that resolve to a WHATWG encoding V8 does not implement; the error
/// message names the encoding, not the label.
const UNIMPLEMENTED_ENCODINGS: &[(&str, &str)] = &[
    ("replacement", "replacement"),
    ("hz-gb-2312", "replacement"),
    ("iso-2022-cn", "replacement"),
    ("iso-2022-cn-ext", "replacement"),
    ("iso-2022-kr", "replacement"),
    ("x-user-defined", "x-user-defined"),
];

impl TextDecoder {
    /// `new TextDecoder(label, { fatal, ignoreBOM })`. The label is trimmed of ASCII
    /// whitespace and ASCII-lowercased before lookup.
    pub fn new(
        label: Option<&JsString>,
        fatal: bool,
        ignore_bom: bool,
    ) -> Result<Self, DecodeError> {
        let label = match label {
            None => "utf-8".to_owned(),
            Some(l) => l.to_string_lossy(),
        };
        let normalized = label
            .trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\x0c' | '\r'))
            .to_ascii_lowercase();
        let encoding = match normalized.as_str() {
            "unicode-1-1-utf-8" | "unicode11utf8" | "unicode20utf8" | "utf-8" | "utf8"
            | "x-unicode20utf8" => Encoding::Utf8,
            "csunicode" | "iso-10646-ucs-2" | "ucs-2" | "unicode" | "unicodefeff" | "utf-16"
            | "utf-16le" => Encoding::Utf16Le,
            "unicodefffe" | "utf-16be" => Encoding::Utf16Be,
            other => {
                let name = UNIMPLEMENTED_ENCODINGS
                    .iter()
                    .find(|(l, _)| *l == other)
                    .map(|(_, n)| (*n).to_owned())
                    .unwrap_or(normalized.clone());
                return Err(DecodeError::UnsupportedEncoding(name));
            }
        };
        Ok(TextDecoder {
            encoding,
            fatal,
            ignore_bom,
        })
    }

    /// `TextDecoder.prototype.decode(bytes)`.
    pub fn decode(&self, bytes: &[u8]) -> Result<JsString, DecodeError> {
        let invalid = || DecodeError::InvalidData(self.encoding.name().to_owned());
        let units = match self.encoding {
            Encoding::Utf8 => decode_utf8(bytes, self.fatal).map_err(|_| invalid())?,
            Encoding::Utf16Le => decode_utf16(bytes, true, self.fatal).map_err(|_| invalid())?,
            Encoding::Utf16Be => decode_utf16(bytes, false, self.fatal).map_err(|_| invalid())?,
        };
        let skip = if !self.ignore_bom && units.first() == Some(&0xfeff) {
            1
        } else {
            0
        };
        Ok(JsString::from_units(units[skip..].to_vec()))
    }
}

const FFFD: u16 = 0xfffd;

/// WHATWG UTF-8 decode: on a malformed sequence emit one U+FFFD and resync at the
/// first byte that was not consumed.
fn decode_utf8(bytes: &[u8], fatal: bool) -> Result<Vec<u16>, ()> {
    fn fail(out: &mut Vec<u16>, fatal: bool) -> Result<(), ()> {
        if fatal {
            Err(())
        } else {
            out.push(FFFD);
            Ok(())
        }
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b < 0x80 {
            out.push(u16::from(b));
            i += 1;
            continue;
        }
        let (needed, mut cp, lower, upper) = match b {
            0xc2..=0xdf => (1, u32::from(b & 0x1f), 0x80, 0xbf),
            0xe0 => (2, 0, 0xa0, 0xbf),
            0xe1..=0xec | 0xee | 0xef => (2, u32::from(b & 0x0f), 0x80, 0xbf),
            0xed => (2, u32::from(b & 0x0f), 0x80, 0x9f),
            0xf0 => (3, 0, 0x90, 0xbf),
            0xf1..=0xf3 => (3, u32::from(b & 0x07), 0x80, 0xbf),
            0xf4 => (3, u32::from(b & 0x07), 0x80, 0x8f),
            _ => {
                fail(&mut out, fatal)?;
                i += 1;
                continue;
            }
        };
        i += 1;
        let mut ok = true;
        for k in 0..needed {
            match bytes.get(i) {
                Some(&c) if (if k == 0 { lower..=upper } else { 0x80..=0xbf }).contains(&c) => {
                    cp = cp * 64 + u32::from(c & 0x3f);
                    i += 1;
                }
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            if cp <= 0xffff {
                out.push(cp as u16);
            } else {
                let v = cp - 0x10000;
                out.push(0xd800 + (v >> 10) as u16);
                out.push(0xdc00 + (v & 0x3ff) as u16);
            }
        } else {
            fail(&mut out, fatal)?;
        }
    }
    Ok(out)
}

/// UTF-16 decode. An unpaired surrogate produces one U+FFFD; when a high surrogate
/// is the last complete unit and a single dangling byte remains, the byte is
/// consumed by that same error (Node's ICU-backed decoder behaviour).
fn decode_utf16(bytes: &[u8], little_endian: bool, fatal: bool) -> Result<Vec<u16>, ()> {
    let read = |i: usize| -> u16 {
        let (a, b) = (bytes[i], bytes[i + 1]);
        if little_endian {
            u16::from_le_bytes([a, b])
        } else {
            u16::from_be_bytes([a, b])
        }
    };
    let mut out = Vec::with_capacity(bytes.len() / 2 + 1);
    let mut i = 0;
    while bytes.len() - i >= 2 {
        let unit = read(i);
        i += 2;
        match unit {
            0xd800..=0xdbff => {
                if bytes.len() - i >= 2 {
                    let next = read(i);
                    if (0xdc00..=0xdfff).contains(&next) {
                        i += 2;
                        let cp = 0x10000
                            + ((u32::from(unit) - 0xd800) << 10)
                            + (u32::from(next) - 0xdc00);
                        out.push(0xd800 + ((cp - 0x10000) >> 10) as u16);
                        out.push(0xdc00 + ((cp - 0x10000) & 0x3ff) as u16);
                    } else {
                        if fatal {
                            return Err(());
                        }
                        out.push(FFFD);
                    }
                } else {
                    if fatal {
                        return Err(());
                    }
                    out.push(FFFD);
                    i = bytes.len();
                }
            }
            0xdc00..=0xdfff => {
                if fatal {
                    return Err(());
                }
                out.push(FFFD);
            }
            _ => out.push(unit),
        }
    }
    if i < bytes.len() {
        if fatal {
            return Err(());
        }
        out.push(FFFD);
    }
    Ok(out)
}

/// `TextEncoder.prototype.encode`: UTF-8 with each lone surrogate -> U+FFFD.
pub fn text_encode(text: &JsString) -> Vec<u8> {
    let units = text.units();
    let mut out = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        let cp = if (0xd800..0xdc00).contains(&u) {
            match units.get(i + 1) {
                Some(&low) if (0xdc00..0xe000).contains(&low) => {
                    i += 1;
                    0x10000 + ((u32::from(u) - 0xd800) << 10) + (u32::from(low) - 0xdc00)
                }
                _ => 0xfffd,
            }
        } else if (0xdc00..0xe000).contains(&u) {
            0xfffd
        } else {
            u32::from(u)
        };
        i += 1;
        match cp {
            0x00..=0x7f => out.push(cp as u8),
            0x80..=0x7ff => {
                out.push(0xc0 | (cp >> 6) as u8);
                out.push(0x80 | (cp & 0x3f) as u8);
            }
            0x800..=0xffff => {
                out.push(0xe0 | (cp >> 12) as u8);
                out.push(0x80 | ((cp >> 6) & 0x3f) as u8);
                out.push(0x80 | (cp & 0x3f) as u8);
            }
            _ => {
                out.push(0xf0 | (cp >> 18) as u8);
                out.push(0x80 | ((cp >> 12) & 0x3f) as u8);
                out.push(0x80 | ((cp >> 6) & 0x3f) as u8);
                out.push(0x80 | (cp & 0x3f) as u8);
            }
        }
    }
    out
}
