#![allow(clippy::disallowed_methods)]

//! Replays `export/runs` (oracle/src/exportRuns.ts): upstream's export worker on the logs of
//! `parity/specs/hub/logs`. For each run the port decodes the log, exports it, and must report
//! the same progress values in the same order and produce the same bytes, or reject.

mod functions {
    use asrs_parity::{Response, Value};

    pub type Function = fn(&[Value]) -> Response;

    pub fn arg(args: &[Value], i: usize) -> Value {
        args.get(i).cloned().unwrap_or(Value::Undefined)
    }
}

#[path = "../../asrs-cli/src/decode.rs"]
#[allow(dead_code)]
mod decode;
#[path = "../../asrs-cli/src/historical_session.rs"]
#[allow(dead_code)]
mod historical_session;
#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;
#[path = "../../asrs-cli/src/log_field.rs"]
#[allow(dead_code)]
mod log_field;
#[path = "../../asrs-cli/src/log_run.rs"]
#[allow(dead_code)]
mod log_run;

use asrs_formats::export::{ExportError, ExportOptions, Format, Sampling, export};
use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_parity::Value;
use asrs_parity::vectors;

use historical_session::{End, Session};

fn options(value: &JsValue) -> ExportOptions {
    let field = |key: &str| value.get(key).unwrap_or_else(|| panic!("no {key}"));
    let text = |key: &str| field(key).as_str().expect(key).to_string_lossy();
    ExportOptions {
        format: match text("format").as_str() {
            "csv-table" => Format::CsvTable,
            "csv-list" => Format::CsvList,
            "wpilog" => Format::Wpilog,
            "mcap" => Format::Mcap,
            other => panic!("format {other}"),
        },
        sampling: match text("samplingMode").as_str() {
            "changes" => Sampling::Changes,
            "fixed" => Sampling::Fixed,
            "akit" => Sampling::Akit,
            other => panic!("samplingMode {other}"),
        },
        sampling_period: field("samplingPeriod").as_f64().expect("samplingPeriod"),
        prefixes: field("prefixes").as_str().expect("prefixes").clone(),
        include_generated: field("includeGenerated")
            .as_bool()
            .expect("includeGenerated"),
    }
}

fn open(file: &str) -> Session {
    let path = vectors::workspace_root()
        .join("parity/specs/export")
        .join(file);
    let mut session = Session::new(false, true);
    session.open(
        &JsString::from(path.to_string_lossy().as_ref()),
        &JsString::new(),
    );
    match session.settle().and_then(|()| session.load_everything()) {
        Ok(()) => session,
        Err(End::NeverReturns) => panic!("{file}: the decode worker never returns"),
        Err(End::Unmodeled(what)) => panic!("{file}: {what}"),
    }
}

#[test]
fn export_runs() {
    let records = vectors::load("export/runs").unwrap_or_else(|e| panic!("{e}"));
    let mut opened: Option<(String, Session)> = None;
    let (mut written, mut rejected, mut rejected_after_progress, mut values) = (0, 0, 0, 0);
    for record in &records {
        let file = record.string("file").expect("file");
        let value = |key: &str| record.value(key).unwrap_or_else(|_| panic!("{key}"));
        let options =
            options(&js_json::to_js(&value("options")).unwrap_or_else(|e| panic!("options: {e}")));
        let at = format!("line {}: {file} {options:?}", record.line);
        if opened.as_ref().is_none_or(|(name, _)| *name != file) {
            opened = Some((file.clone(), open(&file)));
        }
        let log = opened.as_ref().expect("opened").1.log();

        let mut progress = Vec::new();
        let output = export(log, &options, &mut |value| progress.push(value));

        let posted: Vec<u64> = match value("progress") {
            Value::Array(items) => items
                .iter()
                .map(|v| v.as_f64().expect("a number").to_bits())
                .collect(),
            other => panic!("{at}: progress {other:?}"),
        };
        let got: Vec<u64> = progress.iter().map(|v| v.to_bits()).collect();
        assert_eq!(got.len(), posted.len(), "{at}: progress count");
        assert_eq!(got, posted, "{at}: progress");
        values += posted.len();

        match (value("bytes"), output) {
            (Value::Null, Err(ExportError::Rejected)) => {
                rejected += 1;
                rejected_after_progress += usize::from(!posted.is_empty());
            }
            (Value::Bytes(bytes), Ok(output)) => {
                assert!(output.into_bytes() == bytes, "{at}: bytes differ");
                written += 1;
            }
            (want, Ok(_)) => panic!("{at}: exported, upstream {want:?}"),
            (_, Err(e)) => panic!("{at}: {e:?}"),
        }
    }
    assert_eq!(written + rejected, records.len());
    assert!(written > 60 && rejected > 10 && rejected_after_progress > 0 && values > 10_000);
}
