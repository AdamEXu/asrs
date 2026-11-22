//! `Field3dRendererCommand` as `Field3dController` serializes it, read back into the managers'
//! object types.

use asrs_js::json;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::JsValue;
use asrs_log::geometry::{
    AnnotatedPose3d, ModuleVelocity, ModuleVelocitySample, Pose3d, PoseAnnotations,
};
use asrs_log::log::mechanism_state::{MechanismLine, MechanismState};

use crate::managers::{
    AprilTagObj, AprilTagVariant, AxesObj, ConeObj, GamePieceObj, HeatmapObj, RobotKind,
    RobotMechanisms, RobotObj, SwerveModuleSet, TrajectoryObj,
};

/// `Field3dRendererCommand_AnyObj`.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum AnyObj {
    Robot(RobotObj),
    GamePiece(GamePieceObj),
    Trajectory(TrajectoryObj),
    Heatmap(HeatmapObj),
    AprilTag(AprilTagObj),
    AprilTagBuiltIn(AprilTagObj),
    Axes(AxesObj),
    Cone(ConeObj),
}

/// `Field3dRendererCommand_AnyObj["type"]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjType {
    Robot,
    Ghost,
    GamePiece,
    Trajectory,
    Heatmap,
    AprilTag,
    AprilTagBuiltIn,
    Axes,
    Cone,
}

impl AnyObj {
    pub fn obj_type(&self) -> ObjType {
        match self {
            AnyObj::Robot(robot) => match robot.kind {
                RobotKind::Robot => ObjType::Robot,
                RobotKind::Ghost { .. } => ObjType::Ghost,
            },
            AnyObj::GamePiece(_) => ObjType::GamePiece,
            AnyObj::Trajectory(_) => ObjType::Trajectory,
            AnyObj::Heatmap(_) => ObjType::Heatmap,
            AnyObj::AprilTag(_) => ObjType::AprilTag,
            AnyObj::AprilTagBuiltIn(_) => ObjType::AprilTagBuiltIn,
            AnyObj::Axes(_) => ObjType::Axes,
            AnyObj::Cone(_) => ObjType::Cone,
        }
    }
}

/// `cameraOverride`: `null`, a pose, or `undefined` where the controller read `poses[0]` of an
/// empty list (reading `.pose` from it then throws).
#[derive(Clone, Debug, PartialEq)]
pub enum CameraOverride {
    Null,
    Undefined,
    Pose(AnnotatedPose3d),
}

/// `Field3dRendererCommand`, plus its `JSON.stringify` text, which the renderer compares from
/// frame to frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Field3dRendererCommand {
    pub field: JsString,
    pub is_red_alliance: bool,
    pub coordinate_system: JsString,
    pub objects: Vec<AnyObj>,
    pub camera_override: CameraOverride,
    pub auto_driver_station: f64,
    pub all_robot_models: Vec<JsString>,
    pub json: String,
}

fn unmodeled(what: &str) -> JsError {
    JsError::new("Unmodeled", format!("Field3dRendererCommand: {what}"))
}

fn get<'a>(value: &'a JsValue, key: &str) -> Result<&'a JsValue, JsError> {
    value
        .get(key)
        .ok_or_else(|| unmodeled(&format!("missing {key}")))
}

fn string(value: &JsValue, key: &str) -> Result<JsString, JsError> {
    get(value, key)?
        .as_str()
        .cloned()
        .ok_or_else(|| unmodeled(&format!("{key} is not a string")))
}

fn number(value: &JsValue) -> Result<f64, JsError> {
    value.as_f64().ok_or_else(|| unmodeled("expected a number"))
}

fn boolean(value: &JsValue, key: &str) -> Result<bool, JsError> {
    get(value, key)?
        .as_bool()
        .ok_or_else(|| unmodeled(&format!("{key} is not a boolean")))
}

