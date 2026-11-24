//! `asrs-cli export <log file> --options <ExportOptions JSON>`: decodes the file like
//! `decode`, loads every field the way export's `loadAllFields` does, applies the oracle
//! session's pre-export skip checks and runs the export port on the resulting `Log`.
//! Bytes go to `--out` or stdout. Exit 0 when the worker produced output, 3 when it
//! rejected (or the decode worker never returns), 4 when the run was skipped or the
//! branch is one the port does not model yet. The skip checks are the oracle session's
//! (oracle/src/historical/session.ts), for the two option sets upstream never finishes.
//!
//! `asrs-cli export-run --root <dir> --out <dir> --spec <spec.json>` mirrors oracle
//! `historicalRun`: input paths relative to `--root` on stdin, one per line; each input
//! is decoded once, then the spec's export matrix runs on it. Outputs land in
//! `<out>/<input>/export/<name>.<ext>` and one JSON line per input goes to stdout:
//! `{"input": ..., "outputs": [{"name", "outcome", "file"?}]}` where outcome is "ok",
//! "failed", "skipped" or "unmodeled" (a branch the port does not have yet).

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use std::cmp::Ordering;

use asrs_formats::export::{self, ExportOptions, Format, Sampling};
use asrs_formats::mcap_export::value_json;
use asrs_js::json;
use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_log::log::{Log, LoggableType};
use asrs_log::util::upper_bound;
use asrs_parity::json::Json;

use crate::decode::{NEVER_COMPLETES, UNMODELED};
use crate::historical_session::{End, Session};

const USAGE: &str = "usage:
  asrs-cli export <log file> --options <ExportOptions JSON> [--out <file>] [--fixed-limit <n>]
  asrs-cli export-run --root <dir> --out <dir> --spec <spec.json>";

/// Exit status when the export worker rejects, as oracle `export` reports it.
const EXPORT_FAILED: u8 = 3;

const MCAP_CHUNK_SIZE: f64 = 1024.0 * 1024.0;
const MCAP_SCHEMA_OVERHEAD: f64 = 2048.0;
const MCAP_TYPES: &[LoggableType] = &[
    LoggableType::Boolean,
    LoggableType::Number,
    LoggableType::String,
    LoggableType::BooleanArray,
    LoggableType::NumberArray,
];

const EXTENSIONS: [(Format, &str); 4] = [
    (Format::CsvTable, "csv"),
    (Format::CsvList, "csv"),
    (Format::Wpilog, "wpilog"),
    (Format::Mcap, "mcap"),
];

struct Case {
    name: String,
    extension: &'static str,
    options: ExportOptions,
}

fn open_session(path: &Path) -> Result<Session, End> {
    let mut session = Session::new(false, true);
    session.open(
        &JsString::from(path.to_string_lossy().as_ref()),
        &JsString::new(),
    );
    session.settle()?;
    session.load_everything()?;
    Ok(session)
}

fn end_exit(path: &Path, end: &End) -> ExitCode {
    match end {
        End::NeverReturns => {
            eprintln!("{}: upstream's worker never returns", path.display());
            ExitCode::from(NEVER_COMPLETES)
        }
        End::Unmodeled(what) => {
            eprintln!("{}: not modeled: {what}", path.display());
            ExitCode::from(UNMODELED)
        }
    }
}

