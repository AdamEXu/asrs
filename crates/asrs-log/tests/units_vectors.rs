#![allow(clippy::disallowed_methods)]

#[path = "../../asrs-cli/src/units.rs"]
#[allow(dead_code)]
mod cli_units;
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
#[path = "../../asrs-cli/src/log_struct.rs"]
#[allow(dead_code)]
mod log_struct;
#[path = "../../asrs-cli/src/photon.rs"]
#[allow(dead_code)]
mod photon;
#[path = "../../asrs-cli/src/shared.rs"]
#[allow(dead_code)]
mod shared;

use std::collections::HashSet;

use asrs_log::units;
mod tabs_console {
    pub const FUNCTIONS: &[(&str, crate::functions::Function)] = &[];
    pub const CALLS: &[(&str, crate::functions::Function)] = &[];
}

use asrs_parity::vectors::check_fn_vectors;
use asrs_parity::{FnCase, Request, Response, Value};

fn dispatch(case: &FnCase<'_>) -> Response {
    match functions::lookup(case.name()).or_else(|| {
        shared::FUNCTIONS
            .iter()
            .chain(cli_units::FUNCTIONS)
            .find(|(n, _)| *n == case.name())
            .map(|(_, f)| *f)
    }) {
        Some(f) => f(&case.request.args),
        None => cli_units::table_read(case.name(), &case.request.args)
            .unwrap_or_else(|| Response::Error(format!("no port of {}", case.name()))),
    }
}

#[test]
fn units_utc() {
    check_fn_vectors("log/units", dispatch);
}

fn n(x: f64) -> Value {
    Value::number(x)
}

fn s(text: &str) -> Value {
    Value::string(text)
}

fn boxed(v: Value) -> Value {
    Value::Boxed(Box::new(v))
}

fn arr(items: Vec<Value>) -> Value {
    Value::Array(items)
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

const CONVERT: &str = "shared/units:Units.convert";
const CONVERT_PRESET: &str = "shared/units:Units.convertWithPreset";
const SUFFIX_FILTER: &str = "shared/units:Units.getSuffixForFilter";
const FIELD_UNIT: &str = "shared/units:Units.getUnitForField";

fn unit_keys() -> Vec<&'static str> {
    units::all_units().iter().map(|(key, _)| *key).collect()
}

