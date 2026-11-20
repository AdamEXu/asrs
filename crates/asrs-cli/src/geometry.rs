//! `fn` names for `shared/geometry.ts` (conversions and `grab*` readers) plus
//! the `oracle:geometryRun`/`oracle:geometryQuat` helpers, backed by
//! `asrs_log::geometry`.

use asrs_js::number;
use asrs_js::string::JsString;
use asrs_log::geometry::{
    self, Alliance, AnnotatedPose2d, AnnotatedPose3d, Config3dRotation, CoordinateSystem,
    GeometryError, HeatmapTimeRange, ModuleVelocitySample, Pose2d, Pose3d, PoseAnnotations,
    RotationUnits,
};
use asrs_log::log::Log;
use asrs_parity::{Response, Utf16String, Value};

use crate::functions::{arg, quiet_nan, to_number, to_units, type_error};
use crate::log_field::{boolean, js_string};
use crate::log_run;
use crate::shared::{Function, get_prop, strict_equal};

pub const FUNCTIONS: &[(&str, Function)] = &[
    ("shared/geometry:translation2dTo3d", |a| {
        respond(|| {
            Ok(numbers(&geometry::translation_2d_to_3d(translation2(
                &arg(a, 0),
            )?)))
        })
    }),
    ("shared/geometry:translation3dTo2d", |a| {
        respond(|| {
            Ok(numbers(&geometry::translation_3d_to_2d(translation3(
                &arg(a, 0),
            )?)))
        })
    }),
    ("shared/geometry:rotation2dTo3d", |a| {
        respond(|| Ok(numbers(&geometry::rotation_2d_to_3d(num(&arg(a, 0))?))))
    }),
    ("shared/geometry:rotation3dTo2d", |a| {
        respond(|| Ok(n(geometry::rotation_3d_to_2d(rotation3(&arg(a, 0))?))))
    }),
    ("shared/geometry:rotation3dToRPY", |a| {
        respond(|| {
            Ok(numbers(&geometry::rotation_3d_to_rpy(rotation3(&arg(
                a, 0,
            ))?)))
        })
    }),
    ("shared/geometry:pose2dTo3d", |a| {
        respond(|| Ok(pose3_value(&geometry::pose_2d_to_3d(pose2_of(&arg(a, 0))?))))
    }),
    ("shared/geometry:pose3dTo2d", |a| {
        respond(|| Ok(pose2_value(&geometry::pose_3d_to_2d(pose3_of(&arg(a, 0))?))))
    }),
    ("shared/geometry:annotatedPose2dTo3d", |a| {
        respond(|| {
            let v = &arg(a, 0);
            let p = geometry::annotated_pose_2d_to_3d(annotated2_of(v)?);
            Ok(Value::object([
                ("pose", pose3_value(&p.pose)),
                ("annotation", prop(v, "annotation")?),
            ]))
        })
    }),
    ("shared/geometry:annotatedPose3dTo2d", |a| {
        respond(|| {
            let v = &arg(a, 0);
            let p = geometry::annotated_pose_3d_to_2d(annotated3_of(v)?);
            Ok(Value::object([
                ("pose", pose2_value(&p.pose)),
                ("annotation", prop(v, "annotation")?),
            ]))
        })
    }),
    (
        "shared/geometry:convertFromCoordinateSystem",
        convert_pose_fn,
    ),
    ("oracle:geometryQuat", quat_fn),
    ("oracle:geometryRun", geometry_run),
];

fn respond(f: impl FnOnce() -> Result<Value, Response>) -> Response {
    match f() {
        Ok(v) => Response::Ok(v),
        Err(r) => r,
    }
}

fn num(v: &Value) -> Result<f64, Response> {
    to_number(v).map(quiet_nan)
}

fn n(x: f64) -> Value {
    Value::number(quiet_nan(x))
}

fn numbers(xs: &[f64]) -> Value {
    Value::Array(xs.iter().map(|&x| n(x)).collect())
}

fn string(s: &JsString) -> Value {
    Value::String(Utf16String::from_units(s.units().to_vec()))
}

/// `input[name]`; upstream reads properties directly, which throws on null
/// and undefined receivers.
fn prop(v: &Value, name: &str) -> Result<Value, Response> {
    get_prop(v, &name.encode_utf16().collect::<Vec<u16>>())
}

/// `input[i]` for a whole-number index.
fn elem(v: &Value, i: u32) -> Result<Value, Response> {
    get_prop(
        v,
        &number::to_string(f64::from(i))
            .encode_utf16()
            .collect::<Vec<u16>>(),
    )
}