fn parse_options(text: &str) -> Result<ExportOptions, String> {
    let options = json::parse_str(text).map_err(|e| format!("options JSON: {e}"))?;
    let get = |key: &str| {
        options
            .as_object()
            .and_then(|object| object.get_key(&JsString::from(key)))
    };
    let format = match get("format")
        .and_then(JsValue::as_str)
        .and_then(|s| s.as_string())
        .as_deref()
    {
        Some("csv-table") => Format::CsvTable,
        Some("csv-list") => Format::CsvList,
        Some("wpilog") => Format::Wpilog,
        Some("mcap") => Format::Mcap,
        other => return Err(format!("options.format: {other:?} is not an export format")),
    };
    let sampling = match get("samplingMode")
        .and_then(JsValue::as_str)
        .and_then(|s| s.as_string())
        .as_deref()
    {
        Some("changes") => Sampling::Changes,
        Some("fixed") => Sampling::Fixed,
        Some("akit") => Sampling::Akit,
        other => return Err(format!("options.samplingMode: {other:?}")),
    };
    let sampling_period = get("samplingPeriod")
        .and_then(JsValue::as_f64)
        .unwrap_or(0.0);
    let prefixes = get("prefixes")
        .and_then(JsValue::as_str)
        .cloned()
        .unwrap_or_default();
    let include_generated = get("includeGenerated")
        .and_then(JsValue::as_bool)
        .unwrap_or(false);
    Ok(ExportOptions {
        format,
        sampling,
        sampling_period,
        prefixes,
        include_generated,
    })
}

fn run_case(log: &Log, case: &Case, fixed_limit: f64, dir: &Path) -> Json {
    let mut record = vec![("name", Json::string(&case.name))];
    if skip_reason(log, &case.options, fixed_limit).is_some() {
        record.push(("outcome", Json::string("skipped")));
    } else {
        match export::export(log, &case.options, &mut |_| {}) {
            Ok(output) => {
                let file = format!("export/{}.{}", case.name, case.extension);
                match fs::write(dir.join(&file), output.into_bytes()) {
                    Ok(()) => {
                        record.push(("outcome", Json::string("ok")));
                        record.push(("file", Json::string(&file)));
                    }
                    Err(e) => {
                        record.push(("outcome", Json::string("failed")));
                        eprintln!("{}: {e}", dir.join(&file).display());
                    }
                }
            }
            Err(export::ExportError::Rejected) => {
                record.push(("outcome", Json::string("failed")));
            }
            // Only the sampled MCAP bound produces this, and the oracle pre-skips it.
            Err(export::ExportError::NeverCompletes) => {
                record.push(("outcome", Json::string("skipped")));
            }
            Err(export::ExportError::Unported(_)) => {
                record.push(("outcome", Json::string("unmodeled")));
            }
        }
    }
    Json::object(record)
}

fn option_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn export_single(args: &[String]) -> ExitCode {
    let file = args.first().filter(|arg| !arg.starts_with('-'));
    let options_text = option_value(args, "--options");
    let (Some(file), Some(options_text)) = (file, options_text) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let options = match parse_options(&options_text) {
        Ok(options) => options,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let limit: f64 = option_value(args, "--fixed-limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(100000.0);
    let path = PathBuf::from(file);
    if [".hoot", ".revlog", ".wpilogxz"]
        .iter()
        .any(|ext| file.ends_with(ext))
    {
        eprintln!("{file}: not modeled: converted input format");
        return ExitCode::from(UNMODELED);
    }
    let session = match open_session(&path) {
        Ok(session) => session,
        Err(end) => return end_exit(&path, &end),
    };
    if let Some(reason) = skip_reason(session.log(), &options, limit) {
        eprintln!("export skipped: {reason}");
        return ExitCode::from(UNMODELED);
    }
    match export::export(session.log(), &options, &mut |_| {}) {
        Ok(output) => {
            let bytes = output.into_bytes();
            let result = match option_value(args, "--out") {
                Some(out) => fs::write(&out, &bytes).map_err(|e| e.to_string()),
                None => std::io::stdout()
                    .write_all(&bytes)
                    .map_err(|e| e.to_string()),
            };
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::FAILURE
                }
            }
        }
        Err(export::ExportError::Rejected) => {
            eprintln!("export failed");
            ExitCode::from(EXPORT_FAILED)
        }
        Err(export::ExportError::NeverCompletes) => {
            eprintln!("export never completes");
            ExitCode::from(NEVER_COMPLETES)
        }
        Err(export::ExportError::Unported(what)) => {
            eprintln!("export unmodeled: {what}");
            ExitCode::from(UNMODELED)
        }
    }
}

