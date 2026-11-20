//! `asrs-cli decode <log file>`: opens the file as the hub opens a historical log and prints the
//! canonical CVE-1 dump for the "initial" and "full" phases, matching `oracle decode`
//! (oracle/src/decode.ts, oracle/src/historical/dump.ts). Where upstream's worker never returns,
//! nothing is printed and the exit code is `NEVER_COMPLETES`, the oracle's timeout. With
//! `--initial` only the "initial" phase is printed; a partial log (WPILOG) has no "full" phase
//! until lazy field loading is ported (unit F09).

use std::process::ExitCode;

use asrs_formats::historical::HistoricalDataSource;
use asrs_formats::worker::Abort;
use asrs_js::string::JsString;
use asrs_js::time::{SystemClock, V8MathRandom};
use asrs_log::log::{Log, SerializedLog};
use asrs_log::realm::JsRealm;
use asrs_parity::Value;
use asrs_services::historical_start;

use crate::log_field::{serialized_value, string_value};
use crate::log_run;

const USAGE: &str = "usage: asrs-cli decode [--initial] <log file>";
pub(crate) const NEVER_COMPLETES: u8 = 3;
pub(crate) const UNMODELED: u8 = 4;

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
    let mut source = HistoricalDataSource::open_file(path);
    let mut log = Log::new(true);
    let mut realm = JsRealm::new(Box::new(V8MathRandom::from_entropy()));
    let files = historical_start::read_files(source.path());
    match source.handle_main_message(
        &mut log,
        &mut realm,
        files,
        Box::new(V8MathRandom::from_entropy()),
        Box::new(SystemClock),
    ) {
        Ok(Some(Abort::NeverReturns)) => {
            eprintln!("{path}: upstream's worker never returns");
            return ExitCode::from(NEVER_COMPLETES);
        }
        Ok(Some(Abort::Unmodeled(what))) => {
            eprintln!("{path}: not modeled: {what}");
            return ExitCode::from(UNMODELED);
        }
        Ok(_) => {}
        Err(unported) => {
            eprintln!("{path}: {:?} worker is not ported yet", unported.0);
            return ExitCode::FAILURE;
        }
    }
    let serialized = log.to_serialized();
    let status = source.status().name();
    let mut out = String::new();
    if !initial_only && source.log_is_partial() {
        eprintln!("{path}: the full phase of a partial log needs lazy field loading (unit F09)");
        return ExitCode::FAILURE;
    }
    let phases: &[&str] = if initial_only {
        &["initial"]
    } else {
        &["initial", "full"]
    };
    for &phase in phases {
        for line in dump_log(&serialized, phase, status, &[]) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    print!("{out}");
    ExitCode::SUCCESS
}
