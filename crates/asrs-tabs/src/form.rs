//! The form controls upstream controllers keep their settings in, modelled by the strings
//! their `value` reports.

use asrs_js::number;
use asrs_js::string::JsString;

/// A `<select>`: option values and `selectedIndex`. `value` is the selected option's value,
/// or `""` when nothing is selected (`selectedIndex === -1`).
#[derive(Clone, Debug, Default)]
pub struct Select {
    options: Vec<JsString>,
    selected: Option<usize>,
}

impl Select {
    /// An empty `<select>`: `selectedIndex === -1`.
    pub fn new() -> Select {
        Select::default()
    }

    /// Options in markup order; parsing selects the first.
    pub fn with_options(options: Vec<JsString>) -> Select {
        let mut select = Select::new();
        select.replace_options(options);
        select
    }

    pub fn value(&self) -> JsString {
        self.selected
            .and_then(|i| self.options.get(i))
            .cloned()
            .unwrap_or_default()
    }

    /// `select.value = value`: the first option with that value, else `selectedIndex = -1`.
    /// The setter does not run the selectedness algorithm, so nothing is selected instead.
    pub fn set_value(&mut self, value: &JsString) {
        self.selected = self.options.iter().position(|o| o == value);
    }

    /// `select.selectedIndex = index`; out of range selects nothing.
    pub fn set_selected_index(&mut self, index: usize) {
        self.selected = (index < self.options.len()).then_some(index);
    }

    pub fn options(&self) -> &[JsString] {
        &self.options
    }

    /// Removing every option, then appending `options`: inserting an option into a
    /// select with nothing selected selects it, so the first ends up selected.
    pub fn replace_options(&mut self, options: Vec<JsString>) {
        self.selected = (!options.is_empty()).then_some(0);
        self.options = options;
    }
}

/// The `value` of an `<option>` without a `value` attribute whose `innerText` was set to
/// `text`: line breaks become `<br>` elements, and the value is the remaining text with
/// ASCII whitespace stripped and collapsed.
pub fn option_value_from_inner_text(text: &JsString) -> JsString {
    let mut out: Vec<u16> = Vec::new();
    let mut pending_space = false;
    for &u in text.units() {
        match u {
            0x0A | 0x0D => {}
            0x09 | 0x0C | 0x20 => pending_space = !out.is_empty(),
            _ => {
                if std::mem::take(&mut pending_space) {
                    out.push(0x20);
                }
                out.push(u);
            }
        }
    }
    JsString::from_units(out)
}

/// An `<input type="number">`. Assigning `value` runs the value sanitization algorithm:
/// anything but a valid floating-point number that parses to a finite double becomes `""`.
#[derive(Clone, Debug)]
pub struct NumberInput {
    value: JsString,
}

impl NumberInput {
    /// The `value` attribute from markup.
    pub fn new(markup: &str) -> NumberInput {
        let mut input = NumberInput {
            value: JsString::new(),
        };
        input.set_value(JsString::from(markup));
        input
    }

    pub fn value(&self) -> &JsString {
        &self.value
    }

    pub fn set_value(&mut self, value: JsString) {
        let valid = is_valid_floating_point_number(value.units())
            && number::to_number(value.units()).is_finite();
        self.value = if valid { value } else { JsString::new() };
    }

    /// `Number(input.value)`.
    pub fn number(&self) -> f64 {
        number::to_number(self.value.units())
    }
}

/// `/^-?(?:\d+|\d*\.\d+)(?:[eE][-+]?\d+)?$/`, jsdom's (and the HTML spec's) grammar.
fn is_valid_floating_point_number(s: &[u16]) -> bool {
    let digits = |i: &mut usize| {
        let start = *i;
        while s.get(*i).is_some_and(|&u| (0x30..=0x39).contains(&u)) {
            *i += 1;
        }
        *i > start
    };
    let mut i = 0;
    if s.first() == Some(&u16::from(b'-')) {
        i += 1;
    }
    let integer = digits(&mut i);
    if s.get(i) == Some(&u16::from(b'.')) {
        i += 1;
        if !digits(&mut i) {
            return false;
        }
    } else if !integer {
        return false;
    }
    if matches!(s.get(i), Some(&u) if u == u16::from(b'e') || u == u16::from(b'E')) {
        i += 1;
        if matches!(s.get(i), Some(&u) if u == u16::from(b'-') || u == u16::from(b'+')) {
            i += 1;
        }
        if !digits(&mut i) {
            return false;
        }
    }
    i == s.len()
}

/// An `<input type="range">` as jsdom models it. Assigning `value` keeps a valid
/// floating-point number that parses finite and lies in `[min, max]` as written; one
/// below `min` or above `max` becomes that bound, and anything else becomes the default
/// (`min` when `max` is below it, else the midpoint). Nothing rounds to `step` and
/// changing `min` or `max` does not re-sanitize. Chromium differs on all three (#263).
#[derive(Clone, Debug)]
pub struct RangeInput {
    value: JsString,
    min: JsString,
    max: JsString,
}

