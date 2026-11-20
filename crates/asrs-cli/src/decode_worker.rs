//! `asrs-cli decode-worker <log file>`: drives the file's decode worker on its own and prints
//! every message it posts, matching `oracle decodeWorker` (oracle/src/decodeWorker.ts). Where
//! upstream's worker never returns, the output stops there and the exit code is
//! `NEVER_COMPLETES`.

use std::io::Write as _;
use std::process::ExitCode;
use std::sync::Arc;

use asrs_formats::historical::{HistoricalDataSource, create_worker, select_worker};
use asrs_formats::worker::{Abort, DecodeWorker, Request, Response};
use asrs_js::string::JsString;
use asrs_js::time::{SetClock, V8MathRandom};
use asrs_log::log::robot_state::{
    AKIT_TIMESTAMP_KEYS, ALLIANCE_KEYS, AUTONOMOUS_KEYS, DRIVER_STATION_KEYS, ENABLED_KEYS,
    EVENT_KEYS, MATCH_NUMBER_KEYS, MATCH_TYPE_KEYS, SYSTEM_TIME_KEYS, UTILITY_KEYS,
};
use asrs_parity::Value;
use asrs_services::historical_start;

use crate::decode::{NEVER_COMPLETES, UNMODELED, WORKER_FROZEN_MS, dump_log};
use crate::log_field::{serialized_value, string_value};

const USAGE: &str = "usage: asrs-cli decode-worker <log file>";
const MISSING_KEY: &str = "/asrs-oracle/missing";

fn message_lines(response: Response) -> Vec<String> {
    match response {
        Response::Progress(value) => vec![
            Value::object([
                ("type", Value::string("progress")),
                ("value", Value::number(value)),
            ])
            .encode(),
        ],
        Response::Initial { log, is_partial } => {
            let mut lines = vec![
                Value::object([
                    ("type", Value::string("initial")),
                    ("isPartial", Value::Bool(is_partial)),
                ])
                .encode(),
            ];
            lines.extend(dump_log(&log.structured_clone(), "worker", "", &[]));
            lines
        }
        Response::Failed => vec![Value::object([("type", Value::string("failed"))]).encode()],
        Response::Fields(fields) => {
            let mut lines = vec![
                Value::object([
                    ("type", Value::string("fields")),
                    ("count", Value::number(fields.len() as f64)),
                ])
                .encode(),
            ];
            for field in fields {
                let Value::Object(mut pairs) = serialized_value(&field.data.structured_clone())
                else {
                    unreachable!("serialized_value always returns an object")
                };
                pairs.insert(0, ("key".into(), string_value(&field.key)));
                pairs.insert(
                    1,
                    (
                        "generatedParent".into(),
                        Value::Bool(field.generated_parent),
                    ),
                );
                lines.push(Value::Object(pairs).encode());
            }
            lines
        }
    }
}

struct Output(std::io::StdoutLock<'static>);

impl Output {
    fn write(&mut self, lines: &[String]) {
        for line in lines {
            let _ = writeln!(self.0, "{line}");
        }
    }
}

/// One request: its messages, then a `threw` line for a throw. `Err` is how the run ends early.
fn request(
    out: &mut Output,
    worker: &mut dyn DecodeWorker,
    request: Request,
    initial: &mut Option<(bool, Vec<JsString>)>,
) -> Result<(), ExitCode> {
    let end = worker.handle(request, &mut |response| {
        if let Response::Initial { log, is_partial } = &response {
            *initial = Some((
                *is_partial,
                log.fields.iter().map(|(key, _)| key.clone()).collect(),
            ));
        }
        out.write(&message_lines(response));
    });
    match end {
        Ok(()) => Ok(()),
        Err(Abort::Threw) => {
            out.write(&[Value::object([("type", Value::string("threw"))]).encode()]);
            Ok(())
        }
        Err(Abort::NeverReturns) => Err(ExitCode::from(NEVER_COMPLETES)),
        Err(Abort::Unmodeled(what)) => {
            eprintln!("not modeled: {what}");
            Err(ExitCode::from(UNMODELED))
        }
    }
}

fn header(fields: impl IntoIterator<Item = (&'static str, Value)>) -> String {
    Value::object(fields).encode()
}

pub fn run(args: &[String]) -> ExitCode {
    let path = match args {
        [path] if !path.starts_with('-') => path,
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let source = HistoricalDataSource::open_file(path);
    let Some(name) = select_worker(source.path()) else {
        eprintln!("{path}: no worker for this extension");
        return ExitCode::FAILURE;
    };
    let mut worker = match create_worker(
        name,
        Box::new(V8MathRandom::from_entropy()),
        Box::new(SetClock(WORKER_FROZEN_MS)),
    ) {
        Ok(worker) => worker,
        Err(unported) => {
            eprintln!("{path}: {:?} worker is not ported yet", unported.0);
            return ExitCode::FAILURE;
        }
    };
    let files = historical_start::read_files(source.path())
        .into_iter()
        .map(|file| file.map(Arc::from))
        .collect();

    let mut out = Output(std::io::stdout().lock());
    let mut initial = None;
    out.write(&[header([("request", Value::string("start"))])]);
    if let Err(code) = request(
        &mut out,
        worker.as_mut(),
        Request::Start(files),
        &mut initial,
    ) {
        return code;
    }
    let Some((true, keys)) = initial.take() else {
        return ExitCode::SUCCESS;
    };
    let mut requests = keys.clone();
    for group in [
        ENABLED_KEYS,
        AUTONOMOUS_KEYS,
        UTILITY_KEYS,
        ALLIANCE_KEYS,
        DRIVER_STATION_KEYS,
        SYSTEM_TIME_KEYS,
        AKIT_TIMESTAMP_KEYS,
        EVENT_KEYS,
        MATCH_TYPE_KEYS,
        MATCH_NUMBER_KEYS,
    ] {
        requests.extend(group.iter().map(|key| JsString::from(*key)));
    }
    if let Some(first) = keys.first() {
        let mut slashed = JsString::from("/");
        slashed.push_str(first);
        requests.push(slashed);
    }
    requests.push(JsString::from(MISSING_KEY));
    requests.extend(keys.first().cloned());
    for key in requests {
        out.write(&[header([
            ("request", Value::string("parseField")),
            ("key", string_value(&key)),
        ])]);
        if let Err(code) = request(
            &mut out,
            worker.as_mut(),
            Request::ParseField(key),
            &mut initial,
        ) {
            return code;
        }
    }
    ExitCode::SUCCESS
}
