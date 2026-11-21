//! `Field2dRendererCommand` and `Orientation` from `src/shared/renderers/Field2dRenderer.ts`.

use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::geometry::{self, AnnotatedPose2d, ModuleVelocity, ModuleVelocitySample, Pose2d};

/// `Orientation`, the enum the command carries as a number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Orientation {
    Deg0 = 0,
    Deg90 = 1,
    Deg180 = 2,
    Deg270 = 3,
}

impl Orientation {
    pub fn number(self) -> f64 {
        self as u8 as f64
    }

    pub fn from_number(value: f64) -> Option<Orientation> {
        match value {
            0.0 => Some(Self::Deg0),
            1.0 => Some(Self::Deg90),
            2.0 => Some(Self::Deg180),
            3.0 => Some(Self::Deg270),
            _ => None,
        }
    }
}

/// `Field2dRendererCommand`: what `getCommand` returns every frame.
#[derive(Clone, Debug)]
pub struct Field2dCommand {
    pub field: JsString,
    pub orientation: f64,
    pub size: f64,
    pub objects: Vec<Object>,
}

#[derive(Clone, Debug)]
pub enum Object {
    Robot {
        poses: Vec<AnnotatedPose2d>,
        /// `trails` aliases one array per pose (upstream `Array(n).fill([])`), so each
        /// element holds every pushed point.
        trails: Vec<Vec<[f64; 2]>>,
        bumper_color: Option<JsString>,
        vision_targets: Vec<AnnotatedPose2d>,
        swerve_module_velocities: Vec<SwerveModules>,
    },
    Ghost {
        poses: Vec<AnnotatedPose2d>,
        color: Option<JsString>,
        vision_targets: Vec<AnnotatedPose2d>,
        swerve_module_velocities: Vec<SwerveModules>,
    },
    Trajectory {
        color: Option<JsString>,
        size: Option<JsString>,
        poses: Vec<AnnotatedPose2d>,
    },
    Heatmap {
        poses: Vec<AnnotatedPose2d>,
    },
    Arrow {
        poses: Vec<AnnotatedPose2d>,
        position: JsString,
    },
}

#[derive(Clone, Debug)]
pub struct SwerveModules {
    pub values: Vec<ModuleVelocitySample>,
    pub color: Option<JsString>,
}

fn option_js(value: &Option<JsString>) -> JsValue {
    match value {
        Some(s) => s.to_js(),
        None => JsValue::Undefined,
    }
}

fn pose2_js(pose: &Pose2d) -> JsValue {
    let mut o = JsObject::new();
    o.insert(
        "translation",
        JsValue::Array(vec![
            pose.translation[0].to_js(),
            pose.translation[1].to_js(),
        ]),
    );
    o.insert("rotation", pose.rotation.to_js());
    JsValue::Object(o)
}

/// `{pose, annotation}`; `vision` marks poses whose annotation carries the always-present
/// `visionColor`/`visionSize` keys the vision child assigns (possibly `undefined`).
fn annotated_pose_js(pose: &AnnotatedPose2d, vision: bool) -> JsValue {
    let mut annotation = JsObject::new();
    if pose.annotation.has_april_tag_id {
        annotation.insert(
            "aprilTagId",
            pose.annotation
                .april_tag_id
                .map_or(JsValue::Undefined, |v| v.to_js()),
        );
    }
    annotation.insert("is2DSource", pose.annotation.is_2d_source.to_js());
    if vision || pose.annotation.vision_color.is_some() {
        annotation.insert("visionColor", option_js(&pose.annotation.vision_color));
    }
    if vision || pose.annotation.vision_size.is_some() {
        annotation.insert("visionSize", option_js(&pose.annotation.vision_size));
    }
    let mut o = JsObject::new();
    o.insert("pose", pose2_js(&pose.pose));
    o.insert("annotation", JsValue::Object(annotation));
    JsValue::Object(o)
}

fn poses_js(poses: &[AnnotatedPose2d], vision: bool) -> JsValue {
    JsValue::Array(poses.iter().map(|p| annotated_pose_js(p, vision)).collect())
}

