#![allow(clippy::disallowed_methods)]

//! Replays `js/date-parse/*` and `js/date-parse-fuzz/*`: `new Date(value).getTime()` in
//! Electron's V8 for the hand list and both generators in `oracle/src/update/cases.ts`, in each
//! oracle zone. Strings go through both `parse_date` and `date_from_value`; other values through
//! `date_from_value`.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use asrs_js::time::{NamedZone, date_from_value, parse_date};
use asrs_js::value::JsValue;
use asrs_parity::Value;
use asrs_parity::json::Json;
use asrs_parity::vectors;

fn same(expected: f64, actual: f64) -> bool {
    (expected.is_nan() && actual.is_nan()) || expected.to_bits() == actual.to_bits()
}

fn check(spec: &str, zone_name: &str) {
    let id = format!("{spec}/{zone_name}");
    let zone = NamedZone::get(zone_name).expect("zone resolves");
    let records = vectors::load(&id).unwrap_or_else(|e| panic!("{e}"));
    let (mut recorded, mut compared) = (0usize, 0usize);
    let mut failures = Vec::new();
    for (index, record) in records.iter().enumerate() {
        recorded += 1;
        let Json::Object(pairs) = &record.json else {
            panic!("{id} line {}: not an object", record.line)
        };
        let keys: Vec<String> = pairs.iter().map(|(k, _)| k.to_string_lossy()).collect();
        assert_eq!(
            keys,
            ["case", "input", "time"],
            "{id} line {}: fields",
            record.line
        );
        assert_eq!(
            record.field("case").and_then(Json::as_f64),
            Some(index as f64),
            "{id} line {}: case order",
            record.line
        );
        let input = record.value("input").expect("input");
        let Value::Number(bits) = record.value("time").expect("time") else {
            panic!("{id} line {}: time is not a number", record.line)
        };
        let expected = f64::from_bits(bits);
        let value = js_json::to_js(&input).expect("input converts");
        let from_value = date_from_value(&value, &zone)
            .unwrap_or_else(|e| panic!("{id} line {}: {e:?}", record.line));
        let mut actuals = vec![("date_from_value", from_value)];
        if let JsValue::String(text) = &value {
            actuals.push(("parse_date", parse_date(text, &zone)));
        }
        for (name, actual) in actuals {
            if !same(expected, actual) {
                failures.push(format!(
                    "line {}: {name}({}) = {actual}, V8 gives {expected}",
                    record.line,
                    input.encode()
                ));
            }
        }
        compared += 1;
    }
    assert!(
        failures.is_empty(),
        "{id}: {} of {recorded} differ:\n{}",
        failures.len(),
        failures[..failures.len().min(40)].join("\n")
    );
    assert!(recorded > 0, "{id}: no records");
    assert_eq!(recorded, compared, "{id}: compared differs from recorded");
}

#[test]
fn date_parse_utc() {
    check("js/date-parse", "UTC");
}

#[test]
fn date_parse_los_angeles() {
    check("js/date-parse", "America/Los_Angeles");
}

#[test]
fn date_parse_kolkata() {
    check("js/date-parse", "Asia/Kolkata");
}

#[test]
fn date_parse_auckland() {
    check("js/date-parse", "Pacific/Auckland");
}

#[test]
fn date_parse_fuzz_utc() {
    check("js/date-parse-fuzz", "UTC");
}

#[test]
fn date_parse_fuzz_los_angeles() {
    check("js/date-parse-fuzz", "America/Los_Angeles");
}

#[test]
fn date_parse_fuzz_kolkata() {
    check("js/date-parse-fuzz", "Asia/Kolkata");
}

#[test]
fn date_parse_fuzz_auckland() {
    check("js/date-parse-fuzz", "Pacific/Auckland");
}
