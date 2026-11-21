//! The form controls upstream controllers keep their settings in, modelled by the strings
//! their `value` reports.

use asrs_js::math;
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

/// An `<input type="range">`. Assigning `value` validates against the floating-point
/// grammar, falls back to the default (mid-range, or `min` when `max` is below it) and
/// clamps into `[min, max]`. jsdom does not round to `step` on assignment, so neither
/// does this.
#[derive(Clone, Debug)]
pub struct RangeInput {
    value: JsString,
    min: JsString,
    max: JsString,
}

impl RangeInput {
    /// The `value`, `min` and `max` attributes from markup. jsdom stores the `value`
    /// attribute as-is; sanitization runs only when the `value` setter is assigned.
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

    pub fn set_value(&mut self, value: JsString) {
        let n = if is_valid_floating_point_number(value.units()) {
            number::to_number(value.units())
        } else {
            self.default_value()
        };
        let clamped = math::min2(math::max2(n, self.minimum()), self.maximum());
        self.value = JsString::from(number::to_string(clamped));
    }

    /// `input.min = attr` / `input.max = attr`: the attribute stores the raw string and
    /// re-parses on each read; sanitization does not re-run.
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
        let n = parse_floating_point_number(self.min.units());
        if n.is_nan() { 0.0 } else { n }
    }

    fn maximum(&self) -> f64 {
        let n = parse_floating_point_number(self.max.units());
        let max = if n.is_nan() { 100.0 } else { n };
        let min = self.minimum();
        if max < min { min } else { max }
    }

    fn default_value(&self) -> f64 {
        (self.minimum() + self.maximum()) / 2.0
    }
}

/// The HTML spec's "rules for parsing floating-point number values", used for the `min`
/// and `max` attributes: leading ASCII whitespace skipped, then the longest prefix that
/// is a valid floating-point number, else NaN.
fn parse_floating_point_number(s: &[u16]) -> f64 {
    let mut i = 0;
    while s
        .get(i)
        .is_some_and(|&u| matches!(u, 0x09 | 0x0A | 0x0C | 0x0D | 0x20))
    {
        i += 1;
    }
    let start = i;
    if matches!(s.get(i), Some(&u) if u == u16::from(b'-') || u == u16::from(b'+')) {
        i += 1;
    }
    let digits = |i: &mut usize| {
        while s.get(*i).is_some_and(|&u| (0x30..=0x39).contains(&u)) {
            *i += 1;
        }
    };
    digits(&mut i);
    if s.get(i) == Some(&u16::from(b'.')) {
        i += 1;
        digits(&mut i);
    }
    if i == start || s[start..i].iter().all(|&u| !(0x30..=0x39).contains(&u)) {
        return f64::NAN;
    }
    if matches!(s.get(i), Some(&u) if u == u16::from(b'e') || u == u16::from(b'E')) {
        let mut j = i + 1;
        if matches!(s.get(j), Some(&u) if u == u16::from(b'-') || u == u16::from(b'+')) {
            j += 1;
        }
        let exp_start = j;
        digits(&mut j);
        if j > exp_start {
            i = j;
        }
    }
    number::parse_float(&s[start..i])
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
}