fn convert_cases(spec: &mut Spec) {
    // Every (from, to) pair at value 1: each factor's bits show up in some ratio.
    let keys = unit_keys();
    for from in &keys {
        for to in &keys {
            spec.push(CONVERT, vec![n(1.0), s(from), s(to)]);
        }
    }
    let values = [
        0.0,
        -0.0,
        1.0,
        -1.0,
        0.5,
        2.5,
        32.0,
        212.0,
        -40.0,
        100.0,
        273.15,
        -273.15,
        373.15,
        1e-7,
        1e15,
        1e21,
        f64::MAX,
        f64::MIN_POSITIVE,
        f64::EPSILON,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    let pairs = [
        ("celsius", "fahrenheit"),
        ("fahrenheit", "celsius"),
        ("celsius", "kelvin"),
        ("kelvin", "celsius"),
        ("fahrenheit", "kelvin"),
        ("kelvin", "fahrenheit"),
        ("fahrenheit", "fahrenheit"),
        ("kelvin", "kelvin"),
        ("celsius", "celsius"),
        ("meters", "inches"),
        ("inches", "feet"),
        ("radians/second", "rpm"),
        ("meters/second", "feet/second"),
        ("bytes", "kilobytes"),
        ("meters", "kelvin"),
        ("kelvin", "meters"),
    ];
    for value in values {
        for (from, to) in pairs {
            spec.push(CONVERT, vec![n(value), s(from), s(to)]);
        }
    }
    // Non-number `value` args take the ToNumber path inside the arithmetic.
    let odd_values = [
        s("5"),
        s("2.5"),
        s("abc"),
        s(""),
        Value::Null,
        Value::Undefined,
        Value::Bool(true),
        Value::Bool(false),
        arr(vec![n(2.0)]),
        arr(vec![n(1.0), n(2.0)]),
        arr(vec![]),
        obj(vec![]),
        boxed(n(3.0)),
    ];
    for value in odd_values {
        for (from, to) in [
            ("meters", "inches"),
            ("fahrenheit", "celsius"),
            ("badunit", "meters"),
        ] {
            spec.push(CONVERT, vec![value.clone(), s(from), s(to)]);
        }
    }
    // Unit-name oddities: unknown keys, case, prototype names, boxed strings
    // and non-string property keys.
    for (from, to) in [
        (s("badunit"), s("meters")),
        (s("meters"), s("badunit")),
        (s("badunit"), s("alsonotaunit")),
        (s(""), s("meters")),
        (s("toString"), s("meters")),
        (s("meters"), s("toString")),
        (s("constructor"), s("valueOf")),
        (s("__proto__"), s("meters")),
        (s("hasOwnProperty"), s("meters")),
        (s("isPrototypeOf"), s("meters")),
        (s("valueOf"), s("toLocaleString")),
        (boxed(s("meters")), s("inches")),
        (s("meters"), boxed(s("inches"))),
        (boxed(s("kelvin")), s("celsius")),
        (s("kelvin"), boxed(s("kelvin"))),
        (boxed(s("kelvin")), boxed(s("kelvin"))),
        (boxed(s("fahrenheit")), s("celsius")),
        (s("celsius"), boxed(s("fahrenheit"))),
        (boxed(s("fahrenheit")), boxed(s("kelvin"))),
        (boxed(s("badunit")), s("meters")),
        (n(5.0), s("meters")),
        (s("meters"), n(5.0)),
        (Value::Null, s("meters")),
        (s("meters"), Value::Null),
        (Value::Undefined, s("meters")),
        (s("meters"), Value::Undefined),
        (Value::Bool(true), s("meters")),
        (s("meters"), Value::Bool(true)),
        (arr(vec![s("meters")]), s("meters")),
        (obj(vec![]), s("meters")),
        (s("Meters"), s("inches")),
        (s("METERS"), s("INCHES")),
        (s(" meters"), s("inches")),
        (s("meters "), s("inches")),
        (s("fahrenheit"), s("kelvin")),
        (s("celcius"), s("kelvin")),
        (s("celcius"), s("fahrenheit")),
    ] {
        spec.push(CONVERT, vec![n(1.0), from, to]);
    }
}

fn preset_cases(spec: &mut Spec) {
    let presets = [
        obj(vec![("type", Value::Null), ("factor", n(1.0))]),
        obj(vec![
            ("type", Value::Null),
            ("factor", n(1.0)),
            ("from", s("meters")),
            ("to", s("inches")),
        ]),
        obj(vec![("type", Value::Null), ("factor", n(2.0))]),
        obj(vec![("type", Value::Null), ("factor", n(0.0))]),
        obj(vec![("type", Value::Null), ("factor", n(-1.0))]),
        obj(vec![("type", Value::Null)]),
        obj(vec![("type", Value::Null), ("factor", s("1"))]),
        obj(vec![("type", Value::Undefined), ("factor", n(1.0))]),
        obj(vec![("type", s("x")), ("factor", n(1.0))]),
        obj(vec![("type", s("x"))]),
        obj(vec![]),
        obj(vec![
            ("type", s("length")),
            ("from", s("meters")),
            ("to", s("inches")),
            ("factor", n(2.0)),
        ]),
        obj(vec![
            ("type", s("temp")),
            ("from", s("fahrenheit")),
            ("to", s("celsius")),
            ("factor", n(1.0)),
        ]),
        obj(vec![
            ("type", s("temp")),
            ("from", s("kelvin")),
            ("to", s("fahrenheit")),
            ("factor", n(0.5)),
        ]),
        obj(vec![("type", s("length")), ("from", s("meters"))]),
        obj(vec![
            ("type", s("length")),
            ("from", s("meters")),
            ("factor", n(3.0)),
        ]),
        obj(vec![
            ("type", s("length")),
            ("to", s("inches")),
            ("factor", n(3.0)),
        ]),
        obj(vec![
            ("type", s("length")),
            ("from", s("badunit")),
            ("to", s("inches")),
            ("factor", n(1.0)),
        ]),
        obj(vec![
            ("type", s("length")),
            ("from", s("meters")),
            ("to", s("badunit")),
            ("factor", n(1.0)),
        ]),
        obj(vec![
            ("type", s("length")),
            ("from", s("meters")),
            ("to", s("toString")),
            ("factor", n(1.0)),
        ]),
        obj(vec![
            ("type", s("length")),
            ("from", boxed(s("meters"))),
            ("to", boxed(s("inches"))),
            ("factor", n(2.0)),
        ]),
        obj(vec![
            ("type", s("temp")),
            ("from", boxed(s("kelvin"))),
            ("to", boxed(s("kelvin"))),
            ("factor", n(1.0)),
        ]),
        obj(vec![
            ("type", s("temp")),
            ("from", s("kelvin")),
            ("to", boxed(s("kelvin"))),
            ("factor", n(1.0)),
        ]),
        obj(vec![
            ("type", s("temp")),
            ("from", boxed(s("fahrenheit"))),
            ("to", s("celsius")),
            ("factor", n(1.0)),
        ]),
        obj(vec![
            ("type", s("length")),
            ("from", s("meters")),
            ("to", s("inches")),
            ("factor", s("2")),
        ]),
        obj(vec![
            ("type", s("length")),
            ("from", s("meters")),
            ("to", s("inches")),
            ("factor", Value::Bool(true)),
        ]),
        obj(vec![
            ("type", s("length")),
            ("from", s("meters")),
            ("to", s("inches")),
            ("factor", Value::Null),
        ]),
        obj(vec![
            ("type", s("length")),
            ("from", s("meters")),
            ("to", s("inches")),
            ("factor", obj(vec![])),
        ]),
        obj(vec![
            ("type", s("length")),
            ("from", s("meters")),
            ("to", s("inches")),
        ]),
        obj(vec![
            ("type", s("length")),
            ("from", n(5.0)),
            ("to", s("inches")),
            ("factor", n(1.0)),
        ]),
        obj(vec![
            ("type", s("length")),
            ("from", Value::Null),
            ("to", s("inches")),
            ("factor", n(1.0)),
        ]),
        obj(vec![
            ("type", s("length")),
            ("from", s("meters")),
            ("to", Value::Null),
            ("factor", n(1.0)),
        ]),
        obj(vec![
            ("type", s("length")),
            ("from", Value::Undefined),
            ("to", s("inches")),
            ("factor", n(2.0)),
        ]),
    ];
    for preset in &presets {
        for value in [n(5.0), n(-2.5), n(0.0)] {
            spec.push(CONVERT_PRESET, vec![value, preset.clone()]);
        }
    }
    // Non-number values: the noop preset returns the raw value uncoerced.
    for value in [
        s("7"),
        s("x"),
        Value::Null,
        Value::Undefined,
        Value::Bool(true),
        arr(vec![n(1.0)]),
        obj(vec![]),
        boxed(n(4.0)),
    ] {
        for preset in [&presets[0], &presets[2], &presets[11], &presets[26]] {
            spec.push(CONVERT_PRESET, vec![value.clone(), preset.clone()]);
        }
    }
    for preset in [
        Value::Null,
        Value::Undefined,
        n(5.0),
        s("x"),
        Value::Bool(true),
        arr(vec![]),
        boxed(s("preset")),
    ] {
        spec.push(CONVERT_PRESET, vec![n(5.0), preset]);
    }
}

fn suffix_filter_cases(spec: &mut Spec) {
    let mut suffixes: Vec<String> = Vec::new();
    for (_, config) in units::all_units() {
        if !suffixes.iter().any(|x| x == config.suffix) {
            suffixes.push(config.suffix.to_owned());
        }
    }
    for extra in [
        "", "s", "/s", "ps", "²", "³", "x²", "x³", "x/s", "a·s", "m/s²", "m/s³", "°", "/", "µs",
        "V·s", "²²",
    ] {
        suffixes.push(extra.to_owned());
    }
    let filters: Vec<Option<Value>> = vec![
        None,
        Some(n(0.0)),
        Some(n(1.0)),
        Some(n(2.0)),
        Some(n(3.0)),
        Some(n(-1.0)),
        Some(n(0.9999999999)),
        Some(n(1.0000000001)),
        Some(n(f64::NAN)),
        Some(n(f64::INFINITY)),
        Some(n(-0.0)),
        Some(s("1")),
        Some(s("Differentiate")),
        Some(Value::Bool(true)),
        Some(Value::Null),
        Some(boxed(n(1.0))),
    ];
    for suffix in &suffixes {
        for filter in &filters {
            let mut args = vec![s(suffix)];
            if let Some(filter) = filter {
                args.push(filter.clone());
            }
            spec.push(SUFFIX_FILTER, args);
        }
    }
    for suffix in [
        n(5.0),
        Value::Null,
        Value::Undefined,
        Value::Bool(true),
        arr(vec![s("x")]),
        obj(vec![]),
        boxed(s("m/s²")),
        boxed(n(1.0)),
    ] {
        spec.push(SUFFIX_FILTER, vec![suffix.clone(), n(1.0)]);
        spec.push(SUFFIX_FILTER, vec![suffix, n(2.0)]);
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

fn field_cases(spec: &mut Spec) {
    let mut fields: Vec<String> = vec![
        String::new(),
        "x".into(),
        "/".into(),
        "pos".into(),
        "a/b".into(),
        "_".into(),
        " ".into(),
        "x_".into(),
        "x ".into(),
        "/Drive/LeftPositionMeters".into(),
        "Drive/LeftPositionMeters".into(),
        "NT:/SmartDashboard/PoseX".into(),
        "pose.x_meters_per_second".into(),
        "Voltage_V".into(),
        "Current A".into(),
        "tempC".into(),
        "temp_c".into(),
        "SpeedRPM".into(),
        "speed_rpm".into(),
        "xRPM".into(),
        "RPM".into(),
        "rpm".into(),
        "/T:positionMeters/Second".into(),
        "odometry/xMeters".into(),
        "x1Bps".into(),
        "x1bps".into(),
        "fieldK".into(),
        "fieldk".into(),
        "Temp°f".into(),
        "Temp°F".into(),
        "Valueµs".into(),
        "Valueμs".into(),
        "aKelvin".into(),
        "a\u{212a}".into(),
        "field℃".into(),
    ];
    for (key, _) in units::unit_suffix_entries() {
        let k = key.to_string_lossy();
        let cap = capitalize(&k);
        let upper = k.to_uppercase();
        let lower = k.to_lowercase();
        for field in [
            k.clone(),
            format!("pos{k}"),
            format!("pos{cap}"),
            format!("pos{upper}"),
            format!("pos{lower}"),
            format!("x{k}"),
            format!("{k}x"),
            format!("a/{k}"),
            format!("/{k}"),
            format!("a_{k}"),
            format!("a {k}"),
            format!("x1{k}"),
        ] {
            fields.push(field);
        }
    }
    for field in fields {
        spec.push(FIELD_UNIT, vec![s(&field)]);
    }
    for field in [
        n(5.0),
        Value::Null,
        Value::Undefined,
        Value::Bool(true),
        arr(vec![s("posMeters")]),
        obj(vec![]),
        boxed(s("posMeters")),
        boxed(n(1.0)),
    ] {
        spec.push(FIELD_UNIT, vec![field]);
    }
}

/// Direct reads of the upstream tables through `<path>.valueOf` and
/// `names.join`, so factor bits, suffixes and names are compared as stored.
fn table_cases(spec: &mut Spec) {
    let table = |path: String| format!("shared/units:Units.{path}");
    let sep = "\u{0}";
    for (group, group_units) in units::UNIT_GROUPS {
        for (unit, _) in *group_units {
            for field in ["value", "suffix", "pluralizeSuffix"] {
                spec.push(
                    &table(format!("UNIT_GROUPS.{group}.{unit}.{field}.valueOf")),
                    vec![],
                );
            }
            spec.push(
                &table(format!("UNIT_GROUPS.{group}.{unit}.names.join")),
                vec![s(sep)],
            );
        }
    }
    for (unit, _) in units::all_units() {
        spec.push(&table(format!("ALL_UNITS.{unit}.value.valueOf")), vec![]);
        spec.push(&table(format!("GROUP_BY_UNIT.{unit}.valueOf")), vec![]);
    }
    for (ty, children) in units::STRUCT_UNITS {
        for (child, _) in *children {
            spec.push(&table(format!("STRUCT_UNITS.{ty}.{child}.valueOf")), vec![]);
        }
    }
    for (key, _) in units::unit_suffix_entries() {
        let key = key.to_string_lossy();
        spec.push(&table(format!("UNIT_SUFFIXES.{key}.valueOf")), vec![]);
    }
}

#[test]
#[ignore = "rewrites parity/specs/log/units.jsonl; regenerate vectors after with `cargo xtask oracle gen log/units`"]
fn write_units_specs() {
    let mut spec = Spec::new();
    convert_cases(&mut spec);
    preset_cases(&mut spec);
    suffix_filter_cases(&mut spec);
    field_cases(&mut spec);
    table_cases(&mut spec);
    spec.write("log/units");
}
