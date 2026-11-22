// Ported from Blink, Chromium m142 (BSD-3-Clause, Copyright The Chromium Authors). See THIRD_PARTY_NOTICES.md.

//! Blink's parse and serialization of the inline style values the Mechanism and Points
//! renderers assign (`element.style.<property> = value`).

use asrs_canvas::color::{self, Parsed, Slow};
use asrs_js::string::JsError;
use asrs_js::{math, number};

use crate::form::css::format_number;

/// What `element.style.getPropertyValue(property)` returns after `element.style.<property> =
/// value` on an element without that declaration; `Ok(None)` when Blink rejects the value and
/// the declaration is left as it was. `value` is never empty (an empty value removes the
/// declaration instead of being parsed). `quirks` is true on SVG elements, whose inline style
/// Blink parses with quirks-mode behaviour even in a standards-mode document. A property or
/// value outside what is ported here is an `Unmodeled` error rather than a guess.
pub fn serialize(property: &str, value: &str, quirks: bool) -> Result<Option<String>, JsError> {
    let trimmed = trim(value);
    let lower = trimmed.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "inherit" | "initial" | "unset" | "revert" | "revert-layer"
    ) {
        return Ok(Some(lower));
    }
    let unmodeled = || JsError::new("Unmodeled", format!("inline style {property}: {value:?}"));
    Ok(match property {
        "left" | "top" => length(value, true, quirks),
        "width" | "height" => length(value, false, quirks),
        "stroke-width" => stroke_width(trimmed),
        "stroke" | "fill" => return color_value(value, false, true).ok_or_else(unmodeled),
        "background-color" => return color_value(value, quirks, false).ok_or_else(unmodeled),
        "display" => DISPLAY.contains(&lower.as_str()).then_some(lower),
        "transform" => TRANSFORMS
            .iter()
            .find(|(written, _)| *written == value)
            .map(|(_, serialized)| (*serialized).to_owned()),
        _ => return Err(unmodeled()),
    })
}

const DISPLAY: [&str; 22] = [
    "inline",
    "block",
    "list-item",
    "inline-block",
    "table",
    "inline-table",
    "table-row-group",
    "table-header-group",
    "table-footer-group",
    "table-row",
    "table-column-group",
    "table-column",
    "table-cell",
    "table-caption",
    "flex",
    "inline-flex",
    "grid",
    "inline-grid",
    "contents",
    "flow-root",
    "none",
    "math",
];

const TRANSFORMS: [(&str, &str); 3] = [
    (
        "translate(-50%,-50%) scale(1, 1)",
        "translate(-50%, -50%) scale(1, 1)",
    ),
    (
        "translate(-50%,-50%) scale(0.5, 0.5)",
        "translate(-50%, -50%) scale(0.5, 0.5)",
    ),
    (
        "translate(-50%,-50%) scale(0.25, 0.25)",
        "translate(-50%, -50%) scale(0.25, 0.25)",
    ),
];

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c')
}

fn trim(s: &str) -> &str {
    s.trim_matches(is_space)
}

#[derive(Clone, Copy, PartialEq)]
enum Unit {
    Number,
    Pixels,
    Percent,
}

impl Unit {
    fn suffix(self) -> &'static str {
        match self {
            Unit::Number => "",
            Unit::Pixels => "px",
            Unit::Percent => "%",
        }
    }
}

fn clamp_to_float(n: f64) -> f64 {
    math::min2(math::max2(n, -f64::from(f32::MAX)), f64::from(f32::MAX))
}

/// `ParseSimpleLengthValue`, then the tokenizer path when the fast path declines.
fn length(value: &str, negative: bool, quirks: bool) -> Option<String> {
    let parsed = simple_length(value)
        .and_then(|(n, unit)| match unit {
            Unit::Number if n == 0.0 => Some((n, Unit::Pixels)),
            Unit::Number => None,
            _ => Some((n, unit)),
        })
        .filter(|&(n, _)| negative || n >= 0.0);
    let (n, unit) = match parsed {
        Some(p) => p,
        None => {
            let (n, unit) = numeric_token(trim(value))?;
            let unit = match unit {
                Unit::Number if n == 0.0 || quirks => Unit::Pixels,
                Unit::Number => return None,
                u => u,
            };
            if !negative && n < 0.0 {
                return None;
            }
            (n, unit)
        }
    };
    Some(format!("{}{}", format_number(n), unit.suffix()))
}

