#![allow(clippy::disallowed_methods)]

mod assets;
mod canvas;
mod check;
mod corpus;
mod gpui;
mod historical;
mod layout;
mod oracle;
mod pack;
mod parity;
mod public;
mod ui;
mod vectors;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

type Result<T = ()> = std::result::Result<T, String>;

const USAGE: &str = "usage: cargo xtask <command>

commands:
  check     layout rules, gpui vendor and features, rustfmt, clippy, tests
  layout    crate dependency and lint opt-out rules only
  assets    copy upstream assets into assets/
  oracle    parity oracle: setup, gen, fn, smoke (see `cargo xtask oracle`)
  corpus    test corpora: gen, list, fuzz, fetch (see `cargo xtask corpus`)
  gpui      vendored gpui-pre patch and parity feature checks (see `cargo xtask gpui`)
  parity    compare the oracle with the port: cve1, fn, diff (see `cargo xtask parity`)
  canvas    canvas suite: Chromium references, mask-gamma LUT, text metrics, gates (see `cargo xtask canvas`)
  ui        UI theme scenes: icon tints, buttons, gates against Electron (see `cargo xtask ui`)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("check") => check::run(),
        Some("layout") => layout::run(),
        Some("assets") => assets::run(),
        Some("oracle") => oracle::run(&args[1..]),
        Some("corpus") => corpus::run(&args[1..]),
        Some("gpui") => gpui::run(&args[1..]),
        Some("parity") => parity::run(&args[1..]),
        Some("canvas") => canvas::run(&args[1..]),
        Some("ui") => ui::run(&args[1..]),
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

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the workspace")
        .to_path_buf()
}

fn cargo(args: &[&str]) -> Result {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    eprintln!("$ cargo {}", args.join(" "));
    let status = Command::new(cargo)
        .args(args)
        .current_dir(workspace_root())
        .status()
        .map_err(|e| format!("failed to run cargo: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo {} failed ({status})", args.join(" ")))
    }
}
