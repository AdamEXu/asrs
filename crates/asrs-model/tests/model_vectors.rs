#![allow(clippy::disallowed_methods)]

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::path::Path;

use asrs_js::json;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::match_info::{MatchInfo, MatchType};
use asrs_model::build_constants::{self, Distribution};
use asrs_model::json_file;
use asrs_model::preferences::{LiveMode, Preferences, SystemcoreStaticAddress, merge_preferences};
use asrs_model::selection::SelectionMode;
use asrs_model::tab_type::TabType;
use asrs_model::video::{VideoSource, tba_match_key};
use asrs_parity::vectors::check_records;
use asrs_parity::{Record, Value};

fn folder_exists(folder: &JsString) -> bool {
    Path::new(&folder.to_string_lossy()).exists()
}

fn bytes_field(record: &Record, key: &str) -> Result<Option<Vec<u8>>, String> {
    match record.value(key).map_err(|e| e.to_string())? {
        Value::Null => Ok(None),
        Value::Bytes(bytes) => Ok(Some(bytes)),
        other => Err(format!(
            "{key}: expected bytes or null, got {}",
            other.kind()
        )),
    }
}

fn startup(record: &Record) -> Result<Vec<u8>, JsError> {
    let platform = record
        .string("platform")
        .expect("startup records name a platform");
    let file = match bytes_field(record, "file").expect("file") {
        Some(bytes) => Some(json_file::parse(&bytes)?),
        None => None,
    };
    let prefs = Preferences::startup(&platform, file.as_ref(), folder_exists)?;
    json_file::stringify(&prefs.to_js())
}

fn update(record: &Record) -> Result<Vec<u8>, JsError> {
    let file = bytes_field(record, "file")
        .expect("file")
        .expect("update records have a file");
    let data_text = record.string("data").expect("update records carry data");
    let data = json::parse_str(&data_text).expect("update data is valid JSON");
    let mut prefs = json_file::parse(&file)?;
    merge_preferences(&mut prefs, &data)?;
    json_file::stringify(&prefs)
}

