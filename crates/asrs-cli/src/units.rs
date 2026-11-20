//! `fn` names for `shared/units.ts` (`Units.*`), backed by `asrs_log::units`.

use asrs_js::string::JsString;
use asrs_log::{line_graph_filter::LineGraphFilter, units};
use asrs_parity::{Response, Utf16String, Value};

use crate::functions::{arg, ok_number, to_number, type_error};
use crate::shared::{Function, cannot_read, get_prop, prop_key, strict_equal};

pub const FUNCTIONS: &[(&str, Function)] = &[
    ("shared/units:Units.convert", convert_fn),
    (
        "shared/units:Units.convertWithPreset",
        convert_with_preset_fn,
    ),
    (
        "shared/units:Units.getSuffixForFilter",
        get_suffix_for_filter_fn,
    ),
    ("shared/units:Units.getUnitForField", get_unit_for_field_fn),
];

fn invalid_unit() -> Response {
    Response::Throw(Value::string("Invalid unit provided"))
}

/// `ToPropertyKey` for the `in` checks and `ALL_UNITS[name]` reads: a boxed
/// primitive keys like its inner value (`Object("meters")` is `"meters"`).
fn key_of(v: &Value) -> Vec<u16> {
    match v {
        Value::Boxed(inner) => key_of(inner),
        _ => prop_key(v),
    }
}

fn unit_ref(v: &Value, to: bool) -> (String, units::TempUnit) {
    let key = String::from_utf16_lossy(&key_of(v));
    let primitive = matches!(v, Value::String(_));
    let temp = if to {
        units::TempUnit::to_arg(&key, primitive)
    } else {
        units::TempUnit::from_arg(&key, primitive)
    };
    (key, temp)
}

/// An argument upstream calls `.<method>` on: strings and boxed strings have
/// it, null/undefined report the failed read, the rest the missing function.
fn string_arg(v: &Value, receiver: &str, method: &str) -> Result<JsString, Response> {
    match v {
        Value::String(s) => Ok(JsString::from_units(s.units().to_vec())),
        Value::Boxed(inner) => match inner.as_ref() {
            Value::String(s) => Ok(JsString::from_units(s.units().to_vec())),
            _ => Err(type_error(format!("{receiver}.{method} is not a function"))),
        },
        Value::Null | Value::Undefined => Err(cannot_read(v, method)),
        _ => Err(type_error(format!("{receiver}.{method} is not a function"))),
    }
}

fn convert_fn(args: &[Value]) -> Response {
    let (from, from_temp) = unit_ref(&arg(args, 1), false);
    let (to, to_temp) = unit_ref(&arg(args, 2), true);
    // Upstream's `in` gate runs before `value` is coerced.
    if !units::valid_unit_pair(&from, &to) {
        return invalid_unit();
    }
    let value = match to_number(&arg(args, 0)) {
        Ok(v) => v,
        Err(r) => return r,
    };
    match units::convert_with_temp(value, &from, &to, from_temp, to_temp) {
        Ok(x) => ok_number(x),
        Err(units::InvalidUnit) => invalid_unit(),
    }
}

fn convert_with_preset_fn(args: &[Value]) -> Response {
    let preset = arg(args, 1);
    let get = |name: &str| get_prop(&preset, &name.encode_utf16().collect::<Vec<u16>>());
    let type_v = match get("type") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let factor_v = match get("factor") {
        Ok(v) => v,
        Err(r) => return r,
    };
    // `type === null && factor === 1` returns the untouched `value` argument:
    // identity is the raw input, so this comparison runs before any coercion.
    if matches!(type_v, Value::Null) && strict_equal(&factor_v, &Value::number(1.0)) {
        return Response::Ok(arg(args, 0));
    }
    let from_v = match get("from") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let to_v = match get("to") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let field = |v: &Value, to: bool| (!matches!(v, Value::Undefined)).then(|| unit_ref(v, to));
    if let (Some((from, from_temp)), Some((to, to_temp))) =
        (field(&from_v, false), field(&to_v, true))
    {
        if !units::valid_unit_pair(&from, &to) {
            return invalid_unit();
        }
        let value = match to_number(&arg(args, 0)) {
            Ok(v) => v,
            Err(r) => return r,
        };
        match units::convert_with_temp(value, &from, &to, from_temp, to_temp) {
            Ok(converted) => match to_number(&factor_v) {
                Ok(factor) => ok_number(converted * factor),
                Err(r) => r,
            },
            Err(units::InvalidUnit) => invalid_unit(),
        }
    } else {
        let value = match to_number(&arg(args, 0)) {
            Ok(v) => v,
            Err(r) => return r,
        };
        match to_number(&factor_v) {
            Ok(factor) => ok_number(value * factor),
            Err(r) => r,
        }
    }
}

fn get_suffix_for_filter_fn(args: &[Value]) -> Response {
    let suffix = match string_arg(&arg(args, 0), "suffix", "endsWith") {
        Ok(s) => s,
        Err(r) => return r,
    };
    let filter = match arg(args, 1) {
        Value::Number(bits) if f64::from_bits(bits) == 1.0 => LineGraphFilter::Differentiate,
        Value::Number(bits) if f64::from_bits(bits) == 2.0 => LineGraphFilter::Integrate,
        _ => LineGraphFilter::None,
    };
    Response::Ok(Value::String(Utf16String::from_units(
        units::get_suffix_for_filter(&suffix, filter).into_units(),
    )))
}

fn get_unit_for_field_fn(args: &[Value]) -> Response {
    let key = match string_arg(&arg(args, 0), "fieldKey", "toLowerCase") {
        Ok(k) => k,
        Err(r) => return r,
    };
    match units::get_unit_for_field(&key) {
        Some(unit) => Response::Ok(Value::string(unit)),
        None => Response::Ok(Value::Null),
    }
}
