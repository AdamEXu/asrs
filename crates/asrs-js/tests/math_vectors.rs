#![allow(clippy::disallowed_methods)]

#[path = "../../asrs-cli/src/js_math.rs"]
#[allow(dead_code)]
mod js_math;

use std::collections::HashSet;

use asrs_parity::random::Rng;
use asrs_parity::{FnCase, Request, Response, Value, fn_vectors};

fn dispatch(case: &FnCase<'_>) -> Response {
    let args = &case.request.args;
    if case.name() == "oracle:call" {
        let source = args
            .first()
            .and_then(Value::as_str)
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        return match js_math::CALLS.iter().find(|(s, _)| *s == source) {
            Some((_, f)) => f(&args[1..]),
            None => Response::Error(format!("no port of {source:?}")),
        };
    }
    match js_math::FUNCTIONS.iter().find(|(n, _)| *n == case.name()) {
        Some((_, f)) => f(args),
        None => Response::Error(format!("no port of {}", case.name())),
    }
}

fn_vectors!(math_functions, "js/math", dispatch);
fn_vectors!(math_operators, "js/math-ops", dispatch);
#[test]
fn math_remainder() {
    asrs_parity::vectors::check_platform_fn_vectors("js/math-rem", dispatch);
}

fn words(hi: u32) -> f64 {
    f64::from_bits(u64::from(hi) << 32)
}

fn ulps(x: f64, n: i64) -> Vec<f64> {
    (-n..=n)
        .map(|k| f64::from_bits((x.to_bits() as i64 + k) as u64))
        .collect()
}

fn around(points: &[f64], n: i64) -> Vec<f64> {
    let mut out = Vec::new();
    for &p in points {
        if p.is_finite() && p != 0.0 {
            out.extend(ulps(p, n));
            out.extend(ulps(-p, n));
        } else {
            out.push(p);
        }
    }
    out
}

fn specials() -> Vec<f64> {
    let mut out = vec![
        0.0,
        -0.0,
        1.0,
        -1.0,
        0.5,
        -0.5,
        2.0,
        -2.0,
        3.0,
        10.0,
        0.1,
        -0.1,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        f64::from_bits(0x7ff8_0000_0000_0123),
        f64::from_bits(0xfff8_0000_0000_0456),
        f64::MIN_POSITIVE,
        -f64::MIN_POSITIVE,
        5e-324,
        -5e-324,
        f64::from_bits(0x000f_ffff_ffff_ffff),
        f64::MAX,
        -f64::MAX,
        1e-300,
        1e300,
        2147483648.0,
        4294967296.0,
        9007199254740992.0,
        9.223372036854776e18,
        1.8446744073709552e19,
        8.98846567431158e307,
        std::f64::consts::PI,
        -std::f64::consts::PI,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::E,
        1e21,
        0.49999999999999994,
        1.5,
        2.5,
        -1.5,
        -2.5,
        4503599627370495.5,
        1e-7,
        123.456,
        -123.456,
        1e22,
    ];
    out.extend(ulps(1.0, 2));
    out
}

fn random_doubles(rng: &mut Rng, n: usize, max_exponent: i64, min_exponent: i64) -> Vec<f64> {
    (0..n)
        .map(|_| {
            let span = (max_exponent - min_exponent + 1) as u64;
            let exponent = (min_exponent + rng.below(span) as i64 + 1023).clamp(0, 2046) as u64;
            let sign = rng.below(2) << 63;
            f64::from_bits(sign | exponent << 52 | rng.next_u64() >> 12)
        })
        .collect()
}

fn half_pi_multiples() -> Vec<f64> {
    let mut out: Vec<f64> = (-40..=40)
        .map(|k| f64::from(k) * std::f64::consts::FRAC_PI_2)
        .collect();
    for k in [1u64 << 20, 1 << 30, 1 << 52, 1 << 60, 6381956970095103] {
        out.push(k as f64 * std::f64::consts::FRAC_PI_2);
    }
    out.push(f64::from_bits(0x7fe0_0000_0000_0000));
    out.push(6381956970095103.0 * f64::from_bits((1023u64 + 797) << 52));
    around(&out, 1)
}

