//! Deep sweep of `asrs_js::number` against V8 itself: every op streams seeded operands
//! through `oracle/sweep/number.mjs` under the oracle's Electron runtime and through the
//! Rust port, and compares results. Needs `cargo xtask oracle setup`. Run with
//!
//! ```sh
//! cargo test --release -p asrs-js --test number_sweep -- --ignored --nocapture
//! ```
//!
//! `ASRS_NUMBER_SWEEP_SAMPLES` (default 2000000 per op), `ASRS_NUMBER_SWEEP_OPS` (comma
//! separated, default all), `ASRS_NUMBER_SWEEP_SEED` (default 1) and `ASRS_ELECTRON`
//! tune it.

#![allow(clippy::disallowed_methods)]

use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use asrs_js::number::{self, IntlOptions, SignDisplay};
use asrs_parity::random::Rng;

const CHUNK: usize = 1 << 16;
const SENTINEL: u64 = 0x7ff8_dead_beef_0000;
const SHOWN: usize = 8;
const BOUNDARY: usize = 64;

const UPSTREAM_INTL: IntlOptions = IntlOptions {
    min_integer: 1,
    min_fraction: 0,
    max_fraction: 6,
    grouping: false,
    sign_display: SignDisplay::Auto,
};

struct Op {
    name: &'static str,
    parse: bool,
    has_param: bool,
}

const OPS: &[Op] = &[
    Op {
        name: "tostring",
        parse: false,
        has_param: true,
    },
    Op {
        name: "tofixed",
        parse: false,
        has_param: true,
    },
    Op {
        name: "toprecision",
        parse: false,
        has_param: true,
    },
    Op {
        name: "toexponential",
        parse: false,
        has_param: true,
    },
    Op {
        name: "string",
        parse: false,
        has_param: false,
    },
    Op {
        name: "intl",
        parse: false,
        has_param: false,
    },
    Op {
        name: "intl_default",
        parse: false,
        has_param: false,
    },
    Op {
        name: "parsefloat",
        parse: true,
        has_param: false,
    },
    Op {
        name: "numberstr",
        parse: true,
        has_param: false,
    },
    Op {
        name: "parseint",
        parse: true,
        has_param: true,
    },
];

enum Record {
    Fmt { a: f64, b: f64 },
    Parse { units: Vec<u16>, radix: f64 },
}

#[derive(Debug)]
enum Output {
    Text(Result<String, String>),
    Number(u64),
}

fn fmt_rust(op: &Op, a: f64, b: f64) -> Result<String, String> {
    let arg = number::to_integer_or_infinity(b);
    let missing = b.to_bits() == SENTINEL;
    let text = match op.name {
        "tostring" if missing => Ok(number::to_string(a)),
        "tostring" => number::to_string_radix(a, arg).map_err(str::to_owned),
        "tofixed" if missing => number::to_fixed(a, 0.0).map_err(str::to_owned),
        "tofixed" => number::to_fixed(a, arg).map_err(str::to_owned),
        "toprecision" if missing => number::to_precision(a, None).map_err(str::to_owned),
        "toprecision" => number::to_precision(a, Some(arg)).map_err(str::to_owned),
        "toexponential" if missing => number::to_exponential(a, None).map_err(str::to_owned),
        "toexponential" => number::to_exponential(a, Some(arg)).map_err(str::to_owned),
        "string" => Ok(number::to_string(a)),
        "intl" => Ok(number::intl_format(a, UPSTREAM_INTL)),
        "intl_default" => Ok(number::intl_format(a, IntlOptions::default())),
        _ => unreachable!("{}", op.name),
    };
    text.map_err(|m| format!("RangeError: {m}"))
}

const FMT_SPECIALS: &[f64] = &[
    0.0,
    -0.0,
    f64::NAN,
    f64::INFINITY,
    f64::NEG_INFINITY,
    1.0,
    -1.0,
    0.5,
    -0.5,
    2.5,
    -2.5,
    0.1,
    0.15,
    0.05,
    0.125,
    1.005,
    0.30000000000000004,
    f64::MIN_POSITIVE,
    f64::MAX,
    -f64::MAX,
    5e-324,
    -5e-324,
    1e21,
    -1e21,
    9.999999999999999e20,
    1e22,
    9007199254740991.0,
    9007199254740992.0,
    9007199254740993.0,
    4294967295.0,
    4294967296.0,
    2147483647.0,
    -2147483648.0,
    1e-6,
    1e-7,
    0.9999999999999999,
    0.49999999999999994,
    123456789012345680000.0,
    f64::from_bits(0x8000_0000_0000_0001),
];

