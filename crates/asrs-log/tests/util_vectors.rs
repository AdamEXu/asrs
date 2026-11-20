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
#[path = "../../asrs-cli/src/js_msgpack.rs"]
#[allow(dead_code)]
mod js_msgpack;
#[path = "../../asrs-cli/src/js_regex.rs"]
#[allow(dead_code)]
mod js_regex;
#[path = "../../asrs-cli/src/js_string.rs"]
#[allow(dead_code)]
mod js_string;
#[path = "../../asrs-cli/src/js_time.rs"]
#[allow(dead_code)]
mod js_time;
#[path = "../../asrs-cli/src/log_field.rs"]
#[allow(dead_code)]
mod log_field;
#[path = "../../asrs-cli/src/log_run.rs"]
#[allow(dead_code)]
mod log_run;
#[path = "../../asrs-cli/src/photon.rs"]
#[allow(dead_code)]
mod photon;
#[path = "../../asrs-cli/src/shared.rs"]
#[allow(dead_code)]
mod shared;

use std::collections::HashSet;

use asrs_parity::vectors::check_fn_vectors;
use asrs_parity::{FnCase, Request, Response, Value};

fn dispatch(case: &FnCase<'_>) -> Response {
    match functions::lookup(case.name()).or_else(|| {
        shared::FUNCTIONS
            .iter()
            .find(|(n, _)| *n == case.name())
            .map(|(_, f)| *f)
    }) {
        Some(f) => f(&case.request.args),
        None => Response::Error(format!("no port of {}", case.name())),
    }
}

#[test]
fn util_utc() {
    check_fn_vectors("log/util", dispatch);
}

#[test]
fn log_field_sequences() {
    check_fn_vectors("log/field", dispatch);
}

#[test]
fn log_sequences() {
    check_fn_vectors("log/core", dispatch);
}

fn n(x: f64) -> Value {
    Value::number(x)
}

fn s(text: &str) -> Value {
    Value::string(text)
}

fn arr(items: Vec<Value>) -> Value {
    Value::Array(items)
}

fn nums(xs: &[f64]) -> Value {
    arr(xs.iter().map(|&x| n(x)).collect())
}

fn bytes(xs: &[u8]) -> Value {
    Value::Bytes(xs.to_vec())
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::object(pairs)
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

    fn write(&self, id: &str) {
        let path = asrs_parity::vectors::workspace_root().join(format!("parity/specs/{id}.jsonl"));
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        let text: String = self.lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(&path, text).expect("write spec");
        eprintln!("{}: {} requests", path.display(), self.lines.len());
    }
}

