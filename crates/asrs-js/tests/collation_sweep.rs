//! Deep sweep of `asrs_js::collation` against V8 itself: seeded random string pairs stream through
//! `oracle/sweep/collation.mjs` under the oracle's Electron runtime and through the Rust port,
//! and both `localeCompare` forms must agree on every pair. Needs `cargo xtask oracle setup`.
//! Run with
//!
//! ```sh
//! cargo test --release -p asrs-js --test collation_sweep -- --ignored --nocapture
//! ```
//!
//! `ASRS_COLLATION_SWEEP_PAIRS` (default 1000000), `ASRS_COLLATION_SWEEP_SEED` (default 1) and
//! `ASRS_ELECTRON` tune it.

#![allow(clippy::disallowed_methods)]

use std::cmp::Ordering;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

use asrs_js::collation::{locale_compare_numeric_utf16, locale_compare_utf16};
use asrs_parity::json::write_string;
use asrs_parity::random::Rng;

const CHUNK: usize = 4096;
const SHOWN: usize = 10;

const POOLS: &[(u32, u32)] = &[
    (0x20, 0x7e),
    (0x30, 0x39),
    (0x00, 0x1f),
    (0xa0, 0x24f),
    (0x300, 0x36f),
    (0x370, 0x52f),
    (0x590, 0x6ff),
    (0x900, 0xdff),
    (0xe00, 0xeff),
    (0xf00, 0xfff),
    (0x1000, 0x109f),
    (0x1100, 0x11ff),
    (0x1e00, 0x1fff),
    (0x2000, 0x2bff),
    (0x3000, 0x30ff),
    (0x3130, 0x318f),
    (0x4e00, 0x9fff),
    (0xa000, 0xd7ff),
    (0xd800, 0xdfff),
    (0xe000, 0xffff),
    (0xff00, 0xffef),
    (0x10000, 0x1ffff),
    (0x1d7ce, 0x1d7ff),
    (0x1f300, 0x1faff),
    (0x20000, 0x10ffff),
];

const WORDS: &[&str] = &[
    "/RealOutputs/Drive/Module",
    "NT:/SmartDashboard/",
    "/AdvantageKit/DriverStation/Joystick",
    "Module",
    "Robot ",
    "2025 KitBot",
    "v1.",
    "Camera_",
];

fn push_code_point(cp: u32, out: &mut Vec<u16>) {
    if cp >= 0x10000 {
        let c = cp - 0x10000;
        out.push(0xd800 | (c >> 10) as u16);
        out.push(0xdc00 | (c & 0x3ff) as u16);
    } else {
        out.push(cp as u16);
    }
}

fn random_string(rng: &mut Rng) -> Vec<u16> {
    let mut out = Vec::new();
    if rng.below(3) == 0 {
        out.extend(WORDS[rng.below(WORDS.len() as u64) as usize].encode_utf16());
    }
    let len = rng.below(10);
    let pool = POOLS[rng.below(POOLS.len() as u64) as usize];
    for _ in 0..len {
        let (lo, hi) = if rng.below(3) == 0 {
            POOLS[rng.below(POOLS.len() as u64) as usize]
        } else {
            pool
        };
        push_code_point(lo + rng.below(u64::from(hi - lo + 1)) as u32, &mut out);
    }
    out
}

fn mutate(rng: &mut Rng, base: &[u16]) -> Vec<u16> {
    let mut out = base.to_vec();
    for _ in 0..1 + rng.below(2) {
        let at = rng.below(out.len() as u64 + 1) as usize;
        match rng.below(4) {
            0 if at < out.len() => {
                out.remove(at);
            }
            1 if at < out.len() => out[at] ^= 0x20,
            _ => {
                let mut extra = Vec::new();
                let (lo, hi) = POOLS[rng.below(POOLS.len() as u64) as usize];
                push_code_point(lo + rng.below(u64::from(hi - lo + 1)) as u32, &mut extra);
                out.splice(at..at, extra);
            }
        }
    }
    out
}

