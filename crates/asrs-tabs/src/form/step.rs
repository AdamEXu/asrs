//! Stepping an `<input type="number">` without `min` or `max` with the arrow keys:
//! `InputType::StepUpFromLayoutObject` and `ApplyStep`.

use asrs_js::string::JsString;

use super::decimal::Decimal;
use super::{parse_to_decimal_for_number_type, serialize_for_number_type};

struct Step {
    base: Decimal,
    step: Decimal,
    any: bool,
}

impl Step {
    fn new(step: &JsString, value_attribute: &JsString) -> Step {
        let any = step.to_string_lossy().eq_ignore_ascii_case("any");
        let parsed = parse_to_decimal_for_number_type(step.units(), Decimal::nan());
        let valid = parsed.is_finite() && parsed.greater_than(&Decimal::from(0));
        Step {
            base: parse_to_decimal_for_number_type(value_attribute.units(), Decimal::from(0)),
            step: if valid { parsed } else { Decimal::from(1) },
            any,
        }
    }

    /// `StepRange::StepMismatch`.
    fn mismatch(&self, value: &Decimal) -> bool {
        let distance = value.minus(&self.base).abs();
        let mantissa = Decimal::from_double((1u64 << f64::MANTISSA_DIGITS) as f64);
        if distance.divided_by(&mantissa).greater_than(&self.step) {
            return false;
        }
        let steps = distance.divided_by(&self.step).round();
        let remainder = distance.minus(&self.step.times(&steps)).abs();
        let acceptable = self
            .step
            .divided_by(&Decimal::from_double((1u32 << f32::MANTISSA_DIGITS) as f64));
        acceptable.less_than(&remainder) && remainder.less_than(&self.step.minus(&acceptable))
    }

    fn nearest(&self, value: &Decimal, round: impl Fn(&Decimal) -> Decimal) -> Decimal {
        let steps = round(&value.minus(&self.base).divided_by(&self.step));
        self.base.plus(&steps.times(&self.step))
    }
}

/// The value after one step `up` or down from `value`.
pub fn stepped(value: &JsString, value_attribute: &JsString, step: &JsString, up: bool) -> JsString {
    let range = Step::new(step, value_attribute);
    let current = parse_to_decimal_for_number_type(value.units(), Decimal::from(0));
    let mismatch = range.mismatch(&current);
    let ceil = |v: &Decimal| v.negated().floor().negated();
    let next = if !range.any && mismatch {
        range.nearest(&current, |v| if up { ceil(v) } else { v.floor() })
    } else if up {
        current.plus(&range.step)
    } else {
        current.minus(&range.step)
    };
    let huge = next.at_least(&Decimal::from_double(1e21));
    let aligned = if range.any || huge || mismatch {
        next
    } else {
        range.nearest(&next, Decimal::round)
    };
    serialize_for_number_type(&aligned)
}