fn bounds(spec: &mut Spec) {
    let arrays = vec![
        nums(&[]),
        nums(&[0.0]),
        nums(&[1.0, 3.0, 5.0]),
        nums(&[1.0, 1.0, 3.0]),
        nums(&[0.0, 0.0, 0.0]),
        nums(&[-3.0, 0.0, 2.0]),
        nums(&[f64::NAN, 1.0]),
        nums(&[1.0, f64::NAN]),
        nums(&[f64::INFINITY]),
        nums(&[f64::NEG_INFINITY, 0.0]),
        nums(&[-0.0, 0.0]),
        nums(&[3.0, 1.0, 2.0]),
        nums(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0]),
    ];
    let targets = [
        0.0,
        -0.0,
        1.0,
        2.0,
        3.0,
        4.0,
        5.0,
        6.0,
        10.0,
        11.0,
        -1.0,
        0.5,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    let starts: Vec<Option<Value>> = vec![
        None,
        Some(Value::Undefined),
        Some(n(0.0)),
        Some(n(-0.0)),
        Some(n(1.0)),
        Some(n(2.0)),
        Some(n(5.0)),
        Some(n(13.0)),
        Some(n(-1.0)),
        Some(n(-3.0)),
        Some(n(0.5)),
        Some(n(2.5)),
        Some(n(7.9)),
        Some(n(f64::NAN)),
        Some(n(f64::INFINITY)),
        Some(n(4294967295.0)),
        Some(n(1e15)),
    ];
    for array in &arrays {
        for target in targets {
            for start in &starts {
                let mut args = vec![array.clone(), n(target)];
                if let Some(start) = start {
                    args.push(start.clone());
                }
                spec.push("shared/util:lowerBound", args.clone());
                spec.push("shared/util:upperBound", args);
            }
        }
    }
    // `-Infinity` is left out of the matrix: `(-Infinity + high) >>> 1` is
    // always 0, so `low` never moves when `array[0] >= target` and upstream
    // loops forever. The cases that still terminate are generated here.
    for (array, target) in [
        (nums(&[]), 0.0),
        (nums(&[1.0, 3.0, 5.0]), 5.0),
        (nums(&[1.0, 3.0, 5.0]), 4.0),
        (nums(&[1.0, 3.0, 5.0]), 10.0),
        (nums(&[0.0]), 1.0),
        (nums(&[0.0]), f64::NAN),
    ] {
        for kind in ["shared/util:lowerBound", "shared/util:upperBound"] {
            spec.push(kind, vec![array.clone(), n(target), n(f64::NEG_INFINITY)]);
        }
    }
    // Non-number startIndex and non-array / non-number-element inputs take the
    // generic JS path.
    let extras = vec![
        (nums(&[1.0, 3.0, 5.0]), s("2"), n(3.0)),
        (nums(&[1.0, 3.0, 5.0]), s("2.5"), n(3.0)),
        (nums(&[1.0, 3.0, 5.0]), Value::Bool(true), n(3.0)),
        (nums(&[1.0, 3.0, 5.0]), Value::Null, n(3.0)),
        (nums(&[5.0]), Value::Null, n(3.0)),
        (nums(&[1.0, 3.0, 5.0]), arr(vec![n(1.0)]), n(2.0)),
        (s("ace"), n(1.0), Value::Undefined),
        (s("abc"), s("b"), Value::Undefined),
        (n(7.0), n(1.0), Value::Undefined),
        (Value::Null, n(1.0), Value::Undefined),
        (arr(vec![n(1.0), s("3"), n(5.0)]), n(3.0), Value::Undefined),
        (
            arr(vec![n(1.0), Value::Null, n(5.0)]),
            n(0.5),
            Value::Undefined,
        ),
        (
            arr(vec![n(1.0), Value::Hole, n(5.0)]),
            n(1.0),
            Value::Undefined,
        ),
        (nums(&[1.0, 2.0]), Value::string("x"), Value::Undefined),
        (nums(&[1.0, 2.0]), s("0"), Value::Undefined),
        (
            obj(vec![
                ("length", n(3.0)),
                ("0", n(1.0)),
                ("1", n(3.0)),
                ("2", n(5.0)),
            ]),
            n(3.0),
            Value::Undefined,
        ),
        (
            obj(vec![("length", n(2.0)), ("0", n(9.0)), ("1", n(8.0))]),
            n(5.0),
            Value::Undefined,
        ),
        (arr(vec![]), n(1.0), n(2.0)),
    ];
    for (array, target, start) in extras {
        let args = vec![array.clone(), target.clone(), start.clone()];
        spec.push("shared/util:lowerBound", args.clone());
        spec.push("shared/util:upperBound", args);
        let args2 = vec![array, target];
        spec.push("shared/util:lowerBound", args2.clone());
        spec.push("shared/util:upperBound", args2);
    }
}

