//! `asrs-cli decode <log file>`: opens the file as the hub opens a historical log and prints the
//! canonical CVE-1 dump for the "initial" and "full" phases, matching `oracle decode`
//! (oracle/src/decode.ts, oracle/src/historical/dump.ts). Where upstream's worker never returns,
//! nothing is printed and the exit code is `NEVER_COMPLETES`, the oracle's timeout. With
//! `--initial` only the "initial" phase is printed.

use std::process::ExitCode;

use asrs_js::string::JsString;
use asrs_log::log::SerializedLog;
use asrs_parity::Value;

use crate::historical_session::{End, Session};
use crate::log_dump::{serialized_value, string_value};
use crate::log_run;

const USAGE: &str = "usage: asrs-cli decode [--initial] <log file>";
pub(crate) const NEVER_COMPLETES: u8 = 3;
pub(crate) const UNMODELED: u8 = 4;

/// The `new Date().getTime()` a decode worker sees, pinned so workers that throttle progress
/// by wall clock (Road Runner, RLOG) emit a fixed sequence; the oracle pins the same value.
pub(crate) const WORKER_FROZEN_MS: f64 = 1_000_000_000_000.0;

pub(crate) fn dump_log(
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
    let [strings, schemas, protos, structs, arrays, proto_queue] =
        log_run::decoder_state(serialized);
    lines.push(Value::object([strings, schemas]).encode());
    lines.push(Value::object([protos]).encode());
    lines.push(Value::object([structs, arrays, proto_queue]).encode());
    lines
}

pub fn run(args: &[String]) -> ExitCode {
    let (initial_only, path) = match args {
        [flag, path] if flag == "--initial" => (true, path),
        [path] if !path.starts_with("--") => (false, path),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let mut session = Session::new(false, true);
    let root = session.open(&JsString::from(path.as_str()), &JsString::new());
    let mut out = String::new();
    let mut dump = |session: &Session, phase: &str| {
        let loading: Vec<JsString> = session
            .source(root)
            .get_loading_fields()
            .into_iter()
            .collect();
        let serialized = session.log().to_serialized();
        for line in dump_log(&serialized, phase, session.status(root).name(), &loading) {
            out.push_str(&line);
            out.push('\n');
        }
    };
    let result = session
        .settle()
        .map(|()| dump(&session, "initial"))
        .and_then(|()| {
            if initial_only {
                return Ok(());
            }
            session.load_everything()?;
            dump(&session, "full");
            Ok(())
        });
    match result {
        Ok(()) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(End::NeverReturns) => {
            eprintln!("{path}: upstream's worker never returns");
            ExitCode::from(NEVER_COMPLETES)
        }
        Err(End::Unmodeled(what)) => {
            eprintln!("{path}: not modeled: {what}");
            ExitCode::from(UNMODELED)
        }
    }
}