fn unary_inputs(name: &str, rng: &mut Rng) -> Vec<f64> {
    let mut v = specials();
    let thresholds: &[u32] = match name {
        "sin" | "cos" => &[
            0x3e500000, 0x3e400000, 0x3feb6000, 0x400368fd, 0x419921FB, 0x7fe00000,
        ],
        "tan" => &[
            0x3FE921FB, 0x3e300000, 0x3FE59428, 0x4002D97C, 0x413921FB, 0x7fe00000,
        ],
        "asin" | "acos" => &[0x3FF00000, 0x3FE00000, 0x3E400000, 0x3C600000, 0x3FEF3333],
        "atan" | "asinh" => &[
            0x44100000, 0x3FDC0000, 0x3E400000, 0x3FF30000, 0x3FE60000, 0x40038000, 0x41B00000,
            0x40000000, 0x3E300000,
        ],
        "acosh" | "atanh" => &[0x3FF00000, 0x41B00000, 0x40000000, 0x3FE00000, 0x3E300000],
        "exp" | "expm1" | "sinh" | "cosh" | "tanh" => &[
            0x40862E42, 0x3FD62E42, 0x3FD62E43, 0x3FF0A2B2, 0x3E300000, 0x4043687A, 0x3C900000,
            0x3C800000, 0x40360000, 0x3FF00000,
        ],
        "log" | "log2" | "log10" | "log1p" | "cbrt" | "sqrt" => &[
            0x00100000, 0x3FDA827A, 0xBFD2BEC4, 0x3E200000, 0x3C900000, 0x43400000, 0x3FF6A09E,
            0x3FE6A09E,
        ],
        _ => &[],
    };
    let points: Vec<f64> = thresholds.iter().map(|&h| words(h)).collect();
    v.extend(around(&points, 2));
    match name {
        "sin" | "cos" | "tan" => {
            v.extend(half_pi_multiples());
            v.extend((-16..=16).map(|k| f64::from(k) * std::f64::consts::FRAC_PI_4));
            v.extend(random_doubles(rng, 150, 30, -30));
        }
        "exp" | "expm1" | "sinh" | "cosh" | "tanh" => {
            v.extend(around(
                &[
                    709.782712893384,
                    745.1332191019411,
                    709.7822265625,
                    710.4758600739439,
                    22.0,
                ],
                2,
            ));
            for k in [
                -1074, -1022, -1021, -1000, -60, -57, -56, -2, -1, 1, 2, 19, 20, 21, 56, 57, 1023,
                1024,
            ] {
                v.extend(ulps(f64::from(k) * std::f64::consts::LN_2, 1));
            }
            v.extend(random_doubles(rng, 150, 9, -30));
        }
        "log" | "log2" | "log10" | "log1p" | "cbrt" | "sqrt" | "acosh" => {
            for k in -1074..=1023 {
                if k % 7 == 0 || (-3..=3).contains(&k) {
                    v.extend(ulps(js_math_pow(2.0, f64::from(k)), 1));
                }
            }
            for k in -323..=308 {
                v.push(js_math_pow(10.0, f64::from(k)));
            }
            for k in 1..=30 {
                v.push(f64::from(k * k * k));
                v.push(f64::from(k * k));
            }
            v.extend(around(
                &[
                    -1.0,
                    std::f64::consts::SQRT_2,
                    std::f64::consts::FRAC_1_SQRT_2,
                ],
                2,
            ));
            v.extend(
                random_doubles(rng, 150, 1023, -1074)
                    .into_iter()
                    .map(f64::abs),
            );
        }
        "asin" | "acos" | "atanh" => {
            v.extend((-20..=20).map(|k| f64::from(k) / 20.0));
            v.extend(random_doubles(rng, 150, 0, -60));
        }
        "round" | "ceil" | "floor" | "trunc" | "sign" | "abs" | "fround" => {
            for n in -10..=10 {
                v.extend(ulps(f64::from(n) + 0.5, 2));
                v.extend(ulps(f64::from(n), 1));
            }
            v.extend(around(
                &[
                    4503599627370496.0,
                    4503599627370495.5,
                    9007199254740991.0,
                    3.4028234663852886e38,
                    3.4028235677973366e38,
                    1.401298464324817e-45,
                    7.006492321624085e-46,
                    1.1754943508222875e-38,
                    16777217.0,
                ],
                2,
            ));
            v.extend(random_doubles(rng, 150, 60, -30));
        }
        _ => v.extend(random_doubles(rng, 150, 60, -60)),
    }
    v
}

fn js_math_pow(x: f64, y: f64) -> f64 {
    asrs_js::math::pow(x, y)
}

struct Spec {
    lines: Vec<String>,
    seen: HashSet<String>,
}

