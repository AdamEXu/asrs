//! `asrs-cli decode <log file>`: opens the file the way the hub opens a historical log and
//! prints the canonical CVE-1 Log dump for the "initial" and "full" phases, matching
//! `oracle decode` (oracle/src/decode.ts and oracle/src/historical/dump.ts). The session side
//! is `HistoricalDataSource.openFile` and `handleMainMessage`; a worker that never answers
//! leaves the command running, exactly as it leaves the source stuck in DecodingInitial.

mod dslog;

use std::fs;

use asrs_formats::worker;
use asrs_js::string::JsString;
use asrs_log::log::{Log, SerializedLog};
use asrs_parity::Value;

use crate::log_field::{serialized_value, string_value};

const USAGE: &str = "usage: asrs-cli decode <log file>";

type Worker = fn(&[Option<Vec<u8>>]) -> worker::Outcome;

/// `HistoricalDataSource.WORKER_NAMES`: the last extension that matches wins, so later
/// entries shadow earlier ones.
const WORKERS: &[(&str, Worker)] = &[(".dslog", dslog::run)];

fn worker_for(path: &str) -> Option<Worker> {
    WORKERS
        .iter()
        .rev()
        .find(|(extension, _)| path.ends_with(extension))
        .map(|(_, worker)| *worker)
}

fn read_or_null(path: &str) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

/// The `files` list `historical-start` builds: a `.dslog` path also pulls in the sibling
/// `.dsevents`; anything else is a single slot.
fn read_input_files(path: &str) -> Vec<Option<Vec<u8>>> {
    if path.ends_with(".dslog") {
        vec![
            read_or_null(path),
            read_or_null(&format!("{}dsevents", &path[..path.len() - 5])),
        ]
    } else {
        vec![read_or_null(path)]
    }
}

fn open(path: &str) -> (&'static str, Log) {
    let mut log = Log::new(true);
    let files = read_input_files(path);
    if files.iter().all(Option::is_none) {
        return ("Error", log);
    }
    let Some(worker) = worker_for(path) else {
        return ("Error", log);
    };
    match worker(&files) {
        worker::Outcome::Initial { log: payload, .. } => {
            // `mergeWith(Log.fromSerialized(payload), "")` into an empty log: the fields are
            // rebuilt from `source.toSerialized()` and the range, zero and generated parents
            // carry over, so this is the payload serialized a second time.
            log = Log::from_serialized(Log::from_serialized(payload).to_serialized());
            ("Idle", log)
        }
        worker::Outcome::Failed => ("Error", log),
        worker::Outcome::Threw => ("DecodingInitial", log),
    }
}

fn dump_log(
    serialized: &SerializedLog,
    phase: &str,
    status: &str,
    loading_fields: &[JsString],
) -> Vec<String> {
    let mut lines = vec![
        Value::object([
            ("phase", Value::string(phase)),
            ("status", Value::string(status)),
            (
                "loadingFields",
                Value::Array(loading_fields.iter().map(string_value).collect()),
            ),
            ("fieldCount", Value::number(serialized.fields.len() as f64)),
        ])
        .encode(),
    ];
    for (key, field) in serialized.fields.iter() {
        let Value::Object(mut pairs) = serialized_value(field) else {
            unreachable!("serialized_value always returns an object")
        };
        pairs.insert(0, ("key".into(), string_value(key)));
        lines.push(Value::Object(pairs).encode());
    }
    lines.push(
        Value::object([(
            "generatedParents",
            Value::Array(
                serialized
                    .generated_parents
                    .iter()
                    .map(string_value)
                    .collect(),
            ),
        )])
        .encode(),
    );
    lines.push(
        Value::object([
            (
                "timestampRange",
                serialized
                    .timestamp_range
                    .map_or(Value::Null, |[start, end]| {
                        Value::Array(vec![Value::number(start), Value::number(end)])
                    }),
            ),
            (
                "timestampZero",
                serialized.timestamp_zero.map_or(Value::Null, Value::number),
            ),
        ])
        .encode(),
    );
    lines.push(
        Value::object([
            (
                "structSchemaStrings",
                Value::object(Vec::<(&str, Value)>::new()),
            ),
            ("structSchemas", Value::object(Vec::<(&str, Value)>::new())),
        ])
        .encode(),
    );
    lines.push(Value::object([("protoDescriptors", Value::Array(Vec::new()))]).encode());
    lines.push(
        Value::object([
            ("queuedStructs", Value::Array(Vec::new())),
            ("queuedStructArrays", Value::Array(Vec::new())),
            ("queuedProtos", Value::Array(Vec::new())),
        ])
        .encode(),
    );
    lines
}

pub fn run(args: &[String]) -> Result<(), String> {
    let [path] = args else {
        return Err(USAGE.to_owned());
    };
    let mut path = path.as_str();
    let rewritten;
    if path.ends_with(".dsevents") {
        rewritten = format!("{}dslog", &path[..path.len() - 8]);
        path = &rewritten;
    }
    let (status, log) = open(path);
    let mut out = String::new();
    for phase in ["initial", "full"] {
        for line in dump_log(&log.to_serialized(), phase, status, &[]) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    print!("{out}");
    Ok(())
}