fn stroke_width(trimmed: &str) -> Option<String> {
    let (n, unit) = numeric_token(trimmed)?;
    (n >= 0.0).then(|| format!("{}{}", format_number(n), unit.suffix()))
}

/// `ParseSimpleLength`: a `px` or `%` suffix, then `ParseDoubleWithPrefix`.
fn simple_length(value: &str) -> Option<(f64, Unit)> {
    let b = value.as_bytes();
    let (digits, unit) = if b.len() > 2 && b[b.len() - 2..].eq_ignore_ascii_case(b"px") {
        (&b[..b.len() - 2], Unit::Pixels)
    } else if b.len() > 1 && b[b.len() - 1] == b'%' {
        (&b[..b.len() - 1], Unit::Percent)
    } else {
        (b, Unit::Number)
    };
    let digits = &digits[digits
        .iter()
        .take_while(|c| is_space(char::from(**c)))
        .count()..];
    let n = match digits.split_first() {
        None => return None,
        Some((b'-', [])) => return None,
        Some((b'-', rest)) => -positive_double(rest)?,
        Some(_) => positive_double(digits)?,
    };
    Some((clamp_to_float(n), unit))
}

/// `ParsePositiveDouble`, which must consume all of `chars`.
fn positive_double(chars: &[u8]) -> Option<f64> {
    let (length, value) = color::parse_positive_double(chars);
    (length != 0 && length == chars.len()).then_some(value)
}

/// A CSS `<number-token>` at the start of `s`: its value, whether it has integer type, and the
/// rest of `s`.
fn number_token(s: &str) -> Option<(f64, bool, &str)> {
    let b = s.as_bytes();
    let digit = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
    let mut i = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    let start = i;
    while digit(i) {
        i += 1;
    }
    let mut integer = true;
    if b.get(i) == Some(&b'.') && digit(i + 1) {
        integer = false;
        i += 1;
        while digit(i) {
            i += 1;
        }
    }
    if i == start {
        return None;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(b.get(i + 1), Some(b'+' | b'-')));
        if digit(i + 1 + sign) {
            integer = false;
            i += 1 + sign;
            while digit(i) {
                i += 1;
            }
        }
    }
    let units: Vec<u16> = s[..i].encode_utf16().collect();
    let n = number::to_number(&units);
    Some((n, integer, &s[i..]))
}

/// One number, percentage or `px` dimension token making up all of `s`, clamped as Blink
/// clamps lengths.
fn numeric_token(s: &str) -> Option<(f64, Unit)> {
    let (n, _, rest) = number_token(s)?;
    let unit = match rest {
        "" => Unit::Number,
        "%" => Unit::Percent,
        r if r.eq_ignore_ascii_case("px") => Unit::Pixels,
        _ => return None,
    };
    Some((clamp_to_float(n), unit))
}

