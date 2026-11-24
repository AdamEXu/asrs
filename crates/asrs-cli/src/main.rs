#![allow(clippy::disallowed_methods)]

mod assets;
mod canvas_cmd;
mod decode;
mod decode_worker;
mod export_cmd;
mod fn_cmd;
mod functions;
mod geometry;
mod glb;
mod heatmap;
mod historical_session;
mod js_color;
mod js_json;
mod js_math;
mod js_msgpack;
mod js_ops;
mod js_regex;
mod js_string;
mod js_time;
mod live_base;
mod log_field;
mod log_keys;
mod log_run;
mod log_struct;
mod merge_calculator;
mod photon;
mod robot_state;
mod shared;
mod state;
mod tabs_console;
mod units;
mod wpilog;

use std::process::ExitCode;

const USAGE: &str = "usage: asrs-cli <command>

commands:
  fn [--list]   JSONL fn requests on stdin, CVE-1 responses on stdout (same protocol as the oracle)
  decode [--initial] <file>
                decode a historical log and print the canonical CVE-1 dump (same as oracle decode)
  decode-worker <file>
                run the file's decode worker alone and print its messages (same as oracle decodeWorker)
  export <file> --options <json> [--out <file>] [--fixed-limit <n>]
                decode fully, then run the export port (same outcomes as oracle export)
  export-run --root <dir> --out <dir> --spec <spec>
                run the spec's export matrix over stdin inputs (same as oracle historicalRun)
  historical-session <sessions.jsonl>
                run scripted HistoricalDataSource sessions and print their traces (same as
                oracle historicalSession)
  canvas lut    fit the mask-gamma table from the oracle's calibration scenes
  canvas render rasterize the oracle's canvas scenes with asrs-canvas
  heatmap       replay the oracle's heatmap fixtures with asrs-tabs
  glb release <dir> [--jobs N]
                load every Field3d_/Robot_ asset under <dir> in all three render modes
                (same lines as oracle genGlb release)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("fn") => fn_cmd::run(&args[1..]),
        Some("decode") => return decode::run(&args[1..]),
        Some("decode-worker") => return decode_worker::run(&args[1..]),
        Some("export") | Some("export-run") => return export_cmd::run(&args[1..]),
        Some("historical-session") => return historical_session::run(&args[1..]),
        Some("canvas") => canvas_cmd::run(&args[1..]),
        Some("heatmap") => heatmap::run(&args[1..]),
        Some("wpilog") => wpilog::run(&args[1..]),
        Some("glb") => glb::run(&args[1..]),
        _ => Err(USAGE.to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