impl Spec {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            seen: HashSet::new(),
        }
    }

    fn push(&mut self, name: &str, args: Vec<Value>) {
        let line = Request::new(name, args).encode();
        if self.seen.insert(line.clone()) {
            self.lines.push(line);
        }
    }

    fn call(&mut self, source: &str, args: Vec<Value>) {
        let mut all = vec![Value::String(source.into())];
        all.extend(args);
        self.push("oracle:call", all);
    }

    fn write(&self, id: &str) {
        let path = asrs_parity::vectors::workspace_root().join(format!("parity/specs/{id}.jsonl"));
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        let text: String = self.lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(&path, text).expect("write spec");
        eprintln!("{}: {} requests", path.display(), self.lines.len());
    }
}

fn n(x: f64) -> Value {
    Value::number(x)
}

fn bi(text: &str) -> Value {
    Value::BigInt(text.to_owned())
}

const UNARY: [&str; 29] = [
    "abs", "acos", "acosh", "asin", "asinh", "atan", "atanh", "cbrt", "ceil", "clz32", "cos",
    "cosh", "exp", "expm1", "floor", "fround", "log", "log1p", "log10", "log2", "round", "sign",
    "sin", "sinh", "sqrt", "tan", "tanh", "trunc", "hypot",
];

fn pair_values() -> Vec<f64> {
    vec![
        0.0,
        -0.0,
        1.0,
        -1.0,
        2.0,
        -2.0,
        0.5,
        -0.5,
        3.0,
        -3.0,
        10.0,
        1e-300,
        -1e-300,
        1e300,
        -1e300,
        5e-324,
        f64::MAX,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        f64::from_bits(0x7ff8_0000_0000_0123),
        f64::from_bits(0xfff8_0000_0000_0456),
        std::f64::consts::E,
        f64::from_bits(0x3ff0_0000_0000_0001),
        f64::from_bits(0x3fef_ffff_ffff_ffff),
    ]
}