fn compare(record: &Record, actual: Result<Vec<u8>, JsError>) -> Result<(), String> {
    let expected = match record.string("throw") {
        Some(name) => Err(name),
        None => Ok(bytes_field(record, "out")?.ok_or("no out")?),
    };
    let actual = actual.map_err(|e| e.name.to_owned());
    if actual == expected {
        return Ok(());
    }
    let show = |r: &Result<Vec<u8>, String>| match r {
        Ok(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        Err(name) => format!("throw {name}"),
    };
    Err(format!(
        "{}\n  expected {}\n  actual   {}",
        record.text.chars().take(300).collect::<String>(),
        show(&expected),
        show(&actual)
    ))
}

#[test]
fn prefs_flows_match_upstream() {
    check_records("model/prefs", |record| {
        match record.string("flow").as_deref() {
            Some("startup") => compare(record, startup(record)),
            Some("update") => compare(record, update(record)),
            other => Err(format!("unknown flow {other:?}")),
        }
    });
}

#[test]
fn typed_merge_matches_raw_merge() {
    check_records("model/prefs", |record| {
        if record.string("flow").as_deref() != Some("update") || record.string("throw").is_some() {
            return Ok(());
        }
        let data = json::parse_str(&record.string("data").unwrap()).unwrap();
        let mut typed = Preferences::default();
        let mut raw = typed.to_js();
        let typed_result = typed.merge(&data).map_err(|e| e.name);
        let raw_result = merge_preferences(&mut raw, &data).map_err(|e| e.name);
        if typed_result != raw_result {
            return Err(format!("{typed_result:?} != {raw_result:?}"));
        }
        if typed_result.is_ok() && typed.to_js() != raw {
            return Err(format!("{:?} != {raw:?}", typed.to_js()));
        }
        Ok(())
    });
}

fn enum_object(members: impl Iterator<Item = (&'static str, f64)> + Clone) -> JsValue {
    let mut o = JsObject::new();
    for (name, number) in members.clone() {
        o.insert(asrs_js::number::to_string(number), JsValue::string(name));
    }
    for (name, number) in members {
        o.insert(name, JsValue::Number(number));
    }
    JsValue::Object(o)
}

fn schema_value(name: &str) -> Option<JsValue> {
    let words: Vec<&str> = name.split(' ').collect();
    let tab = |i: usize| {
        let number = asrs_js::number::to_number(JsString::from(words[i]).units());
        TabType::from_number(number).expect("a tab type")
    };
    Some(match words[0] {
        "DEFAULT_PREFS" => Preferences::default().to_js(),
        "getLiveModeName" => {
            let mode = LiveMode::from_js_str(&JsString::from(words[1]))?;
            JsValue::string(mode.display_name())
        }
        "getRobotAddress" => {
            let prefs = Preferences {
                robot_address: JsString::from("10.63.28.2"),
                systemcore_static_address: SystemcoreStaticAddress::from_js_str(&JsString::from(
                    words[1],
                ))?,
                ..Preferences::default()
            };
            prefs.robot_address_for(words[2]).to_js()
        }
        "TabType" => enum_object(TabType::ALL.iter().map(|t| (t.name(), t.number()))),
        "getAllTabTypes" => TabType::ALL.to_js(),
        "getDefaultTabTitle" => JsValue::string(tab(1).default_title()),
        "getTabIcon" => JsValue::string(tab(1).icon()),
        "getTabAccelerator" => JsValue::string(tab(1).accelerator()),
        "SelectionMode" => enum_object(SelectionMode::ALL.iter().map(|m| (m.name(), m.number()))),
        "VideoSource" => enum_object(VideoSource::ALL.iter().map(|s| (s.name(), s.number()))),
        "Distribution" => enum_object(Distribution::ALL.iter().map(|d| (d.name(), d.number()))),
        "DISTRIBUTION" => build_constants::DISTRIBUTION.to_js(),
        "IS_LITE" => build_constants::IS_LITE.to_js(),
        "LITE_VERSION" => build_constants::LITE_VERSION.to_js(),
        "BUILD_DATE" if option_env!("ASRS_BUILD_DATE").is_none() => {
            build_constants::BUILD_DATE.to_js()
        }
        "BUILD_DATE" => return None,
        "COPYRIGHT" => build_constants::COPYRIGHT.to_js(),
        _ => return None,
    })
}

#[test]
fn schemas_match_upstream() {
    check_records("model/schemas", |record| {
        let name = record.string("name").ok_or("no name")?;
        let expected = record.value("value").map_err(|e| e.to_string())?;
        let Some(actual) = schema_value(&name) else {
            return if name == "BUILD_DATE" {
                Ok(())
            } else {
                Err(format!("{name}: not ported"))
            };
        };
        let actual = js_json::from_js(&actual);
        if actual == expected {
            Ok(())
        } else {
            Err(format!(
                "{name}\n  expected {}\n  actual   {}",
                expected.encode(),
                actual.encode()
            ))
        }
    });
}

fn f64_at(args: &[JsValue], i: usize) -> Result<f64, String> {
    args.get(i)
        .and_then(JsValue::as_f64)
        .ok_or_else(|| format!("arg {i} is not a number"))
}

#[test]
fn tba_match_keys_match_upstream() {
    check_records("model/tba", |record| {
        let name = record.string("name").ok_or("no name")?;
        if name == "VideoSource" {
            let expected = record.value("value").map_err(|e| e.to_string())?;
            let actual = js_json::from_js(&schema_value("VideoSource").ok_or("not ported")?);
            return if actual == expected {
                Ok(())
            } else {
                Err(format!(
                    "VideoSource\n  expected {}\n  actual   {}",
                    expected.encode(),
                    actual.encode()
                ))
            };
        }
        if name != "tbaMatchKey" {
            return Err(format!("unknown record name {name}"));
        }
        let args = js_json::to_js(&record.value("args").map_err(|e| e.to_string())?)?;
        let args = args.as_array().ok_or("args is not an array")?;
        let event_key = args
            .first()
            .and_then(JsValue::as_str)
            .cloned()
            .ok_or("event key is not a string")?;
        let match_type = MatchType::from_number(f64_at(args, 2)?).ok_or("bad match type")?;
        let info = MatchInfo {
            year: 0.0,
            event: JsString::new(),
            match_type,
            match_number: f64_at(args, 3)?,
        };
        let actual = tba_match_key(&event_key, f64_at(args, 1)?, &info);
        match record.string("throw") {
            Some(expected_throw) => match actual {
                None => Ok(()),
                Some(key) => Err(format!(
                    "{args:?}\n  upstream threw {expected_throw}, port gave {key:?}"
                )),
            },
            None => {
                let expected = record.value("value").map_err(|e| e.to_string())?;
                let actual = actual
                    .map(|k| js_json::from_js(&JsValue::String(k)))
                    .unwrap_or(Value::Null);
                if actual == expected {
                    Ok(())
                } else {
                    Err(format!(
                        "{args:?}\n  expected {}\n  actual   {}",
                        expected.encode(),
                        actual.encode()
                    ))
                }
            }
        }
    });
}
