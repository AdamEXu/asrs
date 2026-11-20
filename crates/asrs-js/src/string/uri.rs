//! `encodeURIComponent` / `decodeURIComponent` with V8's URIError behaviour:
//! lone surrogates on encode, bad `%` sequences and invalid UTF-8 on decode.

use super::{JsError, JsString};

#[derive(Debug)]
pub struct UriError;

impl UriError {
    pub fn into_error(self) -> JsError {
        JsError::new("URIError", "URI malformed")
    }
}

fn is_unreserved(u: u16) -> bool {
    matches!(u,
        0x61..=0x7a | 0x41..=0x5a | 0x30..=0x39
            | 0x2d | 0x5f | 0x2e | 0x21 | 0x7e | 0x2a | 0x27 | 0x28 | 0x29)
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// `encodeURIComponent`: unreserved characters pass through; everything else is
/// UTF-8 encoded and percent-escaped with uppercase hex.
pub fn encode_uri_component(input: &JsString) -> Result<JsString, UriError> {
    let units = input.units();
    let mut out = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        i += 1;
        if is_unreserved(u) {
            out.push(u);
            continue;
        }
        let cp = if (0xd800..0xdc00).contains(&u) {
            match units.get(i) {
                Some(&low) if (0xdc00..0xe000).contains(&low) => {
                    i += 1;
                    0x10000 + ((u32::from(u) - 0xd800) << 10) + (u32::from(low) - 0xdc00)
                }
                _ => return Err(UriError),
            }
        } else if (0xdc00..0xe000).contains(&u) {
            return Err(UriError);
        } else {
            u32::from(u)
        };
        let (a, rest) = match cp {
            0x80..=0x7ff => (0xc0 | (cp >> 6) as u8, 1),
            0x800..=0xffff => (0xe0 | (cp >> 12) as u8, 2),
            _ if cp > 0xffff => (0xf0 | (cp >> 18) as u8, 3),
            _ => (cp as u8, 0),
        };
        out.extend(escape(a));
        for k in 0..rest {
            out.extend(escape(0x80 | ((cp >> (6 * (rest - k - 1))) & 0x3f) as u8));
        }
    }
    Ok(JsString::from_units(out))
}

fn escape(byte: u8) -> [u16; 3] {
    [
        u16::from(b'%'),
        u16::from(HEX[(byte >> 4) as usize]),
        u16::from(HEX[(byte & 0xf) as usize]),
    ]
}

fn hex_value(u: u16) -> Option<u8> {
    match u {
        0x30..=0x39 => Some((u - 0x30) as u8),
        0x41..=0x46 => Some((u - 0x41 + 10) as u8),
        0x61..=0x66 => Some((u - 0x61 + 10) as u8),
        _ => None,
    }
}

/// `decodeURIComponent`. Non-`%` characters pass through verbatim (including lone
/// surrogates); `%XX` sequences must decode as strict UTF-8.
pub fn decode_uri_component(input: &JsString) -> Result<JsString, UriError> {
    let units = input.units();
    let mut out = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        if u != u16::from(b'%') {
            out.push(u);
            i += 1;
            continue;
        }
        let b0 = read_octet(units, i)?;
        i += 3;
        if b0 < 0x80 {
            out.push(u16::from(b0));
            continue;
        }
        let needed = match b0 {
            0xc0..=0xdf => 1,
            0xe0..=0xef => 2,
            0xf0..=0xf4 => 3,
            _ => return Err(UriError),
        };
        let mut octets = vec![b0];
        for _ in 0..needed {
            if units.get(i) != Some(&u16::from(b'%')) {
                return Err(UriError);
            }
            octets.push(read_octet(units, i)?);
            i += 3;
        }
        let (lower, upper) = match b0 {
            0xe0 => (0xa0, 0xbf),
            0xed => (0x80, 0x9f),
            0xf0 => (0x90, 0xbf),
            0xf4 => (0x80, 0x8f),
            _ => (0x80, 0xbf),
        };
        let mut cp = u32::from(match needed {
            1 => b0 & 0x1f,
            2 => b0 & 0x0f,
            _ => b0 & 0x07,
        });
        for (k, &c) in octets[1..].iter().enumerate() {
            let bound = if k == 0 { lower..=upper } else { 0x80..=0xbf };
            if !bound.contains(&c) {
                return Err(UriError);
            }
            cp = cp * 64 + u32::from(c & 0x3f);
        }
        if !(0x80..=0x10ffff).contains(&cp) {
            return Err(UriError);
        }
        if cp <= 0xffff {
            out.push(cp as u16);
        } else {
            let v = cp - 0x10000;
            out.push(0xd800 + (v >> 10) as u16);
            out.push(0xdc00 + (v & 0x3ff) as u16);
        }
    }
    Ok(JsString::from_units(out))
}

fn read_octet(units: &[u16], i: usize) -> Result<u8, UriError> {
    match (units.get(i + 1), units.get(i + 2)) {
        (Some(&h), Some(&l)) => match (hex_value(h), hex_value(l)) {
            (Some(h), Some(l)) => Ok(h * 16 + l),
            _ => Err(UriError),
        },
        _ => Err(UriError),
    }
}