/// An element read for the typed port: it only accepts `f64` inputs, so
/// `undefined` and other non-numbers coerce through `ToNumber`.
fn elem_number(v: &Value, i: u32) -> Result<f64, Response> {
    num(&elem(v, i)?)
}

fn translation2(v: &Value) -> Result<[f64; 2], Response> {
    Ok([elem_number(v, 0)?, elem_number(v, 1)?])
}

fn translation3(v: &Value) -> Result<[f64; 3], Response> {
    Ok([elem_number(v, 0)?, elem_number(v, 1)?, elem_number(v, 2)?])
}

fn rotation3(v: &Value) -> Result<[f64; 4], Response> {
    Ok([
        elem_number(v, 0)?,
        elem_number(v, 1)?,
        elem_number(v, 2)?,
        elem_number(v, 3)?,
    ])
}

fn pose2_of(v: &Value) -> Result<Pose2d, Response> {
    Ok(Pose2d {
        translation: translation2(&prop(v, "translation")?)?,
        rotation: num(&prop(v, "rotation")?)?,
    })
}

fn pose3_of(v: &Value) -> Result<Pose3d, Response> {
    Ok(Pose3d {
        translation: translation3(&prop(v, "translation")?)?,
        rotation: rotation3(&prop(v, "rotation")?)?,
    })
}

fn pose2_value(p: &Pose2d) -> Value {
    Value::object([
        ("translation", numbers(&p.translation)),
        ("rotation", n(p.rotation)),
    ])
}

fn pose3_value(p: &Pose3d) -> Value {
    Value::object([
        ("translation", numbers(&p.translation)),
        ("rotation", numbers(&p.rotation)),
    ])
}

fn annotation_value(a: &PoseAnnotations) -> Value {
    let mut pairs: Vec<(Utf16String, Value)> = Vec::new();
    if a.has_april_tag_id {
        pairs.push((
            "aprilTagId".into(),
            a.april_tag_id.map_or(Value::Undefined, n),
        ));
    }
    pairs.push(("is2DSource".into(), Value::Bool(a.is_2d_source)));
    if let Some(c) = &a.vision_color {
        pairs.push(("visionColor".into(), string(c)));
    }
    if let Some(s) = &a.vision_size {
        pairs.push(("visionSize".into(), string(s)));
    }
    Value::Object(pairs)
}

fn annotation_of(v: &Value) -> Result<PoseAnnotations, Response> {
    let mut a = PoseAnnotations::default();
    if let Some(x) = v.get("is2DSource") {
        a.is_2d_source = match x {
            Value::Bool(b) => *b,
            other => num(other)? != 0.0,
        };
    }
    if let Some(x) = v.get("aprilTagId") {
        a.has_april_tag_id = true;
        a.april_tag_id = match x {
            Value::Undefined => None,
            other => Some(num(other)?),
        };
    }
    for (name, slot) in [
        ("visionColor", &mut a.vision_color),
        ("visionSize", &mut a.vision_size),
    ] {
        if let Some(x) = v.get(name)
            && !matches!(x, Value::Undefined)
        {
            *slot = Some(JsString::from_units(to_units(x)?));
        }
    }
    Ok(a)
}

fn annotated2_of(v: &Value) -> Result<AnnotatedPose2d, Response> {
    Ok(AnnotatedPose2d {
        pose: pose2_of(&prop(v, "pose")?)?,
        annotation: annotation_of(&prop(v, "annotation")?)?,
    })
}

fn annotated3_of(v: &Value) -> Result<AnnotatedPose3d, Response> {
    Ok(AnnotatedPose3d {
        pose: pose3_of(&prop(v, "pose")?)?,
        annotation: annotation_of(&prop(v, "annotation")?)?,
    })
}

fn annotated3_value(p: &AnnotatedPose3d) -> Value {
    Value::object([
        ("pose", pose3_value(&p.pose)),
        ("annotation", annotation_value(&p.annotation)),
    ])
}

fn coordinate_system(v: &Value) -> CoordinateSystem {
    match v.as_str() {
        Some(s) => CoordinateSystem::parse(&JsString::from_units(s.units().to_vec())),
        // Non-strings take upstream's default branch unchanged.
        _ => CoordinateSystem::Other(JsString::from("")),
    }
}

