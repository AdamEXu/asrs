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
            .chain(args.iter().map(Value::encode))
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

/// Values `JsValue` can hold, plus what only CVE-1 can. Valid dates are absent: their text
/// depends on the zone, so they live in `dates_spec`.
fn js_vals() -> Vec<Value> {
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
        n(2.0),
        n(1e21),
        n(f64::NAN),
        n(f64::INFINITY),
        n(-f64::INFINITY),
        n(9e15),
        n(5e-324),
        s(""),
        s("0"),
        s("1"),
        s("01"),
        s("-0"),
        s("-1.5"),
        s(" 12 "),
        s("\u{a0}5\u{feff}"),
        s("0x10"),
        s("-0x10"),
        s("1e3"),
        s("1_0"),
        s("a"),
        s("abc"),
        s("Infinity"),
        s("NaN"),
        s("null"),
        s("\u{1f600}"),
        s("\u{ffff}"),
        Value::String(asrs_parity::Utf16String::from_units(vec![0xd83d])),
        bi("0"),
        bi("5"),
        bi("-5"),
        bi("16"),
        bi("9007199254740993"),
        bi("123456789012345678901234567890"),
        Value::Array(vec![]),
        Value::Array(vec![n(1.0)]),
        Value::Array(vec![n(1.0), n(2.0)]),
        Value::Array(vec![s("a"), Value::Null, Value::Undefined]),
        Value::Array(vec![Value::Array(vec![Value::Array(vec![s("5")])])]),
        Value::Array(vec![obj(&[])]),
        obj(&[]),
        obj(&[("x", n(1.0)), ("length", n(9.0))]),
        obj(&[("y", n(1.0)), ("2", s("two"))]),
        Value::Bytes(vec![]),
        Value::Bytes(vec![7]),
        Value::Bytes(vec![1, 2, 255]),
        date(f64::NAN.to_bits()),
    ]
}

fn cve_vals() -> Vec<Value> {
    vec![
        sym("s"),
        Value::Symbol(None),
        Value::Array(vec![n(1.0), Value::Hole, n(3.0)]),
        ta("Int8Array", vec![n(1.0), n(-2.0)]),
        ta("Float64Array", vec![n(1.5)]),
        ta("BigInt64Array", vec![bi("5")]),
        Value::ArrayBuffer(vec![1, 2, 3, 4]),
        Value::DataView(vec![0, 0, 0, 0, 0, 0, 240, 63]),
        Value::Map(vec![(s("k"), n(1.0))]),
        Value::Set(vec![n(1.0), s("x")]),
        Value::Error("Error".into(), "msg".into()),
        Value::Error("TypeError".into(), "".into()),
        Value::Boxed(Box::new(n(42.0))),
        Value::Boxed(Box::new(s("hi"))),
        Value::Boxed(Box::new(Value::Bool(false))),
    ]
}

/// The operands both sides of a binary operator range over.
fn bin_vals() -> Vec<Value> {
    let skip = [
        n(0.5),
        n(9e15),
        n(5e-324),
        s("abc"),
        s("1e3"),
        s("null"),
        s("\u{1f600}"),
        Value::Array(vec![n(1.0), n(2.0)]),
        obj(&[("x", n(1.0)), ("length", n(9.0))]),
        Value::Bytes(vec![]),
    ];
    js_vals()
        .into_iter()
        .filter(|v| !skip.contains(v))
        .collect()
}

const BINOPS: [&str; 9] = [
    "(a, b) => a + b",
    "(a, b) => a - b",
    "(a, b) => a < b",
    "(a, b) => a <= b",
    "(a, b) => a > b",
    "(a, b) => a >= b",
    "(a, b) => a == b",
    "(a, b) => a === b",
    "(a, b) => [a].includes(b)",
];

const UNOPS: [&str; 5] = [
    "(a) => -a",
    "(a) => +a",
    "(a) => a - 0",
    "(v) => `${v}`",
    "(v) => Boolean(v)",
];

const MEMBERS: [&str; 3] = [
    "(v) => Object.keys(v)",
    "(v) => Object.getOwnPropertyNames(v)",
    "(v) => ({ ...v })",
];

const PROTOS: [&str; 5] = [
    "() => Object.getOwnPropertyNames(Object.prototype)",
    "() => Object.getOwnPropertyNames(Array.prototype)",
    "() => Object.getOwnPropertyNames(Object.getPrototypeOf(Uint8Array.prototype))",
    "() => Object.getOwnPropertyNames(Uint8Array.prototype)",
    "() => Object.getOwnPropertyNames(Date.prototype)",
];

const IN: &str = "(k, v) => k in v";
const GET: &str = "(o, k) => o[k]";
const HAS_OWN: &str = "(o, k) => Object.hasOwn(o, k)";
const SET: &str = "(o, k, v) => { 'use strict'; o[k] = v; return o; }";
const DELETE: &str = "(o, k) => { 'use strict'; delete o[k]; return o; }";
const INDEX: &str = "(k, n) => Array.from({ length: n }, (_, i) => i)[k]";
const SPLICE: &str = "(s, n) => { const l = Array.from({ length: n }, () => 0); l.splice(s, 0, 1); return l.indexOf(1); }";