fn module_velocity_js(sample: &ModuleVelocitySample) -> JsValue {
    match sample {
        ModuleVelocitySample::Full(ModuleVelocity { velocity, angle }) => {
            let mut o = JsObject::new();
            o.insert("velocity", velocity.to_js());
            o.insert("angle", angle.to_js());
            JsValue::Object(o)
        }
        ModuleVelocitySample::Partial { velocity, angle } => {
            let mut o = JsObject::new();
            o.insert("velocity", velocity.map_or(JsValue::Null, |v| v.to_js()));
            o.insert("angle", angle.map_or(JsValue::Null, |v| v.to_js()));
            JsValue::Object(o)
        }
        ModuleVelocitySample::Undefined => JsValue::Undefined,
    }
}

fn swerve_modules_js(modules: &[SwerveModules]) -> JsValue {
    JsValue::Array(
        modules
            .iter()
            .map(|m| {
                let mut o = JsObject::new();
                o.insert(
                    "values",
                    JsValue::Array(m.values.iter().map(module_velocity_js).collect()),
                );
                o.insert("color", option_js(&m.color));
                JsValue::Object(o)
            })
            .collect(),
    )
}

fn trails_js(trails: &[Vec<[f64; 2]>]) -> JsValue {
    JsValue::Array(
        trails
            .iter()
            .map(|trail| {
                JsValue::Array(
                    trail
                        .iter()
                        .map(|[x, y]| JsValue::Array(vec![x.to_js(), y.to_js()]))
                        .collect(),
                )
            })
            .collect(),
    )
}

impl ToJs for Field2dCommand {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("field", self.field.to_js());
        o.insert("orientation", self.orientation.to_js());
        o.insert("size", self.size.to_js());
        o.insert(
            "objects",
            JsValue::Array(
                self.objects
                    .iter()
                    .map(|object| {
                        let mut object_js = JsObject::new();
                        match object {
                            Object::Robot {
                                poses,
                                trails,
                                bumper_color,
                                vision_targets,
                                swerve_module_velocities,
                            } => {
                                object_js.insert("type", "robot".to_js());
                                object_js.insert("poses", poses_js(poses, false));
                                object_js.insert("trails", trails_js(trails));
                                object_js.insert("bumperColor", option_js(bumper_color));
                                object_js.insert("visionTargets", poses_js(vision_targets, true));
                                object_js.insert(
                                    "swerveModuleVelocities",
                                    swerve_modules_js(swerve_module_velocities),
                                );
                            }
                            Object::Ghost {
                                poses,
                                color,
                                vision_targets,
                                swerve_module_velocities,
                            } => {
                                object_js.insert("type", "ghost".to_js());
                                object_js.insert("poses", poses_js(poses, false));
                                object_js.insert("color", option_js(color));
                                object_js.insert("visionTargets", poses_js(vision_targets, true));
                                object_js.insert(
                                    "swerveModuleVelocities",
                                    swerve_modules_js(swerve_module_velocities),
                                );
                            }
                            Object::Trajectory { color, size, poses } => {
                                object_js.insert("type", "trajectory".to_js());
                                object_js.insert("color", option_js(color));
                                object_js.insert("size", option_js(size));
                                object_js.insert("poses", poses_js(poses, false));
                            }
                            Object::Heatmap { poses } => {
                                object_js.insert("type", "heatmap".to_js());
                                object_js.insert("poses", poses_js(poses, false));
                            }
                            Object::Arrow { poses, position } => {
                                object_js.insert("type", "arrow".to_js());
                                object_js.insert("poses", poses_js(poses, false));
                                object_js.insert("position", position.to_js());
                            }
                        }
                        JsValue::Object(object_js)
                    })
                    .collect(),
            ),
        );
        JsValue::Object(o)
    }
}

impl Field2dCommand {
    /// Parses a recorded command back into the fields `render` reads. Only used by the
    /// harness to feed `render` and `lastCommand`; parsing keeps every property it does
    /// not read so `lastRenderState` serializes identically.
    pub fn from_js(value: &JsValue) -> Option<Field2dCommand> {
        let objects = value.get("objects")?.as_array()?;
        Some(Field2dCommand {
            field: value.get("field")?.as_str()?.clone(),
            orientation: value.get("orientation")?.as_f64()?,
            size: value.get("size")?.as_f64()?,
            objects: objects.iter().map(Object::from_js).collect::<Option<_>>()?,
        })
    }
}

