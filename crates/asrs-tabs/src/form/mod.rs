//! The form controls upstream controllers keep their settings in, modelled on Chromium 142's
//! `HTMLInputElement`, `HTMLSelectElement` and `HTMLOptionElement`: what `value` and the other
//! properties read back after each write.

pub mod css;
pub mod decimal;
mod select;
mod step;

use asrs_js::number;
use asrs_js::string::JsString;

use decimal::Decimal;
pub use select::{OptGroup, Select, SelectItem, SelectOption, TypeAhead};

const HTML_SPACES: [u16; 5] = [0x09, 0x0A, 0x0C, 0x0D, 0x20];

fn is_digit(u: u16) -> bool {
    (u16::from(b'0')..=u16::from(b'9')).contains(&u)
}

/// Blink's first-character test shared by both number parsers: `String::ToDouble` and
/// `Decimal::FromString` accept a leading `+` or whitespace, a valid floating-point number does
/// not.
fn starts_like_number(s: &[u16]) -> bool {
    let first = s.first().copied().unwrap_or(0);
    first == u16::from(b'-') || first == u16::from(b'.') || is_digit(first)
}

/// What double-conversion's `StringToDouble` (allowing trailing junk) consumes in full: an
/// optional `-`, digits with at most one `.` and at least one digit, then an exponent that is
/// part of the number only when it has digits.
fn is_whole_double(s: &[u16]) -> bool {
    let mut i = usize::from(s.first() == Some(&u16::from(b'-')));
    let mut digits = 0;
    while s.get(i).copied().is_some_and(is_digit) {
        i += 1;
        digits += 1;
    }
    if s.get(i) == Some(&u16::from(b'.')) {
        i += 1;
        while s.get(i).copied().is_some_and(is_digit) {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return false;
    }
    if matches!(s.get(i), Some(&u) if u == u16::from(b'e') || u == u16::from(b'E')) {
        i += 1;
        if matches!(s.get(i), Some(&u) if u == u16::from(b'-') || u == u16::from(b'+')) {
            i += 1;
        }
        let start = i;
        while s.get(i).copied().is_some_and(is_digit) {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    i == s.len()
}

/// `ParseToDoubleForNumberType`: `None` where Blink returns the fallback, and `+0` for `-0`.
pub fn parse_to_double_for_number_type(s: &[u16]) -> Option<f64> {
    if !starts_like_number(s) || s.last() == Some(&u16::from(b'.')) || !is_whole_double(s) {
        return None;
    }
    let value = number::to_number(s);
    if !value.is_finite() {
        return None;
    }
    Some(if value == 0.0 { 0.0 } else { value })
}

/// `ParseToDecimalForNumberType`.
fn parse_to_decimal_for_number_type(s: &[u16], fallback: Decimal) -> Decimal {
    if !starts_like_number(s) {
        return fallback;
    }
    let value = Decimal::from_units(s);
    if !value.is_finite() {
        return fallback;
    }
    let double_max = Decimal::from_double(f64::MAX);
    if value.less_than(&double_max.negated()) || value.greater_than(&double_max) {
        return fallback;
    }
    if value.is_zero() {
        Decimal::from(0)
    } else {
        value
    }
}

/// `SerializeForNumberType(const Decimal&)`.
fn serialize_for_number_type(value: &Decimal) -> JsString {
    JsString::from(if value.is_zero() {
        if value.is_negative() { "-0" } else { "0" }.to_owned()
    } else {
        value.to_js_string()
    })
}

fn value_as_number(value: &JsString) -> f64 {
    parse_to_double_for_number_type(value.units()).unwrap_or(f64::NAN)
}

/// An `<input type="number">`. Assigning `value` keeps the string when
/// `ParseToDoubleForNumberType` reads it as a finite double, and empties it otherwise.
#[derive(Clone, Debug, Default)]
pub struct NumberInput {
    value: JsString,
    value_attribute: JsString,
    step: JsString,
}

impl NumberInput {
    /// The `value` attribute from markup (`""` when there is none).
    pub fn new(markup: &str) -> NumberInput {
        let mut input = NumberInput {
            value_attribute: JsString::from(markup),
            ..NumberInput::default()
        };
        input.set_value(JsString::from(markup));
        input
    }

    /// Without a `value` attribute, after `input.value = value`.
    pub fn with_value(value: &str) -> NumberInput {
        let mut input = NumberInput::default();
        input.set_value(JsString::from(value));
        input
    }

    /// The `step` attribute (`""` when there is none).
    pub fn step(&self) -> &JsString {
        &self.step
    }

    pub fn set_step(&mut self, attr: JsString) {
        self.step = attr;
    }

    /// The up or down arrow key, which dispatches `input` and `change` when the value changes.
    /// Returns whether it did.
    pub fn arrow_key(&mut self, up: bool) -> bool {
        let stepped = step::stepped(&self.value, &self.value_attribute, &self.step, up);
        let changed = stepped != self.value;
        self.set_value(stepped);
        changed
    }

    pub fn value(&self) -> &JsString {
        &self.value
    }

    pub fn set_value(&mut self, value: JsString) {
        let keep = value.is_empty() || parse_to_double_for_number_type(value.units()).is_some();
        self.value = if keep { value } else { JsString::new() };
    }

    /// The user edits the field, whose editor now shows `visible`.
    pub fn set_visible_value(&mut self, visible: &JsString) {
        self.set_value(convert_from_visible_value(visible));
    }

    /// `Number(input.value)`.
    pub fn number(&self) -> f64 {
        number::to_number(self.value.units())
    }

    pub fn value_as_number(&self) -> f64 {
        value_as_number(&self.value)
    }
}

/// `NumberInputType::HandleBeforeTextInsertedEvent` for en-US: what of `text` the user can insert
/// between `left` and `right` in a number field's editor.
pub fn insertable_number_text(left: &[u16], text: &[u16], right: &[u16]) -> Vec<u16> {
    let is_e = |u: u16| u == u16::from(b'e') || u == u16::from(b'E');
    let is_sign = |u: u16| u == u16::from(b'+') || u == u16::from(b'-');
    let point = u16::from(b'.');
    let has_e = |s: &[u16]| s.iter().any(|&u| is_e(u));
    let signs = |s: &[u16]| s.iter().filter(|&&u| is_sign(u)).count();
    let after_e = |s: &[u16]| s.last().is_some_and(|&u| is_e(u));
    let has_sign_not_after_e = |s: &[u16]| (0..s.len()).any(|i| is_sign(s[i]) && !after_e(&s[..i]));
    let mut left = left.to_vec();
    let start = left.len();
    for &unit in text {
        let unit = match unit {
            0xFF10..=0xFF19 => unit - 0xFF10 + u16::from(b'0'),
            0xFF0E => point,
            0xFF0D | 0x30FC => u16::from(b'-'),
            _ => unit,
        };
        let accepted = if unit == point {
            !left.contains(&point)
                && !right.contains(&point)
                && !has_e(&left)
                && !has_sign_not_after_e(right)
        } else if is_e(unit) {
            !has_e(&left) && !has_e(right) && !right.contains(&point)
        } else if is_sign(unit) {
            signs(&left) + signs(right) < 2
                && (left.is_empty() || after_e(&left) || !(has_e(&left) || has_e(right)))
        } else if is_digit(unit) {
            !(right.first().is_some_and(|&u| is_sign(u)) && (left.is_empty() || after_e(&left)))
        } else {
            false
        };
        if accepted {
            left.push(unit);
        }
    }
    left.split_off(start)
}

/// `NumberInputType::ConvertFromVisibleValue` for en-US: text with an exponent as it is, else
/// `Locale::ConvertFromLocalizedNumber`, which drops whitespace, one leading `+` and a trailing
/// `.`, and reads anything but digits and points after an optional `-` as empty.
fn convert_from_visible_value(visible: &JsString) -> JsString {
    let is = |u: u16, c: u8| u == u16::from(c);
    if visible.units().iter().any(|&u| is(u, b'e') || is(u, b'E')) {
        return visible.clone();
    }
    let input: Vec<u16> = visible
        .units()
        .iter()
        .copied()
        .filter(|u| !matches!(u, 0x09..=0x0D | 0x20))
        .collect();
    let (negative, mut digits) = match input.split_first() {
        Some((&sign, rest)) if is(sign, b'-') => (true, rest),
        _ => (false, &input[..]),
    };
    if !negative && digits.len() >= 2 && is(digits[0], b'+') {
        digits = &digits[1..];
    }
    if !digits.iter().all(|&u| is_digit(u) || is(u, b'.')) {
        return JsString::new();
    }
    let mut converted: Vec<u16> = Vec::with_capacity(digits.len() + 1);
    if negative {
        converted.push(u16::from(b'-'));
    }
    converted.extend_from_slice(digits);
    if converted.len() >= 2 && converted.last() == Some(&u16::from(b'.')) {
        converted.pop();
    }
    JsString::from_units(converted)
}

/// `StepRange` as `RangeInputType::CreateStepRange(kRejectAny)` builds it.
struct StepRange {
    minimum: Decimal,
    maximum: Decimal,
    step: Decimal,
    step_base: Decimal,
    has_step: bool,
}

impl StepRange {
    fn clamp_value(&self, value: &Decimal) -> Decimal {
        let below_max = if self.maximum.less_than(value) {
            self.maximum
        } else {
            *value
        };
        let in_range = if self.minimum.less_than(&below_max) {
            below_max
        } else {
            self.minimum
        };
        if !self.has_step {
            return in_range;
        }
        let rounded = self.step_base.plus(
            &in_range
                .minus(&self.step_base)
                .divided_by(&self.step)
                .round()
                .times(&self.step),
        );
        let clamped = if rounded.greater_than(&self.maximum) {
            rounded.minus(&self.step)
        } else if rounded.less_than(&self.minimum) {
            rounded.plus(&self.step)
        } else {
            rounded
        };
        if clamped.less_than(&self.minimum) || clamped.greater_than(&self.maximum) {
            in_range
        } else {
            clamped
        }
    }

    fn default_value(&self) -> Decimal {
        self.clamp_value(
            &self
                .minimum
                .plus(&self.maximum)
                .divided_by(&Decimal::from(2)),
        )
    }
}

/// An `<input type="range">`. Every `value` it holds is sanitized against the `min`, `max` and
/// `step` attributes: parsed as a `Decimal` (the default when that fails), clamped into range
/// and rounded to `step` from the step base. Changing `min`, `max` or `step` sanitizes the
/// current value again; changing the `value` attribute replaces it only while no script has set
/// `value`.
#[derive(Clone, Debug)]
pub struct RangeInput {
    value: JsString,
    dirty: bool,
    value_attribute: JsString,
    min: JsString,
    max: JsString,
    step: JsString,
}

impl RangeInput {
    /// The element as the parser creates it: the type is initialized once every attribute is
    /// set, so the `value` attribute is sanitized against the final `min`, `max` and `step`. An
    /// absent attribute reads as `""`, which parses as nothing just as a missing one does.
    pub fn new(value: &str, min: &str, max: &str, step: &str) -> RangeInput {
        let mut input = RangeInput {
            value: JsString::new(),
            dirty: false,
            value_attribute: JsString::from(value),
            min: JsString::from(min),
            max: JsString::from(max),
            step: JsString::from(step),
        };
        input.value = input.sanitize(&input.value_attribute);
        input
    }

    pub fn value(&self) -> &JsString {
        &self.value
    }

    pub fn min(&self) -> &JsString {
        &self.min
    }

    pub fn max(&self) -> &JsString {
        &self.max
    }

    pub fn step(&self) -> &JsString {
        &self.step
    }

    pub fn set_value(&mut self, value: JsString) {
        self.value = self.sanitize(&value);
        self.dirty = true;
    }

    /// `input.min = attr` (`setAttribute("min", attr)`).
    pub fn set_min(&mut self, attr: JsString) {
        self.min = attr;
        self.resanitize();
    }

    /// `input.max = attr`.
    pub fn set_max(&mut self, attr: JsString) {
        self.max = attr;
        self.resanitize();
    }

    /// `setAttribute("step", attr)`.
    pub fn set_step(&mut self, attr: JsString) {
        self.step = attr;
        self.resanitize();
    }

    /// `setAttribute("value", attr)`: also the step base while `min` does not parse.
    pub fn set_value_attribute(&mut self, attr: JsString) {
        self.value_attribute = attr;
        if !self.dirty {
            self.value = self.sanitize(&self.value_attribute);
        }
    }

    /// `Number(input.value)`.
    pub fn number(&self) -> f64 {
        number::to_number(self.value.units())
    }

    pub fn value_as_number(&self) -> f64 {
        value_as_number(&self.value)
    }

    fn resanitize(&mut self) {
        self.value = self.sanitize(&self.value);
    }

    fn step_range(&self) -> StepRange {
        let mut step_base = parse_to_decimal_for_number_type(self.min.units(), Decimal::nan());
        if !step_base.is_finite() {
            step_base =
                parse_to_decimal_for_number_type(self.value_attribute.units(), Decimal::from(0));
        }
        let minimum = parse_to_decimal_for_number_type(self.min.units(), Decimal::from(0));
        let maximum = parse_to_decimal_for_number_type(self.max.units(), Decimal::from(100));
        let maximum = if maximum.at_least(&minimum) {
            maximum
        } else {
            minimum
        };
        let step = self.parse_step();
        StepRange {
            minimum,
            maximum,
            step: if step.is_finite() {
                step
            } else {
                Decimal::from(1)
            },
            step_base: if step_base.is_finite() {
                step_base
            } else {
                Decimal::from(1)
            },
            has_step: step.is_finite(),
        }
    }

    /// `StepRange::ParseStep(kRejectAny, ...)` with the range type's default step 1.
    fn parse_step(&self) -> Decimal {
        let default = Decimal::from(1);
        if self.step.is_empty() {
            return default;
        }
        if self.step.to_string_lossy().eq_ignore_ascii_case("any") {
            return Decimal::nan();
        }
        let step = parse_to_decimal_for_number_type(self.step.units(), Decimal::nan());
        if !step.is_finite() || !step.greater_than(&Decimal::from(0)) {
            return default;
        }
        step.times(&Decimal::from(1))
    }

    /// `RangeInputType::SanitizeValue`.
    fn sanitize(&self, proposed: &JsString) -> JsString {
        let range = self.step_range();
        let value = parse_to_decimal_for_number_type(proposed.units(), range.default_value());
        serialize_for_number_type(&range.clamp_value(&value))
    }
}