fn is_hex(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The hashless hex colours quirks mode accepts (`FastParseColorInternal`, then the
/// tokenizer's `ParseHexColor`).
fn quirky_color(value: &str) -> Option<Parsed> {
    if matches!(value.len(), 3 | 6) && is_hex(value) {
        return color::parse_hex(value);
    }
    let trimmed = trim(value);
    let hex = if trimmed.starts_with(|c: char| c.is_ascii_alphabetic()) {
        trimmed.to_owned()
    } else {
        let (n, integer, unit) = number_token(trimmed)?;
        if !integer || !(0.0..1_000_000.0).contains(&n) || !is_hex(unit) {
            return None;
        }
        format!("{:0>6}", format!("{}{unit}", n as i32))
    };
    (matches!(hex.len(), 3 | 6) && is_hex(&hex))
        .then(|| color::parse_hex(&hex))
        .flatten()
}

/// The colour keywords Blink accepts in author style besides the named colours, probed over
/// every keyword of Chromium m142's `css_value_keywords.json5`.
const COLOR_KEYWORDS: [&str; 44] = [
    "-webkit-activelink",
    "-webkit-link",
    "activeborder",
    "activecaption",
    "activetext",
    "appworkspace",
    "background",
    "buttonborder",
    "buttonface",
    "buttonhighlight",
    "buttonshadow",
    "buttontext",
    "canvas",
    "canvastext",
    "captiontext",
    "currentcolor",
    "field",
    "fieldtext",
    "graytext",
    "highlight",
    "highlighttext",
    "inactiveborder",
    "inactivecaption",
    "inactivecaptiontext",
    "infobackground",
    "infotext",
    "linktext",
    "mark",
    "marktext",
    "menu",
    "menutext",
    "scrollbar",
    "selecteditem",
    "selecteditemtext",
    "threeddarkshadow",
    "threedface",
    "threedhighlight",
    "threedlightshadow",
    "threedshadow",
    "transparent",
    "visitedtext",
    "window",
    "windowframe",
    "windowtext",
];

const PAINT_KEYWORDS: [&str; 3] = ["context-fill", "context-stroke", "none"];

fn is_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_') || b >= 0x80
}

/// A whole `<ident-token>` without escapes.
fn is_ident(s: &str) -> bool {
    let starts = match s.as_bytes() {
        [b'-', b'-', ..] => true,
        [b'-', c, ..] | [c, ..] => !c.is_ascii_digit() && *c != b'-',
        [] => false,
    };
    starts && s.bytes().all(is_name_char)
}

/// A `stroke`/`fill` (`paint`) or colour value: `Some(Ok)` as Blink serializes it, `Some(None)`
/// when Blink rejects it, `None` when it is outside the modelled subset: keywords, `#` hex,
/// `rgb[a]()` and `hsl[a]()` in the fast path's syntax, `rgb[a]()` of plain number and
/// percentage tokens, and quirky hashless hex.
fn color_value(value: &str, quirky: bool, paint: bool) -> Option<Option<String>> {
    let trimmed = trim(value);
    let lower = trimmed.to_ascii_lowercase();
    if is_ident(trimmed) {
        let keyword = color::named(&lower).is_some()
            || COLOR_KEYWORDS.contains(&lower.as_str())
            || (paint && PAINT_KEYWORDS.contains(&lower.as_str()));
        if keyword {
            return Some(Some(lower));
        }
        return Some(
            quirky
                .then(|| quirky_color(value))
                .flatten()
                .map(serialize_color),
        );
    }
    if let Some(parsed) = color::parse_fast(value) {
        return Some(Some(serialize_color(parsed)));
    }
    if let Some(parsed) = quirky.then(|| quirky_color(value)).flatten() {
        return Some(Some(serialize_color(parsed)));
    }
    if let Some(hex) = trimmed
        .strip_prefix('#')
        .filter(|hex| hex.bytes().all(is_name_char))
    {
        return Some(color::parse_hex(hex).map(serialize_color));
    }
    let blocks_or_escapes =
        trimmed.contains(['(', ')', '[', ']', '{', '}', '"', '\'', '\\']) || trimmed.contains("/*");
    if !blocks_or_escapes {
        return Some(None);
    }
    let open = trimmed.find('(')?;
    let name = &trimmed[..open];
    if name.contains(is_space) && !name.contains("/*") {
        return Some(None);
    }
    if !matches!(name.to_ascii_lowercase().as_str(), "rgb" | "rgba") {
        return None;
    }
    match color::parse_rgb_tokens(&trimmed[open + 1..]) {
        Slow::Valid(parsed) => Some(Some(serialize_color(parsed))),
        Slow::Invalid => Some(None),
        Slow::Unmodeled => None,
    }
}

/// `Color::SerializeLegacyColorAsCSSColor`. Every value it rounds is non-negative, where C's
/// `round` and `Math.round` agree.
fn serialize_color(parsed: Parsed) -> String {
    let [r, g, b] = parsed.css_channels();
    let a = parsed.alpha();
    if a >= 1.0 {
        return format!("rgb({r}, {g}, {b})");
    }
    let int_alpha = math::min2(math::round(f64::from(a + 1e-7) * 255.0), 255.0);
    let hundredths = math::round(int_alpha * 100.0 / 255.0);
    let two_decimal = (hundredths / 100.0) as f32;
    let alpha = if math::round(f64::from(two_decimal * 255.0)) == int_alpha {
        decimal(hundredths as u32, 2)
    } else {
        decimal(math::round(int_alpha * 1000.0 / 255.0) as u32, 3)
    };
    format!("rgba({r}, {g}, {b}, {alpha})")
}

/// `units / 10^places` without trailing zeros, for `units` in `0..=10^places`.
fn decimal(units: u32, places: usize) -> String {
    let scale = 10u32.pow(places as u32);
    let fraction = format!("{:0places$}", units % scale);
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() {
        format!("{}", units / scale)
    } else {
        format!("{}.{fraction}", units / scale)
    }
}