fn alliance_of(v: &Value) -> Option<Alliance> {
    if strict_equal(v, &Value::string("red")) {
        Some(Alliance::Red)
    } else if strict_equal(v, &Value::string("blue")) {
        Some(Alliance::Blue)
    } else {
        None
    }
}

fn convert_pose_fn(args: &[Value]) -> Response {
    let source = coordinate_system(&arg(args, 1));
    let alliance = alliance_of(&arg(args, 2));
    let (length, width) = match (num(&arg(args, 3)), num(&arg(args, 4))) {
        (Ok(l), Ok(w)) => (l, w),
        (Err(r), _) | (_, Err(r)) => return r,
    };
    respond(|| convert_pose_value(&arg(args, 0), &source, alliance, length, width))
}

/// `convertFromCoordinateSystem` dispatch: arrays recurse, objects with an
/// `annotation` property map `pose`, everything else is a `Pose2d` leaf. A
/// wall-alliance source with an alliance that is not "red" or "blue" falls
/// out of upstream's inner switch into the wall-blue body.
fn convert_pose_value(
    v: &Value,
    source: &CoordinateSystem,
    alliance: Option<Alliance>,
    length: f64,
    width: f64,
) -> Result<Value, Response> {
    if let Value::Array(items) = v {
        return items
            .iter()
            .map(|x| convert_pose_value(x, source, alliance, length, width))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array);
    }
    match v {
        Value::Object(_) | Value::Instance(..) => {}
        _ => {
            let text = to_units(v)
                .map(|u| String::from_utf16_lossy(&u))
                .unwrap_or_else(|_| "[object Object]".to_owned());
            return Err(type_error(format!(
                "Cannot use 'in' operator to search for 'annotation' in {text}"
            )));
        }
    }
    if v.get("annotation").is_some() {
        let pose = convert_pose_leaf(&prop(v, "pose")?, source, alliance, length, width)?;
        return Ok(Value::object([
            ("pose", pose),
            ("annotation", prop(v, "annotation")?),
        ]));
    }
    convert_pose_leaf(v, source, alliance, length, width)
}

fn convert_pose_leaf(
    v: &Value,
    source: &CoordinateSystem,
    alliance: Option<Alliance>,
    length: f64,
    width: f64,
) -> Result<Value, Response> {
    let p = leaf_pose2(v, source)?;
    Ok(pose2_value(&geometry::convert_from_coordinate_system(
        p,
        source,
        alliance.unwrap_or(Alliance::Blue),
        length,
        width,
    )))
}

/// Reads a `Pose2d` leaf, evaluating property reads and coercions in the
/// order the source branch does so an error surfaces on the same operand.
fn leaf_pose2(v: &Value, source: &CoordinateSystem) -> Result<Pose2d, Response> {
    let translation = prop(v, "translation")?;
    let rotation = prop(v, "rotation")?;
    match source {
        CoordinateSystem::CenterRotated => {
            let y = elem_number(&translation, 1)?;
            let x = elem_number(&translation, 0)?;
            Ok(Pose2d {
                translation: [x, y],
                rotation: num(&rotation)?,
            })
        }
        _ => Ok(Pose2d {
            translation: [elem_number(&translation, 0)?, elem_number(&translation, 1)?],
            rotation: num(&rotation)?,
        }),
    }
}

fn quat_fn(args: &[Value]) -> Response {
    let mut rotations = Vec::new();
    for v in arg(args, 0).as_array().unwrap_or(&[]) {
        let axis = prop(v, "axis")
            .ok()
            .and_then(|x| js_string(&x).ok())
            .unwrap_or_else(|| JsString::from(""));
        let degrees = prop(v, "degrees")
            .ok()
            .and_then(|x| num(&x).ok())
            .unwrap_or(f64::NAN);
        rotations.push(Config3dRotation { axis, degrees });
    }
    let q = geometry::rotation_sequence_to_quaternion(&rotations);
    Response::Ok(Value::object([
        ("x", n(q.x)),
        ("y", n(q.y)),
        ("z", n(q.z)),
        ("w", n(q.w)),
    ]))
}

fn uuid_arg(v: &Value) -> Result<Option<String>, String> {
    match v {
        Value::Undefined | Value::Null => Ok(None),
        other => js_string(other).map(|s| Some(s.to_string_lossy())),
    }
}

fn rotation_units(v: &Value) -> RotationUnits {
    if strict_equal(v, &Value::string("degrees")) {
        RotationUnits::Degrees
    } else {
        RotationUnits::Radians
    }
}

