//! Deep sweep of `asrs_js::color` against V8's bundled `color-convert` under the oracle's
//! Electron runtime: every six-digit hex color (0x000000 through 0xffffff by default)
//! goes through `hex.hsl` and back through `hsl.hex` on both sides, and every result is
//! compared. Needs `cargo xtask oracle setup`. Run with
//!
//! ```sh
//! cargo test --release -p asrs-js --test color_sweep -- --ignored --nocapture
//! ```
//!
//! `ASRS_COLOR_SWEEP_INPUTS` (default 16777216), `ASRS_COLOR_SWEEP_START` and
//! `ASRS_ELECTRON` tune it.

#![allow(clippy::disallowed_methods)]

use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use asrs_js::color;

const CHUNK: usize = 1 << 16;
const SHOWN: usize = 8;
const RECORD: usize = 30;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate dir")
        .parent()
        .expect("workspace")
        .to_path_buf()
}

fn electron() -> PathBuf {
    if let Some(path) = std::env::var_os("ASRS_ELECTRON") {
        return PathBuf::from(path);
    }
    let package = root().join("target/oracle/upstream/node_modules/electron");
    let rel = std::fs::read_to_string(package.join("path.txt"))
        .expect("Electron is not installed; run `cargo xtask oracle setup`");
    package.join("dist").join(rel.trim())
}

fn env_number(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn rust_record(rgb: u32) -> [u8; RECORD] {
    let hex = format!("{rgb:06x}");
    let hsl = color::hex_hsl(&hex);
    let mut record = [0u8; RECORD];
    record[0..8].copy_from_slice(&hsl[0].to_le_bytes());
    record[8..16].copy_from_slice(&hsl[1].to_le_bytes());
    record[16..24].copy_from_slice(&hsl[2].to_le_bytes());
    record[24..30].copy_from_slice(color::hsl_hex(hsl).as_bytes());
    record
}

#[test]
#[ignore = "deep sweep against the Electron oracle"]
fn color_deep_sweep() {
    let started = Instant::now();
    let start = env_number("ASRS_COLOR_SWEEP_START", 0);
    let inputs = env_number("ASRS_COLOR_SWEEP_INPUTS", 1 << 24);
    let mut child = Command::new(electron())
        .arg(root().join("oracle/sweep/color.mjs"))
        .env("ELECTRON_RUN_AS_NODE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn the oracle runtime");
    let stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::with_capacity(1 << 20, child.stdout.take().expect("stdout"));
    let (tx, rx) = mpsc::sync_channel::<Vec<u32>>(8);
    let writer = thread::spawn(move || {
        let mut stdin = BufWriter::with_capacity(1 << 20, stdin);
        let mut left = inputs;
        let mut next = start;
        while left > 0 {
            let n = left.min(CHUNK as u64) as usize;
            left -= n as u64;
            let chunk: Vec<u32> = (0..n as u64).map(|i| (next + i) as u32).collect();
            next += n as u64;
            let bytes: Vec<u8> = chunk
                .iter()
                .flat_map(|v| [(v >> 16) as u8, (v >> 8) as u8, *v as u8])
                .collect();
            if tx.send(chunk).is_err() || stdin.write_all(&bytes).is_err() {
                return;
            }
        }
        let _ = stdin.flush();
    });

    let mut samples = 0u64;
    let mut mismatches = 0u64;
    let mut shown = Vec::new();
    let mut buf = Vec::new();
    for chunk in rx {
        buf.resize(chunk.len() * RECORD, 0);
        stdout
            .read_exact(&mut buf)
            .unwrap_or_else(|e| panic!("oracle output ended early: {e}"));
        for (i, &rgb) in chunk.iter().enumerate() {
            let want = &buf[i * RECORD..i * RECORD + RECORD];
            let got = rust_record(rgb);
            if want != got {
                mismatches += 1;
                if shown.len() < SHOWN {
                    let [h, s, l] = [
                        f64::from_le_bytes(want[0..8].try_into().expect("h")),
                        f64::from_le_bytes(want[8..16].try_into().expect("s")),
                        f64::from_le_bytes(want[16..24].try_into().expect("l")),
                    ];
                    shown.push(format!(
                        "#{rgb:06x}: oracle hsl [{h}, {s}, {l}] rt {}, rust hsl {:?} rt {}",
                        std::str::from_utf8(&want[24..30]).unwrap_or("?"),
                        color::hex_hsl(&format!("{rgb:06x}")),
                        std::str::from_utf8(&got[24..30]).unwrap_or("?"),
                    ));
                }
            }
        }
        samples += chunk.len() as u64;
    }
    writer.join().expect("writer thread");
    let status = child.wait().expect("oracle exit");
    assert!(status.success(), "oracle exited with {status}");
    for line in &shown {
        eprintln!("    {line}");
    }
    eprintln!(
        "color sweep: {samples} inputs, {mismatches} mismatches, {:.1}s",
        started.elapsed().as_secs_f64()
    );
    assert_eq!(mismatches, 0, "color sweep found mismatches");
}
