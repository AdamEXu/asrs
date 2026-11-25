//! `asrs-cli decode-bench`: times the decode `asrs-cli decode` does, matching
//! `oracle decodeBench` (oracle/src/decodeBench.ts).

use std::process::ExitCode;
use std::time::Instant;

use asrs_js::string::JsString;

use crate::historical_session::{End, Session};

const USAGE: &str = "usage: asrs-cli decode-bench --startup | [--initial] <log file>";

pub fn run(args: &[String]) -> ExitCode {
    let (initial_only, path) = match args {
        [flag] if flag == "--startup" => return ExitCode::SUCCESS,
        [flag, path] if flag == "--initial" => (true, path),
        [path] if !path.starts_with("--") => (false, path),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let mut session = Session::new(false, true);
    let start = Instant::now();
    session.open(&JsString::from(path.as_str()), &JsString::new());
    let mut result = session.settle();
    let opened = Instant::now();
    if result.is_ok() && !initial_only {
        result = session.load_everything();
    }
    let loaded = Instant::now();
    match result {
        Ok(()) => {}
        Err(End::NeverReturns) => {
            eprintln!("{path}: upstream's worker never returns");
            return ExitCode::FAILURE;
        }
        Err(End::Unmodeled(what)) => {
            eprintln!("{path}: not modeled: {what}");
            return ExitCode::FAILURE;
        }
    }
    let serialized = session.log().to_serialized();
    let samples: usize = serialized
        .fields
        .iter()
        .map(|(_, field)| field.timestamps.len())
        .sum();
    println!(
        r#"{{"initialMs":{},"fieldsMs":{},"fields":{},"samples":{samples}}}"#,
        (opened - start).as_secs_f64() * 1e3,
        (loaded - opened).as_secs_f64() * 1e3,
        serialized.fields.len(),
    );
    ExitCode::SUCCESS
}