fn pair(rng: &mut Rng) -> (Vec<u16>, Vec<u16>) {
    let a = random_string(rng);
    let b = if rng.below(2) == 0 {
        mutate(rng, &a)
    } else {
        random_string(rng)
    };
    (a, b)
}

fn symbol(order: Ordering) -> u8 {
    match order {
        Ordering::Less => b'<',
        Ordering::Equal => b'=',
        Ordering::Greater => b'>',
    }
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

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn env_number(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.replace('_', "").parse().ok())
        .unwrap_or(default)
}

fn escape(units: &[u16]) -> String {
    let mut out = String::from("\"");
    for &u in units {
        if (0x20..0x7f).contains(&u) && u != u16::from(b'"') && u != u16::from(b'\\') {
            out.push(char::from(u as u8));
        } else {
            out.push_str(&format!("\\u{u:04x}"));
        }
    }
    out.push('"');
    out
}

#[test]
#[ignore = "deep sweep: needs the oracle runtime"]
fn collation_matches_v8_on_random_pairs() {
    let pairs = env_number("ASRS_COLLATION_SWEEP_PAIRS", 1_000_000);
    let seed = env_number("ASRS_COLLATION_SWEEP_SEED", 1);
    let mut child = Command::new(electron())
        .arg(root().join("oracle/sweep/collation.mjs"))
        .env("ELECTRON_RUN_AS_NODE", "1")
        .env("LANG", "en_US.UTF-8")
        .env("LC_ALL", "en_US.UTF-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn the oracle runtime");
    let stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::with_capacity(1 << 20, child.stdout.take().expect("stdout"));
    let (tx, rx) = mpsc::sync_channel::<Vec<(Vec<u16>, Vec<u16>)>>(8);
    let writer = thread::spawn(move || {
        let mut stdin = BufWriter::with_capacity(1 << 20, stdin);
        let mut rng = Rng::new(seed);
        let mut left = pairs;
        while left > 0 {
            let n = left.min(CHUNK as u64) as usize;
            left -= n as u64;
            let chunk: Vec<_> = (0..n).map(|_| pair(&mut rng)).collect();
            let mut text = String::new();
            for (a, b) in &chunk {
                text.push('[');
                write_string(a, &mut text);
                text.push(',');
                write_string(b, &mut text);
                text.push_str("]\n");
            }
            stdin
                .write_all(text.as_bytes())
                .expect("write to the oracle");
            if tx.send(chunk).is_err() {
                break;
            }
        }
        stdin.flush().expect("flush the oracle input");
    });

    let mut mismatches = 0u64;
    let mut checked = 0u64;
    let mut shown = Vec::new();
    for chunk in rx {
        let mut expected = vec![0u8; chunk.len() * 2];
        stdout
            .read_exact(&mut expected)
            .expect("read the oracle results");
        for (i, (a, b)) in chunk.iter().enumerate() {
            let actual = [
                symbol(locale_compare_utf16(a, b)),
                symbol(locale_compare_numeric_utf16(a, b)),
            ];
            checked += 1;
            if actual != expected[i * 2..i * 2 + 2] {
                mismatches += 1;
                if shown.len() < SHOWN {
                    shown.push(format!(
                        "{} vs {}: V8 {}, port {}",
                        escape(a),
                        escape(b),
                        String::from_utf8_lossy(&expected[i * 2..i * 2 + 2]),
                        String::from_utf8_lossy(&actual)
                    ));
                }
            }
        }
    }
    writer.join().expect("writer thread");
    child.wait().expect("oracle exit");
    eprintln!("collation sweep: {checked} pairs, {mismatches} mismatches");
    assert!(
        mismatches == 0,
        "{mismatches} of {checked} pairs differ from V8:\n{}",
        shown.join("\n")
    );
}