const RADIX_SPECIALS: &[f64] = &[
    f64::from_bits(SENTINEL),
    f64::NAN,
    f64::INFINITY,
    f64::NEG_INFINITY,
    -0.0,
    1e21,
    4294967312.0,
    4294967349.0,
    16.9,
    36.9,
    -16.9,
    2.0,
    10.0,
    36.0,
    37.0,
];

fn param(rng: &mut Rng) -> f64 {
    match rng.below(10) {
        0..=1 => f64::from_bits(SENTINEL),
        2..=6 => rng.below(41) as f64 - 2.0,
        7 => rng.below(105) as f64 - 3.0,
        8 => rng.below(41) as f64 - 2.0 + 0.5,
        _ => RADIX_SPECIALS[rng.below(RADIX_SPECIALS.len() as u64) as usize],
    }
}

fn random_f64(rng: &mut Rng) -> f64 {
    match rng.below(10) {
        0..=2 => f64::from_bits(rng.next_u64()),
        3..=4 => {
            let sign = rng.next_u64() & (1u64 << 63);
            let exp = rng.below(2046) << 52;
            f64::from_bits(sign | exp | (rng.next_u64() & 0xf_ffff_ffff_ffff))
        }
        5 => {
            let mant = rng.next_u64() & 0xf_ffff_ffff_ffff;
            f64::from_bits(mant | (rng.below(2046) << 52) | (rng.next_u64() & (1u64 << 63)))
        }
        6 => {
            let digits = 1 + rng.below(9);
            let n = (rng.next_u64() % 10u64.pow(digits as u32)) as f64;
            n / 10f64.powi(rng.below(8) as i32) * if rng.below(2) == 0 { 1.0 } else { -1.0 }
        }
        7 => {
            let n = rng.below(23) as i32;
            let base = 10f64.powi(n - 10);
            f64::from_bits(base.to_bits().wrapping_add(rng.below(9)))
        }
        8 => {
            let tie: f64 = [
                0.5, 1.5, 2.5, 0.25, 0.75, 0.125, 0.375, 0.0625, 1.005, 0.045, 0.15, 0.35, 2.675,
                99.995, 9.995, 0.9995, 4.45,
            ][rng.below(17) as usize];
            f64::from_bits(tie.to_bits().wrapping_add(rng.below(7)) | rng.next_u64() & (1u64 << 63))
        }
        _ => (rng.next_u64() as i64 >> rng.below(63) as u8) as f64,
    }
}

fn fmt_operand(rng: &mut Rng, i: usize) -> (f64, f64) {
    let a = if i < BOUNDARY {
        FMT_SPECIALS[i % FMT_SPECIALS.len()]
    } else {
        random_f64(rng)
    };
    (a, param(rng))
}

const PARSE_CORPUS: &[&str] = &[
    "",
    " ",
    "0",
    "-0",
    "+0",
    "00",
    "0123",
    "1.5",
    "1.",
    ".5",
    ".",
    "1e3",
    "1e-3",
    "1e+",
    "0x10",
    "-0x10",
    "0Xff",
    "0x",
    "0xg",
    "0o17",
    "0o9",
    "0b101",
    "0b2",
    "-0b1",
    "Infinity",
    "-Infinity",
    "+Infinity",
    "Infinit",
    "Infinity1.5",
    "NaN",
    "9007199254740993",
    "123abc",
    "abc",
    "1_000",
    "1e309",
    "1e-999",
    "-",
    "+",
    "z",
    "0z",
    "010",
    "9",
    "ff",
    "zzzz",
    "0x7fffffffffffffff",
    "0x8000000000000000",
    "  12  ",
    "\u{feff}3",
    "0x000000000000000000000000000000000000001",
    "8888888888888888888",
];

fn string_ops(rng: &mut Rng, units: &mut Vec<u16>) {
    units.clear();
    let len = match rng.below(10) {
        0 => rng.below(3),
        1..=6 => 1 + rng.below(24),
        7 => 20 + rng.below(120),
        8 => 200 + rng.below(400),
        _ => 300 + rng.below(200),
    };
    const DIGITS: &[u8] = b"0123456789";
    const SPECIALS: &[u16] = &[
        b'x' as u16,
        b'X' as u16,
        b'o' as u16,
        b'O' as u16,
        b'b' as u16,
        b'B' as u16,
        b'e' as u16,
        b'E' as u16,
        b'.' as u16,
        b'+' as u16,
        b'-' as u16,
        b'i' as u16,
        b'I' as u16,
        b'f' as u16,
        b'n' as u16,
        b't' as u16,
        b'y' as u16,
        b'z' as u16,
        b'Z' as u16,
        b'a' as u16,
        b'A' as u16,
        0x20,
        0x09,
        0x0a,
        0x0c,
        0xa0,
        0x1680,
        0x2028,
        0x2029,
        0x3000,
        0xfeff,
        0x7f,
        0xff,
    ];
    for _ in 0..len {
        units.push(match rng.below(10) {
            0..=4 => DIGITS[rng.below(10) as usize] as u16,
            5..=7 => SPECIALS[rng.below(SPECIALS.len() as u64) as usize],
            8 => (rng.below(0x60) + 0x20) as u16,
            _ => rng.below(0x10000) as u16,
        });
    }
    match rng.below(12) {
        0 => {
            units.insert(0, 0x20);
            units.push(0x20);
        }
        1 => units.insert(0, b'-' as u16),
        2 => {
            units.splice(0..0, [b'0' as u16, b'x' as u16]);
        }
        3 => {
            units.splice(0..0, [b'0' as u16, b'b' as u16]);
        }
        4 => {
            units.splice(0..0, [b'0' as u16, b'o' as u16]);
        }
        5 => units.insert(0, 0xfeff),
        6 => units.insert(0, b'+' as u16),
        7 => {
            let pos = rng.below(units.len() as u64 + 1) as usize;
            units.splice(pos..pos, b"Infinity".iter().map(|&c| c as u16));
        }
        _ => {}
    }
}