fn heatmap_range(v: &Value) -> HeatmapTimeRange {
    if strict_equal(v, &Value::string("auto")) {
        HeatmapTimeRange::Auto
    } else if strict_equal(v, &Value::string("teleop")) {
        HeatmapTimeRange::Teleop
    } else if strict_equal(v, &Value::string("teleop-no-endgame")) {
        HeatmapTimeRange::TeleopNoEndgame
    } else if strict_equal(v, &Value::string("full")) {
        HeatmapTimeRange::Full
    } else if strict_equal(v, &Value::string("visible")) {
        HeatmapTimeRange::Visible
    } else {
        HeatmapTimeRange::Enabled
    }
}

fn timeline_range(v: &Value) -> Option<[f64; 2]> {
    match v {
        Value::Array(items) if items.len() >= 2 => Some([
            items[0].as_f64().unwrap_or(f64::NAN),
            items[1].as_f64().unwrap_or(f64::NAN),
        ]),
        _ => None,
    }
}

fn annotated_poses(poses: &[AnnotatedPose3d]) -> Value {
    Value::Array(poses.iter().map(annotated3_value).collect())
}

fn module_velocities(vs: &[ModuleVelocitySample]) -> Value {
    Value::Array(
        vs.iter()
            .map(|s| match s {
                ModuleVelocitySample::Full(m) => {
                    Value::object([("velocity", n(m.velocity)), ("angle", n(m.angle))])
                }
                ModuleVelocitySample::Partial { velocity, angle } => Value::object([
                    ("velocity", velocity.map_or(Value::Null, n)),
                    ("angle", angle.map_or(Value::Null, n)),
                ]),
                ModuleVelocitySample::Undefined => Value::Undefined,
            })
            .collect(),
    )
}

fn throw(e: GeometryError) -> Value {
    Value::object([("throw", Value::Error(e.name().into(), e.message().into()))])
}

