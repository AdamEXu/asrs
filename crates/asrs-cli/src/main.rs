#![allow(clippy::disallowed_methods)]

mod fn_cmd;
mod functions;

use std::process::ExitCode;

const USAGE: &str = "usage: asrs-cli <command>

commands:
  fn [--list]   JSONL fn requests on stdin, CVE-1 responses on stdout (same protocol as the oracle)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("fn") => fn_cmd::run(&args[1..]),
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
