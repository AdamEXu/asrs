//! `oracle:mergeCalculatorRun`, backed by `asrs_log::log::merge_calculator`.

use asrs_js::string::JsString;
use asrs_js::value::JsObject;
use asrs_log::log::{Log, SerializedLog, merge_calculator};
use asrs_parity::{Response, Value};

use crate::functions::{Function, arg, quiet_nan, to_number};
use crate::log_run;

pub const FUNCTIONS: &[(&str, Function)] = &[("oracle:mergeCalculatorRun", merge_calculator_run)];

fn serialized_log(v: &Value) -> Result<SerializedLog, String> {
    let member = |name: &str| v.get(name).cloned().unwrap_or(Value::Undefined);
    let mut fields = JsObject::new();
    match member("fields") {
        Value::Object(pairs) => {
            for (key, field) in pairs {
                fields.insert(
                    JsString::from_units(key.units().to_vec()),
                    log_run::serialized_field(&field)?,
                );
            }
        }
        _ => return Err("mergeCalculatorRun: serialized log needs an object `fields`".to_owned()),
    }
    let generated_parents = match member("generatedParents") {
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::String(s) => Ok(JsString::from_units(s.units().to_vec())),
                _ => Err("mergeCalculatorRun: generatedParents holds strings".to_owned()),
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => Vec::new(),
    };
    let number = |v: &Value| {
        to_number(v)
            .map(quiet_nan)
            .map_err(|_| "mergeCalculatorRun: timestamp must be a number".to_owned())
    };
    let timestamp_range = match member("timestampRange") {
        Value::Array(items) if items.len() == 2 => Some([number(&items[0])?, number(&items[1])?]),
        _ => None,
    };
    let timestamp_zero = match member("timestampZero") {
        Value::Null | Value::Undefined => None,
        v => Some(number(&v)?),
    };
    Ok(SerializedLog {
        fields,
        generated_parents,
        timestamp_range,
        timestamp_zero,
    })
}

fn merge_calculator_run(args: &[Value]) -> Response {
    let target = match serialized_log(&arg(args, 1)).map(Log::from_serialized) {
        Ok(log) => log,
        Err(e) => return Response::Error(e),
    };
    let source = match serialized_log(&arg(args, 2)).map(Log::from_serialized) {
        Ok(log) => log,
        Err(e) => return Response::Error(e),
    };
    Response::Ok(Value::number(merge_calculator::get_merge_offset(
        &target, &source,
    )))
}