fn array<'a>(value: &'a JsValue, key: &str) -> Result<&'a [JsValue], JsError> {
    get(value, key)?
        .as_array()
        .ok_or_else(|| unmodeled(&format!("{key} is not an array")))
}

fn numbers<const N: usize>(value: &JsValue, key: &str) -> Result<[f64; N], JsError> {
    let items = array(value, key)?;
    if items.len() != N {
        return Err(unmodeled(&format!("{key} has {} elements", items.len())));
    }
    let mut out = [0.0; N];
    for (slot, item) in out.iter_mut().zip(items) {
        *slot = number(item)?;
    }
    Ok(out)
}

fn optional_string(value: &JsValue, key: &str) -> Result<Option<JsString>, JsError> {
    match value.get(key) {
        None | Some(JsValue::Undefined) => Ok(None),
        Some(JsValue::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(unmodeled(&format!("{key} is not a string"))),
    }
}

/// `null` (a non-finite number after `jsonCopy`) reads as `None`.
fn nullable_number(value: &JsValue) -> Result<Option<f64>, JsError> {
    match value {
        JsValue::Null => Ok(None),
        other => number(other).map(Some),
    }
}

pub fn annotated_pose(value: &JsValue) -> Result<AnnotatedPose3d, JsError> {
    let pose = get(value, "pose")?;
    let annotation = get(value, "annotation")?;
    let april_tag_id = annotation.get("aprilTagId");
    Ok(AnnotatedPose3d {
        pose: Pose3d {
            translation: numbers(pose, "translation")?,
            rotation: numbers(pose, "rotation")?,
        },
        annotation: PoseAnnotations {
            is_2d_source: boolean(annotation, "is2DSource")?,
            has_april_tag_id: april_tag_id.is_some(),
            april_tag_id: match april_tag_id {
                None | Some(JsValue::Undefined) => None,
                Some(id) => Some(number(id)?),
            },
            vision_color: optional_string(annotation, "visionColor")?,
            vision_size: optional_string(annotation, "visionSize")?,
        },
    })
}

fn poses(value: &JsValue, key: &str) -> Result<Vec<AnnotatedPose3d>, JsError> {
    array(value, key)?.iter().map(annotated_pose).collect()
}

fn mechanism(value: &JsValue) -> Result<Option<MechanismState>, JsError> {
    if value.is_null() {
        return Ok(None);
    }
    let coordinates = |line: &JsValue, key: &str| -> Result<Vec<Option<f64>>, JsError> {
        array(line, key)?.iter().map(nullable_number).collect()
    };
    let lines = array(value, "lines")?
        .iter()
        .map(|line| {
            let color = match get(line, "color")? {
                JsValue::String(s) => Some(s.clone()),
                JsValue::Number(x) if *x == 0.0 => None,
                _ => return Err(unmodeled("mechanism line color")),
            };
            Ok(MechanismLine {
                start: coordinates(line, "start")?,
                end: coordinates(line, "end")?,
                color,
                weight: nullable_number(get(line, "weight")?)?,
            })
        })
        .collect::<Result<_, JsError>>()?;
    Ok(Some(MechanismState {
        background_color: string(value, "backgroundColor")?,
        dimensions: array(value, "dimensions")?
            .iter()
            .map(number)
            .collect::<Result<_, _>>()?,
        lines,
    }))
}

fn module_sample(value: &JsValue) -> Result<ModuleVelocitySample, JsError> {
    if value.is_undefined() {
        return Ok(ModuleVelocitySample::Undefined);
    }
    let velocity = nullable_number(get(value, "velocity")?)?;
    let angle = nullable_number(get(value, "angle")?)?;
    Ok(match (velocity, angle) {
        (Some(velocity), Some(angle)) => {
            ModuleVelocitySample::Full(ModuleVelocity { velocity, angle })
        }
        (velocity, angle) => ModuleVelocitySample::Partial { velocity, angle },
    })
}

fn robot(value: &JsValue, kind: RobotKind) -> Result<RobotObj, JsError> {
    let mechanisms = get(value, "mechanisms")?;
    let swerve_module_velocities = array(value, "swerveModuleVelocities")?
        .iter()
        .map(|set| {
            Ok(SwerveModuleSet {
                values: array(set, "values")?
                    .iter()
                    .map(module_sample)
                    .collect::<Result<_, JsError>>()?,
                color: string(set, "color")?,
            })
        })
        .collect::<Result<_, JsError>>()?;
    Ok(RobotObj {
        kind,
        model: string(value, "model")?,
        poses: poses(value, "poses")?,
        components: poses(value, "components")?,
        mechanisms: RobotMechanisms {
            xz: mechanism(get(mechanisms, "xz")?)?,
            yz: mechanism(get(mechanisms, "yz")?)?,
        },
        vision_targets: poses(value, "visionTargets")?,
        swerve_module_velocities,
    })
}

fn april_tag(value: &JsValue) -> Result<AprilTagObj, JsError> {
    let variant = get(value, "variant")?;
    Ok(AprilTagObj {
        poses: poses(value, "poses")?,
        variant: AprilTagVariant {
            family: string(variant, "family")?,
            inches: number(get(variant, "inches")?)?,
        },
    })
}

pub fn any_obj(value: &JsValue) -> Result<AnyObj, JsError> {
    let kind = string(value, "type")?;
    Ok(match kind.to_string_lossy().as_str() {
        "robot" => AnyObj::Robot(robot(value, RobotKind::Robot)?),
        "ghost" => AnyObj::Robot(robot(
            value,
            RobotKind::Ghost {
                color: string(value, "color")?,
            },
        )?),
        "gamePiece" => AnyObj::GamePiece(GamePieceObj {
            variant: string(value, "variant")?,
            poses: poses(value, "poses")?,
        }),
        "trajectory" => AnyObj::Trajectory(TrajectoryObj {
            color: string(value, "color")?,
            size: string(value, "size")?,
            poses: poses(value, "poses")?,
        }),
        "heatmap" => AnyObj::Heatmap(HeatmapObj {
            poses: poses(value, "poses")?,
        }),
        "aprilTag" => AnyObj::AprilTag(april_tag(value)?),
        "aprilTagBuiltIn" => AnyObj::AprilTagBuiltIn(april_tag(value)?),
        "axes" => AnyObj::Axes(AxesObj {
            poses: poses(value, "poses")?,
        }),
        "cone" => AnyObj::Cone(ConeObj {
            color: string(value, "color")?,
            position: string(value, "position")?,
            poses: poses(value, "poses")?,
        }),
        other => return Err(unmodeled(&format!("object type {other}"))),
    })
}

impl Field3dRendererCommand {
    pub fn from_js(value: &JsValue) -> Result<Self, JsError> {
        let camera_override = match get(value, "cameraOverride")? {
            JsValue::Null => CameraOverride::Null,
            JsValue::Undefined => CameraOverride::Undefined,
            pose => CameraOverride::Pose(annotated_pose(pose)?),
        };
        Ok(Field3dRendererCommand {
            field: string(value, "field")?,
            is_red_alliance: boolean(value, "isRedAlliance")?,
            coordinate_system: string(value, "coordinateSystem")?,
            objects: array(value, "objects")?
                .iter()
                .map(any_obj)
                .collect::<Result<_, _>>()?,
            camera_override,
            auto_driver_station: number(get(value, "autoDriverStation")?)?,
            all_robot_models: array(value, "allRobotModels")?
                .iter()
                .map(|model| {
                    model
                        .as_str()
                        .cloned()
                        .ok_or_else(|| unmodeled("allRobotModels holds a non-string"))
                })
                .collect::<Result<_, _>>()?,
            json: json::stringify(value)
                .map_err(|e| JsError::new(e.name, e.message.to_string_lossy()))?
                .unwrap_or_default(),
        })
    }
}
