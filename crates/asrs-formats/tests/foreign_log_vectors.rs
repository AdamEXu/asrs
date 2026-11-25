//! Replays `log/foreign-decode` and `log/foreign-exports` (oracle/src/foreignLogs.ts): what the
//! decode workers post for logs whose field names `Object.prototype` defines, and what the export
//! worker writes for such names and for values of other types than their field's.

use std::sync::Arc;

use asrs_formats::csv::CsvWorker;
use asrs_formats::export::{ExportError, ExportOptions, Format, Sampling, export};
use asrs_formats::rlog::RlogWorker;
use asrs_formats::roadrunner::RoadRunnerWorker;
use asrs_formats::worker::{Abort, DecodeWorker, Request, Response, run_request};
use asrs_formats::wpilog_worker::WpilogWorker;
use asrs_js::string::JsString;
use asrs_js::time::{SetClock, SetRng};
use asrs_js::value::{JsObject, JsValue};
use asrs_log::log::{Log, LogValues, LoggableType, SerializedLogField, StructuredType};
use asrs_log::realm::JsRealm;
use asrs_parity::Utf16String;
use asrs_parity::json::Json;
use asrs_parity::vectors::{self, Record};

fn realm() -> JsRealm {
    JsRealm::new(Box::new(SetRng::new(vec![0.5])))
}

fn js_string(json: &Json) -> JsString {
    JsString::from_units(json.as_str().expect("string").units().to_vec())
}

fn text(record: &Record, key: &str) -> String {
    let value = record.field(key).and_then(Json::as_str).expect("string");
    value.to_string_lossy()
}

fn array<'a>(record: &'a Record, key: &str) -> &'a [Json] {
    record.field(key).and_then(Json::as_array).expect("array")
}

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_js(value: &JsString) -> Json {
    Json::String(Utf16String::from_units(value.units().to_vec()))
}

fn optional(value: Option<&JsString>) -> Json {
    value.map_or(Json::Null, from_js)
}

fn values(values: &LogValues) -> Json {
    fn list<T>(items: &[T], json: impl Fn(&T) -> Json) -> Json {
        Json::Array(items.iter().map(json).collect())
    }
    match values {
        LogValues::Raw(c) => list(c, |bytes| {
            let bytes = bytes.iter().enumerate();
            let pairs = bytes.map(|(i, b)| {
                let index = Utf16String::from(format!("{i}").as_str());
                (index, Json::Number(f64::from(*b)))
            });
            Json::Object(pairs.collect())
        }),
        LogValues::Boolean(c) => list(c, |b| Json::Bool(*b)),
        LogValues::Number(c) => list(c, |x| Json::Number(*x)),
        LogValues::String(c) => list(c, from_js),
        LogValues::BooleanArray(c) => list(c, |v| list(v, |b| Json::Bool(*b))),
        LogValues::NumberArray(c) => list(c, |v| list(v, |x| Json::Number(*x))),
        LogValues::StringArray(c) => list(c, |v| list(v, from_js)),
        LogValues::Empty(c) => list(c, |()| Json::Null),
        LogValues::Foreign(..) => panic!("a file holds no foreign values"),
    }
}

fn field(key: &JsString, data: &SerializedLogField) -> Json {
    Json::Array(vec![
        from_js(key),
        Json::Number(f64::from(data.get_type() as u8)),
        Json::Array(data.timestamps.iter().map(|t| Json::Number(*t)).collect()),
        values(&data.values),
        match &data.structured_type {
            StructuredType::Named(name) => from_js(name),
            _ => Json::Null,
        },
        optional(data.wpilib_type.as_ref()),
        from_js(&data.metadata_string),
    ])
}

fn reply(response: Response) -> Option<Json> {
    Some(Json::Array(match response {
        Response::Progress(_) => return None,
        Response::Failed => vec![Json::string("failed")],
        Response::Initial { log, is_partial } => vec![
            Json::string("initial"),
            Json::Bool(is_partial),
            Json::Array(log.fields.iter().map(|(k, f)| field(k, f)).collect()),
        ],
        Response::Fields(fields) => vec![
            Json::string("fields"),
            Json::Array(fields.iter().map(|f| field(&f.key, &f.data)).collect()),
        ],
    }))
}