impl RangeInput {
    /// The `value`, `min` and `max` attributes from markup. jsdom sanitizes the `value`
    /// attribute against the `min` and `max` present when it is parsed, and upstream's
    /// markup lists `value` first, so it is kept as written.
    pub fn new(value: &str, min: &str, max: &str) -> RangeInput {
        RangeInput {
            value: JsString::from(value),
            min: JsString::from(min),
            max: JsString::from(max),
        }
    }

    pub fn value(&self) -> &JsString {
        &self.value
    }

    pub fn max(&self) -> &JsString {
        &self.max
    }

    pub fn set_value(&mut self, value: JsString) {
        let (min, max) = (self.minimum(), self.maximum());
        self.value = if !is_valid_floating_point_number(value.units())
            || parse_floating_point_number(&value).is_none()
        {
            JsString::from(number::to_string(if max < min {
                min
            } else {
                (min + max) / 2.0
            }))
        } else {
            let n = number::to_number(value.units());
            if n < min {
                JsString::from(number::to_string(min))
            } else if n > max {
                JsString::from(number::to_string(max))
            } else {
                value
            }
        };
    }

    /// `input.min = attr` / `input.max = attr`: the attribute stores the raw string and
    /// re-parses on each read.
    pub fn set_min(&mut self, attr: JsString) {
        self.min = attr;
    }

    pub fn set_max(&mut self, attr: JsString) {
        self.max = attr;
    }

    /// `Number(input.value)`.
    pub fn number(&self) -> f64 {
        number::to_number(self.value.units())
    }

    fn minimum(&self) -> f64 {
        parse_floating_point_number(&self.min).unwrap_or(0.0)
    }

    fn maximum(&self) -> f64 {
        parse_floating_point_number(&self.max).unwrap_or(100.0)
    }
}

/// jsdom's `parseFloatingPointNumber`: `parseFloat` after a leading run of ASCII
/// whitespace only (any other whitespace `trimStart` would skip fails), finite or `None`.
fn parse_floating_point_number(s: &JsString) -> Option<f64> {
    let skipped = s.len() - s.trim_start().len();
    if s.units()[..skipped]
        .iter()
        .any(|u| !matches!(u, 0x09 | 0x0A | 0x0C | 0x0D | 0x20))
    {
        return None;
    }
    Some(number::parse_float(s.units())).filter(|n| n.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_input_sanitizes() {
        let cases = [
            ("5", "5"),
            ("-0.5", "-0.5"),
            (".5", ".5"),
            ("-.5", "-.5"),
            ("1e3", "1e3"),
            ("1E+3", "1E+3"),
            ("2e-1", "2e-1"),
            ("5.", ""),
            ("+5", ""),
            (" 5", ""),
            ("5 ", ""),
            ("abc", ""),
            ("NaN", ""),
            ("Infinity", ""),
            ("1e400", ""),
            ("", ""),
            ("-", ""),
            ("1e", ""),
            ("0x10", ""),
        ];
        for (input, expected) in cases {
            let mut n = NumberInput::new("1");
            n.set_value(JsString::from(input));
            assert_eq!(n.value(), &JsString::from(expected), "{input:?}");
        }
    }

    #[test]
    fn option_values_strip_and_collapse() {
        for (text, expected) in [
            ("Xbox Controller", "Xbox Controller"),
            ("  Pad \t X  ", "Pad X"),
            ("Two\nLines", "TwoLines"),
            ("a \r\n b", "a b"),
            ("   ", ""),
        ] {
            assert_eq!(
                option_value_from_inner_text(&JsString::from(text)),
                JsString::from(expected),
                "{text:?}"
            );
        }
    }

    #[test]
    fn range_input_sanitizes_like_jsdom() {
        let mut r = RangeInput::new("0", "1", "1");
        r.set_max(JsString::from("300"));
        assert_eq!(r.value(), &JsString::from("0"));
        for (input, expected) in [
            ("59.639997482299805", "59.639997482299805"),
            ("1.4999999999999999", "1.4999999999999999"),
            ("7.000000000000001", "7.000000000000001"),
            ("1e1", "1e1"),
            ("abc", "150.5"),
            ("", "150.5"),
            ("1e400", "150.5"),
            ("+5", "150.5"),
            ("1e3", "300"),
            ("301", "300"),
            ("-5", "1"),
            ("0", "1"),
            ("45", "45"),
        ] {
            r.set_value(JsString::from(input));
            assert_eq!(r.value(), &JsString::from(expected), "{input:?}");
        }
        r.set_value(JsString::from("300"));
        r.set_max(JsString::from("100"));
        assert_eq!(r.value(), &JsString::from("300"));
        r.set_max(JsString::from("10.5"));
        r.set_value(JsString::from("11"));
        assert_eq!(r.value(), &JsString::from("10.5"));
        r.set_max(JsString::from("0"));
        r.set_value(JsString::from("5"));
        assert_eq!(r.value(), &JsString::from("0"));
        r.set_value(JsString::from("abc"));
        assert_eq!(r.value(), &JsString::from("1"));
        for (max, expected) in [
            ("abc", "50.5"),
            ("1e400", "50.5"),
            (" 120", "60.5"),
            ("121x", "61"),
            ("\u{a0}120", "50.5"),
        ] {
            r.set_max(JsString::from(max));
            r.set_value(JsString::from("abc"));
            assert_eq!(r.value(), &JsString::from(expected), "{max:?}");
        }
    }
}
