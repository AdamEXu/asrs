#![allow(clippy::disallowed_methods)]

mod canvas_cmd;
mod fn_cmd;
mod functions;
mod js_color;
mod js_json;
mod js_math;
mod js_msgpack;
mod js_regex;
mod js_string;
mod js_time;
mod shared;

use std::process::ExitCode;

const USAGE: &str = "usage: asrs-cli <command>

commands:
  fn [--list]   JSONL fn requests on stdin, CVE-1 responses on stdout (same protocol as the oracle)
  canvas lut    fit the mask-gamma table from the oracle's calibration scenes
  canvas render rasterize the oracle's canvas scenes with asrs-canvas";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("fn") => fn_cmd::run(&args[1..]),
        Some("canvas") => canvas_cmd::run(&args[1..]),
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