fn math_spec() -> Spec {
    let mut rng = Rng::new(0x6a02);
    let mut spec = Spec::new();
    for name in UNARY {
        for x in unary_inputs(name, &mut rng) {
            spec.push(&format!("Math.{name}"), vec![n(x)]);
        }
    }
    let pairs = pair_values();
    let exponents = [
        0.0,
        -0.0,
        1.0,
        -1.0,
        2.0,
        -2.0,
        0.5,
        -0.5,
        3.0,
        -3.0,
        1.0 / 3.0,
        0.1,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        1e10,
        -1e10,
        1075.0,
        -1075.0,
        1024.0,
        9007199254740992.0,
        9007199254740991.0,
    ];
    for &a in &pairs {
        for &b in &pairs {
            spec.push("Math.atan2", vec![n(a), n(b)]);
            spec.push("Math.max", vec![n(a), n(b)]);
            spec.push("Math.min", vec![n(a), n(b)]);
            spec.push("Math.hypot", vec![n(a), n(b)]);
            spec.push("Math.imul", vec![n(a), n(b)]);
        }
        for &e in &exponents {
            spec.push("Math.pow", vec![n(a), n(e)]);
        }
    }
    for (y, x) in [
        (1e300, 1e-300),
        (1e-300, -1e300),
        (js_math_pow(2.0, 61.0), 1.0),
        (1.0, -js_math_pow(2.0, 61.0)),
        (-1.0, -js_math_pow(2.0, 61.0)),
        (1.0, 1.0),
        (-0.0, -1.0),
        (0.0, -0.0),
    ] {
        spec.push("Math.atan2", vec![n(y), n(x)]);
    }
    for _ in 0..300 {
        let v = random_doubles(&mut rng, 2, 40, -40);
        spec.push("Math.atan2", vec![n(v[0]), n(v[1])]);
        spec.push("Math.pow", vec![n(v[0].abs()), n(v[1])]);
        let w = random_doubles(&mut rng, 5, 600, -600);
        for len in 0..=5 {
            let args: Vec<Value> = w[..len].iter().map(|&x| n(x)).collect();
            spec.push("Math.hypot", args.clone());
            spec.push("Math.max", args.clone());
            spec.push("Math.min", args);
        }
    }
    for args in [
        vec![],
        vec![f64::NAN, f64::INFINITY],
        vec![f64::NAN, 1.0, f64::NEG_INFINITY],
        vec![1.0, 2.0, f64::NAN, 3.0],
        vec![1.0, 2.0, 3.0, f64::INFINITY],
        vec![-0.0, -0.0, -0.0],
        vec![-0.0, -0.0, -0.0, -0.0],
        vec![3.0, 4.0, 12.0, 84.0],
        vec![1e308, 1e308, 1e308, 1e308, 1e308],
        vec![
            f64::from_bits(0x7ff8_0000_0000_0123),
            f64::from_bits(0xfff8_0000_0000_0456),
        ],
        vec![-0.0, 0.0],
        vec![0.0, -0.0],
    ] {
        let args: Vec<Value> = args.into_iter().map(n).collect();
        spec.push("Math.hypot", args.clone());
        spec.push("Math.max", args.clone());
        spec.push("Math.min", args);
    }

    let mut integers = vec![
        0.0,
        -0.0,
        1.0,
        -1.0,
        9007199254740993.0,
        f64::MAX,
        -f64::MAX,
        1e21,
        1e300,
    ];
    for k in [31, 32, 52, 53, 54, 63, 64, 65, 100, 1023] {
        let p = js_math_pow(2.0, f64::from(k));
        integers.extend([p, -p, p - 1.0, p + 2.0]);
    }
    integers.extend(random_doubles(&mut rng, 100, 1023, 52));
    for &x in &integers {
        spec.push("BigInt", vec![n(x)]);
    }
    for x in [
        0.5,
        1.5,
        -2.5,
        1e-7,
        -1.5e-7,
        4503599627370495.5,
        0.1,
        5e-324,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        123456.789,
    ] {
        spec.push("BigInt", vec![n(x)]);
    }
    for s in [
        "",
        "  ",
        "0",
        "-0",
        "123",
        "-123",
        "+5",
        "0x1F",
        "0X1f",
        "0b101",
        "0B11",
        "0o17",
        "0O7",
        "0x",
        "1e3",
        "1.5",
        "abc",
        " 42 ",
        "\u{a0}42\u{feff}",
        "\n\t7\r",
        "-0x10",
        "1_000",
        "9007199254740993",
        "00012",
        "- 1",
        "0xg",
        "123n",
        "123456789012345678901234567890123456789012345678901234567890",
    ] {
        spec.push("BigInt", vec![Value::String(s.into())]);
    }
    let big = [
        "0",
        "1",
        "-1",
        "9223372036854775807",
        "9223372036854775808",
        "-9223372036854775808",
        "-9223372036854775809",
        "18446744073709551615",
        "18446744073709551616",
        "18446744073709551617",
        "-18446744073709551616",
        "123456789012345678901234567890",
        "-123456789012345678901234567890",
        "9007199254740993",
        "9007199254740995",
        "-9007199254740993",
        "18446744073709553664",
        "18446744073709553665",
        "18446744073709555712",
        "179769313486231570814527423731704356798070567525844996598917476803157260780028538760589558632766878171540458953514382464234321326889464182768467546703537516986049910576551282076245490090389328944075868508455133942304583236903222948165808559332123348274797826204144723168738177180919299881250404026184124858368",
    ];
    let max = asrs_js::math::BigInt::from_f64(f64::MAX).expect("integral");
    let half_ulp = asrs_js::math::BigInt::from_i64(1).shl(970);
    let tie = format!("{}", max.add(&half_ulp));
    let below_tie = format!(
        "{}",
        max.add(&half_ulp).sub(&asrs_js::math::BigInt::from_i64(1))
    );
    let big: Vec<&str> = big
        .iter()
        .copied()
        .chain([tie.as_str(), below_tie.as_str()])
        .collect();
    for s in big {
        spec.push("Number", vec![bi(s)]);
        spec.push("BigInt.asIntN", vec![n(64.0), bi(s)]);
        spec.push("BigInt.asUintN", vec![n(64.0), bi(s)]);
    }
    spec
}

/// Skips a Smi divided by `+0`: on Windows V8 answers `+NaN` from the interpreter's Smi path
/// and `-NaN` from optimized code, so the oracle's result depends on when the site tiers up.
#[allow(clippy::float_cmp)]
fn push_rem(rem: &mut Spec, a: f64, b: f64) {
    let smi = |x: f64| {
        x.trunc() == x
            && (-1_073_741_824.0..1_073_741_824.0).contains(&x)
            && !(x == 0.0 && x.is_sign_negative())
    };
    if smi(a) && b == 0.0 && b.is_sign_positive() {
        return;
    }
    rem.call("(a, b) => a % b", vec![n(a), n(b)]);
}

fn rem_boundaries(rem: &mut Spec) {
    let smi_edges = [
        1073741823.0,
        1073741824.0,
        -1073741824.0,
        -1073741825.0,
        2147483647.0,
        -2147483648.0,
        -0.0,
        0.0,
        -7.0,
        7.0,
        1.5,
    ];
    for &a in &smi_edges {
        for b in [0.0, -0.0, 1.0, -1.0, 4.0, 0.25, f64::INFINITY] {
            push_rem(rem, a, b);
        }
    }
}