fn grab_call(log: &Log, call: &[Value]) -> Result<Value, Response> {
    let name = js_string(&arg(call, 0))
        .map(|s| s.to_string_lossy())
        .map_err(Response::Error)?;
    let key = || js_string(&arg(call, 1)).map_err(Response::Error);
    let ts = |i: usize| num(&arg(call, i));
    let uuid = |i: usize| uuid_arg(&arg(call, i)).map_err(Response::Error);
    fn ok(v: Value) -> Result<Value, Response> {
        Ok(Value::object([("ok", v)]))
    }
    macro_rules! poses {
        ($e:expr) => {
            match $e {
                Ok(p) => ok(annotated_poses(&p)),
                Err(g) => Ok(throw(g)),
            }
        };
    }
    match name.as_str() {
        "grabPosesAuto" => poses!(geometry::grab_poses_auto(
            log,
            &key()?,
            &js_string(&arg(call, 2)).map_err(Response::Error)?,
            ts(3)?,
            uuid(4)?.as_deref(),
            rotation_units(&arg(call, 5)),
        )),
        "grabNumberRotation" => poses!(geometry::grab_number_rotation(
            log,
            &key()?,
            ts(2)?,
            rotation_units(&arg(call, 3)),
            uuid(4)?.as_deref(),
        )),
        "grabRotation2d" => poses!(geometry::grab_rotation_2d(
            log,
            &key()?,
            ts(2)?,
            uuid(3)?.as_deref()
        )),
        "grabRotation3d" => poses!(geometry::grab_rotation_3d(
            log,
            &key()?,
            ts(2)?,
            uuid(3)?.as_deref()
        )),
        "grabRotation2dArray" => poses!(geometry::grab_rotation_2d_array(
            log,
            &key()?,
            ts(2)?,
            uuid(3)?.as_deref()
        )),
        "grabRotation3dArray" => poses!(geometry::grab_rotation_3d_array(
            log,
            &key()?,
            ts(2)?,
            uuid(3)?.as_deref()
        )),
        "grabTargetCorner" => poses!(geometry::grab_target_corner(
            log,
            &key()?,
            ts(2)?,
            uuid(3)?.as_deref()
        )),
        "grabTargetCornerArray" => poses!(geometry::grab_target_corner_array(
            log,
            &key()?,
            ts(2)?,
            uuid(3)?.as_deref()
        )),
        "grabTranslation2d" => poses!(geometry::grab_translation_2d(
            log,
            &key()?,
            ts(2)?,
            uuid(3)?.as_deref()
        )),
        "grabTranslation3d" => poses!(geometry::grab_translation_3d(
            log,
            &key()?,
            ts(2)?,
            uuid(3)?.as_deref()
        )),
        "grabTranslation2dArray" => poses!(geometry::grab_translation_2d_array(
            log,
            &key()?,
            ts(2)?,
            uuid(3)?.as_deref()
        )),
        "grabTranslation3dArray" => poses!(geometry::grab_translation_3d_array(
            log,
            &key()?,
            ts(2)?,
            uuid(3)?.as_deref()
        )),
        "grabPose2d" => poses!(geometry::grab_pose_2d(
            log,
            &key()?,
            ts(2)?,
            uuid(3)?.as_deref()
        )),
        "grabPose3d" => poses!(geometry::grab_pose_3d(
            log,
            &key()?,
            ts(2)?,
            uuid(3)?.as_deref()
        )),
        "grabPose2dArray" => poses!(geometry::grab_pose_2d_array(
            log,
            &key()?,
            ts(2)?,
            uuid(3)?.as_deref()
        )),
        "grabPose3dArray" => poses!(geometry::grab_pose_3d_array(
            log,
            &key()?,
            ts(2)?,
            uuid(3)?.as_deref()
        )),
        "grabChoreoSample" => poses!(geometry::grab_choreo_sample(
            log,
            &key()?,
            ts(2)?,
            uuid(3)?.as_deref()
        )),
        "grabChoreoSampleArray" => poses!(geometry::grab_choreo_sample_array(
            log,
            &key()?,
            ts(2)?,
            uuid(3)?.as_deref()
        )),
        "grabTrajectory" => poses!(geometry::grab_trajectory(
            log,
            &key()?,
            ts(2)?,
            uuid(3)?.as_deref()
        )),
        "grabAprilTag" => poses!(geometry::grab_april_tag(
            log,
            &key()?,
            ts(2)?,
            uuid(3)?.as_deref()
        )),
        "grabAprilTagArray" => poses!(geometry::grab_april_tag_array(
            log,
            &key()?,
            ts(2)?,
            uuid(3)?.as_deref()
        )),
        "grabHeatmapData" => poses!(geometry::grab_heatmap_data(
            log,
            &key()?,
            &js_string(&arg(call, 2)).map_err(Response::Error)?,
            heatmap_range(&arg(call, 3)),
            timeline_range(&arg(call, 4)),
            uuid(5)?.as_deref(),
        )),
        "grabModuleVelocities" => {
            let arrangement = match &arg(call, 3) {
                Value::Undefined => None,
                v => Some(js_string(v).map_err(Response::Error)?),
            };
            ok(module_velocities(&geometry::grab_module_velocities(
                log,
                &key()?,
                ts(2)?,
                arrangement.as_ref(),
                uuid(4)?.as_deref(),
            )))
        }
        "grabRobotVelocities" => {
            let v = geometry::grab_robot_velocities(log, &key()?, ts(2)?, uuid(3)?.as_deref());
            ok(Value::object([
                ("vx", n(v.vx)),
                ("vy", n(v.vy)),
                ("omega", n(v.omega)),
            ]))
        }
        other => Ok(Value::object([(
            "throw",
            Value::Error(
                "Error".into(),
                format!("geometryRun: unknown call {other:?}").into(),
            ),
        )])),
    }
}

fn geometry_run(args: &[Value]) -> Response {
    let cache = match boolean(&arg(args, 0)) {
        Ok(c) => c,
        Err(e) => return Response::Error(e),
    };
    let ops = match arg(args, 1).as_array() {
        Some(ops) => ops.to_vec(),
        None => return Response::Error("geometryRun: ops must be an array".to_owned()),
    };
    let calls = match arg(args, 2).as_array() {
        Some(calls) => calls.to_vec(),
        None => return Response::Error("geometryRun: calls must be an array".to_owned()),
    };
    let (log, _) = match log_run::build_log(cache, &ops) {
        Ok(built) => built,
        Err(e) => return Response::Error(e),
    };
    let mut out = Vec::new();
    for call in &calls {
        let result = match call.as_array() {
            Some(call) => grab_call(&log, call),
            None => Ok(Value::object([(
                "throw",
                Value::Error(
                    "Error".into(),
                    "geometryRun: a call must be an array".into(),
                ),
            )])),
        };
        out.push(match result {
            Ok(v) => v,
            Err(r) => return r,
        });
    }
    Response::Ok(Value::Array(out))
}
