//! The parts of `download` 8's dependency tree the asset downloader reproduces: `url-parse-lax`
//! (`prepend-http` plus legacy `url.parse`), the WHATWG pathname basename `download` names a
//! file by, `filenamify` 3, `content-disposition` 0.5 and `ext-name` 5's mime lookup.

use crate::Invalid;
use asrs_js::string::JsString;

use crate::ext_list::EXT_LIST;
use crate::legacy_url::{self, LegacyUrl};
use crate::path::PathStyle;

/// `url-parse-lax(input)` with `{https: true}`: `prepend-http` on the trimmed input, then legacy
/// `url.parse`.
pub fn url_parse_lax(input: &JsString) -> Result<LegacyUrl, Invalid> {
    let trimmed = input.trim().to_string_lossy();
    let trimmed = trimmed.as_str();
    let with_protocol = if prepend_http_preserved(trimmed) || already_slashed(trimmed) {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    legacy_url::parse(&with_protocol, false)
}

/// `prepend-http`: `^\.*\/` or `^(?!localhost)\w+:` keeps the trimmed source.
fn prepend_http_preserved(url: &str) -> bool {
    let dots = url.bytes().take_while(|&b| b == b'.').count();
    if url[dots..].starts_with('/') {
        return true;
    }
    if url.starts_with("localhost") {
        return false;
    }
    let word = url
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    word > 0 && url.as_bytes().get(word) == Some(&b':')
}

/// The negative lookahead in `prepend-http`'s replacement: `^(?:\w+:)?\/\/`.
fn already_slashed(url: &str) -> bool {
    if url.starts_with("//") {
        return true;
    }
    let word = url
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    word > 0 && url.as_bytes().get(word) == Some(&b':') && url[word + 1..].starts_with("//")
}

const WINDOWS_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
    "com8", "com9", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// `filenamify` 3.0.0 with the default `!` replacement, on UTF-16 code units.
pub fn filenamify(name: &str) -> String {
    let reserved = |u: u16| {
        matches!(u, 0..=0x1f)
            || matches!(
                u as u8 as char,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            ) && u < 0x80
    };
    let control = |u: u16| matches!(u, 0..=0x1f | 0x80..=0x9f);
    let units: Vec<u16> = name.encode_utf16().collect();
    let mut out: Vec<u16> = Vec::new();
    let mut leading = true;
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        if reserved(u) || control(u) {
            out.push(u16::from(b'!'));
        } else if leading && u == u16::from(b'.') {
            while i + 1 < units.len() && units[i + 1] == u16::from(b'.') {
                i += 1;
            }
            out.push(u16::from(b'!'));
        } else {
            out.push(u);
        }
        if u != u16::from(b'.') || !leading {
            leading = false;
        }
        i += 1;
    }
    let mut collapsed: Vec<u16> = Vec::new();
    for &u in &out {
        if u == u16::from(b'!') && collapsed.last() == Some(&u16::from(b'!')) {
            continue;
        }
        collapsed.push(u);
    }
    if collapsed.len() > 1 {
        while collapsed.first() == Some(&u16::from(b'!')) {
            collapsed.remove(0);
        }
        while collapsed.last() == Some(&u16::from(b'!')) {
            collapsed.pop();
        }
    }
    let mut s = String::from_utf16_lossy(&collapsed);
    let lowered = JsString::from(s.as_str()).to_lower_case().to_string_lossy();
    if WINDOWS_NAMES.iter().any(|n| *n == lowered) {
        s.push('!');
    }
    let units: Vec<u16> = s.encode_utf16().take(100).collect();
    String::from_utf16_lossy(&units)
}

fn is_token(b: u8) -> bool {
    matches!(b,
        b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'0'..=b'9'
            | b'A'..=b'Z' | b'^' | b'_' | b'`' | b'a'..=b'z' | b'|' | b'~')
}

fn match_param(s: &[u16], at: usize) -> Option<(usize, String, String)> {
    let mut i = at;
    if s.get(i) != Some(&0x3b) {
        return None;
    }
    i += 1;
    while matches!(s.get(i), Some(&0x09) | Some(&0x20)) {
        i += 1;
    }
    let key_start = i;
    while i < s.len() && s[i] < 0x80 && is_token(s[i] as u8) {
        i += 1;
    }
    if i == key_start {
        return None;
    }
    let key = String::from_utf16_lossy(&s[key_start..i]);
    while matches!(s.get(i), Some(&0x09) | Some(&0x20)) {
        i += 1;
    }
    if s.get(i) != Some(&0x3d) {
        return None;
    }
    i += 1;
    while matches!(s.get(i), Some(&0x09) | Some(&0x20)) {
        i += 1;
    }
    let value = if s.get(i) == Some(&0x22) {
        i += 1;
        let mut value = String::new();
        loop {
            match s.get(i) {
                None => return None,
                Some(&0x22) => {
                    i += 1;
                    break;
                }
                Some(&0x5c) => match s.get(i + 1) {
                    Some(&c) if (0x20..=0x7e).contains(&c) => {
                        value.push(char::from_u32(u32::from(c)).unwrap_or('\u{fffd}'));
                        i += 2;
                    }
                    _ => return None,
                },
                Some(&u) if matches!(u, 0x20 | 0x21 | 0x23..=0x5b | 0x5d..=0x7e | 0x80..=0xff) => {
                    value.push(char::from_u32(u32::from(u)).unwrap_or('\u{fffd}'));
                    i += 1;
                }
                _ => return None,
            }
        }
        value
    } else {
        let value_start = i;
        while i < s.len() && s[i] < 0x80 && is_token(s[i] as u8) {
            i += 1;
        }
        if i == value_start {
            return None;
        }
        String::from_utf16_lossy(&s[value_start..i])
    };
    while matches!(s.get(i), Some(&0x09) | Some(&0x20)) {
        i += 1;
    }
    Some((i, key, value))
}

