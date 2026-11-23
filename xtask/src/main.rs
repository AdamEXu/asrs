#![allow(clippy::disallowed_methods)]

mod app;
mod assets;
mod bundle;
mod canvas;
mod check;
mod corpus;
mod coverage;
mod docs;
mod dom;
mod field3d;
mod glb;
mod gpui;
mod harness;
mod historical;
mod host;
mod layout;
mod libm;
mod licenses;
mod mutate;
mod oracle;
mod pack;
mod parity;
mod pixel;
mod public;
mod sessions;
mod tables;
mod text;
mod ui;
mod vectors;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

type Result<T = ()> = std::result::Result<T, String>;

const USAGE: &str = "usage: cargo xtask <command>

commands:
  app       Electron scenario driver: run upstream on parity/scenarios, compare, normalize (see `cargo xtask app`)
  check     layout rules, harness strictness, gpui vendor and features, rustfmt, clippy, tests
  layout    crate dependency and lint opt-out rules only
  harness   strictness lint over replay harnesses only (see `cargo xtask harness help`)
  assets    copy upstream assets into assets/
  docs      build upstream's embedded documentation into target/docs/build (see `cargo xtask docs help`)
  licenses  third-party license bundle for releases into target/licenses (needs cargo-about)
  bundle    release directory: app, LGPL sin/cos library, license bundle (see `cargo xtask bundle help`)
  libm      build or plant-test the LGPL sin/cos library asrs loads at runtime (see `cargo xtask libm help`)
  oracle    parity oracle: setup, gen, fn, smoke (see `cargo xtask oracle`)
  corpus    test corpora: gen, list, fuzz, fetch (see `cargo xtask corpus`)
  host      Windows and Linux render hosts: start, sync, run, fetch, check (see `cargo xtask host`)
  gpui      vendored gpui patches and parity feature checks (see `cargo xtask gpui`)
  mutate    mutation testing of port files or recorded vectors (see `cargo xtask mutate`)
  parity    compare the oracle with the port: cve1, fn, diff (see `cargo xtask parity`)
  pixel     section 2.5 pixel gates between upstream and port runs, port scenarios, selftest
            (see `cargo xtask pixel`)
  canvas    canvas suite: Chromium references, mask-gamma LUT, text metrics, gates (see `cargo xtask canvas`)
  ui        UI theme scenes: icon tints, buttons, gates against Electron (see `cargo xtask ui`)
  field3d   3D reference scenes against the wgpu renderer, gate 4 (see `cargo xtask field3d`)
  text      DOM text scenes for the Text element, gates against Electron (see `cargo xtask text`)
  tables    table generators from upstream TypeScript (see `cargo xtask tables`)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("app") => app::run(&args[1..]),
        Some("check") => check::run(),
        Some("layout") => layout::run(),
        Some("harness") => harness::run(&args[1..]),
        Some("assets") => assets::run(),
        Some("docs") => docs::run(&args[1..]),
        Some("licenses") => licenses::run(),
        Some("bundle") => bundle::run(&args[1..]),
        Some("libm") => libm::run(&args[1..]),
        Some("oracle") => oracle::run(&args[1..]),
        Some("corpus") => corpus::run(&args[1..]),
        Some("gpui") => gpui::run(&args[1..]),
        Some("mutate") => mutate::run(&args[1..]),
        Some("parity") => parity::run(&args[1..]),
        Some("pixel") => pixel::run(&args[1..]),
        Some("canvas") => canvas::run(&args[1..]),
        Some("ui") => ui::run(&args[1..]),
        Some("field3d") => field3d::run(&args[1..]),
        Some("text") => text::run(&args[1..]),
        Some("host") => host::run(&args[1..]),
        Some("tables") => tables::run(&args[1..]),
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

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
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