const KEYS: [&str; 10] = [
    "0", "1", "2", "-1", "-0", "1.5", "01", "x", "length", "foo bar",
];

/// Members only the prototype chain has. `ops::get` reads them as `undefined` (issues 64
/// and 87), so they are checked through `in`, `hasOwn`, writes and deletes but not reads.
const INHERITED: [&str; 12] = [
    "toString",
    "hasOwnProperty",
    "constructor",
    "valueOf",
    "BYTES_PER_ELEMENT",
    "byteOffset",
    "buffer",
    "at",
    "set",
    "getHours",
    "__proto__",
    "toBase64",
];

fn member_vals() -> Vec<Value> {
    vec![
        Value::Undefined,
        Value::Null,
        Value::Bool(true),
        n(1.5),
        n(-0.0),
        s(""),
        s("ab"),
        bi("5"),
        Value::Array(vec![]),
        Value::Array(vec![n(1.0), n(2.0)]),
        Value::Array(vec![s("a"), s("b"), s("c")]),
        obj(&[]),
        obj(&[("x", n(1.0)), ("0", s("zero")), ("length", n(9.0))]),
        obj(&[("toString", n(1.0)), ("constructor", Value::Null)]),
        Value::Bytes(vec![]),
        Value::Bytes(vec![1, 2, 255]),
        date(0),
    ]
}

/// Writes and deletes that leave an array hole, which `JsValue` has no form for.
fn leaves_hole(source: &str, target: &Value, key: &str, value: Option<&Value>) -> bool {
    let Value::Array(items) = target else {
        return false;
    };
    let index = key.parse::<usize>().ok().filter(|i| i.to_string() == key);
    if source == DELETE {
        return index.is_some_and(|i| i < items.len());
    }
    let grown = match value {
        _ if index.is_some() => index,
        Some(Value::Number(bits)) if key == "length" => Some(f64::from_bits(*bits) as usize),
        Some(Value::String(_)) if key == "length" => Some(2),
        _ => None,
    };
    grown.is_some_and(|i| i > items.len())
}

fn ops_spec() -> (Spec, Spec) {
    let mut spec = Spec::new();
    for source in PROTOS {
        spec.call(source, vec![]);
    }
    for v in js_vals().into_iter().chain(cve_vals()) {
        for source in UNOPS {
            spec.call(source, vec![v.clone()]);
        }
    }
    for v in js_vals().into_iter().chain(member_vals()) {
        for source in MEMBERS {
            spec.call(source, vec![v.clone()]);
        }
    }
    // `ToPrimitive` does not call an object's own `toString` (issue 87).
    let keys = js_vals().into_iter().chain(member_vals());
    for v in keys.filter(|v| v.get("toString").is_none()) {
        for len in [0.0, 3.0] {
            spec.call(INDEX, vec![v.clone(), n(len)]);
            if !matches!(v, Value::BigInt(_)) {
                spec.call(SPLICE, vec![v.clone(), n(len)]);
            }
        }
    }
    for target in member_vals() {
        for (key, inherited) in KEYS
            .iter()
            .map(|k| (*k, false))
            .chain(INHERITED.iter().map(|k| (*k, true)))
        {
            // The CVE-1 encoder names an object by its `constructor`, which these change.
            let hides_class = matches!(key, "constructor" | "__proto__")
                && matches!(target, Value::Bytes(_) | Value::Date(_));
            spec.call(IN, vec![s(key), target.clone()]);
            spec.call(HAS_OWN, vec![target.clone(), s(key)]);
            if !inherited {
                spec.call(GET, vec![target.clone(), s(key)]);
            }
            for value in [
                n(1.0),
                n(300.0),
                n(1.5),
                n(-1.0),
                n(f64::NAN),
                s("2"),
                Value::Null,
                bi("7"),
            ] {
                if !leaves_hole(SET, &target, key, Some(&value)) && !hides_class {
                    spec.call(SET, vec![target.clone(), s(key), value]);
                }
            }
            if !leaves_hole(DELETE, &target, key, None) {
                spec.call(DELETE, vec![target.clone(), s(key)]);
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
        for a in cve_vals() {
            for b in bin.iter().step_by(3) {
                spec.call(source, vec![a.clone(), b.clone()]);
                spec.call(source, vec![b.clone(), a.clone()]);
            }
            for b in cve_vals() {
                spec.call(source, vec![a.clone(), b]);
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
        spec.call("(v) => `${v}`", vec![d.clone()]);
        spec.call("(v) => Boolean(v)", vec![d.clone()]);
        spec.call("(a) => +a", vec![d.clone()]);
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
