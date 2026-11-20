//! Deep sweep of `asrs_js::math` against V8 itself: every op streams seeded operands through
//! `oracle/sweep/math.mjs` under the oracle's Electron runtime and through the Rust port, and
//! compares the result bits. Needs `cargo xtask oracle setup`. Run with
//!
//! ```sh
//! cargo test --release -p asrs-js --test math_sweep -- --ignored --nocapture
//! ```
//!
//! `ASRS_MATH_SWEEP_SAMPLES` (default 50000000 per op), `ASRS_MATH_SWEEP_OPS` (comma
//! separated, default all), `ASRS_MATH_SWEEP_SEED` (default 1), `ASRS_MATH_SWEEP_JOBS` and
//! `ASRS_ELECTRON` tune it.

#![allow(clippy::approx_constant, clippy::disallowed_methods)]

use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use asrs_js::math::{self, BigInt};
use asrs_parity::random::Rng;

const CHUNK: usize = 1 << 16;
const SENTINEL: u64 = 0x7ff8_dead_beef_0000;
const SHOWN: usize = 8;

#[derive(Clone, Copy)]
enum Domain {
    General,
    Trig,
    Unit,
    Positive,
    Exp,
    Atan,
    Round,
    Int,
    PowBase,
    PowExp,
    Integral,
    IntBits,
}

type RustFn = fn(&[f64]) -> u64;

struct Op {
    name: &'static str,
    domains: &'static [Domain],
    rust: RustFn,
}

fn f(x: f64) -> u64 {
    x.to_bits()
}

fn bigint_or_nan(x: f64, g: impl Fn(&BigInt) -> BigInt) -> u64 {
    match BigInt::from_f64(x) {
        Ok(r) => g(&r).to_f64().to_bits(),
        Err(_) => math::NAN.to_bits(),
    }
}

use Domain::*;

