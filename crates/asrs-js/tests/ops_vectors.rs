//! `js/ops` and `js/ops-dates` check the `asrs_js::ops` operator layer against `oracle:call`
//! vectors. `js/ops` covers every request whose answer is zone-independent; Date coercions to
//! text live in `js/ops-dates`, which runs once per zone like `js/time-zones`.

#![allow(clippy::disallowed_methods)]

#[path = "../../asrs-cli/src/functions.rs"]
#[allow(dead_code)]
mod functions;
#[path = "../../asrs-cli/src/js_color.rs"]
#[allow(dead_code)]
mod js_color;
#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;
#[path = "../../asrs-cli/src/js_math.rs"]
#[allow(dead_code)]
mod js_math;
#[path = "../../asrs-cli/src/js_ops.rs"]
#[allow(dead_code)]
mod js_ops;
#[path = "../../asrs-cli/src/js_regex.rs"]
#[allow(dead_code)]
mod js_regex;
#[path = "../../asrs-cli/src/js_string.rs"]
#[allow(dead_code)]
mod js_string;
mod js_msgpack {
    pub const FUNCTIONS: &[(&str, crate::functions::Function)] = &[];
}
mod photon {
    pub const FUNCTIONS: &[(&str, crate::functions::Function)] = &[];
}
mod log_field {
    pub const FUNCTIONS: &[(&str, crate::functions::Function)] = &[];
}
mod log_keys {
    pub const FUNCTIONS: &[(&str, crate::functions::Function)] = &[];
}
mod log_run {
    pub const FUNCTIONS: &[(&str, crate::functions::Function)] = &[];
}
mod tabs_console {
    pub const FUNCTIONS: &[(&str, crate::functions::Function)] = &[];
    pub const CALLS: &[(&str, crate::functions::Function)] = &[];
}
mod log_struct {
    pub const FUNCTIONS: &[(&str, crate::functions::Function)] = &[];
}

#[path = "../../asrs-cli/src/js_time.rs"]
mod js_time;

use std::collections::HashSet;

use asrs_js::time::NamedZone;
use asrs_parity::vectors::check_fn_vectors;
use asrs_parity::{FnCase, Response, Value, fn_vectors};

fn dispatch(case: &FnCase<'_>) -> Response {
    match functions::lookup(case.name()) {
        Some(f) => f(&case.request.args),
        None => Response::Error(format!("no port of {}", case.name())),
    }
}

fn check_zoned(zone: &str, id: &str) {
    let named = NamedZone::get(zone).expect("zone resolves");
    js_time::with_zone(named, || check_fn_vectors(id, dispatch));
}

fn_vectors!(ops, "js/ops", dispatch);

#[test]
fn ops_dates_utc() {
    check_zoned("UTC", "js/ops-dates/UTC");
}

#[test]
fn ops_dates_los_angeles() {
    check_zoned("America/Los_Angeles", "js/ops-dates/America/Los_Angeles");
}

#[test]
fn ops_dates_kolkata() {
    check_zoned("Asia/Kolkata", "js/ops-dates/Asia/Kolkata");
}