fn parse_operand(rng: &mut Rng, i: usize, parseint: bool) -> (Vec<u16>, f64) {
    let units = if i < BOUNDARY {
        PARSE_CORPUS[i % PARSE_CORPUS.len()]
            .encode_utf16()
            .collect()
    } else {
        let mut u = Vec::new();
        string_ops(rng, &mut u);
        u
    };
    let radix = if parseint {
        match rng.below(5) {
            0 => rng.below(40) as f64 - 2.0,
            1 => f64::from_bits(rng.next_u64()),
            2 => RADIX_SPECIALS[rng.below(RADIX_SPECIALS.len() as u64) as usize],
            _ => rng.below(40) as f64 - 2.0 + 0.5,
        }
    } else {
        f64::NAN
    };
    (units, radix)
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
        .map(|v| v.replace('_', ""))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

struct Outcome {
    samples: u64,
    mismatches: u64,
    shown: Vec<String>,
    seconds: f64,
}

fn fill(reader: &mut BufReader<impl Read>, pending: &mut Vec<u8>, buf: &mut [u8]) -> bool {
    match reader.read(buf) {
        Ok(0) => false,
        Ok(n) => {
            pending.extend_from_slice(&buf[..n]);
            true
        }
        Err(e) => panic!("oracle read failed: {e}"),
    }
}

fn sweep(op: &Op, samples: u64, seed: u64) -> Outcome {
    let started = Instant::now();
    let mut child = Command::new(electron())
        .arg(root().join("oracle/sweep/number.mjs"))
        .arg(op.name)
        .env("ELECTRON_RUN_AS_NODE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn the oracle runtime");
    let stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::with_capacity(1 << 20, child.stdout.take().expect("stdout"));
    let (tx, rx) = mpsc::sync_channel::<Vec<Record>>(4);
    let name_seed = op
        .name
        .bytes()
        .fold(seed, |h, b| h.wrapping_mul(131).wrapping_add(u64::from(b)));
    let parse = op.parse;
    let parseint = op.name == "parseint";
    let has_param = op.has_param;
    let writer = thread::spawn(move || {
        let mut stdin = BufWriter::with_capacity(1 << 20, stdin);
        let mut rng = Rng::new(name_seed);
        let mut left = samples + BOUNDARY as u64;
        while left > 0 {
            let n = left.min(CHUNK as u64) as usize;
            left -= n as u64;
            let mut chunk = Vec::with_capacity(n);
            let mut bytes = Vec::new();
            for i in 0..n {
                if parse {
                    let (units, radix) = parse_operand(&mut rng, i, parseint);
                    bytes.extend_from_slice(&(units.len() as u32).to_le_bytes());
                    for u in &units {
                        bytes.extend_from_slice(&u.to_le_bytes());
                    }
                    if parseint {
                        bytes.extend_from_slice(&radix.to_le_bytes());
                    }
                    chunk.push(Record::Parse { units, radix });
                } else {
                    let (a, b) = fmt_operand(&mut rng, i);
                    bytes.extend_from_slice(&a.to_le_bytes());
                    if has_param {
                        bytes.extend_from_slice(&b.to_le_bytes());
                    }
                    chunk.push(Record::Fmt { a, b });
                }
            }
            if tx.send(chunk).is_err() || stdin.write_all(&bytes).is_err() {
                return;
            }
        }
        let _ = stdin.flush();
    });

    let mut outcome = Outcome {
        samples: 0,
        mismatches: 0,
        shown: Vec::new(),
        seconds: 0.0,
    };
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 1 << 20];
    for chunk in rx {
        let mut wanted: Vec<Output> = Vec::with_capacity(chunk.len());
        while wanted.len() < chunk.len() {
            let mut pos = 0usize;
            while wanted.len() < chunk.len() {
                if op.parse {
                    if pos + 8 > pending.len() {
                        break;
                    }
                    wanted.push(Output::Number(u64::from_le_bytes(
                        pending[pos..pos + 8].try_into().expect("8"),
                    )));
                    pos += 8;
                } else {
                    if pos + 2 > pending.len() {
                        break;
                    }
                    let len = u16::from_le_bytes([pending[pos], pending[pos + 1]]) as usize;
                    if len == 0xffff {
                        if pos + 4 > pending.len() {
                            break;
                        }
                        let mlen =
                            u16::from_le_bytes([pending[pos + 2], pending[pos + 3]]) as usize;
                        if pos + 4 + mlen > pending.len() {
                            break;
                        }
                        wanted.push(Output::Text(Err(String::from_utf8_lossy(
                            &pending[pos + 4..pos + 4 + mlen],
                        )
                        .into_owned())));
                        pos += 4 + mlen;
                    } else {
                        if pos + 2 + len > pending.len() {
                            break;
                        }
                        wanted.push(Output::Text(Ok(String::from_utf8_lossy(
                            &pending[pos + 2..pos + 2 + len],
                        )
                        .into_owned())));
                        pos += 2 + len;
                    }
                }
            }
            pending.drain(..pos);
            if wanted.len() < chunk.len() && !fill(&mut stdout, &mut pending, &mut buf) {
                break;
            }
        }
        for (i, rec) in chunk.iter().enumerate() {
            let Some(want) = wanted.get(i) else { break };
            let (got, desc) = match rec {
                Record::Fmt { a, b } => (
                    Output::Text(fmt_rust(op, *a, *b)),
                    format!("{}({:016x}, {:016x})", op.name, a.to_bits(), b.to_bits()),
                ),
                Record::Parse { units, radix } => {
                    let bits = match op.name {
                        "parsefloat" => number::parse_float(units).to_bits(),
                        "numberstr" => number::to_number(units).to_bits(),
                        "parseint" => number::parse_int(units, *radix).to_bits(),
                        _ => unreachable!(),
                    };
                    let text: String = units
                        .iter()
                        .take(40)
                        .map(|&u| char::from_u32(u32::from(u)).unwrap_or('\u{fffd}'))
                        .collect();
                    (
                        Output::Number(bits),
                        format!("{}({text:?}, {:016x})", op.name, radix.to_bits()),
                    )
                }
            };
            let matched = match (want, &got) {
                (Output::Text(Ok(w)), Output::Text(Ok(g))) => w == g,
                (Output::Text(Err(w)), Output::Text(Err(g))) => w == g,
                (Output::Number(w), Output::Number(g)) => w == g,
                _ => false,
            };
            if !matched {
                outcome.mismatches += 1;
                if outcome.shown.len() < SHOWN {
                    outcome
                        .shown
                        .push(format!("{desc} = v8 {want:?}, rust {got:?}"));
                }
            }
        }
        outcome.samples += chunk.len() as u64;
    }
    writer.join().expect("writer thread");
    let status = child.wait().expect("oracle exit");
    assert!(status.success(), "{}: oracle exited with {status}", op.name);
    outcome.seconds = started.elapsed().as_secs_f64();
    outcome
}

