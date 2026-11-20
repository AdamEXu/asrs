//! `shared/geometry.ts`: pose types, format and coordinate conversions, and the
//! `grab*` readers that pull poses and swerve data out of a `Log`.

use asrs_js::math;
use asrs_js::number;
use asrs_js::string::JsString;

use crate::log::{Log, LoggableType, RawValue};
use crate::units;
use crate::util;

pub type Translation2d = [f64; 2]; // meters (x, y)
pub type Rotation2d = f64; // radians
pub type Translation3d = [f64; 3]; // meters (x, y, z)
pub type Rotation3d = [f64; 4]; // radians (w, x, y, z)

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose2d {
    pub translation: Translation2d,
    pub rotation: Rotation2d,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose3d {
    pub translation: Translation3d,
    pub rotation: Rotation3d,
}

pub const TRANSLATION_2D_ZERO: Translation2d = [0.0, 0.0];
pub const ROTATION_2D_ZERO: Rotation2d = 0.0;
pub const POSE_2D_ZERO: Pose2d = Pose2d {
    translation: TRANSLATION_2D_ZERO,
    rotation: ROTATION_2D_ZERO,
};
pub const TRANSLATION_3D_ZERO: Translation3d = [0.0, 0.0, 0.0];
pub const ROTATION_3D_ZERO: Rotation3d = [1.0, 0.0, 0.0, 0.0];
pub const POSE_3D_ZERO: Pose3d = Pose3d {
    translation: TRANSLATION_3D_ZERO,
    rotation: ROTATION_3D_ZERO,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PoseAnnotations {
    pub is_2d_source: bool,
    /// `aprilTagId`; `None` is upstream's `undefined` default. Only
    /// `grabAprilTag` emits the key at all.
    pub has_april_tag_id: bool,
    pub april_tag_id: Option<f64>,
    pub vision_color: Option<JsString>,
    pub vision_size: Option<JsString>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnnotatedPose2d {
    pub pose: Pose2d,
    pub annotation: PoseAnnotations,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnnotatedPose3d {
    pub pose: Pose3d,
    pub annotation: PoseAnnotations,
}

/// `ModuleVelocity` as decoded: `{velocity, angle}`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModuleVelocity {
    pub velocity: f64,
    pub angle: Rotation2d,
}

/// One element of `grabModuleVelocities`' result. The arrangement pass copies
/// the states through `jsonCopy` (non-finite components come back `null`) and
/// can assign `undefined` when an index falls outside the array.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModuleVelocitySample {
    /// The original object, or a jsonCopy whose components stayed finite.
    Full(ModuleVelocity),
    /// A jsonCopy'd object where non-finite components became `null`.
    Partial {
        velocity: Option<f64>,
        angle: Option<f64>,
    },
    /// `undefined` assigned by the arrangement.
    Undefined,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RobotVelocities {
    pub vx: f64,
    pub vy: f64,
    pub omega: f64,
}

/// `Config3d_Rotation` from AdvantageScopeAssets.ts.
#[derive(Clone, Debug, PartialEq)]
pub struct Config3dRotation {
    pub axis: JsString,
    pub degrees: f64,
}

/// `CoordinateSystem` from AdvantageScopeAssets.ts, plus the `Other` case for
/// strings outside the union (they take upstream's `default` branch).
#[derive(Clone, Debug, PartialEq)]
pub enum CoordinateSystem {
    WallAlliance,   // "wall-alliance", FRC 2022
    WallBlue,       // "wall-blue", FRC 2023-2026
    CenterRotated,  // "center-rotated", FTC traditional
    CenterRed,      // "center-red", Systemcore; also takes the default branch
    Other(JsString),
}

impl CoordinateSystem {
    pub fn parse(system: &JsString) -> Self {
        match system.as_string().as_deref() {
            Some("wall-alliance") => Self::WallAlliance,
            Some("wall-blue") => Self::WallBlue,
            Some("center-rotated") => Self::CenterRotated,
            Some("center-red") => Self::CenterRed,
            _ => Self::Other(system.clone()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alliance {
    Red,
    Blue,
}

/// `"radians" | "degrees"` for `grabPosesAuto`'s `numberRotationUnits`.
/// Upstream only checks `=== "degrees"`; everything else behaves as radians.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotationUnits {
    Degrees,
    Other,
}

/// `timeRange` for `grabHeatmapData`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeatmapTimeRange {
    Enabled,
    Auto,
    Teleop,
    TeleopNoEndgame,
    Full,
    Visible,
}

/// What a `grab*` call can throw, mirrored so the harness can compare `name`
/// and `message` of the upstream error.
#[derive(Clone, Debug, PartialEq)]
pub enum GeometryError {
    /// `indexArray` on a length outside a JS array's range (`RangeError:
    /// Invalid array length`).
    RangeError(&'static str),
    /// `window.selection.getTimelineRange()` with no selection (`"visible"`
    /// heatmap only).
    TypeError(String),
}

impl GeometryError {
    pub fn name(&self) -> &'static str {
        match self {
            Self::RangeError(_) => "RangeError",
            Self::TypeError(_) => "TypeError",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::RangeError(m) => m,
            Self::TypeError(m) => m,
        }
    }
}

pub const APRIL_TAG_36H11_COUNT: f64 = 587.0;
pub const APRIL_TAG_16H5_COUNT: f64 = 30.0;
pub const HEATMAP_DT: f64 = 0.25;

// FORMAT CONVERSION UTILITIES

pub fn translation_2d_to_3d(input: Translation2d) -> Translation3d {
    [input[0], input[1], 0.0]
}

pub fn translation_3d_to_2d(input: Translation3d) -> Translation2d {
    [input[0], input[1]]
}

pub fn rotation_2d_to_3d(input: Rotation2d) -> Rotation3d {
    // https://en.wikipedia.org/wiki/Conversion_between_quaternions_and_Euler_angles#Euler_angles_to_quaternion_conversion
    [math::cos(input * 0.5), 0.0, 0.0, math::sin(input * 0.5)]
}

pub fn rotation_3d_to_2d(input: Rotation3d) -> Rotation2d {
    // https://en.wikipedia.org/wiki/Conversion_between_quaternions_and_Euler_angles#Quaternion_to_Euler_angles_(in_3-2-1_sequence)_conversion
    math::atan2(
        2.0 * (input[0] * input[3] + input[1] * input[2]),
        1.0 - 2.0 * (input[2] * input[2] + input[3] * input[3]),
    )
}

/// `rotation3dToRPY` (wpimath/algorithms.md): `[roll, pitch, yaw]`.
pub fn rotation_3d_to_rpy(input: Rotation3d) -> [f64; 3] {
    let (w, x, y, z) = (input[0], input[1], input[2], input[3]);

    let mut roll = 0.0;
    let pitch;
    let yaw;

    {
        let cxcy = 1.0 - 2.0 * (x * x + y * y);
        let sxcy = 2.0 * (w * x + y * z);
        let cy_sq = cxcy * cxcy + sxcy * sxcy;
        if cy_sq > 1e-20 {
            roll = math::atan2(sxcy, cxcy);
        }
    }
    {
        let ratio = 2.0 * (w * y - z * x);
        if math::abs(ratio) >= 1.0 {
            pitch = (math::PI / 2.0) * math::sign(ratio);
        } else {
            pitch = math::asin(ratio);
        }
    }
    {
        let cycz = 1.0 - 2.0 * (y * y + z * z);
        let cysz = 2.0 * (w * z + x * y);
        let cy_sq = cycz * cycz + cysz * cysz;
        if cy_sq > 1e-20 {
            yaw = math::atan2(cysz, cycz);
        } else {
            yaw = math::atan2(2.0 * w * z, w * w - z * z);
        }
    }
    [roll, pitch, yaw]
}

/// `THREE.Quaternion`, only the pieces `rotationSequenceToQuaternion` uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quaternion {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

impl Quaternion {
    pub fn new(x: f64, y: f64, z: f64, w: f64) -> Self {
        Self { x, y, z, w }
    }

    fn identity() -> Self {
        Self::new(0.0, 0.0, 0.0, 1.0)
    }

    /// `setFromAxisAngle`; the axis is assumed normalized.
    fn from_axis_angle(axis: [f64; 3], angle: f64) -> Self {
        let half_angle = angle / 2.0;
        let s = math::sin(half_angle);
        Self::new(axis[0] * s, axis[1] * s, axis[2] * s, math::cos(half_angle))
    }

    fn multiply_quaternions(a: Self, b: Self) -> Self {
        let (qax, qay, qaz, qaw) = (a.x, a.y, a.z, a.w);
        let (qbx, qby, qbz, qbw) = (b.x, b.y, b.z, b.w);
        Self::new(
            qax * qbw + qaw * qbx + qay * qbz - qaz * qby,
            qay * qbw + qaw * qby + qaz * qbx - qax * qbz,
            qaz * qbw + qaw * qbz + qax * qby - qay * qbx,
            qaw * qbw - qax * qbx - qay * qby - qaz * qbz,
        )
    }

    /// `premultiply`: `this = q * this`.
    fn premultiply(self, q: Self) -> Self {
        Self::multiply_quaternions(q, self)
    }
}

/// `rotationSequenceToQuaternion`: premultiplies each axis rotation in order.
pub fn rotation_sequence_to_quaternion(rotations: &[Config3dRotation]) -> Quaternion {
    let mut quaternion = Quaternion::identity();
    for rotation in rotations {
        let mut axis = [0.0, 0.0, 0.0];
        if rotation.axis == JsString::from("x") {
            axis[0] = 1.0;
        }
        if rotation.axis == JsString::from("y") {
            axis[1] = 1.0;
        }
        if rotation.axis == JsString::from("z") {
            axis[2] = 1.0;
        }
        let radians = units::convert(rotation.degrees, "degrees", "radians")
            .expect("degrees and radians are always valid units");
        quaternion = quaternion.premultiply(Quaternion::from_axis_angle(axis, radians));
    }
    quaternion
}

pub fn pose_2d_to_3d(input: Pose2d) -> Pose3d {
    Pose3d {
        translation: translation_2d_to_3d(input.translation),
        rotation: rotation_2d_to_3d(input.rotation),
    }
}

pub fn pose_3d_to_2d(input: Pose3d) -> Pose2d {
    Pose2d {
        translation: translation_3d_to_2d(input.translation),
        rotation: rotation_3d_to_2d(input.rotation),
    }
}

pub fn annotated_pose_2d_to_3d(input: AnnotatedPose2d) -> AnnotatedPose3d {
    AnnotatedPose3d {
        pose: pose_2d_to_3d(input.pose),
        annotation: input.annotation,
    }
}

pub fn annotated_pose_3d_to_2d(input: AnnotatedPose3d) -> AnnotatedPose2d {
    AnnotatedPose2d {
        pose: pose_3d_to_2d(input.pose),
        annotation: input.annotation,
    }
}

// COORDINATE SYSTEM CONVERSION

/// What `convertFromCoordinateSystem` can transform: a `Pose2d`, an
/// `AnnotatedPose2d`, or an array of either.
pub trait ConvertPose {
    fn convert_from_coordinate_system(
        self,
        source: &CoordinateSystem,
        alliance: Alliance,
        field_length: f64,
        field_width: f64,
    ) -> Self;
}

impl ConvertPose for Pose2d {
    fn convert_from_coordinate_system(
        self,
        source: &CoordinateSystem,
        alliance: Alliance,
        field_length: f64,
        field_width: f64,
    ) -> Self {
        match source {
            CoordinateSystem::WallAlliance => match alliance {
                Alliance::Blue => Pose2d {
                    translation: [
                        field_length / 2.0 - self.translation[0],
                        field_width / 2.0 - self.translation[1],
                    ],
                    rotation: self.rotation + math::PI,
                },
                Alliance::Red => Pose2d {
                    translation: [
                        self.translation[0] - field_length / 2.0,
                        self.translation[1] - field_width / 2.0,
                    ],
                    rotation: self.rotation,
                },
            },
            CoordinateSystem::WallBlue => Pose2d {
                translation: [
                    field_length / 2.0 - self.translation[0],
                    field_width / 2.0 - self.translation[1],
                ],
                rotation: self.rotation + math::PI,
            },
            CoordinateSystem::CenterRotated => Pose2d {
                translation: [self.translation[1], -self.translation[0]],
                rotation: self.rotation - math::PI / 2.0,
            },
            _ => self,
        }
    }
}

impl ConvertPose for AnnotatedPose2d {
    fn convert_from_coordinate_system(
        self,
        source: &CoordinateSystem,
        alliance: Alliance,
        field_length: f64,
        field_width: f64,
    ) -> Self {
        AnnotatedPose2d {
            pose: self
                .pose
                .convert_from_coordinate_system(source, alliance, field_length, field_width),
            annotation: self.annotation,
        }
    }
}

impl<T: ConvertPose> ConvertPose for Vec<T> {
    fn convert_from_coordinate_system(
        self,
        source: &CoordinateSystem,
        alliance: Alliance,
        field_length: f64,
        field_width: f64,
    ) -> Self {
        self.into_iter()
            .map(|x| {
                x.convert_from_coordinate_system(source, alliance, field_length, field_width)
            })
            .collect()
    }
}

pub fn convert_from_coordinate_system<P: ConvertPose>(
    pose: P,
    source: &CoordinateSystem,
    alliance: Alliance,
    field_length: f64,
    field_width: f64,
) -> P {
    pose.convert_from_coordinate_system(source, alliance, field_length, field_width)
}

// LOG READING UTILITIES

/// `getOrDefault` with the defaults the grab functions use (`0`, `null`,
/// `undefined`). Kept local so the signatures stay positional like upstream.
fn number_or(log: &Log, key: JsString, timestamp: f64, uuid: Option<&str>) -> f64 {
    crate::log::log_util::get_or_default(log, &key, timestamp, 0.0, uuid)
}

fn number_or_undef(log: &Log, key: JsString, timestamp: f64, uuid: Option<&str>) -> Option<f64> {
    if log.get_type(&key) == Some(LoggableType::Number)
        && let Some(data) = log.get::<f64>(&key, timestamp, timestamp, uuid, None)
        && data.timestamps.first().is_some_and(|t| *t <= timestamp)
    {
        return Some(data.values[0]);
    }
    None
}

fn raw_or_null(log: &Log, key: JsString, timestamp: f64, uuid: Option<&str>) -> Option<RawValue> {
    if log.get_type(&key) == Some(LoggableType::Raw)
        && let Some(data) = log.get::<RawValue>(&key, timestamp, timestamp, uuid, None)
        && data.timestamps.first().is_some_and(|t| *t <= timestamp)
    {
        return Some(data.values[0].clone());
    }
    None
}

fn child(key: &JsString, suffix: &str) -> JsString {
    let mut out = key.clone();
    out.push_str(&JsString::from(suffix));
    out
}

fn index_child(key: &JsString, index: f64) -> JsString {
    let mut out = key.clone();
    out.push_str(&JsString::from("/"));
    out.push_str(&JsString::from(number::to_string(index)));
    out
}

/// `indexArray(...)` mapped to grab children, or upstream's `RangeError` for a
/// length a JS array cannot hold.
fn indices(log: &Log, key: JsString, timestamp: f64, uuid: Option<&str>) -> Result<Vec<f64>, GeometryError> {
    let length = number_or(log, key, timestamp, uuid);
    util::index_array(length).ok_or(GeometryError::RangeError("Invalid array length"))
}

/// `grabPosesAuto`: dispatch on the source's log type.
pub fn grab_poses_auto(
    log: &Log,
    key: &JsString,
    log_type: &JsString,
    timestamp: f64,
    uuid: Option<&str>,
    number_rotation_units: RotationUnits,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    match log_type.as_string().as_deref() {
        Some("Number") => grab_number_rotation(log, key, timestamp, number_rotation_units, uuid),
        Some("Rotation2d") => grab_rotation_2d(log, key, timestamp, uuid),
        Some("Rotation3d") => grab_rotation_3d(log, key, timestamp, uuid),
        Some("Rotation2d[]") => grab_rotation_2d_array(log, key, timestamp, uuid),
        Some("Rotation3d[]") => grab_rotation_3d_array(log, key, timestamp, uuid),
        Some("TargetCorner:16f6ac0dedc8eaccb951f4895d9e18b6[]") => {
            grab_target_corner_array(log, key, timestamp, uuid)
        }
        Some("Translation2d") => grab_translation_2d(log, key, timestamp, uuid),
        Some("Translation3d") => grab_translation_3d(log, key, timestamp, uuid),
        Some("Translation2d[]") => grab_translation_2d_array(log, key, timestamp, uuid),
        Some("Translation3d[]") => grab_translation_3d_array(log, key, timestamp, uuid),
        Some("Pose2d") | Some("Transform2d") => grab_pose_2d(log, key, timestamp, uuid),
        Some("Pose3d") | Some("Transform3d") => grab_pose_3d(log, key, timestamp, uuid),
        Some("Pose2d[]") | Some("Transform2d[]") => grab_pose_2d_array(log, key, timestamp, uuid),
        Some("Pose3d[]") | Some("Transform3d[]") => grab_pose_3d_array(log, key, timestamp, uuid),
        Some("Trajectory") => grab_trajectory(log, key, timestamp, uuid),
        Some("DifferentialSample[]") | Some("SwerveSample[]") => {
            grab_choreo_sample_array(log, key, timestamp, uuid)
        }
        _ => Ok(Vec::new()),
    }
}

pub fn grab_number_rotation(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    unit: RotationUnits,
    uuid: Option<&str>,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    let mut value = number_or(log, key.clone(), timestamp, uuid);
    if unit == RotationUnits::Degrees {
        value = units::convert(value, "degrees", "radians")
            .expect("degrees and radians are always valid units");
    }
    Ok(vec![AnnotatedPose3d {
        pose: Pose3d {
            translation: TRANSLATION_3D_ZERO,
            rotation: rotation_2d_to_3d(value),
        },
        annotation: PoseAnnotations {
            is_2d_source: true,
            ..PoseAnnotations::default()
        },
    }])
}

pub fn grab_rotation_2d(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    uuid: Option<&str>,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    Ok(vec![AnnotatedPose3d {
        pose: Pose3d {
            translation: TRANSLATION_3D_ZERO,
            rotation: rotation_2d_to_3d(number_or(
                log,
                child(key, "/value"),
                timestamp,
                uuid,
            )),
        },
        annotation: PoseAnnotations {
            is_2d_source: true,
            ..PoseAnnotations::default()
        },
    }])
}

pub fn grab_rotation_3d(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    uuid: Option<&str>,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    Ok(vec![AnnotatedPose3d {
        pose: Pose3d {
            translation: TRANSLATION_3D_ZERO,
            rotation: [
                number_or(log, child(key, "/q/w"), timestamp, uuid),
                number_or(log, child(key, "/q/x"), timestamp, uuid),
                number_or(log, child(key, "/q/y"), timestamp, uuid),
                number_or(log, child(key, "/q/z"), timestamp, uuid),
            ],
        },
        annotation: PoseAnnotations {
            is_2d_source: false,
            ..PoseAnnotations::default()
        },
    }])
}

pub fn grab_rotation_2d_array(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    uuid: Option<&str>,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    let mut array = Vec::new();
    for index in indices(log, child(key, "/length"), timestamp, uuid)? {
        array.extend(grab_rotation_2d(log, &index_child(key, index), timestamp, None)?);
    }
    Ok(array)
}

pub fn grab_rotation_3d_array(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    uuid: Option<&str>,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    let mut array = Vec::new();
    for index in indices(log, child(key, "/length"), timestamp, uuid)? {
        array.extend(grab_rotation_3d(log, &index_child(key, index), timestamp, None)?);
    }
    Ok(array)
}

pub fn grab_target_corner_array(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    uuid: Option<&str>,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    let mut array = Vec::new();
    for index in indices(log, child(key, "/length"), timestamp, uuid)? {
        array.extend(grab_target_corner(log, &index_child(key, index), timestamp, None)?);
    }
    Ok(array)
}

pub fn grab_target_corner(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    uuid: Option<&str>,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    Ok(vec![AnnotatedPose3d {
        pose: Pose3d {
            translation: translation_2d_to_3d([
                number_or(log, child(key, "/x"), timestamp, uuid),
                number_or(log, child(key, "/y"), timestamp, uuid),
            ]),
            rotation: ROTATION_3D_ZERO,
        },
        annotation: PoseAnnotations {
            is_2d_source: true,
            ..PoseAnnotations::default()
        },
    }])
}

pub fn grab_translation_2d(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    uuid: Option<&str>,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    Ok(vec![AnnotatedPose3d {
        pose: Pose3d {
            translation: translation_2d_to_3d([
                number_or(log, child(key, "/x"), timestamp, uuid),
                number_or(log, child(key, "/y"), timestamp, uuid),
            ]),
            rotation: ROTATION_3D_ZERO,
        },
        annotation: PoseAnnotations {
            is_2d_source: true,
            ..PoseAnnotations::default()
        },
    }])
}

pub fn grab_translation_3d(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    uuid: Option<&str>,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    Ok(vec![AnnotatedPose3d {
        pose: Pose3d {
            translation: [
                number_or(log, child(key, "/x"), timestamp, uuid),
                number_or(log, child(key, "/y"), timestamp, uuid),
                number_or(log, child(key, "/z"), timestamp, uuid),
            ],
            rotation: ROTATION_3D_ZERO,
        },
        annotation: PoseAnnotations {
            is_2d_source: false,
            ..PoseAnnotations::default()
        },
    }])
}

fn read_f64_le(data: &[u8], offset: usize) -> f64 {
    f64::from_le_bytes(data[offset..offset + 8].try_into().expect("8-byte window"))
}

pub fn grab_translation_2d_array(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    uuid: Option<&str>,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    // Read directly from the raw WPILib struct byte array when present
    // (#477 workaround for large game piece pose arrays).
    if let Some(raw_data) = raw_or_null(log, key.clone(), timestamp, uuid) {
        let mut poses = Vec::new();
        // A Translation2d WPILib struct is 2 little-endian doubles (16 bytes).
        let mut i = 0usize;
        while i + 16 <= raw_data.len() {
            poses.push(AnnotatedPose3d {
                pose: Pose3d {
                    translation: translation_2d_to_3d([
                        read_f64_le(&raw_data, i),
                        read_f64_le(&raw_data, i + 8),
                    ]),
                    rotation: ROTATION_3D_ZERO,
                },
                annotation: PoseAnnotations {
                    is_2d_source: true,
                    ..PoseAnnotations::default()
                },
            });
            i += 16;
        }
        return Ok(poses);
    }

    let mut array = Vec::new();
    for index in indices(log, child(key, "/length"), timestamp, uuid)? {
        array.extend(grab_translation_2d(log, &index_child(key, index), timestamp, None)?);
    }
    Ok(array)
}

pub fn grab_translation_3d_array(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    uuid: Option<&str>,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    if let Some(raw_data) = raw_or_null(log, key.clone(), timestamp, uuid) {
        let mut poses = Vec::new();
        // A Translation3d WPILib struct is 3 little-endian doubles (24 bytes).
        let mut i = 0usize;
        while i + 24 <= raw_data.len() {
            poses.push(AnnotatedPose3d {
                pose: Pose3d {
                    translation: [
                        read_f64_le(&raw_data, i),
                        read_f64_le(&raw_data, i + 8),
                        read_f64_le(&raw_data, i + 16),
                    ],
                    rotation: ROTATION_3D_ZERO,
                },
                annotation: PoseAnnotations {
                    is_2d_source: false,
                    ..PoseAnnotations::default()
                },
            });
            i += 24;
        }
        return Ok(poses);
    }

    let mut array = Vec::new();
    for index in indices(log, child(key, "/length"), timestamp, uuid)? {
        array.extend(grab_translation_3d(log, &index_child(key, index), timestamp, None)?);
    }
    Ok(array)
}

pub fn grab_pose_2d(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    uuid: Option<&str>,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    Ok(vec![AnnotatedPose3d {
        pose: pose_2d_to_3d(Pose2d {
            translation: [
                number_or(log, child(key, "/translation/x"), timestamp, uuid),
                number_or(log, child(key, "/translation/y"), timestamp, uuid),
            ],
            rotation: number_or(log, child(key, "/rotation/value"), timestamp, uuid),
        }),
        annotation: PoseAnnotations {
            is_2d_source: true,
            ..PoseAnnotations::default()
        },
    }])
}

pub fn grab_choreo_sample(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    uuid: Option<&str>,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    Ok(vec![AnnotatedPose3d {
        pose: pose_2d_to_3d(Pose2d {
            translation: [
                number_or(log, child(key, "/pose/translation/x"), timestamp, uuid),
                number_or(log, child(key, "/pose/translation/y"), timestamp, uuid),
            ],
            rotation: number_or(log, child(key, "/pose/rotation/value"), timestamp, uuid),
        }),
        annotation: PoseAnnotations {
            is_2d_source: true,
            ..PoseAnnotations::default()
        },
    }])
}

pub fn grab_pose_3d(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    uuid: Option<&str>,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    Ok(vec![AnnotatedPose3d {
        pose: Pose3d {
            translation: [
                number_or(log, child(key, "/translation/x"), timestamp, uuid),
                number_or(log, child(key, "/translation/y"), timestamp, uuid),
                number_or(log, child(key, "/translation/z"), timestamp, uuid),
            ],
            rotation: [
                number_or(log, child(key, "/rotation/q/w"), timestamp, uuid),
                number_or(log, child(key, "/rotation/q/x"), timestamp, uuid),
                number_or(log, child(key, "/rotation/q/y"), timestamp, uuid),
                number_or(log, child(key, "/rotation/q/z"), timestamp, uuid),
            ],
        },
        annotation: PoseAnnotations {
            is_2d_source: false,
            ..PoseAnnotations::default()
        },
    }])
}

pub fn grab_pose_2d_array(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    uuid: Option<&str>,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    // A Pose2d/Transform2d WPILib struct is 3 little-endian doubles (24 bytes).
    if let Some(raw_data) = raw_or_null(log, key.clone(), timestamp, uuid) {
        let mut poses = Vec::new();
        let mut i = 0usize;
        while i + 24 <= raw_data.len() {
            poses.push(AnnotatedPose3d {
                pose: pose_2d_to_3d(Pose2d {
                    translation: [
                        read_f64_le(&raw_data, i),
                        read_f64_le(&raw_data, i + 8),
                    ],
                    rotation: read_f64_le(&raw_data, i + 16),
                }),
                annotation: PoseAnnotations {
                    is_2d_source: true,
                    ..PoseAnnotations::default()
                },
            });
            i += 24;
        }
        return Ok(poses);
    }

    let mut array = Vec::new();
    for index in indices(log, child(key, "/length"), timestamp, uuid)? {
        array.extend(grab_pose_2d(log, &index_child(key, index), timestamp, None)?);
    }
    Ok(array)
}

pub fn grab_choreo_sample_array(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    uuid: Option<&str>,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    let mut array = Vec::new();
    for index in indices(log, child(key, "/length"), timestamp, uuid)? {
        array.extend(grab_choreo_sample(log, &index_child(key, index), timestamp, None)?);
    }
    Ok(array)
}

pub fn grab_pose_3d_array(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    uuid: Option<&str>,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    // A Pose3d/Transform3d WPILib struct is 7 little-endian doubles (56 bytes).
    if let Some(raw_data) = raw_or_null(log, key.clone(), timestamp, uuid) {
        let mut poses = Vec::new();
        let mut i = 0usize;
        while i + 56 <= raw_data.len() {
            poses.push(AnnotatedPose3d {
                pose: Pose3d {
                    translation: [
                        read_f64_le(&raw_data, i),
                        read_f64_le(&raw_data, i + 8),
                        read_f64_le(&raw_data, i + 16),
                    ],
                    rotation: [
                        read_f64_le(&raw_data, i + 24),
                        read_f64_le(&raw_data, i + 32),
                        read_f64_le(&raw_data, i + 40),
                        read_f64_le(&raw_data, i + 48),
                    ],
                },
                annotation: PoseAnnotations {
                    is_2d_source: false,
                    ..PoseAnnotations::default()
                },
            });
            i += 56;
        }
        return Ok(poses);
    }

    let mut array = Vec::new();
    for index in indices(log, child(key, "/length"), timestamp, uuid)? {
        array.extend(grab_pose_3d(log, &index_child(key, index), timestamp, None)?);
    }
    Ok(array)
}

pub fn grab_trajectory(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    uuid: Option<&str>,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    let mut array = Vec::new();
    for index in indices(log, child(key, "/states/length"), timestamp, uuid)? {
        let state = index_child(&child(key, "/states"), index);
        array.extend(grab_pose_3d(log, &child(&state, "/pose"), timestamp, None)?);
    }
    Ok(array)
}

pub fn grab_april_tag(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    uuid: Option<&str>,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    Ok(vec![AnnotatedPose3d {
        pose: Pose3d {
            translation: [
                number_or(log, child(key, "/pose/translation/x"), timestamp, uuid),
                number_or(log, child(key, "/pose/translation/y"), timestamp, uuid),
                number_or(log, child(key, "/pose/translation/z"), timestamp, uuid),
            ],
            rotation: [
                number_or(log, child(key, "/pose/rotation/q/w"), timestamp, uuid),
                number_or(log, child(key, "/pose/rotation/q/x"), timestamp, uuid),
                number_or(log, child(key, "/pose/rotation/q/y"), timestamp, uuid),
                number_or(log, child(key, "/pose/rotation/q/z"), timestamp, uuid),
            ],
        },
        annotation: PoseAnnotations {
            has_april_tag_id: true,
            april_tag_id: number_or_undef(log, child(key, "/ID"), timestamp, uuid),
            is_2d_source: false,
            ..PoseAnnotations::default()
        },
    }])
}

pub fn grab_april_tag_array(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    uuid: Option<&str>,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    let mut array = Vec::new();
    for index in indices(log, child(key, "/length"), timestamp, uuid)? {
        array.extend(grab_april_tag(log, &index_child(key, index), timestamp, None)?);
    }
    Ok(array)
}

/// `grabHeatmapData`. `timeline_range` stands in for
/// `window.selection.getTimelineRange()` under `time_range == Visible`; upstream
/// throws a TypeError when there is no selection.
pub fn grab_heatmap_data(
    log: &Log,
    key: &JsString,
    log_type: &JsString,
    time_range: HeatmapTimeRange,
    timeline_range: Option<[f64; 2]>,
    uuid: Option<&str>,
) -> Result<Vec<AnnotatedPose3d>, GeometryError> {
    let mut poses = Vec::new();
    let is_full_log = time_range == HeatmapTimeRange::Full;
    let state_ranges = (!is_full_log).then(|| crate::log::robot_state::get_robot_state_ranges(log));
    let is_valid = |timestamp: f64| -> Result<bool, GeometryError> {
        if is_full_log {
            return Ok(true);
        }
        let Some(state_ranges) = &state_ranges else {
            return Ok(false);
        };
        let current = state_ranges.iter().rfind(|range| range.start <= timestamp);
        Ok(match time_range {
            HeatmapTimeRange::Enabled => current.map(|r| r.mode) != Some(crate::log::robot_state::RobotMode::Disabled),
            HeatmapTimeRange::Auto => current.map(|r| r.mode) == Some(crate::log::robot_state::RobotMode::Auto),
            HeatmapTimeRange::Teleop => current.map(|r| r.mode) == Some(crate::log::robot_state::RobotMode::Teleop),
            HeatmapTimeRange::TeleopNoEndgame => {
                current.map(|r| r.mode) == Some(crate::log::robot_state::RobotMode::Teleop)
                    && current.and_then(|r| r.end).is_some_and(|end| end - timestamp > 30.0)
            }
            HeatmapTimeRange::Visible => {
                let Some(timeline) = timeline_range else {
                    return Err(GeometryError::TypeError(
                        "Cannot read properties of undefined (reading 'getTimelineRange')".to_owned(),
                    ));
                };
                timestamp >= timeline[0] && timestamp <= timeline[1]
            }
            HeatmapTimeRange::Full => unreachable!(),
        })
    };
    let range = log.get_timestamp_range();
    let mut sample_time = range[0];
    while sample_time < range[1] {
        if is_valid(sample_time)? {
            poses.extend(grab_poses_auto(
                log,
                key,
                log_type,
                sample_time,
                uuid,
                RotationUnits::Other,
            )?);
        }
        sample_time += HEATMAP_DT;
    }
    Ok(poses)
}

/// `grabModuleVelocities`, including the arrangement pass and its jsonCopy.
pub fn grab_module_velocities(
    log: &Log,
    key: &JsString,
    timestamp: f64,
    arrangement: Option<&JsString>,
    uuid: Option<&str>,
) -> Vec<ModuleVelocitySample> {
    let mut velocities: Vec<ModuleVelocitySample> = Vec::new();
    let length = number_or(log, child(key, "/length"), timestamp, uuid);

    let velocity_key = if log.get_structured_type(key) == Some(&JsString::from("SwerveModuleState[]")) {
        "speed"
    } else {
        "velocity"
    };

    let mut i = 0.0;
    while i < length {
        velocities.push(ModuleVelocitySample::Full(ModuleVelocity {
            velocity: number_or(
                log,
                child(&index_child(key, i), &format!("/{velocity_key}")),
                timestamp,
                uuid,
            ),
            angle: number_or(log, child(&index_child(key, i), "/angle/value"), timestamp, uuid),
        }));
        i += 1.0;
    }

    if velocities.len() == 4
        && let Some(arrangement) = arrangement
    {
        // jsonCopy: JSON.stringify turns non-finite components into null and -0
        // into +0, and JSON.parse reads them back.
        let copy_component = |x: f64| {
            if x.is_finite() {
                Some(if x == 0.0 { 0.0 } else { x })
            } else {
                None
            }
        };
        let copied: Vec<ModuleVelocitySample> = velocities
            .iter()
            .map(|sample| match sample {
                ModuleVelocitySample::Full(v) => {
                    match (copy_component(v.velocity), copy_component(v.angle)) {
                        (Some(velocity), Some(angle)) => {
                            ModuleVelocitySample::Full(ModuleVelocity { velocity, angle })
                        }
                        (velocity, angle) => ModuleVelocitySample::Partial { velocity, angle },
                    }
                }
                _ => *sample,
            })
            .collect();
        for (target_index, part) in arrangement
            .split(Some(&JsString::from(",")), None)
            .iter()
            .enumerate()
        {
            let source_index = number::to_number(part.units());
            let value = if source_index >= 0.0 && source_index.fract() == 0.0 {
                copied
                    .get(source_index as usize)
                    .copied()
                    .unwrap_or(ModuleVelocitySample::Undefined)
            } else {
                ModuleVelocitySample::Undefined
            };
            if target_index < velocities.len() {
                velocities[target_index] = value;
            } else {
                velocities.push(value);
            }
        }
    }

    velocities
}

pub fn grab_robot_velocities(log: &Log, key: &JsString, timestamp: f64, uuid: Option<&str>) -> RobotVelocities {
    RobotVelocities {
        vx: number_or(log, child(key, "/vx"), timestamp, uuid),
        vy: number_or(log, child(key, "/vy"), timestamp, uuid),
        omega: number_or(log, child(key, "/omega"), timestamp, uuid),
    }
}