#[test]
fn ops_dates_auckland() {
    check_zoned("Pacific/Auckland", "js/ops-dates/Pacific/Auckland");
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

    fn call(&mut self, source: &str, args: Vec<Value>) {
        let all = std::iter::once(Value::String(source.into()).encode())
            .chain(args.iter().map(arg_text))
            .collect::<Vec<_>>()
            .join(",");
        self.line(&format!(r#"{{"fn":"oracle:call","args":[{all}]}}"#));
    }

    fn line(&mut self, line: &str) {
        if self.seen.insert(line.to_owned()) {
            self.lines.push(line.to_owned());
        }
    }

    fn write(&self, id: &str) {
        let path = asrs_parity::vectors::workspace_root().join(format!("parity/specs/{id}.jsonl"));
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        let text: String = self.lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(&path, text).expect("write spec");
        eprintln!("{}: {} requests", path.display(), self.lines.len());
    }
}

/// `box` args for the BigInt/Symbol wrappers: responses encode them as `c` instances, but
/// the oracle accepts only the `box` tag inbound.
fn arg_text(v: &Value) -> String {
    match v {
        Value::Boxed(inner) if matches!(inner.as_ref(), Value::BigInt(_) | Value::Symbol(_)) => {
            format!("{{\"box\":{}}}", inner.encode())
        }
        _ => v.encode(),
    }
}

fn n(x: f64) -> Value {
    Value::number(x)
}

fn s(text: &str) -> Value {
    Value::string(text)
}

fn bi(text: &str) -> Value {
    Value::BigInt(text.to_owned())
}

fn sym(desc: &str) -> Value {
    Value::Symbol(Some(desc.into()))
}

fn obj(pairs: &[(&str, Value)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(k, v)| ((*k).into(), v.clone()))
            .collect(),
    )
}

fn ta(name: &str, items: Vec<Value>) -> Value {
    Value::TypedArray(name.to_owned(), items)
}

fn date(bits: u64) -> Value {
    Value::Date(bits)
}

/// Every value kind the ops layer sees. Dates are absent: `+`, `==` and `String` on a Date are
/// zone-dependent and live in `dates_spec` instead.
fn vals() -> Vec<Value> {
    vec![
        Value::Undefined,
        Value::Null,
        Value::Bool(true),
        Value::Bool(false),
        n(0.0),
        n(-0.0),
        n(1.0),
        n(-1.5),
        n(0.5),
        n(1e21),
        n(f64::NAN),
        n(f64::INFINITY),
        n(-f64::INFINITY),
        n(9e15),
        n(2.5e-324),
        s(""),
        s("0"),
        s("1"),
        s("-1.5"),
        s(" 12 "),
        s("0x10"),
        s("1e3"),
        s("a"),
        s("abc"),
        s("Infinity"),
        s("\u{1f600}"),
        bi("0"),
        bi("5"),
        bi("-5"),
        bi("123456789012345678901234567890"),
        sym("s"),
        Value::Symbol(None),
        Value::Array(vec![]),
        Value::Array(vec![n(1.0), n(2.0)]),
        Value::Array(vec![n(1.0), Value::Hole, n(3.0)]),
        Value::Array(vec![s("a"), s("b")]),
        obj(&[]),
        obj(&[("x", n(1.0)), ("length", n(9.0))]),
        Value::Bytes(vec![1, 2, 255]),
        ta("Int8Array", vec![n(1.0), n(-2.0)]),
        ta("Float64Array", vec![n(1.5), n(2.0)]),
        ta("BigInt64Array", vec![bi("5"), bi("-7")]),
        Value::ArrayBuffer(vec![1, 2, 3, 4]),
        Value::DataView(vec![0, 0, 0, 0, 0, 0, 240, 63]),
        Value::Map(vec![(s("k"), n(1.0)), (n(2.0), s("v"))]),
        Value::Set(vec![n(1.0), s("x")]),
        Value::Error("Error".into(), "msg".into()),
        Value::Error("TypeError".into(), "bad".into()),
        Value::Boxed(Box::new(n(42.0))),
        Value::Boxed(Box::new(s("hi"))),
        Value::Boxed(Box::new(Value::Bool(true))),
        Value::Boxed(Box::new(bi("7"))),
        Value::Boxed(Box::new(sym("b"))),
    ]
}

/// The binary-op matrix drops Symbols and the object kinds that only matter to `+`/`==`
/// through coercion; those are covered one-sidedly below.
fn bin_vals() -> Vec<Value> {
    vec![
        Value::Undefined,
        Value::Null,
        Value::Bool(true),
        Value::Bool(false),
        n(0.0),
        n(-0.0),
        n(1.0),
        n(-1.5),
        n(1e21),
        n(f64::NAN),
        n(f64::INFINITY),
        n(9e15),
        s(""),
        s("0"),
        s("1"),
        s("-1.5"),
        s(" 12 "),
        s("0x10"),
        s("a"),
        s("Infinity"),
        bi("0"),
        bi("5"),
        bi("-5"),
        bi("123456789012345678901234567890"),
        Value::Array(vec![]),
        Value::Array(vec![n(1.0), n(2.0)]),
        Value::Array(vec![s("a"), s("b")]),
        obj(&[]),
        obj(&[("x", n(1.0))]),
        Value::Boxed(Box::new(n(42.0))),
        Value::Boxed(Box::new(s("hi"))),
        Value::Bytes(vec![1, 2]),
    ]
}

const BINOPS: [&str; 10] = [
    "(a, b) => a + b",
    "(a, b) => a - b",
    "(a, b) => a < b",
    "(a, b) => a <= b",
    "(a, b) => a > b",
    "(a, b) => a >= b",
    "(a, b) => a == b",
    "(a, b) => a != b",
    "(a, b) => a === b",
    "(a, b) => a !== b",
];

const UNOPS: [&str; 9] = [
    "(a) => -a",
    "(a) => +a",
    "(v) => typeof v",
    "(v) => Boolean(v)",
    "(v) => Number(v)",
    "(v) => Object(v)",
    "(v) => [...v]",
    "(v) => ({ ...v })",
    "(v) => Object.getOwnPropertyNames(v)",
];

/// Receivers for member reads, `in`, hasOwn, writes and deletes.
fn member_vals() -> Vec<Value> {
    vec![
        Value::Undefined,
        Value::Null,
        Value::Bool(true),
        n(1.5),
        s("ab"),
        bi("5"),
        sym("s"),
        Value::Array(vec![n(1.0), n(2.0)]),
        Value::Array(vec![Value::Hole, n(2.0)]),
        obj(&[]),
        obj(&[("x", n(1.0)), ("0", s("zero")), ("length", n(9.0))]),
        Value::Bytes(vec![1, 2, 255]),
        ta("Int8Array", vec![n(1.0), n(2.0)]),
        ta("Float64Array", vec![n(1.5)]),
        ta("BigInt64Array", vec![bi("5")]),
        Value::ArrayBuffer(vec![1, 2, 3, 4]),
        Value::DataView(vec![1, 2, 3, 4]),
        Value::Map(vec![(s("k"), n(1.0))]),
        Value::Set(vec![n(1.0)]),
        Value::Error("Error".into(), "msg".into()),
        Value::Boxed(Box::new(n(42.0))),
        Value::Boxed(Box::new(s("hi"))),
        Value::Boxed(Box::new(bi("7"))),
        Value::Boxed(Box::new(sym("b"))),
    ]
}

const KEYS: [&str; 22] = [
    "0",
    "1",
    "2",
    "-1",
    "1.5",
    "x",
    "length",
    "size",
    "toString",
    "hasOwnProperty",
    "constructor",
    "valueOf",
    "BYTES_PER_ELEMENT",
    "byteLength",
    "byteOffset",
    "buffer",
    "message",
    "stack",
    "get",
    "set",
    "add",
    "foo bar",
];

fn ops_spec() -> (Spec, Spec) {
    let mut spec = Spec::new();
    let vals = vals();
    for v in &vals {
        for source in UNOPS {
            spec.call(source, vec![v.clone()]);
        }
        spec.call("(v) => Object.keys(v)", vec![v.clone()]);
        spec.call("(v) => String(v)", vec![v.clone()]);
        for key in KEYS {
            spec.call("(k, v) => k in v", vec![s(key), v.clone()]);
            // `err.stack` reads the oracle's live stack text; `in`/hasOwn only test
            // own-ness, which is deterministic.
            if !(key == "stack" && matches!(v, Value::Error(..))) {
                spec.call("(o, k) => o[k]", vec![v.clone(), s(key)]);
            }
            spec.call("(o, k) => Object.hasOwn(o, k)", vec![v.clone(), s(key)]);
        }
        spec.call("(k, v) => k in v", vec![sym("k"), v.clone()]);
        spec.call("(o, k) => o[k]", vec![v.clone(), n(0.0)]);
        spec.call("(o, k) => o[k]", vec![v.clone(), sym("k")]);
    }
    for receiver in member_vals() {
        for key in KEYS {
            // A `valueOf` own property on a `box` wrapper breaks the oracle's encoder,
            // which reads `o.valueOf()` when re-encoding it. `c` instances are safe.
            if key == "valueOf"
                && matches!(&receiver, Value::Boxed(inner)
                    if !matches!(inner.as_ref(), Value::BigInt(_) | Value::Symbol(_)))
            {
                continue;
            }
            spec.call(
                "(o, k, v) => { o[k] = v; return o; }",
                vec![receiver.clone(), s(key), n(7.0)],
            );
            spec.call(
                "(o, k, v) => { 'use strict'; o[k] = v; return o; }",
                vec![receiver.clone(), s(key), n(7.0)],
            );
            spec.call(
                "(o, k, v) => { 'use strict'; o[k] = v; return o; }",
                vec![receiver.clone(), s(key), bi("7")],
            );
            spec.call(
                "(o, k) => { delete o[k]; return o; }",
                vec![receiver.clone(), s(key)],
            );
            spec.call(
                "(o, k) => { 'use strict'; delete o[k]; return o; }",
                vec![receiver.clone(), s(key)],
            );
            // Sloppy mode still throws when the element conversion fails.
            if matches!(receiver, Value::Bytes(_) | Value::TypedArray(..)) {
                spec.call(
                    "(o, k, v) => { o[k] = v; return o; }",
                    vec![receiver.clone(), s(key), bi("7")],
                );
            }
        }
    }
    let bin = bin_vals();
    for source in BINOPS {
        for a in &bin {
            for b in &bin {
                spec.call(source, vec![a.clone(), b.clone()]);
            }
        }
    }
    for source in ["(a, b) => a == b", "(a, b) => a === b"] {
        for b in &bin {
            spec.call(source, vec![sym("s"), b.clone()]);
            spec.call(source, vec![b.clone(), sym("s")]);
        }
        spec.call(source, vec![sym("s"), sym("s")]);
        spec.call(source, vec![sym("s"), Value::Symbol(None)]);
    }
    // Map/Set/TypedArray/Date operands hit the object arms of `+`, `==` and `<`.
    let exotics = [
        Value::Map(vec![(s("k"), n(1.0))]),
        Value::Set(vec![n(1.0)]),
        Value::ArrayBuffer(vec![1, 2]),
        Value::DataView(vec![1, 2]),
        Value::Error("Error".into(), "m".into()),
        ta("Int8Array", vec![n(1.0), n(2.0)]),
        ta("BigInt64Array", vec![bi("5")]),
    ];
    for source in ["(a, b) => a + b", "(a, b) => a < b", "(a, b) => a == b"] {
        for a in &exotics {
            for b in &bin {
                spec.call(source, vec![a.clone(), b.clone()]);
                spec.call(source, vec![b.clone(), a.clone()]);
            }
        }
    }
    (spec, dates_spec())
}

/// Date requests: any coercion reaching `Date.prototype.toString`/`toUTCString` depends on the
/// oracle zone, so these run once per zone from `ops-dates.fn.json`.
fn dates_spec() -> Spec {
    let mut spec = Spec::new();
    let epoch = date(0);
    let ts = date(1_700_000_000_000f64.to_bits());
    let invalid = date(f64::NAN.to_bits());
    let others = [
        n(1.0),
        s("x"),
        s(""),
        Value::Null,
        Value::Undefined,
        Value::Bool(true),
        bi("5"),
        sym("s"),
        Value::Array(vec![n(1.0)]),
        obj(&[]),
    ];
    for d in [&epoch, &ts, &invalid] {
        spec.call("(v) => String(v)", vec![d.clone()]);
        spec.call("(v) => Boolean(v)", vec![d.clone()]);
        spec.call("(v) => Number(v)", vec![d.clone()]);
        for o in &others {
            spec.call("(a, b) => a + b", vec![d.clone(), o.clone()]);
            spec.call("(a, b) => a + b", vec![o.clone(), d.clone()]);
            spec.call("(a, b) => a == b", vec![d.clone(), o.clone()]);
            spec.call("(a, b) => a == b", vec![o.clone(), d.clone()]);
            spec.call("(a, b) => a < b", vec![d.clone(), o.clone()]);
            spec.call("(a, b) => a < b", vec![o.clone(), d.clone()]);
        }
        for d2 in [&epoch, &ts, &invalid] {
            spec.call("(a, b) => a + b", vec![d.clone(), d2.clone()]);
            spec.call("(a, b) => a < b", vec![d.clone(), d2.clone()]);
            spec.call("(a, b) => a === b", vec![d.clone(), d2.clone()]);
            spec.call("(a, b) => a == b", vec![d.clone(), d2.clone()]);
        }
    }
    spec
}

#[test]
#[ignore = "rewrites parity/specs/js/ops*; regenerate vectors after with `cargo xtask oracle gen js/ops js/ops-dates`"]
fn write_ops_specs() {
    let (ops, dates) = ops_spec();
    ops.write("js/ops");
    dates.write("js/ops-dates.requests");
}
