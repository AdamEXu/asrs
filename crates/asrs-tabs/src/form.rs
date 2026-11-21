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