const OPS: &[Op] = &[
    Op {
        name: "abs",
        domains: &[Round],
        rust: |a| f(math::abs(a[0])),
    },
    Op {
        name: "acos",
        domains: &[Unit],
        rust: |a| f(math::acos(a[0])),
    },
    Op {
        name: "acosh",
        domains: &[Positive],
        rust: |a| f(math::acosh(a[0])),
    },
    Op {
        name: "asin",
        domains: &[Unit],
        rust: |a| f(math::asin(a[0])),
    },
    Op {
        name: "asinh",
        domains: &[Atan],
        rust: |a| f(math::asinh(a[0])),
    },
    Op {
        name: "atan",
        domains: &[Atan],
        rust: |a| f(math::atan(a[0])),
    },
    Op {
        name: "atanh",
        domains: &[Unit],
        rust: |a| f(math::atanh(a[0])),
    },
    Op {
        name: "cbrt",
        domains: &[Positive],
        rust: |a| f(math::cbrt(a[0])),
    },
    Op {
        name: "ceil",
        domains: &[Round],
        rust: |a| f(math::ceil(a[0])),
    },
    Op {
        name: "clz32",
        domains: &[Int],
        rust: |a| f(f64::from(math::clz32(a[0]))),
    },
    Op {
        name: "cos",
        domains: &[Trig],
        rust: |a| f(math::cos(a[0])),
    },
    Op {
        name: "cosh",
        domains: &[Exp],
        rust: |a| f(math::cosh(a[0])),
    },
    Op {
        name: "exp",
        domains: &[Exp],
        rust: |a| f(math::exp(a[0])),
    },
    Op {
        name: "expm1",
        domains: &[Exp],
        rust: |a| f(math::expm1(a[0])),
    },
    Op {
        name: "floor",
        domains: &[Round],
        rust: |a| f(math::floor(a[0])),
    },
    Op {
        name: "fround",
        domains: &[Round],
        rust: |a| f(math::fround(a[0])),
    },
    Op {
        name: "log",
        domains: &[Positive],
        rust: |a| f(math::log(a[0])),
    },
    Op {
        name: "log1p",
        domains: &[Positive],
        rust: |a| f(math::log1p(a[0])),
    },
    Op {
        name: "log10",
        domains: &[Positive],
        rust: |a| f(math::log10(a[0])),
    },
    Op {
        name: "log2",
        domains: &[Positive],
        rust: |a| f(math::log2(a[0])),
    },
    Op {
        name: "round",
        domains: &[Round],
        rust: |a| f(math::round(a[0])),
    },
    Op {
        name: "sign",
        domains: &[Round],
        rust: |a| f(math::sign(a[0])),
    },
    Op {
        name: "sin",
        domains: &[Trig],
        rust: |a| f(math::sin(a[0])),
    },
    Op {
        name: "sinh",
        domains: &[Exp],
        rust: |a| f(math::sinh(a[0])),
    },
    Op {
        name: "sqrt",
        domains: &[Positive],
        rust: |a| f(math::sqrt(a[0])),
    },
    Op {
        name: "tan",
        domains: &[Trig],
        rust: |a| f(math::tan(a[0])),
    },
    Op {
        name: "tanh",
        domains: &[Exp],
        rust: |a| f(math::tanh(a[0])),
    },
    Op {
        name: "trunc",
        domains: &[Round],
        rust: |a| f(math::trunc(a[0])),
    },
    Op {
        name: "hypot1",
        domains: &[General],
        rust: |a| f(math::hypot(a)),
    },
    Op {
        name: "to_int32",
        domains: &[Int],
        rust: |a| f(f64::from(math::to_int32(a[0]))),
    },
    Op {
        name: "to_uint32",
        domains: &[Int],
        rust: |a| f(f64::from(math::to_uint32(a[0]))),
    },
    Op {
        name: "to_int8",
        domains: &[Int],
        rust: |a| f(f64::from(math::to_int8(a[0]))),
    },
    Op {
        name: "to_uint8",
        domains: &[Int],
        rust: |a| f(f64::from(math::to_uint8(a[0]))),
    },
    Op {
        name: "to_uint8_clamp",
        domains: &[Round],
        rust: |a| f(f64::from(math::to_uint8_clamp(a[0]))),
    },
    Op {
        name: "to_int16",
        domains: &[Int],
        rust: |a| f(f64::from(math::to_int16(a[0]))),
    },
    Op {
        name: "to_uint16",
        domains: &[Int],
        rust: |a| f(f64::from(math::to_uint16(a[0]))),
    },
    Op {
        name: "atan2",
        domains: &[Trig, Trig],
        rust: |a| f(math::atan2(a[0], a[1])),
    },
    Op {
        name: "pow",
        domains: &[PowBase, PowExp],
        rust: |a| f(math::pow(a[0], a[1])),
    },
    Op {
        name: "exponent_op",
        domains: &[PowBase, PowExp],
        rust: |a| f(math::pow(a[0], a[1])),
    },
    Op {
        name: "rem",
        domains: &[General, Round],
        rust: |a| f(math::rem(a[0], a[1])),
    },
    Op {
        name: "max2",
        domains: &[Round, Round],
        rust: |a| f(math::max(a)),
    },
    Op {
        name: "min2",
        domains: &[Round, Round],
        rust: |a| f(math::min(a)),
    },
    Op {
        name: "hypot2",
        domains: &[General, General],
        rust: |a| f(math::hypot(a)),
    },
    Op {
        name: "imul",
        domains: &[Int, Int],
        rust: |a| f(f64::from(math::imul(a[0], a[1]))),
    },
    Op {
        name: "shift_left",
        domains: &[Int, Int],
        rust: |a| f(f64::from(math::shift_left(a[0], a[1]))),
    },
    Op {
        name: "shift_right",
        domains: &[Int, Int],
        rust: |a| f(f64::from(math::shift_right(a[0], a[1]))),
    },
    Op {
        name: "shift_right_unsigned",
        domains: &[Int, Int],
        rust: |a| f(f64::from(math::shift_right_unsigned(a[0], a[1]))),
    },
    Op {
        name: "max3",
        domains: &[Round, Round, Round],
        rust: |a| f(math::max(a)),
    },
    Op {
        name: "min3",
        domains: &[Round, Round, Round],
        rust: |a| f(math::min(a)),
    },
    Op {
        name: "hypot3",
        domains: &[General, General, General],
        rust: |a| f(math::hypot(a)),
    },
    Op {
        name: "hypot4",
        domains: &[General, General, General, General],
        rust: |a| f(math::hypot(a)),
    },
    Op {
        name: "hypot5",
        domains: &[General, General, General, General, General],
        rust: |a| f(math::hypot(a)),
    },
    Op {
        name: "bigint_from_number",
        domains: &[Integral],
        rust: |a| match BigInt::from_f64(a[0]) {
            Ok(r) => r.as_int64() as u64,
            Err(_) => SENTINEL,
        },
    },
    Op {
        name: "bigint_to_number_i64",
        domains: &[IntBits],
        rust: |a| BigInt::from_i64(a[0].to_bits() as i64).to_f64().to_bits(),
    },
    Op {
        name: "bigint_to_number_u64",
        domains: &[IntBits],
        rust: |a| BigInt::from_u64(a[0].to_bits()).to_f64().to_bits(),
    },
    Op {
        name: "bigint_arith",
        domains: &[Integral],
        rust: |a| {
            bigint_or_nan(a[0], |r| {
                r.shl(75)
                    .add(&r.shr(3))
                    .sub(&r.shl(20))
                    .add(&BigInt::from_i64(5))
            })
        },
    },
    Op {
        name: "bigint_bitwise",
        domains: &[Integral],
        rust: |a| {
            bigint_or_nan(a[0], |r| {
                r.bitand(&BigInt::from_u64(0xfffff0000fffff))
                    .bitor(&r.shr(40))
                    .bitxor(&r.shl(2))
            })
        },
    },
];