#[test]
fn decode() {
    let records = vectors::load("log/foreign-decode").unwrap_or_else(|e| panic!("{e}"));
    let (mut threw, mut failed) = (0, 0);
    for record in &records {
        let name = text(record, "name");
        let clock = Box::new(SetClock(0.0));
        let mut worker: Box<dyn DecodeWorker> = match text(record, "worker").as_str() {
            "wpilogWorker" => Box::new(WpilogWorker::new(realm())),
            "csvWorker" => Box::new(CsvWorker),
            "rlogWorker" => Box::new(RlogWorker::new(realm(), clock)),
            "roadRunnerWorker" => Box::new(RoadRunnerWorker::new(clock)),
            other => panic!("worker {other}"),
        };
        let file = Arc::from(bytes(&text(record, "file")));
        let mut requests = vec![Request::Start(vec![Some(file)])];
        let keys = array(record, "requests").iter();
        requests.extend(keys.map(|key| Request::ParseField(js_string(key))));
        let mut replies = Vec::new();
        for request in requests {
            let (responses, end) = run_request(&mut *worker, request);
            let mut got: Vec<Json> = responses.into_iter().filter_map(reply).collect();
            failed += usize::from(got == [Json::Array(vec![Json::string("failed")])]);
            match end {
                Ok(()) => {}
                Err(Abort::Threw) => {
                    threw += 1;
                    got.push(Json::string("threw"));
                }
                Err(other) => panic!("{name}: {other:?}"),
            }
            replies.push(Json::Array(got));
        }
        let expected: Vec<Json> = array(record, "replies")
            .iter()
            .map(|reply| {
                let items = reply.as_array().expect("reply").iter();
                Json::Array(
                    items
                        .map(|item| match item.as_array() {
                            Some([kind, _]) if *kind == Json::string("threw") => kind.clone(),
                            _ => item.clone(),
                        })
                        .collect(),
                )
            })
            .collect();
        assert_eq!(replies, expected, "{name}");
    }
    assert!(
        records.len() >= 160 && threw >= 60 && failed >= 40,
        "{threw} {failed}"
    );
}

fn to_js(json: &Json) -> JsValue {
    match json {
        Json::Null => JsValue::Null,
        Json::Bool(b) => JsValue::Bool(*b),
        Json::Number(x) => JsValue::Number(*x),
        Json::String(_) => JsValue::String(js_string(json)),
        Json::Array(items) => JsValue::Array(items.iter().map(to_js).collect()),
        Json::Object(pairs) => {
            let mut object = JsObject::new();
            for (key, value) in pairs {
                object.insert(JsString::from_units(key.units().to_vec()), to_js(value));
            }
            JsValue::Object(object)
        }
    }
}

fn log_of(ops: &[Json]) -> Log {
    let mut log = Log::new(true);
    for op in ops {
        let Some([method, key, timestamp, value]) = op.as_array() else {
            panic!("op {op:?}");
        };
        let ty = match method.as_str().expect("method").to_string_lossy().as_str() {
            "putBoolean" => LoggableType::Boolean,
            "putNumber" => LoggableType::Number,
            "putString" => LoggableType::String,
            other => panic!("method {other}"),
        };
        let timestamp = timestamp.as_f64().expect("timestamp");
        log.put_untyped(&js_string(key), timestamp, ty, to_js(value))
            .expect("an op that does not throw");
    }
    log
}

#[test]
fn exports() {
    let records = vectors::load("log/foreign-exports").unwrap_or_else(|e| panic!("{e}"));
    let mut rejected = 0;
    for record in &records {
        let name = text(record, "name");
        let options = record.field("options").expect("options");
        let option = |key: &str| {
            let value = options.get(key).and_then(Json::as_str).expect("option");
            value.to_string_lossy()
        };
        let options = ExportOptions {
            format: match option("format").as_str() {
                "csv-table" => Format::CsvTable,
                "csv-list" => Format::CsvList,
                "wpilog" => Format::Wpilog,
                "mcap" => Format::Mcap,
                other => panic!("format {other}"),
            },
            sampling: match option("samplingMode").as_str() {
                "changes" => Sampling::Changes,
                "fixed" => Sampling::Fixed,
                "akit" => Sampling::Akit,
                other => panic!("sampling {other}"),
            },
            sampling_period: options
                .get("samplingPeriod")
                .and_then(Json::as_f64)
                .expect("period"),
            prefixes: JsString::new(),
            include_generated: true,
        };
        let log = log_of(array(record, "ops"));
        let name = format!("{name}: {options:?}");
        match export(&log, &options, &mut |_| {}) {
            Ok(output) => {
                let got = Json::string(&hex(&output.into_bytes()));
                assert_eq!(Some(&got), record.field("bytes"), "{name}");
            }
            Err(ExportError::Rejected) => {
                rejected += 1;
                assert_eq!(record.field("bytes"), Some(&Json::Null), "{name}");
            }
            Err(other) => panic!("{name}: {other:?}"),
        }
    }
    assert!(records.len() >= 460 && rejected >= 200, "{rejected}");
}