/// The oracle session's `export` pre-checks: fixed sampling over a range that needs more
/// than `fixed_limit` samples or a step that vanishes in float precision, and MCAP with
/// fixed or AdvantageKit sampling once the unawaited writes outgrow one 1 MiB chunk.
fn skip_reason(log: &Log, options: &ExportOptions, fixed_limit: f64) -> Option<&'static str> {
    if options.sampling == Sampling::Fixed {
        let [start, end] = log.get_timestamp_range();
        let step = options.sampling_period / 1000.0;
        let samples = (end - start) / step;
        if !js_le(samples, fixed_limit) || !js_gt(start + step, start) || !js_gt(end + step, end) {
            return Some("fixed sampling range too large");
        }
    }
    if options.format == Format::Mcap
        && options.sampling != Sampling::Changes
        && let Ok(Some(timestamps)) = export::sample_timestamps(log, options)
        && mcap_chunk_bound(log, &timestamps) > MCAP_CHUNK_SIZE
    {
        return Some("unawaited MCAP writes past one chunk");
    }
    None
}

fn js_le(a: f64, b: f64) -> bool {
    matches!(a.partial_cmp(&b), Some(Ordering::Less | Ordering::Equal))
}

fn js_gt(a: f64, b: f64) -> bool {
    matches!(a.partial_cmp(&b), Some(Ordering::Greater))
}

/// `mcapChunkBound`: a byte bound on the first chunk under sampled MCAP export, over
/// every log field of an MCAP schema type, regardless of the export's field selection.
fn mcap_chunk_bound(log: &Log, timestamps: &[f64]) -> f64 {
    let mut bound = MCAP_SCHEMA_OVERHEAD;
    for key in log.get_field_keys() {
        if !log.get_type(&key).is_some_and(|t| MCAP_TYPES.contains(&t)) {
            continue;
        }
        bound += 64.0 + asrs_js::string::text_encode(&key).len() as f64;
        let Some(set) = log.get_range(&key, f64::NEG_INFINITY, f64::INFINITY, None) else {
            continue;
        };
        let mut next = 0usize;
        for &timestamp in timestamps {
            next = upper_bound(set.timestamps(), timestamp, next);
            if next != 0 {
                bound += 31.0 + value_json(&set, next - 1).map_or(0.0, |json| json.len() as f64);
            }
        }
    }
    bound
}

