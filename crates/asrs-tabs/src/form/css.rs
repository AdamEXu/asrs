//! An element's inline `style` for the properties upstream controllers write, as Chromium 142
//! parses and serializes them: `style.<prop> = v` keeps the specified value only when it is
//! valid for the property (an invalid one leaves the previous value), `""` removes the
//! declaration, and reads return the serialized specified value.

use asrs_canvas::color;
use asrs_js::{math, number};
use asrs_js::string::JsString;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Property {
    Width,
    Height,
    Left,
    Top,
    Right,
    Color,
    BackgroundColor,
    Opacity,
    Display,
}

impl Property {
    /// The `CSSStyleDeclaration` attribute name.
    pub fn from_idl(name: &str) -> Option<Property> {
        Some(match name {
            "width" => Property::Width,
            "height" => Property::Height,
            "left" => Property::Left,
            "top" => Property::Top,
            "right" => Property::Right,
            "color" => Property::Color,
            "backgroundColor" => Property::BackgroundColor,
            "opacity" => Property::Opacity,
            "display" => Property::Display,
            _ => return None,
        })
    }

    fn css_name(self) -> &'static str {
        match self {
            Property::Width => "width",
            Property::Height => "height",
            Property::Left => "left",
            Property::Top => "top",
            Property::Right => "right",
            Property::Color => "color",
            Property::BackgroundColor => "background-color",
            Property::Opacity => "opacity",
            Property::Display => "display",
        }
    }

    fn parse(self, value: &str) -> Option<String> {
        let lower = value.to_ascii_lowercase();
        if CSS_WIDE_KEYWORDS.contains(&lower.as_str()) {
            return Some(lower);
        }
        match self {
            Property::Width | Property::Height => {
                if SIZE_KEYWORDS.contains(&lower.as_str()) {
                    return Some(lower);
                }
                length_percentage(&lower, false)
            }
            Property::Left | Property::Top | Property::Right => {
                if lower == "auto" {
                    return Some(lower);
                }
                length_percentage(&lower, true)
            }
            Property::Color | Property::BackgroundColor => color_value(&lower),
            Property::Opacity => {
                let (n, rest) = css_number(&lower)?;
                match rest {
                    "" => Some(format_number(n)),
                    "%" => Some(format_number(n / 100.0)),
                    _ => None,
                }
            }
            Property::Display => DISPLAY_KEYWORDS
                .contains(&lower.as_str())
                .then_some(lower),
        }
    }
}

const CSS_WIDE_KEYWORDS: [&str; 5] = ["initial", "inherit", "unset", "revert", "revert-layer"];
const SIZE_KEYWORDS: [&str; 4] = ["auto", "min-content", "max-content", "fit-content"];
const DISPLAY_KEYWORDS: [&str; 13] = [
    "none",
    "block",
    "inline",
    "inline-block",
    "flex",
    "inline-flex",
    "grid",
    "inline-grid",
    "contents",
    "flow-root",
    "list-item",
    "table",
    "table-cell",
];
const LENGTH_UNITS: [&str; 16] = [
    "px", "em", "rem", "ex", "ch", "vw", "vh", "vmin", "vmax", "cm", "mm", "q", "in", "pt", "pc",
    "%",
];

/// A CSS `<number-token>` at the start of `s` and the rest of `s`.
fn css_number(s: &str) -> Option<(f64, &str)> {
    let b = s.as_bytes();
    let mut i = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    let int_start = i;
    while b.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    let mut digits = i - int_start;
    if b.get(i) == Some(&b'.') && b.get(i + 1).is_some_and(u8::is_ascii_digit) {
        i += 1;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return None;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        if b.get(j).is_some_and(u8::is_ascii_digit) {
            while b.get(j).is_some_and(u8::is_ascii_digit) {
                j += 1;
            }
            i = j;
        }
    }
    let units: Vec<u16> = s[..i].encode_utf16().collect();
    let n = number::to_number(&units);
    n.is_finite().then_some((n, &s[i..]))
}

fn length_percentage(s: &str, negative: bool) -> Option<String> {
    let (n, unit) = css_number(s)?;
    if !negative && n < 0.0 {
        return None;
    }
    if unit.is_empty() {
        return (n == 0.0).then(|| "0px".to_owned());
    }
    LENGTH_UNITS
        .contains(&unit)
        .then(|| format!("{}{unit}", format_number(n)))
}

/// Blink's `FormatNumber`: `%.6g`, with `-0` written as `0`.
fn format_number(n: f64) -> String {
    if n == 0.0 {
        return "0".to_owned();
    }
    let scientific = format!("{n:.5e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let units: Vec<u16> = exponent.encode_utf16().collect();
    let exponent = number::to_number(&units) as i32;
    if !(-4..6).contains(&exponent) {
        let mantissa = trim_fraction(mantissa);
        let sign = if exponent < 0 { '-' } else { '+' };
        return format!("{mantissa}e{sign}{:02}", exponent.unsigned_abs());
    }
    let decimals = usize::try_from(5 - exponent).unwrap_or(0);
    trim_fraction(&format!("{n:.decimals$}")).to_owned()
}

fn trim_fraction(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

fn color_value(lower: &str) -> Option<String> {
    if lower == "currentcolor" || lower == "transparent" {
        return Some(lower.to_owned());
    }
    let rgba = color::parse(lower)?;
    if lower.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Some(lower.to_owned());
    }
    let [r, g, b] = rgba.to_u8();
    if rgba.a >= 1.0 {
        return Some(format!("rgb({r}, {g}, {b})"));
    }
    Some(format!("rgba({r}, {g}, {b}, {})", serialize_alpha(rgba.a)))
}

/// CSSOM's `<alphavalue>`: two decimals when they round-trip through 8 bits, else three.
fn serialize_alpha(alpha: f32) -> String {
    let alpha = f64::from(alpha);
    let byte = math::round(alpha * 255.0);
    let two = math::round(alpha * 100.0) / 100.0;
    let value = if math::round(two * 255.0) == byte {
        two
    } else {
        math::round(alpha * 1000.0) / 1000.0
    };
    number::to_string(value)
}

/// The declarations of one element's inline style, in the order they were added.
#[derive(Clone, Debug, Default)]
pub struct Style {
    declarations: Vec<(Property, String)>,
}

impl Style {
    pub fn new() -> Style {
        Style::default()
    }

    /// `style.<prop> = value`.
    pub fn set(&mut self, property: Property, value: &JsString) {
        let text = value.to_string_lossy();
        let trimmed = text.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c'));
        if trimmed.is_empty() {
            self.declarations.retain(|(p, _)| *p != property);
            return;
        }
        let Some(parsed) = property.parse(trimmed) else {
            return;
        };
        match self.declarations.iter_mut().find(|(p, _)| *p == property) {
            Some((_, v)) => *v = parsed,
            None => self.declarations.push((property, parsed)),
        }
    }

    /// `style.<prop>`.
    pub fn get(&self, property: Property) -> JsString {
        self.declarations
            .iter()
            .find(|(p, _)| *p == property)
            .map(|(_, v)| JsString::from(v.as_str()))
            .unwrap_or_default()
    }

    /// `style.cssText`.
    pub fn css_text(&self) -> JsString {
        let text: Vec<String> = self
            .declarations
            .iter()
            .map(|(p, v)| format!("{}: {v};", p.css_name()))
            .collect();
        JsString::from(text.join(" "))
    }
}
