#![allow(clippy::disallowed_methods)]

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

#[path = "../../asrs-cli/src/assets.rs"]
#[allow(dead_code)]
mod cli_assets;

use asrs_js::json;
use asrs_js::value::JsValue;
use asrs_model::assets::{self, Parsed, TypeError};
use asrs_parity::vectors::check_records;
use asrs_parity::{FnCase, Json, Record, Response, Utf16String, Value, diff, fn_vectors};

fn dispatch(case: &FnCase<'_>) -> Response {
    match cli_assets::FUNCTIONS
        .iter()
        .find(|(n, _)| *n == case.name())
    {
        Some((_, f)) => f(&case.request.args),
        None => Response::Error(format!("no port of {}", case.name())),
    }
}

fn_vectors!(assets_parsers, "assets/parsers", dispatch);

#[test]
fn assets_bundled() {
    check_records("assets/bundled", check_doc);
}

#[test]
fn assets_release() {
    check_records("assets/release", check_doc);
}

#[test]
fn assets_malformed() {
    check_records("assets/malformed", check_doc);
}

#[test]
fn assets_builtin() {
    check_records("assets/builtin", check_builtin);
}

fn error_value(e: &json::JsonError) -> Value {
    Value::Error(
        e.name.into(),
        Utf16String::from_units(e.message.units().to_vec()),
    )
}

fn type_error_value(e: &TypeError) -> Value {
    Value::Error(
        "TypeError".into(),
        Utf16String::from_units(e.message.units().to_vec()),
    )
}

fn outcome(result: Result<Parsed<JsValue>, TypeError>) -> Result<Value, Value> {
    match result {
        Ok(Parsed::Config(config)) => Ok(js_json::from_js(&config)),
        Ok(Parsed::Invalid) => Ok(Value::string("invalid")),
        Ok(Parsed::Skip) => Ok(Value::string("skip")),
        Err(e) => Err(type_error_value(&e)),
    }
}

fn parser_result(kind: &str, raw: &JsValue) -> Result<Result<Value, Value>, String> {
    Ok(match kind {
        "field2d" => outcome(assets::parse_field_2d(raw).map(|p| p.map(|c| c.to_js()))),
        "field3d" => outcome(assets::parse_field_3d(raw).map(|p| p.map(|c| c.to_js()))),
        "robot" => outcome(assets::parse_robot(raw).map(|p| p.map(|c| c.to_js()))),
        "joystick" => outcome(assets::parse_joystick(raw).map(|p| p.map(|c| c.to_js()))),
        other => return Err(format!("unknown kind {other:?}")),
    })
}

/// `{ok: tree}` / `{throw: tree}` envelopes: the parser's result tree or the error tree.
fn expect_envelope(
    record: &Record,
    key: &str,
    outcome: Result<Value, Value>,
) -> Result<(), String> {
    let env = record
        .field(key)
        .ok_or_else(|| format!("record has no {key:?}"))?;
    let (tag, actual) = match &outcome {
        Ok(v) => ("ok", v),
        Err(v) => ("throw", v),
    };
    let expected_json = env
        .get(tag)
        .ok_or_else(|| format!("{key}: oracle envelope is not {tag:?}"))?;
    let expected = Value::from_json(expected_json).map_err(|e| e.to_string())?;
    if expected.encode() == actual.encode() {
        Ok(())
    } else {
        Err(format!(
            "{key}: {}",
            diff::report(&expected, actual).unwrap_or_else(|| "encodings differ".to_owned())
        ))
    }
}

fn check_doc(record: &Record) -> Result<(), String> {
    if let Some(error) = record.string("error") {
        return Err(format!("oracle emitted an error record: {error}"));
    }
    let kind = record.string("kind").ok_or("record has no kind")?;
    let doc = record
        .field("doc")
        .and_then(Json::as_str)
        .ok_or("record has no doc")?;
    let parsed = match json::parse_units(doc.units()) {
        Ok(value) => value,
        Err(e) => return expect_envelope(record, "parse", Err(error_value(&e))),
    };
    expect_envelope(record, "parse", Ok(js_json::from_js(&parsed)))?;
    let out = parser_result(&kind, &parsed)?;
    expect_envelope(record, "out", out)
}

fn stations_value(stations: &[[f64; 2]]) -> Value {
    Value::Array(
        stations
            .iter()
            .map(|p| Value::Array(vec![Value::number(p[0]), Value::number(p[1])]))
            .collect(),
    )
}

fn builtin_value(id: &str) -> Result<Value, String> {
    Ok(match id {
        "constants/FRC_STANDARD_FIELD_LENGTH" => Value::number(assets::frc_standard_field_length()),
        "constants/FRC_STANDARD_FIELD_WIDTH" => Value::number(assets::frc_standard_field_width()),
        "constants/FTC_STANDARD_FIELD_LENGTH" => Value::number(assets::ftc_standard_field_length()),
        "constants/FTC_STANDARD_FIELD_WIDTH" => Value::number(assets::ftc_standard_field_width()),
        "constants/ALLIANCE_STATION_WIDTH" => Value::number(assets::alliance_station_width()),
        "constants/DEFAULT_DRIVER_STATIONS_FRC" => {
            stations_value(assets::default_driver_stations_frc())
        }
        "constants/DEFAULT_DRIVER_STATIONS_FTC" => {
            stations_value(assets::default_driver_stations_ftc())
        }
        _ => {
            let index = id
                .strip_prefix("BuiltIn3dFields/")
                .and_then(|i| i.parse::<usize>().ok())
                .ok_or_else(|| format!("unknown builtin record id {id:?}"))?;
            js_json::from_js(&assets::built_in_3d_fields()[index].to_js())
        }
    })
}

fn check_builtin(record: &Record) -> Result<(), String> {
    let id = record.string("id").ok_or("record has no id")?;
    let expected_json = record
        .field("out")
        .and_then(|env| env.get("ok"))
        .ok_or("record has no out.ok")?;
    let expected = Value::from_json(expected_json).map_err(|e| e.to_string())?;
    let actual = builtin_value(&id)?;
    if expected.encode() == actual.encode() {
        Ok(())
    } else {
        Err(format!(
            "{id}: {}",
            diff::report(&expected, &actual).unwrap_or_else(|| "encodings differ".to_owned())
        ))
    }
}

#[test]
fn rotations_feed_geometry() {
    let raw = json::parse_units(
        &r#"{"name":"r","rotations":[{"degrees":-90,"axis":"x","note":1},{"axis":"y","degrees":180}]}"#
            .encode_utf16()
            .collect::<Vec<u16>>(),
    )
    .expect("valid JSON");
    let Ok(Parsed::Config(robot)) = assets::parse_robot(&raw) else {
        panic!("robot config parses");
    };
    let direct = asrs_log::geometry::rotation_sequence_to_quaternion(&[
        asrs_log::geometry::Config3dRotation {
            axis: "x".into(),
            degrees: -90.0,
        },
        asrs_log::geometry::Config3dRotation {
            axis: "y".into(),
            degrees: 180.0,
        },
    ]);
    assert_eq!(
        assets::rotation_sequence_to_quaternion(&robot.rotations),
        direct
    );
}