#[test]
#[ignore = "deep sweep against the Electron oracle; minutes of CPU"]
fn number_deep_sweep() {
    let samples = env_number("ASRS_NUMBER_SWEEP_SAMPLES", 2_000_000);
    let seed = env_number("ASRS_NUMBER_SWEEP_SEED", 1);
    let wanted: Option<Vec<String>> = std::env::var("ASRS_NUMBER_SWEEP_OPS")
        .ok()
        .map(|v| v.split(',').map(str::to_owned).collect());
    let ops: Vec<&Op> = OPS
        .iter()
        .filter(|op| {
            wanted
                .as_ref()
                .is_none_or(|w| w.iter().any(|n| n == op.name))
        })
        .collect();
    assert!(!ops.is_empty(), "no ops selected");
    let mut total = 0u64;
    let mut failed = Vec::new();
    for op in &ops {
        let outcome = sweep(op, samples, seed);
        eprintln!(
            "{:<15} {:>11} samples {:>9} mismatches {:>7.1}s",
            op.name, outcome.samples, outcome.mismatches, outcome.seconds
        );
        for line in &outcome.shown {
            eprintln!("    {line}");
        }
        total += outcome.samples;
        if outcome.mismatches > 0 {
            failed.push(op.name);
        }
    }
    eprintln!(
        "number sweep: {} ops, {total} samples, seed {seed}, failing ops: {failed:?}",
        ops.len()
    );
    assert!(failed.is_empty(), "ops with mismatches: {failed:?}");
}
