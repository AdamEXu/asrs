#![allow(clippy::disallowed_methods)]

#[path = "../../asrs-cli/src/js_string.rs"]
#[allow(dead_code)]
mod js_string;

use asrs_js::string::{JsString, TextDecoder, text_encode};
use asrs_parity::vectors::check_records;
use asrs_parity::{FnCase, Json, Record, Response, fn_vectors};

fn dispatch(case: &FnCase<'_>) -> Response {
    match js_string::FUNCTIONS.iter().find(|(n, _)| *n == case.name()) {
        Some((_, f)) => f(&case.request.args),
        None => Response::Error(format!("no port of {}", case.name())),
    }
}

fn_vectors!(string_functions, "jstring/ops", dispatch);

#[test]
fn case_data_vectors_match() {
    check_records("jstring/case-data", check_case_record);
}

fn check_case_record(record: &Record) -> Result<(), String> {
    if let Some(cp) = record.field("cp").and_then(Json::as_f64) {
        let input = JsString::from_code_point(&[cp])
            .map_err(|e| format!("cp {cp}: from_code_point: {e:?}"))?;
        check_mapped(record, "l", &input.to_lower_case(), &input)?;
        check_mapped(record, "u", &input.to_upper_case(), &input)?;
        return Ok(());
    }
    if let Some(s) = record.field("s").and_then(Json::as_str) {
        let input = JsString::from_units(s.units().to_vec());
        check_mapped(record, "l", &input.to_lower_case(), &input)?;
        check_mapped(record, "u", &input.to_upper_case(), &input)?;
        return Ok(());
    }
    // "cased" / "caseIgnorable" records back the generated table, not a check.
    Ok(())
}

fn check_mapped(
    record: &Record,
    key: &str,
    actual: &JsString,
    input: &JsString,
) -> Result<(), String> {
    let expected: Vec<u16> = match record.field(key).and_then(Json::as_str) {
        Some(s) => s.units().to_vec(),
        None => input.units().to_vec(),
    };
    if actual.units() == &expected[..] {
        Ok(())
    } else {
        Err(format!(
            "{}: {key}: expected {:?}, actual {:?}",
            record.json.to_compact(),
            escape(&expected),
            escape(actual.units()),
        ))
    }
}

fn escape(units: &[u16]) -> String {
    let mut out = String::new();
    for &u in units {
        if (0x20..0x7f).contains(&u) {
            out.push(char::from_u32(u32::from(u)).unwrap());
        } else {
            out.push_str(&format!("\\u{{{u:x}}}"));
        }
    }
    out
}

#[test]
fn utf8_vectors_match() {
    check_records("jstring/utf8", check_utf8_record);
}

fn check_utf8_record(record: &Record) -> Result<(), String> {
    if let Some(text) = record.field("e").and_then(Json::as_str) {
        let hex = record
            .string("h")
            .ok_or_else(|| format!("{}: encode record without \"h\"", record.json.to_compact()))?;
        let bytes = text_encode(&JsString::from_units(text.units().to_vec()));
        let actual: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        return if actual == hex {
            Ok(())
        } else {
            Err(format!(
                "encode {:?}: expected {hex}, actual {actual}",
                escape(text.units())
            ))
        };
    }
    let label = record
        .field("l")
        .and_then(Json::as_str)
        .map(|s| JsString::from_units(s.units().to_vec()));
    let fatal = record.field("f").is_some();
    let ignore_bom = record.field("b").is_some();
    let hex = record.string("d").unwrap_or_default();
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    let result = TextDecoder::new(label.as_ref(), fatal, ignore_bom).and_then(|d| d.decode(&bytes));
    match (
        record.field("s").and_then(Json::as_str),
        record.field("e").and_then(Json::as_array),
    ) {
        (Some(expected), _) => match result {
            Ok(actual) if actual.units() == expected.units() => Ok(()),
            Ok(actual) => Err(format!(
                "{}: expected {:?}, actual {:?}",
                record.json.to_compact(),
                escape(expected.units()),
                escape(actual.units()),
            )),
            Err(e) => Err(format!(
                "{}: expected a string, got {:?}",
                record.json.to_compact(),
                e.into_error(),
            )),
        },
        (None, Some(err)) => match result {
            Err(e) => {
                let e = e.into_error();
                let name = err[0]
                    .as_str()
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default();
                let message = err[1]
                    .as_str()
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default();
                if e.name == name && e.message == message {
                    Ok(())
                } else {
                    Err(format!(
                        "{}: expected {name}: {message}, got {}: {}",
                        record.json.to_compact(),
                        e.name,
                        e.message,
                    ))
                }
            }
            Ok(s) => Err(format!(
                "{}: expected a throw, got {:?}",
                record.json.to_compact(),
                escape(s.units()),
            )),
        },
        _ => Err(format!("{}: no expected output", record.json.to_compact())),
    }
}