fn random_mantissa(rng: &mut Rng) -> u64 {
    match rng.below(4) {
        0 => 0,
        1 => rng.next_u64() >> (12 + rng.below(52)),
        _ => rng.next_u64() >> 12,
    }
}

fn with_exponent(exponent: i64, rng: &mut Rng) -> f64 {
    let sign = rng.below(2) << 63;
    let biased = (exponent + 1023).clamp(0, 2046) as u64;
    f64::from_bits(sign | (biased << 52) | random_mantissa(rng))
}

fn nudge(x: f64, rng: &mut Rng) -> f64 {
    let steps = match rng.below(3) {
        0 => 0,
        1 => rng.below(5) as i64 - 2,
        _ => rng.below(129) as i64 - 64,
    };
    if !x.is_finite() || x == 0.0 {
        return if steps == 0 {
            x
        } else {
            f64::from_bits(steps.unsigned_abs())
        };
    }
    let bits = x.to_bits() as i64 + steps;
    f64::from_bits(bits as u64)
}

fn uniform(rng: &mut Rng, lo: f64, hi: f64) -> f64 {
    let u = (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
    lo + (hi - lo) * u
}

fn decimal(rng: &mut Rng) -> f64 {
    let digits = 1 + rng.below(9) as u32;
    let int = rng.below(10u64.pow(digits)) as f64;
    let scale = math::pow(10.0, rng.below(7) as f64);
    let v = int / scale;
    if rng.below(2) == 0 { v } else { -v }
}

const SPECIALS: [f64; 12] = [
    0.0,
    -0.0,
    1.0,
    -1.0,
    0.5,
    -0.5,
    2.0,
    f64::INFINITY,
    f64::NEG_INFINITY,
    f64::NAN,
    f64::MIN_POSITIVE,
    f64::MAX,
];

fn pick(items: &[f64], rng: &mut Rng) -> f64 {
    items[rng.below(items.len() as u64) as usize]
}

fn focus(rng: &mut Rng, domain: Domain) -> f64 {
    let half_pi = std::f64::consts::FRAC_PI_2;
    let ln2 = std::f64::consts::LN_2;
    match domain {
        General => match rng.below(3) {
            0 => uniform(rng, -1e3, 1e3),
            1 => with_exponent(rng.below(120) as i64 - 60, rng),
            _ => with_exponent(rng.below(2046) as i64 - 1022, rng),
        },
        Trig => match rng.below(6) {
            0 => uniform(rng, -10.0, 10.0),
            1 => with_exponent(rng.below(60) as i64 - 30, rng),
            2 => nudge(
                (rng.below(1 << 21) as f64 - (1 << 20) as f64) * half_pi,
                rng,
            ),
            3 => nudge(rng.below(1 << 62) as f64 * half_pi, rng),
            4 => nudge(
                pick(
                    &[
                        0.126,
                        0.855469,
                        2.426265,
                        105414350.0,
                        1.4901161193847656e-8,
                        0.7853981633974483,
                        2.356194490192345,
                        3.9269908169872414,
                        823549.6,
                    ],
                    rng,
                ),
                rng,
            ),
            _ => uniform(rng, -1e9, 1e9),
        },
        Unit => match rng.below(4) {
            0 => uniform(rng, -1.0, 1.0),
            1 => with_exponent(-(rng.below(60) as i64), rng),
            2 => nudge(
                pick(&[1.0, -1.0, 0.5, -0.5, 0.975, -0.975, 0.4375, 1e-8], rng),
                rng,
            ),
            _ => uniform(rng, -1.1, 1.1),
        },
        Positive => match rng.below(4) {
            0 => uniform(rng, 0.0, 100.0),
            1 => with_exponent(rng.below(2046) as i64 - 1022, rng).abs(),
            2 => nudge(
                pick(
                    &[
                        1.0,
                        2.0,
                        10.0,
                        0.5,
                        std::f64::consts::SQRT_2,
                        1e-3,
                        100.0,
                        1000.0,
                        1e15,
                        4.0,
                        0.7071067811865476,
                    ],
                    rng,
                ),
                rng,
            ),
            _ => nudge(math::pow(10.0, rng.below(600) as f64 - 300.0), rng),
        },
        Exp => match rng.below(4) {
            0 => uniform(rng, -750.0, 750.0),
            1 => uniform(rng, -25.0, 25.0),
            2 => with_exponent(rng.below(70) as i64 - 60, rng),
            _ => nudge(
                pick(
                    &[
                        709.782712893384,
                        -745.1332191019411,
                        0.5 * ln2,
                        1.5 * ln2,
                        22.0,
                        56.0 * ln2,
                        710.4758600739439,
                        1.0,
                        -1.0,
                        (rng.below(2000) as f64 - 1000.0) * ln2,
                    ],
                    rng,
                ),
                rng,
            ),
        },
        Atan => match rng.below(4) {
            0 => uniform(rng, -10.0, 10.0),
            1 => with_exponent(rng.below(140) as i64 - 70, rng),
            2 => nudge(
                pick(
                    &[
                        0.4375,
                        0.6875,
                        1.1875,
                        2.4375,
                        7.378697629483821e19,
                        2.0,
                        268435456.0,
                    ],
                    rng,
                ),
                rng,
            ),
            _ => uniform(rng, -1e6, 1e6),
        },
        Round => match rng.below(5) {
            0 => nudge((rng.below(1 << 20) as f64 - (1 << 19) as f64) + 0.5, rng),
            1 => nudge(rng.below(1 << 20) as f64 - (1 << 19) as f64, rng),
            2 => uniform(rng, -1e6, 1e6),
            3 => nudge(
                pick(
                    &[
                        4503599627370496.0,
                        9007199254740992.0,
                        0.49999999999999994,
                        255.5,
                        254.5,
                        3.4028234663852886e38,
                        1.401298464324817e-45,
                    ],
                    rng,
                ),
                rng,
            ),
            _ => with_exponent(rng.below(140) as i64 - 70, rng),
        },
        Int => match rng.below(5) {
            0 => (rng.next_u64() as i64 >> rng.below(64)) as f64,
            1 => nudge(
                pick(
                    &[
                        2147483648.0,
                        -2147483648.0,
                        4294967296.0,
                        4294967295.0,
                        9007199254740992.0,
                        9.223372036854776e18,
                        1.8446744073709552e19,
                        1.9342813113834067e25,
                        31.0,
                        32.0,
                        33.0,
                        255.5,
                    ],
                    rng,
                ),
                rng,
            ),
            2 => uniform(rng, -1e10, 1e10),
            3 => rng.below(70) as f64 - 5.0,
            _ => with_exponent(rng.below(110) as i64 - 10, rng),
        },
        PowBase => match rng.below(6) {
            0 => nudge(1.0, rng),
            1 => nudge(-1.0, rng),
            2 => uniform(rng, -10.0, 10.0),
            3 => rng.below(21) as f64 - 10.0,
            4 => decimal(rng),
            _ => with_exponent(rng.below(2046) as i64 - 1022, rng),
        },
        PowExp => match rng.below(6) {
            0 => rng.below(41) as f64 - 20.0,
            1 => (rng.below(41) as f64 - 20.0) + 0.5,
            2 => uniform(rng, -10.0, 10.0),
            3 => pick(
                &[
                    0.5,
                    2.0,
                    -0.5,
                    -2.0,
                    1.0 / 3.0,
                    1e10,
                    -1e10,
                    1075.0,
                    -1075.0,
                    3.0,
                ],
                rng,
            ),
            4 => decimal(rng),
            _ => with_exponent(rng.below(140) as i64 - 70, rng),
        },
        Integral => match rng.below(4) {
            0 => with_exponent(52 + rng.below(972) as i64, rng),
            1 => (rng.next_u64() as i64 >> rng.below(64)) as f64,
            2 => nudge(
                pick(
                    &[
                        9.223372036854776e18,
                        1.8446744073709552e19,
                        4294967296.0,
                        1.7976931348623157e308,
                    ],
                    rng,
                ),
                rng,
            ),
            _ => with_exponent(rng.below(140) as i64 - 70, rng),
        },
        IntBits => {
            let bits = rng.next_u64() >> rng.below(64);
            let bits = match rng.below(3) {
                0 => bits,
                1 => (bits | 1 << 63) & !((1u64 << rng.below(12)) - 1),
                _ => bits.wrapping_neg(),
            };
            f64::from_bits(bits)
        }
    }
}

/// Signaling NaNs are left out: V8 quiets them inconsistently between its builtins and
/// optimized code (see the CVE-1 contract in oracle/README.md).
fn sample(rng: &mut Rng, domain: Domain) -> f64 {
    if let IntBits = domain {
        return focus(rng, domain);
    }
    let x = sample_any(rng, domain);
    if x.is_nan() {
        f64::from_bits(x.to_bits() | 1 << 51)
    } else {
        x
    }
}

fn sample_any(rng: &mut Rng, domain: Domain) -> f64 {
    match rng.below(20) {
        0..=2 => f64::from_bits(rng.next_u64()),
        3..=5 => with_exponent(rng.below(2047) as i64 - 1023, rng),
        6..=7 => decimal(rng),
        8 => pick(&SPECIALS, rng),
        _ => focus(rng, domain),
    }
}

/// Correlated operand tuples: equal magnitudes, swapped signs, shared NaN payloads.
fn operands(rng: &mut Rng, domains: &[Domain], out: &mut Vec<f64>) {
    let start = out.len();
    for &d in domains {
        out.push(sample(rng, d));
    }
    if domains.len() > 1 && rng.below(8) == 0 {
        let first = out[start];
        let i = start + 1 + rng.below(domains.len() as u64 - 1) as usize;
        out[i] = match rng.below(4) {
            0 => first,
            1 => -first,
            2 => nudge(first, rng),
            _ => first * pick(&[0.5, 2.0, 1e-20, 1e20, 3.0], rng),
        };
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

fn sweep(op: &Op, samples: u64, seed: u64) -> Outcome {
    let started = Instant::now();
    let mut child = Command::new(electron())
        .arg(root().join("oracle/sweep/math.mjs"))
        .arg(op.name)
        .env("ELECTRON_RUN_AS_NODE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn the oracle runtime");
    let stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::with_capacity(1 << 20, child.stdout.take().expect("stdout"));
    let (tx, rx) = mpsc::sync_channel::<Vec<f64>>(8);
    let arity = op.domains.len();
    let domains = op.domains;
    let name_seed = op
        .name
        .bytes()
        .fold(seed, |h, b| h.wrapping_mul(131).wrapping_add(u64::from(b)));
    let writer = thread::spawn(move || {
        let mut stdin = BufWriter::with_capacity(1 << 20, stdin);
        let mut rng = Rng::new(name_seed);
        let mut left = samples;
        while left > 0 {
            let n = left.min(CHUNK as u64) as usize;
            left -= n as u64;
            let mut chunk = Vec::with_capacity(n * arity);
            for _ in 0..n {
                operands(&mut rng, domains, &mut chunk);
            }
            let bytes: Vec<u8> = chunk.iter().flat_map(|v| v.to_le_bytes()).collect();
            if tx.send(chunk).is_err() {
                return;
            }
            if stdin.write_all(&bytes).is_err() {
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
    let mut buf = Vec::new();
    for chunk in rx {
        let n = chunk.len() / arity;
        buf.resize(n * 8, 0);
        stdout
            .read_exact(&mut buf)
            .unwrap_or_else(|e| panic!("{}: oracle output ended early: {e}", op.name));
        for (i, args) in chunk.chunks_exact(arity).enumerate() {
            let want = u64::from_le_bytes(buf[i * 8..i * 8 + 8].try_into().expect("8 bytes"));
            let got = (op.rust)(args);
            if want != got {
                outcome.mismatches += 1;
                if outcome.shown.len() < SHOWN {
                    let inputs: Vec<String> = args
                        .iter()
                        .map(|a| format!("{:016x} ({a:e})", a.to_bits()))
                        .collect();
                    outcome.shown.push(format!(
                        "{}({}) = v8 {want:016x} ({:e}), rust {got:016x} ({:e})",
                        op.name,
                        inputs.join(", "),
                        f64::from_bits(want),
                        f64::from_bits(got)
                    ));
                }
            }
        }
        outcome.samples += n as u64;
    }
    writer.join().expect("writer thread");
    let status = child.wait().expect("oracle exit");
    assert!(status.success(), "{}: oracle exited with {status}", op.name);
    outcome.seconds = started.elapsed().as_secs_f64();
    outcome
}

#[test]
#[ignore = "deep sweep against the Electron oracle; minutes of CPU"]
fn math_deep_sweep() {
    let samples = env_number("ASRS_MATH_SWEEP_SAMPLES", 50_000_000);
    let seed = env_number("ASRS_MATH_SWEEP_SEED", 1);
    let wanted: Option<Vec<String>> = std::env::var("ASRS_MATH_SWEEP_OPS")
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
    let cores = thread::available_parallelism().map_or(4, |n| n.get());
    let jobs = env_number("ASRS_MATH_SWEEP_JOBS", (cores / 3).max(1) as u64) as usize;
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::new());
    thread::scope(|s| {
        for _ in 0..jobs {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(op) = ops.get(i) else { break };
                    let outcome = sweep(op, samples, seed);
                    eprintln!(
                        "{:<22} {:>11} samples {:>9} mismatches {:>7.1}s",
                        op.name, outcome.samples, outcome.mismatches, outcome.seconds
                    );
                    for line in &outcome.shown {
                        eprintln!("    {line}");
                    }
                    results.lock().expect("results").push((op.name, outcome));
                }
            });
        }
    });
    let results = results.into_inner().expect("results");
    let total: u64 = results.iter().map(|(_, o)| o.samples).sum();
    let failed: Vec<&str> = results
        .iter()
        .filter(|(_, o)| o.mismatches > 0)
        .map(|(n, _)| *n)
        .collect();
    eprintln!(
        "math sweep: {} ops, {total} samples, seed {seed}, failing ops: {failed:?}",
        results.len()
    );
    assert!(failed.is_empty(), "ops with mismatches: {failed:?}");
}