/// `content-disposition` 0.5.4 `parse`: returns the `filename` parameter (the
/// `filename*` form overwrites) or `Err` when the header does not parse.
pub fn disposition_filename(header: &str) -> Result<Option<String>, Invalid> {
    let s: Vec<u16> = header.encode_utf16().collect();
    let mut i = 0;
    while i < s.len() && s[i] < 0x80 && is_token(s[i] as u8) {
        i += 1;
    }
    if i == 0 {
        return Err(Invalid);
    }
    let mut j = i;
    while matches!(s.get(j), Some(&0x09) | Some(&0x20)) {
        j += 1;
    }
    if j != s.len() && s[j] != 0x3b {
        return Err(Invalid);
    }
    let mut index = j;
    let mut names: Vec<String> = Vec::new();
    let mut params: Vec<(String, String)> = Vec::new();
    while let Some((next, key_raw, value)) = match_param(&s, index) {
        index = next;
        let key = JsString::from(key_raw.as_str())
            .to_lower_case()
            .to_string_lossy();
        if names.contains(&key) {
            return Err(Invalid);
        }
        names.push(key.clone());
        if key.ends_with('*') {
            let key = key[..key.len() - 1].to_owned();
            let value = decode_ext_value(&value)?;
            if let Some(p) = params.iter_mut().find(|(k, _)| *k == key) {
                p.1 = value;
            } else {
                params.push((key, value));
            }
            continue;
        }
        if params.iter().any(|(k, _)| *k == key) {
            continue;
        }
        params.push((key, value));
    }
    if index != s.len() {
        return Err(Invalid);
    }
    Ok(params
        .iter()
        .find(|(k, _)| k == "filename")
        .map(|(_, v)| v.clone()))
}

fn decode_ext_value(value: &str) -> Result<String, Invalid> {
    let mut parts = value.splitn(3, '\'');
    let charset = JsString::from(parts.next().ok_or(Invalid)?)
        .to_lower_case()
        .to_string_lossy();
    if charset.is_empty()
        || !charset.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'+'
                        | b'-'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'{'
                        | b'}'
                        | b'~'
                )
        })
    {
        return Err(Invalid);
    }
    let language = parts.next().ok_or(Invalid)?;
    let alpha = |seg: &str| seg.bytes().all(|b| b.is_ascii_alphabetic());
    let mut segments = language.split('-');
    let primary = segments.next().unwrap_or_default();
    let subtags: Vec<&str> = segments.collect();
    let language_ok = language.is_empty()
        || (alpha(primary) && (4..=8).contains(&primary.len()) && subtags.is_empty())
        || (alpha(primary)
            && (2..=3).contains(&primary.len())
            && subtags.len() <= 3
            && subtags.iter().all(|t| t.len() == 3 && alpha(t)));
    if !language_ok {
        return Err(Invalid);
    }
    let encoded = parts.next().ok_or(Invalid)?;
    if encoded.is_empty()
        || !encoded.bytes().all(|b| {
            b == b'%'
                || b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#'
                        | b'$'
                        | b'&'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
    {
        return Err(Invalid);
    }
    let mut bytes = Vec::new();
    let mut chars = encoded.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let h: String = chars.by_ref().take(2).collect();
            if h.len() != 2 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(Invalid);
            }
            bytes.push(u8::from_str_radix(&h, 16).map_err(|_| Invalid)?);
        } else {
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    match charset.as_str() {
        "utf-8" => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        "iso-8859-1" => Ok(bytes
            .iter()
            .map(|&b| match b {
                0x20..=0x7e | 0xa0..=0xff => char::from(b),
                _ => '?',
            })
            .collect()),
        _ => Err(Invalid),
    }
}

/// `path.basename(new URL(href).pathname)` with the host's `path`; `Err` when `new URL` throws.
pub fn url_path_basename(href: &str, style: PathStyle) -> Result<String, Invalid> {
    let url = url::Url::parse(href).map_err(|_| Invalid)?;
    Ok(style
        .basename(&JsString::from(url.path()))
        .to_string_lossy())
}

/// `ext-name`'s `mime(header)`: every extension mime-db maps to exactly this string, in
/// `ext-list`'s length-sorted key order.
pub fn ext_name_mime(mime: &str) -> Vec<&'static str> {
    EXT_LIST
        .iter()
        .filter(|(_, m)| *m == mime)
        .map(|(ext, _)| *ext)
        .collect()
}
