#![allow(clippy::disallowed_methods)]

#[path = "../../asrs-cli/src/units.rs"]
#[allow(dead_code)]
mod cli_units;
#[path = "../../asrs-cli/src/functions.rs"]
#[allow(dead_code)]
mod functions;
#[path = "../../asrs-cli/src/geometry.rs"]
#[allow(dead_code)]
mod geometry;
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

use asrs_parity::random::Rng;
use asrs_parity::vectors::check_fn_vectors;
use asrs_parity::{FnCase, Request, Response, Value};

fn dispatch(case: &FnCase<'_>) -> Response {
    match functions::lookup(case.name()).or_else(|| {
        shared::FUNCTIONS
            .iter()
            .chain(cli_units::FUNCTIONS)
            .chain(geometry::FUNCTIONS)
            .find(|(n, _)| *n == case.name())
            .map(|(_, f)| *f)
    }) {
        Some(f) => f(&case.request.args),
        None => cli_units::table_read(case.name(), &case.request.args)
            .unwrap_or_else(|| Response::Error(format!("no port of {}", case.name()))),
    }
}

#[test]
fn geometry() {
    check_fn_vectors("log/geometry", dispatch);
}

#[test]
fn geometry_math() {
    check_fn_vectors("log/geometry-math", dispatch);
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

fn obj(items: Vec<(&str, Value)>) -> Value {
    Value::object(items)
}

fn specials() -> Vec<f64> {
    vec![
        0.0,
        -0.0,
        1.0,
        -1.0,
        0.5,
        -0.5,
        2.0,
        -2.0,
        3.0,
        std::f64::consts::PI,
        -std::f64::consts::PI,
        std::f64::consts::FRAC_PI_2,
        0.1,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        f64::from_bits(0x7ff8_0000_0000_0123),
        f64::from_bits(0xfff8_0000_0000_0456),
        f64::from_bits(0x7ff4_0000_0000_0001),
        f64::from_bits(0xfff4_0000_0000_0002),
        f64::MIN_POSITIVE,
        5e-324,
        f64::MAX,
        -f64::MAX,
        1e-300,
        -1e-300,
        1e300,
        -1e300,
        1e21,
        -1e21,
        1e-7,
        123.456,
        -0.75,
    ]
}

fn doubles(rng: &mut Rng, n_: usize) -> Vec<f64> {
    let mut out = specials();
    out.extend((0..n_).map(|_| {
        let exponent = rng.below(2100) as i64 - 53;
        let exponent = exponent.clamp(0, 2046) as u64;
        let sign = rng.below(2) << 63;
        f64::from_bits(sign | exponent << 52 | rng.next_u64() >> 12)
    }));
    out
}

fn pose2(x: f64, y: f64, r: f64) -> Value {
    obj(vec![
        ("translation", arr(vec![n(x), n(y)])),
        ("rotation", n(r)),
    ])
}

fn pose3(x: f64, y: f64, z: f64, w: f64, qx: f64, qy: f64, qz: f64) -> Value {
    obj(vec![
        ("translation", arr(vec![n(x), n(y), n(z)])),
        ("rotation", arr(vec![n(w), n(qx), n(qy), n(qz)])),
    ])
}

fn annotations() -> Vec<Value> {
    vec![
        obj(vec![("is2DSource", Value::Bool(true))]),
        obj(vec![("is2DSource", Value::Bool(false))]),
        obj(vec![
            ("aprilTagId", n(7.0)),
            ("is2DSource", Value::Bool(false)),
        ]),
        obj(vec![
            ("aprilTagId", Value::Undefined),
            ("is2DSource", Value::Bool(false)),
        ]),
        obj(vec![
            ("is2DSource", Value::Bool(true)),
            ("visionColor", s("#ff0000")),
            ("visionSize", s("large")),
        ]),
        obj(vec![]),
    ]
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

fn geometry_spec() -> Spec {
    let mut rng = Rng::new(0x1236);
    let mut spec = Spec::new();
    let xs = doubles(&mut rng, 150);
    for &x in &xs {
        for &y in &xs[..40] {
            spec.push(
                "shared/geometry:translation2dTo3d",
                vec![arr(vec![n(x), n(y)])],
            );
            spec.push(
                "shared/geometry:translation3dTo2d",
                vec![arr(vec![n(x), n(y), n(xs[2])])],
            );
            spec.push(
                "shared/geometry:rotation3dTo2d",
                vec![arr(vec![n(x), n(y), n(xs[2]), n(xs[3])])],
            );
            spec.push(
                "shared/geometry:rotation3dToRPY",
                vec![arr(vec![n(x), n(y), n(xs[2]), n(xs[3])])],
            );
            spec.push("shared/geometry:pose2dTo3d", vec![pose2(x, y, xs[4])]);
            spec.push(
                "shared/geometry:pose3dTo2d",
                vec![pose3(x, y, xs[4], xs[5], xs[6], xs[7], xs[8])],
            );
        }
        spec.push("shared/geometry:rotation2dTo3d", vec![n(x)]);
    }
    // Normalized quaternions plus near-degenerate cy_sq cases for RPY.
    for _ in 0..300 {
        let v: Vec<f64> = (0..4)
            .map(|_| rng.below(2000) as f64 / 1000.0 - 1.0)
            .collect();
        let norm = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2] + v[3] * v[3]).sqrt();
        let q: Vec<Value> = if norm > 0.0 {
            v.iter().map(|c| n(c / norm)).collect()
        } else {
            v.iter().map(|&c| n(c)).collect()
        };
        spec.push("shared/geometry:rotation3dTo2d", vec![arr(q.clone())]);
        spec.push("shared/geometry:rotation3dToRPY", vec![arr(q)]);
    }
    for axis in ["x", "y", "z", "X", "q", ""] {
        for &d in &xs[..24] {
            spec.push(
                "oracle:geometryQuat",
                vec![arr(vec![obj(vec![("axis", s(axis)), ("degrees", n(d))])])],
            );
        }
    }
    for seq in [
        vec![("x", 90.0), ("y", 90.0)],
        vec![("z", -90.0), ("x", 45.0), ("y", 30.0)],
        vec![("q", 1e300)],
        vec![("x", f64::NAN)],
        vec![("y", f64::INFINITY)],
        vec![("x", -0.0)],
    ] {
        spec.push(
            "oracle:geometryQuat",
            vec![arr(seq
                .iter()
                .map(|(a, d)| obj(vec![("axis", s(a)), ("degrees", n(*d))]))
                .collect())],
        );
    }
    spec.push("oracle:geometryQuat", vec![arr(vec![])]);
    spec.push(
        "oracle:geometryQuat",
        vec![arr(vec![obj(vec![
            ("axis", Value::Null),
            ("degrees", n(90.0)),
        ])])],
    );
    spec.push(
        "oracle:geometryQuat",
        vec![arr(vec![obj(vec![("degrees", n(45.0))])])],
    );

    for annotation in annotations() {
        spec.push(
            "shared/geometry:annotatedPose2dTo3d",
            vec![obj(vec![
                ("pose", pose2(1.5, -2.5, 0.75)),
                ("annotation", annotation.clone()),
            ])],
        );
        spec.push(
            "shared/geometry:annotatedPose3dTo2d",
            vec![obj(vec![
                ("pose", pose3(1.0, 2.0, 3.0, 1.0, 0.0, 0.0, 0.0)),
                ("annotation", annotation),
            ])],
        );
    }

    let systems: Vec<Value> = vec![
        s("wall-alliance"),
        s("wall-blue"),
        s("center-rotated"),
        s("center-red"),
        s("other"),
        s(""),
        n(3.0),
        Value::Null,
        Value::Undefined,
    ];
    let alliances: Vec<Value> = vec![
        s("red"),
        s("blue"),
        s("green"),
        s(""),
        n(1.0),
        Value::Null,
        Value::Undefined,
    ];
    let dims = [0.0, 16.541, 8.211, -4.0, f64::NAN, f64::INFINITY, 1e300];
    for sys in &systems {
        for alliance in &alliances {
            for &fl in &dims[..4] {
                for &fw in &dims[..3] {
                    spec.push(
                        "shared/geometry:convertFromCoordinateSystem",
                        vec![
                            pose2(1.0, -2.0, 0.5),
                            sys.clone(),
                            alliance.clone(),
                            n(fl),
                            n(fw),
                        ],
                    );
                }
            }
        }
    }
    // Pose shapes: annotated, arrays, nested arrays and mixed content.
    let ann_pose = obj(vec![
        ("pose", pose2(3.0, 4.0, -1.25)),
        (
            "annotation",
            obj(vec![
                ("aprilTagId", n(2.0)),
                ("is2DSource", Value::Bool(true)),
            ]),
        ),
    ]);
    for sys in [
        &systems[0],
        &systems[1],
        &systems[2],
        &systems[3],
        &systems[6],
    ] {
        for alliance in [&alliances[0], &alliances[1], &alliances[2], &alliances[6]] {
            for pose in [
                ann_pose.clone(),
                arr(vec![pose2(1.0, 2.0, 0.1), pose2(-3.0, 4.0, 2.2)]),
                arr(vec![ann_pose.clone(), pose2(0.0, 0.0, 0.0)]),
                arr(vec![arr(vec![pose2(5.0, 6.0, 7.0)])]),
                arr(vec![]),
                pose2(f64::NAN, f64::INFINITY, -1e300),
            ] {
                spec.push(
                    "shared/geometry:convertFromCoordinateSystem",
                    vec![pose, sys.clone(), alliance.clone(), n(16.541), n(8.211)],
                );
            }
        }
    }
    // Non-object poses take upstream's `in` TypeError or leaf reads.
    for pose in [
        n(5.0),
        s("x"),
        Value::Null,
        Value::Undefined,
        Value::Bool(true),
    ] {
        spec.push(
            "shared/geometry:convertFromCoordinateSystem",
            vec![pose, s("wall-blue"), s("red"), n(16.0), n(8.0)],
        );
    }
    spec
}

#[test]
#[ignore = "rewrites parity/specs/log/geometry-math.jsonl; regenerate vectors after with `cargo xtask oracle gen log/geometry-math`"]
fn write_geometry_specs() {
    geometry_spec().write("log/geometry-math");
}