fn annotated_pose_of(value: &JsValue) -> Option<AnnotatedPose2d> {
    let pose = value.get("pose")?;
    let annotation = value.get("annotation")?;
    let translation = pose.get("translation")?.as_array()?;
    Some(AnnotatedPose2d {
        pose: Pose2d {
            translation: [
                translation.first()?.as_f64()?,
                translation.get(1)?.as_f64()?,
            ],
            rotation: pose.get("rotation")?.as_f64()?,
        },
        annotation: geometry::PoseAnnotations {
            is_2d_source: annotation.get("is2DSource")?.as_bool()?,
            has_april_tag_id: annotation.get("aprilTagId").is_some(),
            april_tag_id: annotation.get("aprilTagId").and_then(JsValue::as_f64),
            vision_color: annotation
                .get("visionColor")
                .and_then(|v| v.as_str())
                .cloned(),
            vision_size: annotation
                .get("visionSize")
                .and_then(|v| v.as_str())
                .cloned(),
        },
    })
}

fn annotated_poses_of(value: Option<&JsValue>) -> Option<Vec<AnnotatedPose2d>> {
    value?
        .as_array()?
        .iter()
        .map(annotated_pose_of)
        .collect::<Option<_>>()
}

fn swerve_modules_of(value: Option<&JsValue>) -> Option<Vec<SwerveModules>> {
    value?
        .as_array()?
        .iter()
        .map(|m| {
            let values = m
                .get("values")?
                .as_array()?
                .iter()
                .map(module_velocity_of)
                .collect::<Option<_>>()?;
            Some(SwerveModules {
                values,
                color: m.get("color").and_then(|v| v.as_str()).cloned(),
            })
        })
        .collect::<Option<_>>()
}

fn module_velocity_of(value: &JsValue) -> Option<ModuleVelocitySample> {
    match value {
        JsValue::Undefined => Some(ModuleVelocitySample::Undefined),
        _ => {
            let number_or_null = |v: Option<&JsValue>| -> Option<Option<f64>> {
                match v {
                    Some(JsValue::Number(n)) => Some(Some(*n)),
                    Some(JsValue::Null) => Some(None),
                    _ => None,
                }
            };
            let velocity = number_or_null(value.get("velocity"))?;
            let angle = number_or_null(value.get("angle"))?;
            Some(match (velocity, angle) {
                (Some(v), Some(a)) => ModuleVelocitySample::Full(ModuleVelocity {
                    velocity: v,
                    angle: a,
                }),
                (velocity, angle) => ModuleVelocitySample::Partial { velocity, angle },
            })
        }
    }
}

impl Object {
    fn from_js(value: &JsValue) -> Option<Object> {
        match value.get("type")?.as_str()?.as_string()?.as_str() {
            "robot" => Some(Object::Robot {
                poses: annotated_poses_of(value.get("poses"))?,
                trails: value
                    .get("trails")?
                    .as_array()?
                    .iter()
                    .map(|trail| {
                        trail
                            .as_array()?
                            .iter()
                            .map(|p| {
                                let pair = p.as_array()?;
                                Some([pair.first()?.as_f64()?, pair.get(1)?.as_f64()?])
                            })
                            .collect::<Option<Vec<_>>>()
                    })
                    .collect::<Option<_>>()?,
                bumper_color: value.get("bumperColor").and_then(|v| v.as_str()).cloned(),
                vision_targets: annotated_poses_of(value.get("visionTargets"))?,
                swerve_module_velocities: swerve_modules_of(value.get("swerveModuleVelocities"))?,
            }),
            "ghost" => Some(Object::Ghost {
                poses: annotated_poses_of(value.get("poses"))?,
                color: value.get("color").and_then(|v| v.as_str()).cloned(),
                vision_targets: annotated_poses_of(value.get("visionTargets"))?,
                swerve_module_velocities: swerve_modules_of(value.get("swerveModuleVelocities"))?,
            }),
            "trajectory" => Some(Object::Trajectory {
                color: value.get("color").and_then(|v| v.as_str()).cloned(),
                size: value.get("size").and_then(|v| v.as_str()).cloned(),
                poses: annotated_poses_of(value.get("poses"))?,
            }),
            "heatmap" => Some(Object::Heatmap {
                poses: annotated_poses_of(value.get("poses"))?,
            }),
            "arrow" => Some(Object::Arrow {
                poses: annotated_poses_of(value.get("poses"))?,
                position: value.get("position")?.as_str()?.clone(),
            }),
            _ => None,
        }
    }
}