fn misc(spec: &mut Spec) {
    // arraysEqual
    for (a, b) in [
        (arr(vec![]), arr(vec![])),
        (nums(&[1.0, 2.0, 3.0]), nums(&[1.0, 2.0, 3.0])),
        (nums(&[1.0, 2.0]), nums(&[1.0, 2.0, 3.0])),
        (nums(&[1.0, 2.0, 4.0]), nums(&[1.0, 2.0, 3.0])),
        (arr(vec![s("a"), s("b")]), s("ab")),
        (s("ab"), arr(vec![s("a"), s("b")])),
        (arr(vec![Value::Null]), arr(vec![Value::Null])),
        (arr(vec![Value::Null]), arr(vec![Value::Undefined])),
        (arr(vec![Value::Undefined]), nums(&[5.0])),
        (arr(vec![Value::Hole]), nums(&[5.0])),
        (arr(vec![Value::Hole, n(1.0)]), arr(vec![n(9.0), n(1.0)])),
        (nums(&[5.0]), n(5.0)),
        (n(5.0), n(5.0)),
        (n(5.0), nums(&[5.0])),
        (arr(vec![arr(vec![n(1.0)])]), arr(vec![arr(vec![n(1.0)])])),
        (arr(vec![n(f64::NAN)]), arr(vec![n(f64::NAN)])),
        (bytes(&[1, 2]), nums(&[1.0, 2.0])),
        (bytes(&[1, 2]), bytes(&[1, 2])),
        (arr(vec![n(-0.0)]), arr(vec![n(0.0)])),
    ] {
        spec.push("shared/util:arraysEqual", vec![a, b]);
    }

    // setsEqual
    let set = |xs: Vec<Value>| Value::Set(xs);
    let map = |pairs: Vec<(Value, Value)>| Value::Map(pairs);
    for (a, b) in [
        (set(vec![]), set(vec![])),
        (set(vec![n(1.0), n(2.0)]), set(vec![n(2.0), n(1.0)])),
        (set(vec![n(1.0)]), set(vec![n(1.0), n(2.0)])),
        (set(vec![n(1.0), n(4.0)]), set(vec![n(1.0), n(2.0)])),
        (set(vec![n(f64::NAN)]), set(vec![n(f64::NAN)])),
        (set(vec![n(0.0)]), set(vec![n(-0.0)])),
        (set(vec![s("a")]), set(vec![s("a")])),
        (set(vec![n(1.0)]), map(vec![(n(1.0), s("x"))])),
        (map(vec![(n(1.0), s("x"))]), set(vec![n(1.0)])),
        (set(vec![]), map(vec![])),
        (n(5.0), n(6.0)),
        (n(5.0), set(vec![])),
        (set(vec![n(1.0)]), n(5.0)),
        (Value::Null, Value::Null),
        (set(vec![Value::Null]), set(vec![Value::Null])),
    ] {
        spec.push("shared/util:setsEqual", vec![a, b]);
    }

    // checkArrayType
    for (value, type_name) in [
        (nums(&[1.0, 2.0]), s("number")),
        (arr(vec![n(1.0), s("a")]), s("number")),
        (arr(vec![]), s("number")),
        (arr(vec![]), n(5.0)),
        (arr(vec![s("a"), s("b")]), s("string")),
        (arr(vec![Value::Bool(true)]), s("boolean")),
        (arr(vec![Value::Null]), s("object")),
        (arr(vec![Value::Undefined]), s("undefined")),
        (arr(vec![Value::Hole]), s("undefined")),
        (arr(vec![arr(vec![])]), s("object")),
        (arr(vec![Value::BigInt("5".into())]), s("bigint")),
        (nums(&[1.0]), n(5.0)),
        (s("ab"), s("string")),
        (bytes(&[1]), s("object")),
    ] {
        spec.push("shared/util:checkArrayType", vec![value, type_name]);
    }

    // indexArray
    for length in [
        n(0.0),
        n(1.0),
        n(5.0),
        n(-1.0),
        n(0.5),
        n(2.9),
        n(f64::NAN),
        n(-0.0),
        s("5"),
        Value::Null,
        Value::Undefined,
        Value::Bool(true),
        n(100.0),
        n(4294967296.0),
        n(1e300),
        n(f64::INFINITY),
    ] {
        spec.push("shared/util:indexArray", vec![length]);
    }

    // charIsCapital
    for (input, index) in [
        (s("A"), n(0.0)),
        (s("a"), n(0.0)),
        (s("1"), n(0.0)),
        (s("5aB"), n(2.0)),
        (s("abc"), n(5.0)),
        (s("abc"), n(-1.0)),
        (s("aBc"), n(1.0)),
        (s("É"), n(0.0)),
        (s("é"), n(0.0)),
        (s("ß"), n(0.0)),
        (s(""), n(0.0)),
        (s("ab"), n(1.5)),
        (s("ab"), n(f64::NAN)),
        (s("ab"), n(f64::INFINITY)),
        (s("ab"), s("1")),
        (s("ab"), Value::Undefined),
        (Value::Null, n(0.0)),
        (Value::Undefined, n(0.0)),
        (n(5.0), n(0.0)),
    ] {
        spec.push("shared/util:charIsCapital", vec![input, index]);
    }

    // calcMockProgress: default maxPercent only on undefined.
    for args in [
        vec![n(0.0)],
        vec![n(10.0)],
        vec![n(-1.0)],
        vec![n(f64::NAN)],
        vec![n(f64::INFINITY)],
        vec![n(10.0), n(0.6)],
        vec![n(10.0), n(0.0)],
        vec![n(10.0), n(1.0)],
        vec![n(10.0), n(-0.5)],
        vec![n(10.0), n(f64::NAN)],
        vec![n(10.0), Value::Undefined],
        vec![n(10.0), s("0.5")],
        vec![n(10.0), Value::Null],
    ] {
        spec.push("shared/util:calcMockProgress", args);
    }

    // shiftColor: full clamp range, malformed hex, non-number shifts.
    for color in [
        "#2b66a2", "#ffffff", "#000000", "#808080", "#abcdef", "#abc", "#12345", "#", "#gggggg",
        "", "#1234567", "#fff",
    ] {
        for shift in [0.0, 1.0, -1.0, 10.0, -50.0, 255.0, -255.0, 300.0, 0.5, -0.5] {
            spec.push("shared/util:shiftColor", vec![s(color), n(shift)]);
        }
    }
    for shift in [
        n(f64::NAN),
        n(f64::INFINITY),
        s("5"),
        s("abc"),
        Value::Null,
        Value::Bool(true),
        arr(vec![n(5.0)]),
        Value::Undefined,
    ] {
        spec.push("shared/util:shiftColor", vec![s("#102030"), shift]);
    }
    for color in [
        Value::Null,
        Value::Undefined,
        n(5.0),
        arr(vec![s("#1020")]),
        bytes(&[35, 49]),
    ] {
        spec.push("shared/util:shiftColor", vec![color, n(1.0)]);
    }

    // zfill
    for (number_arg, length) in [
        (s("5"), n(3.0)),
        (s("abc"), n(5.0)),
        (s("5"), n(0.0)),
        (s("5"), n(-1.0)),
        (s("5"), n(f64::NAN)),
        (s("5"), n(2.5)),
        (s("5"), n(1.0)),
        (s("12345"), n(3.0)),
        (s(""), n(5.0)),
        (s("5"), s("3")),
        (n(5.0), n(3.0)),
        (Value::Bool(true), n(3.0)),
        (Value::Null, n(3.0)),
        (arr(vec![n(1.0), n(2.0)]), n(10.0)),
    ] {
        spec.push("shared/util:zfill", vec![number_arg, length]);
    }

    // cleanFloat
    for x in [
        n(0.0),
        n(-0.0),
        n(0.1 + 0.2),
        n(1.2345678901),
        n(-1.2345678901),
        n(1e-7),
        n(-1e-7),
        n(123456.7890123456),
        n(f64::NAN),
        n(f64::INFINITY),
        n(f64::NEG_INFINITY),
        n(0.0000005),
        n(-0.0000005),
        n(2.5),
        n(4_503_599_627_370_495.5),
        n(1e21),
        s("5.5"),
        Value::Null,
        Value::Bool(true),
        arr(vec![n(5.0)]),
        arr(vec![n(5.0), n(5.0)]),
    ] {
        spec.push("shared/util:cleanFloat", vec![x]);
    }

    // scaleValue / scaleValueClamped
    for (value, old, new) in [
        (n(0.0), nums(&[0.0, 1.0]), nums(&[0.0, 1.0])),
        (n(0.5), nums(&[0.0, 1.0]), nums(&[0.0, 100.0])),
        (n(5.0), nums(&[0.0, 10.0]), nums(&[10.0, -10.0])),
        (n(-1.0), nums(&[-5.0, 5.0]), nums(&[0.0, 100.0])),
        (n(15.0), nums(&[0.0, 10.0]), nums(&[0.0, 1.0])),
        (n(5.0), nums(&[5.0, 5.0]), nums(&[0.0, 1.0])),
        (n(0.0), nums(&[0.0, 0.0]), nums(&[0.0, 1.0])),
        (n(f64::NAN), nums(&[0.0, 1.0]), nums(&[0.0, 1.0])),
        (
            n(1.0),
            obj(vec![("0", n(0.0)), ("1", n(1.0))]),
            nums(&[0.0, 10.0]),
        ),
        (n(1.0), s("05"), nums(&[0.0, 1.0])),
        (n(1.0), arr(vec![Value::Null, n(10.0)]), nums(&[0.0, 1.0])),
        (n(0.25), nums(&[0.0, 1.0]), nums(&[100.0, 200.0])),
    ] {
        spec.push(
            "shared/util:scaleValue",
            vec![value.clone(), old.clone(), new.clone()],
        );
        spec.push("shared/util:scaleValueClamped", vec![value, old, new]);
    }

    // ValueScaler: constructor output and duck-typed prototype method.
    for (old, new) in [
        (nums(&[0.0, 10.0]), nums(&[0.0, 1.0])),
        (nums(&[-5.0, 5.0]), nums(&[0.0, 100.0])),
        (nums(&[5.0, 5.0]), nums(&[0.0, 1.0])),
        (nums(&[0.0, 0.0]), nums(&[0.0, 1.0])),
        (nums(&[0.0, 1.0]), nums(&[5.0, -5.0])),
    ] {
        spec.push("new shared/util:ValueScaler", vec![old, new]);
    }
    for (receiver, value) in [
        (obj(vec![("a", n(2.0)), ("b", n(3.0))]), n(5.0)),
        (obj(vec![("a", n(0.1)), ("b", n(-50.0))]), n(250.0)),
        (obj(vec![]), n(5.0)),
        (obj(vec![("a", s("2")), ("b", n(1.0))]), n(5.0)),
        (Value::Null, n(5.0)),
        (Value::Undefined, n(5.0)),
        (n(3.0), n(5.0)),
    ] {
        spec.push(
            "shared/util:ValueScaler.prototype.calculate",
            vec![receiver, value],
        );
    }

    // clampValue
    for (v, lo, hi) in [
        (5.0, 0.0, 10.0),
        (-5.0, 0.0, 10.0),
        (15.0, 0.0, 10.0),
        (f64::NAN, 0.0, 10.0),
        (5.0, 10.0, 0.0),
        (5.0, f64::NEG_INFINITY, f64::INFINITY),
        (0.5, 0.0, 0.0),
        (5.0, f64::NAN, 10.0),
        (5.0, 0.0, f64::NAN),
        (-0.0, 0.0, 1.0),
    ] {
        spec.push("shared/util:clampValue", vec![n(v), n(lo), n(hi)]);
    }

    // transformPx
    for (origin, rotation, rel) in [
        (nums(&[100.0, 50.0]), n(0.0), nums(&[10.0, 0.0])),
        (
            nums(&[100.0, 50.0]),
            n(std::f64::consts::FRAC_PI_2),
            nums(&[10.0, 0.0]),
        ),
        (
            nums(&[0.0, 0.0]),
            n(std::f64::consts::PI),
            nums(&[3.0, 4.0]),
        ),
        (
            nums(&[10.0, 20.0]),
            n(-std::f64::consts::FRAC_PI_2),
            nums(&[-5.0, 12.0]),
        ),
        (nums(&[0.0, 0.0]), n(f64::NAN), nums(&[1.0, 1.0])),
        (nums(&[0.0, 0.0]), n(1.0), nums(&[0.0, 0.0])),
        (n(5.0), n(1.0), nums(&[1.0, 1.0])),
        (nums(&[0.0, 0.0]), n(1.0), n(5.0)),
    ] {
        spec.push("shared/util:transformPx", vec![origin, rotation, rel]);
    }

    // wrapRadians
    for r in [
        0.0,
        std::f64::consts::PI,
        -std::f64::consts::PI,
        3.0 * std::f64::consts::PI,
        -3.0 * std::f64::consts::PI,
        std::f64::consts::PI + 0.1,
        -std::f64::consts::PI - 0.1,
        2.0 * std::f64::consts::PI,
        -2.0 * std::f64::consts::PI,
        4.0 * std::f64::consts::PI,
        -4.0 * std::f64::consts::PI,
        0.1,
        -0.1,
        1e16,
        -1e16,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        spec.push("shared/util:wrapRadians", vec![n(r)]);
    }

    // concatBuffers
    for arrays in [
        arr(vec![bytes(&[0x00, 0xff]), bytes(&[0xaa])]),
        arr(vec![]),
        arr(vec![bytes(&[])]),
        arr(vec![bytes(&[0x00]), bytes(&[]), bytes(&[0x01, 0x02, 0x03])]),
        arr(vec![n(5.0)]),
        arr(vec![Value::Null]),
        arr(vec![arr(vec![n(1.0), n(2.0)])]),
        arr(vec![bytes(&[0x00]), n(5.0)]),
        s("ab"),
        Value::Null,
        n(5.0),
        Value::Undefined,
        Value::TypedArray("Uint16Array".into(), vec![n(4660.0), n(22136.0)]),
        arr(vec![bytes(&[0xde, 0xad])]),
    ] {
        spec.push("shared/util:concatBuffers", vec![arrays]);
    }

    // calcAxisStepSize
    for (data, pixel, target) in [
        (nums(&[0.0, 100.0]), 1000.0, 100.0),
        (nums(&[0.0, 1.0]), 500.0, 100.0),
        (nums(&[0.0, 7.0]), 300.0, 50.0),
        (nums(&[-50.0, 50.0]), 400.0, 80.0),
        (nums(&[0.0, 0.0]), 100.0, 10.0),
        (nums(&[0.0, 1e20]), 100.0, 10.0),
        (nums(&[0.0, 1.0]), 1000.0, 0.0),
        (nums(&[0.0, 2.5e-3]), 600.0, 120.0),
        (nums(&[0.0, 3.0]), 100.0, 33.0),
        (nums(&[0.0, 9.0]), 200.0, 100.0),
        (nums(&[0.0, 40.0]), 100.0, 25.0),
        (nums(&[0.0, 60.0]), 300.0, 60.0),
        (nums(&[0.0, 80.0]), 400.0, 50.0),
        (nums(&[0.0, 15.0]), 250.0, 100.0),
        (nums(&[100.0, 0.0]), 200.0, 50.0),
    ] {
        spec.push(
            "shared/util:calcAxisStepSize",
            vec![data, n(pixel), n(target)],
        );
    }

    // getSpiralIndex
    for x in -3..=3 {
        for y in -3..=3 {
            spec.push("shared/util:getSpiralIndex", vec![n(x as f64), n(y as f64)]);
        }
    }
    for (x, y) in [
        (0.5, 0.5),
        (-0.5, 1.5),
        (f64::NAN, 0.0),
        (0.0, f64::NAN),
        (f64::INFINITY, 1.0),
        (1e10, -1e10),
        (-0.0, 0.0),
    ] {
        spec.push("shared/util:getSpiralIndex", vec![n(x), n(y)]);
    }

    // jsonCopy
    for value in [
        obj(vec![("a", n(1.0)), ("b", s("x"))]),
        arr(vec![n(1.0), Value::Null, s("y")]),
        n(5.5),
        s("text"),
        Value::Null,
        Value::Bool(true),
        obj(vec![("nested", obj(vec![("deep", arr(vec![n(1.0)]))]))]),
        Value::Undefined,
        arr(vec![Value::Hole, n(1.0)]),
        obj(vec![("bad", Value::Undefined)]),
    ] {
        spec.push("shared/util:jsonCopy", vec![value]);
    }
}

#[test]
#[ignore = "rewrites parity/specs/log/util.jsonl; regenerate vectors after with `cargo xtask oracle gen log/util`"]
fn write_util_specs() {
    let mut spec = Spec::new();
    bounds(&mut spec);
    misc(&mut spec);
    spec.write("log/util");
}