/// `js/math-ops` and the platform-dependent `%` requests of `js/math-rem`.
fn ops_specs() -> (Spec, Spec) {
    let mut rng = Rng::new(0x6a03);
    let mut spec = Spec::new();
    let mut rem = Spec::new();
    spec.call(
        "() => [Math.E, Math.LN10, Math.LN2, Math.LOG10E, Math.LOG2E, Math.PI, Math.SQRT1_2, Math.SQRT2]",
        vec![],
    );
    let mut ints = specials();
    for p in [31.0, 32.0, 53.0, 63.0, 64.0, 84.0, 85.0] {
        let p = js_math_pow(2.0, p);
        ints.extend([
            p,
            -p,
            p - 1.0,
            p + 1.0,
            p - 0.5,
            p + 0.5,
            -p - 1.0,
            -p + 1.0,
        ]);
    }
    ints.extend([
        4294967295.5,
        255.5,
        254.5,
        256.0,
        -128.5,
        65535.5,
        32767.5,
        300.0,
        -300.0,
        127.5,
    ]);
    ints.extend(random_doubles(&mut rng, 100, 70, -5));
    for &x in &ints {
        for source in [
            "(a) => a | 0",
            "(a) => a >>> 0",
            "(a) => new Int8Array([a])[0]",
            "(a) => new Uint8Array([a])[0]",
            "(a) => new Uint8ClampedArray([a])[0]",
            "(a) => new Int16Array([a])[0]",
            "(a) => new Uint16Array([a])[0]",
        ] {
            spec.call(source, vec![n(x)]);
        }
        spec.push("Math.clz32", vec![n(x)]);
    }
    let counts = [
        0.0,
        1.0,
        5.0,
        31.0,
        32.0,
        33.0,
        -1.0,
        0.5,
        4294967297.0,
        f64::NAN,
        -0.0,
    ];
    let values = [
        1.0,
        -1.0,
        2147483647.0,
        -2147483648.0,
        4294967295.0,
        123456789.75,
        -987654321.25,
        f64::NAN,
        1e20,
    ];
    for &x in &values {
        for &c in &counts {
            for source in ["(a, b) => a << b", "(a, b) => a >> b", "(a, b) => a >>> b"] {
                spec.call(source, vec![n(x), n(c)]);
            }
        }
        for &y in &values {
            for source in ["(a, b) => a & b", "(a, b) => a | b", "(a, b) => a ^ b"] {
                spec.call(source, vec![n(x), n(y)]);
            }
        }
    }
    let pairs = pair_values();
    for &a in &pairs {
        for &b in &pairs {
            push_rem(&mut rem, a, b);
            spec.call("(a, b) => a ** b", vec![n(a), n(b)]);
        }
    }
    for _ in 0..300 {
        let v = random_doubles(&mut rng, 2, 60, -60);
        push_rem(&mut rem, v[0], v[1]);
        spec.call("(a, b) => a ** b", vec![n(v[0].abs()), n(v[1] / 1e15)]);
    }
    let bigs = [
        "0",
        "1",
        "-1",
        "5",
        "-5",
        "255",
        "-256",
        "4294967295",
        "-4294967296",
        "9223372036854775807",
        "-9223372036854775808",
        "18446744073709551616",
        "-123456789012345678901234567890",
        "340282366920938463463374607431768211455",
    ];
    let shifts = ["0", "1", "3", "32", "63", "64", "100", "-1", "-3", "-64"];
    for a in bigs {
        for c in shifts {
            spec.call("(a, b) => a << b", vec![bi(a), bi(c)]);
            spec.call("(a, b) => a >> b", vec![bi(a), bi(c)]);
        }
        for b in bigs {
            for source in [
                "(a, b) => a & b",
                "(a, b) => a | b",
                "(a, b) => a ^ b",
                "(a, b) => a + b",
                "(a, b) => a - b",
                "(a, b) => a < b",
            ] {
                spec.call(source, vec![bi(a), bi(b)]);
            }
        }
    }
    rem_boundaries(&mut rem);
    (spec, rem)
}

#[test]
#[ignore = "rewrites parity/specs/js/math*; regenerate vectors after with `cargo xtask oracle gen js/math js/math-ops js/math-rem` and `cargo xtask host windows-arm gen js/math-rem/windows-aarch64` (likewise linux)"]
fn write_math_specs() {
    math_spec().write("js/math");
    let (ops, rem) = ops_specs();
    ops.write("js/math-ops");
    rem.write("js/math-rem.requests");
}