fn matrix_cases(spec: &JsValue) -> Result<Vec<(Case, f64)>, String> {
    let matrix = spec
        .as_object()
        .and_then(|object| object.get_key(&JsString::from("export")))
        .ok_or("spec has no export matrix")?;
    let get = |key: &str| {
        matrix
            .as_object()
            .and_then(|o| o.get_key(&JsString::from(key)))
    };
    let strings = |key: &str| -> Result<Vec<String>, String> {
        get(key)
            .and_then(JsValue::as_array)
            .map(|values| {
                values
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(|s| s.to_string_lossy())
                            .ok_or_else(|| format!("spec.{key} has a non-string entry"))
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .ok_or_else(|| format!("spec.{key} missing"))?
    };
    let booleans = |key: &str| -> Result<Vec<bool>, String> {
        get(key)
            .and_then(JsValue::as_array)
            .map(|values| {
                values
                    .iter()
                    .map(|value| {
                        value
                            .as_bool()
                            .ok_or_else(|| format!("spec.{key} has a non-bool entry"))
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .ok_or_else(|| format!("spec.{key} missing"))?
    };
    let formats = strings("formats")?;
    let sampling_modes = strings("samplingModes")?;
    let generated = booleans("includeGenerated")?;
    let prefixes = strings("prefixes")?;
    let period = get("samplingPeriod")
        .and_then(JsValue::as_f64)
        .ok_or("spec.samplingPeriod missing")?;
    let fixed_limit = get("fixedSampleLimit")
        .and_then(JsValue::as_f64)
        .unwrap_or(100000.0);
    let mut cases = Vec::new();
    for format in &formats {
        for sampling in &sampling_modes {
            for &include_generated in &generated {
                for (index, prefix) in prefixes.iter().enumerate() {
                    let options_text = format!(
                        r#"{{"format":{},"samplingMode":{},"samplingPeriod":{period},"prefixes":{},"includeGenerated":{include_generated}}}"#,
                        json::stringify(&JsValue::String(JsString::from(format.as_str())))
                            .map_err(|e| e.to_string())?
                            .unwrap_or_default(),
                        json::stringify(&JsValue::String(JsString::from(sampling.as_str())))
                            .map_err(|e| e.to_string())?
                            .unwrap_or_default(),
                        json::stringify(&JsValue::String(JsString::from(prefix.as_str())))
                            .map_err(|e| e.to_string())?
                            .unwrap_or_default(),
                    );
                    let options = parse_options(&options_text)?;
                    let extension = EXTENSIONS
                        .iter()
                        .find(|(format_option, _)| format_option == &options.format)
                        .map(|(_, extension)| *extension)
                        .ok_or("format without an extension")?;
                    cases.push((
                        Case {
                            name: format!(
                                "{}.{}.{}.p{index}",
                                format,
                                sampling,
                                if include_generated {
                                    "generated"
                                } else {
                                    "original"
                                }
                            ),
                            extension,
                            options,
                        },
                        fixed_limit,
                    ));
                }
            }
        }
    }
    Ok(cases)
}

fn export_run(args: &[String]) -> ExitCode {
    let (Some(root), Some(out), Some(spec)) = (
        option_value(args, "--root"),
        option_value(args, "--out"),
        option_value(args, "--spec"),
    ) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let spec_text = match fs::read_to_string(&spec) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("{spec}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let spec_json = match json::parse_str(&spec_text) {
        Ok(spec) => spec,
        Err(e) => {
            eprintln!("{spec}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let cases = match matrix_cases(&spec_json) {
        Ok(cases) => cases,
        Err(e) => {
            eprintln!("{spec}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut stdin = String::new();
    if std::io::stdin().read_to_string(&mut stdin).is_err() {
        eprintln!("export-run: could not read stdin");
        return ExitCode::FAILURE;
    }
    let stdout = std::io::stdout();
    let mut out_lines = stdout.lock();
    for input in stdin.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let path = Path::new(&root).join(input);
        let dir = Path::new(&out).join(input);
        let export_dir = dir.join("export");
        if let Err(e) = fs::create_dir_all(&export_dir) {
            eprintln!("{}: {e}", export_dir.display());
            return ExitCode::FAILURE;
        }
        let line = match open_session(&path) {
            Ok(session) => {
                let mut outputs = Vec::with_capacity(cases.len());
                for (case, fixed_limit) in &cases {
                    outputs.push(run_case(session.log(), case, *fixed_limit, &dir));
                }
                Json::object([
                    ("input", Json::string(input)),
                    ("outputs", Json::Array(outputs)),
                ])
            }
            Err(end) => Json::object([
                ("input", Json::string(input)),
                (
                    "error",
                    Json::string(match end {
                        End::NeverReturns => "decode worker never returns",
                        End::Unmodeled(what) => what,
                    }),
                ),
            ]),
        };
        let _ = writeln!(out_lines, "{}", line.to_compact());
        let _ = out_lines.flush();
    }
    ExitCode::SUCCESS
}

pub fn run(args: &[String]) -> ExitCode {
    if args.iter().any(|arg| arg == "--spec") {
        export_run(args)
    } else {
        export_single(args)
    }
}
