#![allow(clippy::disallowed_methods)]

mod assets;
mod canvas_cmd;
mod decode;
mod fn_cmd;
mod functions;
mod geometry;
mod heatmap;
mod js_color;
mod js_json;
mod js_math;
mod js_msgpack;
mod js_regex;
mod js_string;
mod js_time;
mod live_base;
mod log_field;
mod log_keys;
mod log_run;
mod log_struct;
mod photon;
mod shared;
mod tabs_console;
mod units;
mod wpilog;

use std::process::ExitCode;

const USAGE: &str = "usage: asrs-cli <command>

commands:
  fn [--list]   JSONL fn requests on stdin, CVE-1 responses on stdout (same protocol as the oracle)
  decode <file> decode a historical log and print the canonical CVE-1 dump (same as oracle decode)
  canvas lut    fit the mask-gamma table from the oracle's calibration scenes
  canvas render rasterize the oracle's canvas scenes with asrs-canvas
  heatmap       replay the oracle's heatmap fixtures with asrs-tabs";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("fn") => fn_cmd::run(&args[1..]),
        Some("decode") => return decode::run(&args[1..]),
        Some("canvas") => canvas_cmd::run(&args[1..]),
        Some("heatmap") => heatmap::run(&args),
        Some("wpilog") => wpilog::run(&args[1..]),
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
