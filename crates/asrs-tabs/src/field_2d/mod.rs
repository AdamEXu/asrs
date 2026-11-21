//! `src/hub/controllers/Field2dController.ts` on `configs::field_2d`: the field/size/orientation
//! controls, the per-frame command built from the source list, and the `Field2dRenderer`
//! camera state (`Field2dCameraMode`, zoom, pan, `lastRenderState`) behind `set-2d-camera`,
//! scroll events and context-menu mouse events.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use asrs_js::json;
use asrs_js::math;
use asrs_js::string::{JsError, JsString};
use asrs_js::time::create_uuid;
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::geometry::{
    self, Alliance, AnnotatedPose2d, AnnotatedPose3d, HeatmapTimeRange, ModuleVelocity,
    ModuleVelocitySample, Pose2d, RotationUnits, annotated_pose_3d_to_2d,
    convert_from_coordinate_system, grab_heatmap_data, grab_module_velocities, grab_poses_auto,
    rotation_3d_to_2d,
};
use asrs_log::log::robot_state::{ALLIANCE_KEYS, get_is_red_alliance};
use asrs_log::units;
use asrs_log::util::scale_value;
use asrs_model::assets::{AdvantageScopeAssets, Config2d};
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::preferences::CoordinateSystemPreference;
use asrs_model::source_list::{SourceList, configs};
use asrs_model::source_list_config::SourceListItemState;
use asrs_model::tab_controller::{Command, TabController, TabRenderer};

use crate::cx::TabCx;
use crate::list_cx::{SourceListCx, js_error, with_env};

const TRAIL_LENGTH_SECS: f64 = 3.0;

fn js_eq(s: &JsString, lit: &str) -> bool {
    s.units().iter().copied().eq(lit.encode_utf16())
}
const TRAIL_DT: f64 = 0.1;
const HEATMAP_DT: f64 = 0.25;

/// `Orientation`, the enum the command carries as a number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Orientation {
    Deg0 = 0,
    Deg90 = 1,
    Deg180 = 2,
    Deg270 = 3,
}

impl Orientation {
    fn number(self) -> f64 {
        self as u8 as f64
    }

    fn from_number(value: f64) -> Option<Orientation> {
        match value {
            0.0 => Some(Self::Deg0),
            1.0 => Some(Self::Deg90),
            2.0 => Some(Self::Deg180),
            3.0 => Some(Self::Deg270),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum SizeSetting {
    Large,
    Medium,
    Small,
}

impl SizeSetting {
    const ALL: [SizeSetting; 3] = [Self::Large, Self::Medium, Self::Small];

    fn name(self) -> &'static str {
        match self {
            Self::Large => "large",
            Self::Medium => "medium",
            Self::Small => "small",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum CoordAlert {
    None,
    Frc,
    Ftc,
}

/// The `.field-2d-settings` DOM: `FIELD_SELECT` (a select with an FRC optgroup then an FTC
/// optgroup), `FIELD_SOURCE`, and `SIZE_SWITCHER` (three children with a `selected` class).
#[derive(Clone, Debug, Default)]
pub struct Field2dControls {
    /// `FIELD_SELECT.value`; `""` when `selectedIndex` is -1 or the select is empty.
    pub field: JsString,
    /// FRC optgroup options, `[value, label]` pairs.
    pub frc: Vec<(JsString, JsString)>,
    /// FTC optgroup options, `[value, label]` pairs.
    pub ftc: Vec<(JsString, JsString)>,
    /// `FIELD_SOURCE.hidden`.
    pub source_hidden: bool,
    /// `SETTINGS` has the `ftc` class (otherwise `frc`).
    pub is_ftc: bool,
    /// The `SIZE_SWITCHER` index with `selected` (0..=2).
    pub size: usize,
}

impl Field2dControls {
    fn option_values(&self) -> impl Iterator<Item = &JsString> {
        self.frc
            .iter()
            .chain(self.ftc.iter())
            .map(|(value, _)| value)
    }

    /// `FIELD_SELECT.value = x`: a value that is not an option leaves `selectedIndex` -1,
    /// which reads back as `""`.
    fn set_field(&mut self, value: JsString) {
        self.field = if self.option_values().any(|v| *v == value) {
            value
        } else {
            JsString::new()
        };
    }
}

/// The DOM the renderer reads: `CONTAINER`/`SCROLL_CONTAINER` rects, `devicePixelRatio`,
/// the `satellite` body class and the `<img>` natural sizes the fixture provides. Image
/// `load` events land between frames, so `pending` counts loads the harness has not flushed.
#[derive(Debug, Default)]
pub struct Field2dDom {
    /// `CONTAINER.getBoundingClientRect()`: `[x, y, width, height]`.
    pub container: [f64; 4],
    /// `SCROLL_CONTAINER.getBoundingClientRect()`.
    pub scroll_container: [f64; 4],
    /// `window.devicePixelRatio`.
    pub device_pixel_ratio: f64,
    /// `document.body.classList.contains("satellite")`.
    pub satellite: bool,
    /// Natural `[width, height]` per `IMAGE.src`; a missing entry is a broken image.
    pub images: HashMap<JsString, [f64; 2]>,
    /// `new Date().getTime()` at the DOM snapshot, for `scrollSensor.periodic()`.
    pub now: f64,
}

impl Field2dDom {
    /// `CONTAINER.clientWidth` (`Math.round` of the rect).
    pub fn client_width(&self) -> f64 {
        math::round(self.container[2])
    }

    /// `CONTAINER.clientHeight`.
    pub fn client_height(&self) -> f64 {
        math::round(self.container[3])
    }

    /// `[IMAGE.width, IMAGE.height]` for `src`; `[0, 0]` while unloaded or missing.
    pub fn image_size(&self, src: &JsString) -> [f64; 2] {
        self.images.get(src).copied().unwrap_or([0.0, 0.0])
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
            has_april_tag_id: false,
            april_tag_id: None,
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

/// `Field2dController`. Its `UUID` belongs to `Tabs` (drawn before construction), so the
/// constructor only draws the `SourceList`'s.
pub struct Field2dController {
    list: SourceList,
    /// The `FIELD_SELECT`/settings DOM state this controller mutates.
    pub controls: Field2dControls,
    orientation: Orientation,
    size: SizeSetting,
    last_coordinate_system_alert: CoordAlert,
    /// `window.preferences.prefersFTC` after the `change` handler mutates it; only read
    /// back while preferences exist.
    prefers_ftc: Option<bool>,
}

impl Field2dController {
    pub fn new<C: SourceListCx + ?Sized>(cx: &mut C) -> Field2dController {
        let uuid = create_uuid(cx.tab_rng());
        Self::with_uuid(cx, uuid)
    }

    /// `new` with a recorded `SourceList` UUID instead of a `createUUID` draw.
    pub fn with_uuid<C: SourceListCx + ?Sized>(cx: &mut C, uuid: String) -> Field2dController {
        let mut controller = Field2dController {
            list: SourceList::new(configs::field_2d(), uuid, false),
            controls: Field2dControls::default(),
            orientation: Orientation::Deg0,
            size: SizeSetting::Large,
            last_coordinate_system_alert: CoordAlert::None,
            prefers_ftc: None,
        };
        controller.update_field_options(cx);
        controller.update_switchers();
        controller
    }

    /// The controller's `SourceList` (its `sl` id in the DOM harness).
    pub fn source_list(&self) -> &SourceList {
        &self.list
    }

    pub fn source_list_mut(&mut self) -> &mut SourceList {
        &mut self.list
    }

    fn prefers_ftc<C: TabCx + ?Sized>(&self, cx: &C) -> bool {
        cx.tab_preferences()
            .map(|p| self.prefers_ftc.unwrap_or(p.prefers_ftc))
            .unwrap_or(false)
    }

    fn field_config<'a>(
        assets: Option<&'a AdvantageScopeAssets>,
        field: &JsString,
    ) -> Option<&'a Config2d> {
        assets?.field2ds.iter().find(|f| f.id == *field)
    }

    /// `updateFieldOptions`.
    fn update_field_options<C: TabCx + ?Sized>(&mut self, cx: &C) {
        let value = self.controls.field.clone();
        self.controls.frc.clear();
        self.controls.ftc.clear();
        let mut options = Vec::new();
        if let Some(assets) = cx.tab_assets() {
            for field in &assets.field2ds {
                options.push(field.id.clone());
                let group = if field.is_ftc {
                    &mut self.controls.ftc
                } else {
                    &mut self.controls.frc
                };
                group.push((field.id.clone(), field.name.clone()));
            }
        }
        if options.contains(&value) {
            self.controls.field = value;
        } else {
            self.select_default_field(cx);
        }
        self.update_field_dependent_controls(cx);
    }

    /// `selectDefaultField`.
    fn select_default_field<C: TabCx + ?Sized>(&mut self, cx: &C) {
        let prefers_ftc = self.prefers_ftc(cx);
        let primary = if prefers_ftc {
            &self.controls.ftc
        } else {
            &self.controls.frc
        };
        let secondary = if prefers_ftc {
            &self.controls.frc
        } else {
            &self.controls.ftc
        };
        if let Some((value, _)) = primary.first().or_else(|| secondary.first()) {
            self.controls.field = value.clone();
        } else {
            self.controls.field = JsString::new();
        }
    }

    /// `updateFieldDependentControls`.
    fn update_field_dependent_controls<C: TabCx + ?Sized>(&mut self, cx: &C) {
        let field_config = Self::field_config(cx.tab_assets(), &self.controls.field);
        self.controls.source_hidden = field_config.is_some_and(|f| f.source_url.is_none());
        self.controls.is_ftc = field_config.is_some_and(|f| f.is_ftc);
    }

    /// `updateSwitchers`.
    fn update_switchers(&mut self) {
        self.controls.size = SizeSetting::ALL
            .iter()
            .position(|s| *s == self.size)
            .unwrap_or(0);
    }

    /// The `FIELD_SELECT` `change` handler; `value` is the select's `value` after the event
    /// props applied it (already `""` for an unknown option).
    pub fn select_field<C: TabCx + ?Sized>(&mut self, cx: &mut C, value: JsString) {
        self.controls.field = value;
        let is_ftc = Self::field_config(cx.tab_assets(), &self.controls.field).map(|f| f.is_ftc);
        if let Some(is_ftc) = is_ftc {
            if cx.tab_preferences().is_some() {
                self.prefers_ftc = Some(is_ftc);
            }
            let mut data = JsObject::new();
            data.insert("prefersFTC", is_ftc.to_js());
            cx.send_main_message(NamedMessage::new(
                HubToMain::UpdatePreferences,
                JsValue::Object(data),
            ));
        }
        self.update_field_dependent_controls(cx);
    }

    /// The `FIELD_SOURCE` `click` handler.
    pub fn open_source<C: TabCx + ?Sized>(&mut self, cx: &mut C) {
        let url = Self::field_config(cx.tab_assets(), &self.controls.field)
            .and_then(|f| f.source_url.clone())
            .map_or(JsValue::Undefined, |s| s.to_js());
        cx.send_main_message(NamedMessage::new(HubToMain::OpenLink, url));
    }

    /// An `ORIENTATION_SWITCHER` child click: index 0 turns counter-clockwise, 1 clockwise.
    pub fn click_orientation(&mut self, index: usize) {
        if index == 0 {
            self.orientation = match self.orientation {
                Orientation::Deg270 => Orientation::Deg0,
                o => Orientation::from_number(o.number() + 1.0).unwrap_or(o),
            };
        } else {
            self.orientation = match self.orientation {
                Orientation::Deg0 => Orientation::Deg270,
                o => Orientation::from_number(o.number() - 1.0).unwrap_or(o),
            };
        }
    }

    /// A `SIZE_SWITCHER` child click.
    pub fn click_size(&mut self, index: usize) {
        if index < 3 {
            self.size = SizeSetting::ALL[index];
        }
        self.update_switchers();
    }

    /// The `getCommand` coordinate-system expression and warning messages.
    fn coordinate_system<C: TabCx + ?Sized>(
        &mut self,
        cx: &mut C,
        field_data: Option<&Config2d>,
    ) -> geometry::CoordinateSystem {
        use asrs_model::assets::CoordinateSystem as FieldSystem;
        let coordinate_system = match cx.tab_preferences().map(|p| p.coordinate_system) {
            Some(CoordinateSystemPreference::Automatic) => field_data.map(|f| f.coordinate_system),
            Some(CoordinateSystemPreference::WallAlliance) => Some(FieldSystem::WallAlliance),
            Some(CoordinateSystemPreference::WallBlue) => Some(FieldSystem::WallBlue),
            Some(CoordinateSystemPreference::CenterRotated) => Some(FieldSystem::CenterRotated),
            Some(CoordinateSystemPreference::CenterRed) => Some(FieldSystem::CenterRed),
            None => None,
        }
        .unwrap_or(FieldSystem::CenterRed);
        if let Some(field) = field_data
            && cx.tab_preferences().is_some()
        {
            let current = if !field.is_ftc && field.coordinate_system != FieldSystem::CenterRed {
                CoordAlert::Frc
            } else if field.is_ftc && field.coordinate_system == FieldSystem::CenterRed {
                CoordAlert::Ftc
            } else {
                CoordAlert::None
            };
            if current != self.last_coordinate_system_alert {
                if cx.tab_preferences().map(|p| p.coordinate_system)
                    == Some(CoordinateSystemPreference::Automatic)
                {
                    let name = match current {
                        CoordAlert::Frc => Some(HubToMain::FrcCoordinateSystemWarning),
                        CoordAlert::Ftc => Some(HubToMain::FtcCoordinateSystemWarning),
                        CoordAlert::None => None,
                    };
                    if let Some(name) = name {
                        cx.send_main_message(NamedMessage::new(name, JsValue::Undefined));
                    }
                }
                self.last_coordinate_system_alert = current;
            }
        }
        geometry::CoordinateSystem::from(coordinate_system)
    }
}

fn option<'a>(source: &'a SourceListItemState, key: &str) -> Option<&'a JsString> {
    source.options.get(key)
}

fn heatmap_time_range(source: &SourceListItemState) -> HeatmapTimeRange {
    match option(source, "timeRange") {
        Some(v) if js_eq(v, "auto") => HeatmapTimeRange::Auto,
        Some(v) if js_eq(v, "teleop") => HeatmapTimeRange::Teleop,
        Some(v) if js_eq(v, "teleop-no-endgame") => HeatmapTimeRange::TeleopNoEndgame,
        Some(v) if js_eq(v, "full") => HeatmapTimeRange::Full,
        Some(v) if js_eq(v, "visible") => HeatmapTimeRange::Visible,
        _ => HeatmapTimeRange::Enabled,
    }
}

/// `Math.ceil(start / TRAIL_DT) * TRAIL_DT` sample times between `start` and `end`.
fn trail_samples(start: f64, end: f64) -> Vec<f64> {
    let mut timestamps = vec![start];
    let mut t = math::ceil(start / TRAIL_DT) * TRAIL_DT;
    while t < end {
        timestamps.push(t);
        t += TRAIL_DT;
    }
    timestamps.push(end);
    timestamps
}

impl<C: SourceListCx + ?Sized> TabController<C> for Field2dController {
    fn save_state(&self, _cx: &C) -> JsValue {
        let mut o = JsObject::new();
        o.insert(
            "sources",
            JsValue::Array(self.list.state().iter().map(ToJs::to_js).collect()),
        );
        o.insert("field", self.controls.field.to_js());
        o.insert("orientation", self.orientation.number().to_js());
        o.insert("size", self.size.name().to_js());
        JsValue::Object(o)
    }

    fn restore_state(&mut self, cx: &mut C, state: &JsValue) -> Result<(), JsError> {
        if !matches!(state, JsValue::Object(_) | JsValue::Array(_)) {
            return Ok(());
        }
        self.update_field_options(cx);
        if let Some(sources) = state.get("sources") {
            with_env(cx, |env| self.list.set_state_value(env, sources)).map_err(js_error)?;
        }
        if let Some(field) = state.get("field")
            && let Some(field) = field.as_str()
        {
            let field = field.clone();
            self.controls.set_field(field);
            if self.controls.field.units().is_empty() {
                self.select_default_field(cx);
            }
        }
        if let Some(orientation) = state.get("orientation").and_then(JsValue::as_f64)
            && let Some(orientation) = Orientation::from_number(orientation)
        {
            self.orientation = orientation;
        }
        if let Some(size) = state.get("size").and_then(JsValue::as_str) {
            for candidate in SizeSetting::ALL {
                if js_eq(size, candidate.name()) {
                    self.size = candidate;
                }
            }
        }
        self.update_field_dependent_controls(cx);
        self.update_switchers();
        Ok(())
    }

    fn refresh(&mut self, cx: &mut C) -> Result<(), JsError> {
        with_env(cx, |env| self.list.refresh(env)).map_err(js_error)
    }

    fn new_assets(&mut self, cx: &mut C) -> Result<(), JsError> {
        self.update_field_options(cx);
        Ok(())
    }

    fn active_fields(&self, _cx: &C) -> Vec<JsString> {
        let mut fields = self.list.active_fields();
        fields.extend(ALLIANCE_KEYS.iter().map(|k| JsString::from(*k)));
        fields
    }

    fn show_timeline(&self, _cx: &C) -> bool {
        true
    }

    fn command(&mut self, cx: &mut C, uuid: &JsValue) -> Result<Command, JsError> {
        let time = cx.tab_selection().get_render_time();
        let timestamp_range = cx.tab_log().get_timestamp_range();
        let sources = {
            let log = cx.tab_log();
            self.list.displayed_state(&log)
        };

        // Heatmap "visible" samples call `selection.getTimelineRange()` once per
        // `HEATMAP_DT` step inside `grabHeatmapData`; make the same reads here in the
        // same order and feed the range it returned.
        let mut heatmap_range: HashMap<usize, Option<[f64; 2]>> = HashMap::new();
        for (i, source) in sources.iter().enumerate() {
            if !js_eq(&source.item_type, "heatmap")
                || heatmap_time_range(source) != HeatmapTimeRange::Visible
            {
                continue;
            }
            let mut count = 0usize;
            let mut t = timestamp_range[0];
            while t < timestamp_range[1] {
                count += 1;
                t += HEATMAP_DT;
            }
            let mut range = None;
            for _ in 0..count {
                range = Some(cx.tab_selection().get_timeline_range());
            }
            heatmap_range.insert(i, range);
        }

        let field_data = Self::field_config(cx.tab_assets(), &self.controls.field).cloned();
        let field_width = field_data.as_ref().map_or(0.0, |f| {
            units::convert(f.width_inches, "inches", "meters").unwrap_or(f64::NAN)
        });
        let field_height = field_data.as_ref().map_or(0.0, |f| {
            units::convert(f.height_inches, "inches", "meters").unwrap_or(f64::NAN)
        });
        let coordinate_system = self.coordinate_system(cx, field_data.as_ref());
        let is_red_alliance = time.is_some_and(|t| {
            let log = cx.tab_log();
            get_is_red_alliance(&log, t)
        });
        let alliance = if is_red_alliance {
            Alliance::Red
        } else {
            Alliance::Blue
        };
        let uuid_str = uuid.as_str().map(JsString::to_string_lossy);

        let config = configs::field_2d();
        let mut objects = Vec::new();
        let mut i = 0;
        while i < sources.len() {
            let source = sources[i].clone();
            let type_config = config
                .types
                .iter()
                .find(|t| js_eq(&source.item_type, t.key.as_str()));
            if type_config.is_some_and(|t| t.child_of.is_some()) {
                i += 1;
                continue;
            }
            let mut children = Vec::new();
            while i + 1 < sources.len()
                && config
                    .types
                    .iter()
                    .find(|t| js_eq(&sources[i + 1].item_type, t.key.as_str()))
                    .is_some_and(|t| t.child_of.is_some())
            {
                i += 1;
                children.push(sources[i].clone());
            }

            let is_heatmap = js_eq(&source.item_type, "heatmap");
            let log = cx.tab_log();
            let mut pose3ds: Vec<AnnotatedPose3d> = Vec::new();
            if !is_heatmap {
                if let Some(time) = time {
                    pose3ds = grab_poses_auto(
                        &log,
                        &source.log_key,
                        &source.log_type,
                        time,
                        uuid_str.as_deref(),
                        RotationUnits::Radians,
                    )
                    .map_err(geometry_error)?;
                }
            } else {
                pose3ds = grab_heatmap_data(
                    &log,
                    &source.log_key,
                    &source.log_type,
                    heatmap_time_range(&source),
                    heatmap_range.get(&i).copied().flatten(),
                    uuid_str.as_deref(),
                )
                .map_err(geometry_error)?;
            }
            let mut poses: Vec<AnnotatedPose2d> =
                pose3ds.into_iter().map(annotated_pose_3d_to_2d).collect();

            let mut shared_trail: Vec<[f64; 2]> = Vec::new();
            if let Some(time) = time
                && js_eq(&source.item_type, "robot")
            {
                let start = math::max(&[timestamp_range[0], time - TRAIL_LENGTH_SECS]);
                let end = math::min(&[timestamp_range[1], time + TRAIL_LENGTH_SECS]);
                for sample in trail_samples(start, end) {
                    let mut trail_poses: Vec<AnnotatedPose2d> = match grab_poses_auto(
                        &log,
                        &source.log_key,
                        &source.log_type,
                        sample,
                        uuid_str.as_deref(),
                        RotationUnits::Radians,
                    ) {
                        Ok(p) => p.into_iter().map(annotated_pose_3d_to_2d).collect(),
                        Err(e) => return Err(geometry_error(e)),
                    };
                    if trail_poses.len() != poses.len() {
                        continue;
                    }
                    if field_data.is_some() {
                        trail_poses = convert_from_coordinate_system(
                            trail_poses,
                            &coordinate_system,
                            alliance,
                            field_width,
                            field_height,
                        );
                    }
                    for pose in &trail_poses {
                        shared_trail.push(pose.pose.translation);
                    }
                }
            }
            let trails = vec![shared_trail; poses.len()];

            let mut vision_targets: Vec<AnnotatedPose2d> = Vec::new();
            let mut swerve_module_velocities: Vec<SwerveModules> = Vec::new();
            for child in &children {
                if js_eq(&child.item_type, "rotationOverride")
                    || js_eq(&child.item_type, "rotationOverrideLegacy")
                {
                    let units = match option(child, "units") {
                        Some(u) if js_eq(u, "degrees") => RotationUnits::Degrees,
                        _ => RotationUnits::Radians,
                    };
                    let rotations = grab_poses_auto(
                        &log,
                        &child.log_key,
                        &child.log_type,
                        time.unwrap_or(0.0),
                        uuid_str.as_deref(),
                        units,
                    )
                    .map_err(geometry_error)?;
                    if let Some(first) = rotations.first() {
                        let rotation = rotation_3d_to_2d(first.pose.rotation);
                        for pose in &mut poses {
                            pose.pose.rotation = rotation;
                        }
                    }
                } else if js_eq(&child.item_type, "vision") {
                    let mut vision_pose3ds = grab_poses_auto(
                        &log,
                        &child.log_key,
                        &child.log_type,
                        time.unwrap_or(0.0),
                        uuid_str.as_deref(),
                        RotationUnits::Radians,
                    )
                    .map_err(geometry_error)?;
                    for annotated in &mut vision_pose3ds {
                        annotated.annotation.vision_color = option(child, "color").cloned();
                        annotated.annotation.vision_size = option(child, "size").cloned();
                    }
                    vision_targets.extend(vision_pose3ds.into_iter().map(annotated_pose_3d_to_2d));
                } else if js_eq(&child.item_type, "swerveModuleVelocities") {
                    let values = grab_module_velocities(
                        &log,
                        &child.log_key,
                        time.unwrap_or(0.0),
                        option(child, "arrangement"),
                        uuid_str.as_deref(),
                    );
                    swerve_module_velocities.push(SwerveModules {
                        values,
                        color: option(child, "color").cloned(),
                    });
                }
            }
            vision_targets.reverse();
            swerve_module_velocities.reverse();

            if field_data.is_some() {
                poses = convert_from_coordinate_system(
                    poses,
                    &coordinate_system,
                    alliance,
                    field_width,
                    field_height,
                );
                vision_targets = convert_from_coordinate_system(
                    vision_targets,
                    &coordinate_system,
                    alliance,
                    field_width,
                    field_height,
                );
            }
            drop(log);

            let object = if js_eq(&source.item_type, "robot") {
                let bumpers = option(&source, "bumpers");
                Some(Object::Robot {
                    poses,
                    trails,
                    bumper_color: match bumpers {
                        Some(b) if b.units().is_empty() => {
                            Some(JsString::from(if is_red_alliance {
                                "#ff0000"
                            } else {
                                "#0000ff"
                            }))
                        }
                        other => other.cloned(),
                    },
                    vision_targets,
                    swerve_module_velocities,
                })
            } else if js_eq(&source.item_type, "ghost") {
                Some(Object::Ghost {
                    poses,
                    color: option(&source, "color").cloned(),
                    vision_targets,
                    swerve_module_velocities,
                })
            } else if js_eq(&source.item_type, "trajectory") {
                Some(Object::Trajectory {
                    color: option(&source, "color").cloned(),
                    size: option(&source, "size").cloned(),
                    poses,
                })
            } else if js_eq(&source.item_type, "heatmap") {
                Some(Object::Heatmap { poses })
            } else if js_eq(&source.item_type, "arrow") {
                let position = match option(&source, "position") {
                    Some(p) if js_eq(p, "center") || js_eq(p, "back") || js_eq(p, "front") => {
                        p.clone()
                    }
                    _ => JsString::from("center"),
                };
                Some(Object::Arrow { poses, position })
            } else {
                None
            };
            if let Some(object) = object {
                objects.push(object);
            }
            i += 1;
        }

        let size = match self.size {
            SizeSetting::Large => {
                if field_data.as_ref().is_some_and(|f| f.is_ftc) {
                    18.0
                } else {
                    30.0
                }
            }
            SizeSetting::Medium => {
                if field_data.as_ref().is_some_and(|f| f.is_ftc) {
                    16.0
                } else {
                    27.0
                }
            }
            SizeSetting::Small => {
                if field_data.as_ref().is_some_and(|f| f.is_ftc) {
                    14.0
                } else {
                    24.0
                }
            }
        };

        objects.reverse();
        Ok(Rc::new(Field2dCommand {
            field: self.controls.field.clone(),
            orientation: self.orientation.number(),
            size,
            objects,
        }))
    }
}

fn geometry_error(error: geometry::GeometryError) -> JsError {
    JsError::new(error.name(), error.message())
}

/// `Field2dCameraMode` is a plain number: `set2DCamera` accepts `0..=2` but stores any
/// value that passes, and `restoreState` stores whatever `trackingMode` was.
pub type CameraMode = f64;
pub const CAMERA_UNLOCKED: CameraMode = 0.0;
pub const CAMERA_ROBOT: CameraMode = 1.0;
pub const CAMERA_ROBOT_AND_ROTATION: CameraMode = 2.0;

const SCROLL_SIZE_PX: f64 = 1000000.0;
const SCROLL_RESET_MS: f64 = 1000.0;

/// `Field2dRenderer`'s camera state: everything `saveState`, `set2DCamera`, scroll and
/// context-menu events mutate, plus the `lastRenderState` gate and the image bookkeeping
/// that decides whether a render can proceed.
pub struct Field2dRenderer {
    dom: Rc<RefCell<Field2dDom>>,
    last_image_source: JsString,
    aspect_ratio: f64,
    last_render_state: String,
    image_load_count: u64,
    pending_image_loads: u64,
    zoom: f64,
    pan: [JsValue; 2],
    tracking_mode: CameraMode,
    jump_to_robot: bool,
    last_tracking_mode: CameraMode,
    last_field_center: Option<[f64; 2]>,
    is_ftc: bool,
    last_command: Option<Field2dCommand>,
    /// `scrollSensor.panActive` / `panLastCursor{X,Y}` / `lastCursor{X,Y}`.
    pan_active: bool,
    pan_last_cursor: [f64; 2],
    last_cursor: [f64; 2],
    /// `scrollSensor.lastScrollUpdate` (`new Date().getTime()` at the last `update()`).
    last_scroll_update: f64,
    /// `scrollSensor.resetNext`: the constructor arms it for the first `periodic()`.
    reset_next: bool,
    /// `scrollSensor.lastScrollLeft`/`lastScrollTop`.
    last_scroll: [f64; 2],
    /// The context-menu `startPx`.
    start_px: Option<[f64; 2]>,
    /// Scroll-callback args a `mouse` replay computed, pending the `scroll` record that
    /// carries them.
    pub pending_scrolls: Vec<[f64; 5]>,
}

fn pan_number(value: &JsValue) -> f64 {
    match value {
        JsValue::Number(n) => *n,
        JsValue::String(s) => asrs_js::number::to_number(s.units()),
        JsValue::Null => 0.0,
        JsValue::Bool(b) => i32::from(*b) as f64,
        _ => f64::NAN,
    }
}

impl Field2dRenderer {
    pub fn new(dom: Rc<RefCell<Field2dDom>>) -> Field2dRenderer {
        Field2dRenderer {
            dom,
            last_image_source: JsString::new(),
            aspect_ratio: 1.0,
            last_render_state: String::new(),
            image_load_count: 0,
            pending_image_loads: 0,
            zoom: 1.0,
            pan: [0.0.to_js(), 0.0.to_js()],
            tracking_mode: CAMERA_UNLOCKED,
            jump_to_robot: false,
            last_tracking_mode: CAMERA_UNLOCKED,
            last_field_center: None,
            is_ftc: false,
            last_command: None,
            pan_active: false,
            pan_last_cursor: [0.0, 0.0],
            last_cursor: [0.0, 0.0],
            last_scroll_update: 0.0,
            reset_next: true,
            last_scroll: [0.0, 0.0],
            start_px: None,
            pending_scrolls: Vec::new(),
        }
    }

    /// `IMAGE` "load" events scheduled via `setTimeout` fire between frames.
    pub fn flush_image_loads(&mut self) {
        self.image_load_count += self.pending_image_loads;
        self.pending_image_loads = 0;
    }

    /// `imageLoadCount`, compared against the `loads` snapshot in each render record.
    pub fn image_load_count(&self) -> u64 {
        self.image_load_count
    }

    /// `scrollSensor.reset()`: re-centers the virtual scroll field. Assigning
    /// `scrollLeft`/`scrollTop` fires no `scroll` event, so `update()` never re-enters.
    fn scroll_reset(&mut self) {
        self.last_scroll = [SCROLL_SIZE_PX / 2.0; 2];
    }

    /// `scrollSensor.periodic()`: `render()` runs it first, reading `new Date().getTime()`
    /// from the DOM snapshot.
    fn periodic(&mut self, now: f64) {
        if self.reset_next || now - self.last_scroll_update > SCROLL_RESET_MS {
            self.reset_next = false;
            self.scroll_reset();
        }
    }

    /// `scrollSensor`'s `scroll` event handler (`update()`). `scroll_left`/`scroll_top`
    /// are the container's at dispatch and `now` is `new Date().getTime()`; the scroll
    /// delta goes onto `pending_scrolls` like a mouse pan does.
    pub fn scroll_event(&mut self, scroll_left: f64, scroll_top: f64, now: f64) {
        self.last_scroll_update = now;
        if self.reset_next {
            self.reset_next = false;
            self.scroll_reset();
        }
        let (offset_width, offset_height) = {
            let dom = self.dom.borrow();
            (
                math::round(dom.scroll_container[2]),
                math::round(dom.scroll_container[3]),
            )
        };
        if offset_width == 0.0 && offset_height == 0.0 {
            self.reset_next = true;
            return;
        }
        let dx = scroll_left - self.last_scroll[0];
        let dy = scroll_top - self.last_scroll[1];
        self.last_scroll = [scroll_left, scroll_top];
        self.pending_scrolls
            .push([dx, dy, 0.0, self.last_cursor[0], self.last_cursor[1]]);
    }

    /// `setTrackingMode`.
    fn set_tracking_mode(&mut self, mode: CameraMode) {
        if mode != CAMERA_UNLOCKED {
            self.jump_to_robot = true;
        }
        self.tracking_mode = mode;
    }

    /// `set2DCamera`.
    pub fn set_2d_camera<C: TabCx + ?Sized>(&mut self, cx: &mut C, index: f64) {
        if (0.0..=2.0).contains(&index) {
            self.set_tracking_mode(index);
            if let Some(command) = self.last_command.clone() {
                self.render_command(cx, &command);
            }
        }
    }

    /// The scroll sensor callback: zoom/pan, then `requestAnimationFrame(render)`.
    /// Returns `true` when a render was scheduled.
    pub fn scroll(
        &mut self,
        cx: &mut dyn TabCx,
        dx: f64,
        dy: f64,
        is_pan: bool,
        cursor_x: f64,
        cursor_y: f64,
    ) -> bool {
        let Some(command) = self.last_command.clone() else {
            return false;
        };
        let is_vertical = command.orientation == Orientation::Deg90.number()
            || command.orientation == Orientation::Deg270.number();
        let (container_width, container_height, satellite, image_size) = {
            let dom = self.dom.borrow();
            (
                dom.client_width(),
                dom.client_height(),
                dom.satellite,
                dom.image_size(&self.last_image_source),
            )
        };
        let visible_y_offset = if satellite { 0.0 } else { -13.0 };
        let bounding_width = if is_vertical {
            container_height - math::abs(visible_y_offset * 2.0)
        } else {
            container_width
        };
        let bounding_height = if is_vertical {
            container_width
        } else {
            container_height - math::abs(visible_y_offset * 2.0)
        };
        let Some(field_data) = cx
            .tab_assets()
            .and_then(|a| a.field2ds.iter().find(|f| f.id == command.field))
            .cloned()
        else {
            return false;
        };
        let field_width = field_data.bottom_right[0] - field_data.top_left[0];
        let field_height = field_data.bottom_right[1] - field_data.top_left[1];
        let margin = math::min(&[
            field_data.top_left[1],
            image_size[1] - field_data.bottom_right[1],
            field_data.top_left[0],
            image_size[0] - field_data.bottom_right[0],
        ]);
        let extended_field_width = field_width + margin * 2.0;
        let extended_field_height = field_height + margin * 2.0;
        let constrain_height =
            bounding_width / bounding_height > extended_field_width / extended_field_height;
        let base_image_scalar = if constrain_height {
            bounding_height / extended_field_height
        } else {
            bounding_width / extended_field_width
        };
        let canvas_center_x = cursor_x - container_width / 2.0;
        let canvas_center_y = cursor_y - container_height / 2.0;
        let pan_x = pan_number(&self.pan[0]);
        let pan_y = pan_number(&self.pan[1]);
        if is_pan {
            self.pan = [
                (pan_x + dx / (base_image_scalar * self.zoom)).to_js(),
                (pan_y + dy / (base_image_scalar * self.zoom)).to_js(),
            ];
        } else {
            let zoom_change = dy * -0.005;
            let old_zoom = self.zoom;
            self.zoom = math::min(&[10.0, math::max(&[1.0, self.zoom + zoom_change])]);
            if self.zoom != old_zoom {
                self.pan = [
                    (pan_x
                        + (canvas_center_x / base_image_scalar)
                            * (1.0 / old_zoom - 1.0 / self.zoom))
                        .to_js(),
                    (pan_y
                        + (canvas_center_y / base_image_scalar)
                            * (1.0 / old_zoom - 1.0 / self.zoom))
                        .to_js(),
                ];
            }
        }
        true
    }

    /// `mouse` events on `SCROLL_CONTAINER`: `ScrollSensor` pan tracking plus the
    /// context-menu `startPx`. `kind` is `down`, `move`, `up` or `leave`; `x`/`y` are
    /// `clientX`/`clientY`.
    pub fn mouse(
        &mut self,
        cx: &mut dyn TabCx,
        kind: &str,
        button: f64,
        shift: bool,
        x: f64,
        y: f64,
    ) {
        let scroll_rect = self.dom.borrow().scroll_container;
        match kind {
            "down" => {
                if !shift {
                    self.pan_active = true;
                    self.pan_last_cursor = [x - scroll_rect[0], y - scroll_rect[1]];
                }
                if button == 2.0 {
                    self.start_px = Some([x, y]);
                }
            }
            "move" => {
                let cursor = [x - scroll_rect[0], y - scroll_rect[1]];
                self.last_cursor = cursor;
                if self.pan_active {
                    let dx = self.pan_last_cursor[0] - cursor[0];
                    let dy = self.pan_last_cursor[1] - cursor[1];
                    self.pan_last_cursor = cursor;
                    self.pending_scrolls
                        .push([dx, dy, 1.0, cursor[0], cursor[1]]);
                }
            }
            "up" => {
                self.pan_active = false;
                if let Some(start) = self.start_px
                    && x == start[0]
                    && y == start[1]
                {
                    let mut data = JsObject::new();
                    data.insert("position", JsValue::Array(vec![x.to_js(), y.to_js()]));
                    data.insert("selectedIndex", self.tracking_mode.to_js());
                    cx.send_main_message(NamedMessage::new(
                        HubToMain::Ask2dCamera,
                        JsValue::Object(data),
                    ));
                }
                self.start_px = None;
            }
            "leave" => {
                self.pan_active = false;
            }
            _ => {}
        }
    }

    /// `render(command)` with assets supplied separately so `set2DCamera` and the scroll
    /// rAF can re-run it.
    pub fn render_command<C: TabCx + ?Sized>(&mut self, cx: &C, command: &Field2dCommand) {
        let now = self.dom.borrow().now;
        self.periodic(now);
        let assets = cx.tab_assets();
        let dom = self.dom.borrow();
        let container_width = dom.client_width();
        let container_height = dom.client_height();
        let satellite = dom.satellite;
        let dpr = dom.device_pixel_ratio;

        self.last_command = Some(command.clone());

        let Some(field_data) =
            assets.and_then(|a| a.field2ds.iter().find(|f| f.id == command.field))
        else {
            return;
        };
        let field_data = field_data.clone();
        if field_data.is_ftc != self.is_ftc {
            self.is_ftc = field_data.is_ftc;
            self.zoom = 1.0;
            self.pan = [0.0.to_js(), 0.0.to_js()];
            self.tracking_mode = CAMERA_UNLOCKED;
        }

        let primary_robot_pose = command.objects.iter().find_map(|object| match object {
            Object::Robot { poses, .. } if !poses.is_empty() => Some(poses[0].pose),
            _ => None,
        });

        let is_vertical = command.orientation == Orientation::Deg90.number()
            || command.orientation == Orientation::Deg270.number();
        let visible_y_offset = if satellite { 0.0 } else { -13.0 };

        let mut width = if is_vertical {
            container_height
        } else {
            container_width
        };
        let mut height = if is_vertical {
            container_width
        } else {
            container_height
        };
        let bounding_width = if is_vertical {
            container_height - math::abs(visible_y_offset * 2.0)
        } else {
            container_width
        };
        let bounding_height = if is_vertical {
            container_width
        } else {
            container_height - math::abs(visible_y_offset * 2.0)
        };
        if self.tracking_mode == CAMERA_ROBOT_AND_ROTATION {
            let max_dim = math::hypot(&[container_width, container_height]);
            width = max_dim;
            height = max_dim;
        }

        let render_state = JsValue::Array(vec![
            width.to_js(),
            height.to_js(),
            dpr.to_js(),
            command.to_js(),
            (self.image_load_count as f64).to_js(),
            self.zoom.to_js(),
            self.pan[0].clone(),
            self.pan[1].clone(),
            self.tracking_mode.to_js(),
        ]);
        let render_state_string = json::stringify(&render_state)
            .ok()
            .flatten()
            .unwrap_or_default();
        if render_state_string == self.last_render_state {
            return;
        }
        self.last_render_state = render_state_string;

        let mut canvas_rotation = match command.orientation {
            o if o == Orientation::Deg0.number() => 0.0,
            o if o == Orientation::Deg90.number() => -math::PI / 2.0,
            o if o == Orientation::Deg180.number() => math::PI,
            o if o == Orientation::Deg270.number() => math::PI / 2.0,
            _ => 0.0,
        };
        if self.tracking_mode == CAMERA_ROBOT_AND_ROTATION
            && let Some(pose) = primary_robot_pose
        {
            canvas_rotation += pose.rotation;
        }

        if field_data.path != self.last_image_source {
            self.last_image_source = field_data.path.clone();
            let [w, h] = dom.image_size(&self.last_image_source);
            if w > 0.0 && h > 0.0 {
                self.pending_image_loads += 1;
            }
        }
        let [image_width, image_height] = dom.image_size(&self.last_image_source);
        if !(image_width > 0.0 && image_height > 0.0) {
            return;
        }

        let field_width = field_data.bottom_right[0] - field_data.top_left[0];
        let field_height = field_data.bottom_right[1] - field_data.top_left[1];
        let field_center_x = field_width * 0.5 + field_data.top_left[0];
        let field_center_y = field_height * 0.5 + field_data.top_left[1];

        let mut target_field_x = field_center_x;
        let mut target_field_y = field_center_y;
        if self.tracking_mode > CAMERA_UNLOCKED
            && let Some(pose) = primary_robot_pose
        {
            let position_inches = [
                units::convert(pose.translation[0], "meters", "inches").unwrap_or(f64::NAN),
                units::convert(pose.translation[1], "meters", "inches").unwrap_or(f64::NAN),
            ];
            target_field_x = scale_value(
                position_inches[0],
                [
                    -field_data.width_inches / 2.0,
                    field_data.width_inches / 2.0,
                ],
                [field_data.top_left[0], field_data.top_left[0] + field_width],
            );
            target_field_y = scale_value(
                position_inches[1],
                [
                    -field_data.height_inches / 2.0,
                    field_data.height_inches / 2.0,
                ],
                [
                    field_data.top_left[1] + field_height,
                    field_data.top_left[1],
                ],
            );
        }

        if self.jump_to_robot {
            self.pan = [0.0.to_js(), 0.0.to_js()];
            self.jump_to_robot = false;
        } else if self.last_tracking_mode != self.tracking_mode
            && self.tracking_mode == CAMERA_UNLOCKED
            && let Some(last_center) = self.last_field_center
        {
            let dx = last_center[0] - target_field_x;
            let dy = last_center[1] - target_field_y;
            self.pan = [
                (dx * math::cos(canvas_rotation) - dy * math::sin(canvas_rotation)).to_js(),
                (dx * math::sin(canvas_rotation) + dy * math::cos(canvas_rotation)).to_js(),
            ];
        }
        self.last_tracking_mode = self.tracking_mode;

        let pan_x = pan_number(&self.pan[0]);
        let pan_y = pan_number(&self.pan[1]);
        let mut pan_field_x =
            pan_x * math::cos(-canvas_rotation) - pan_y * math::sin(-canvas_rotation);
        let mut pan_field_y =
            pan_x * math::sin(-canvas_rotation) + pan_y * math::cos(-canvas_rotation);
        let mut field_center_x = target_field_x + pan_field_x;
        let mut field_center_y = target_field_y + pan_field_y;

        let top_margin = field_data.top_left[1];
        let bottom_margin = image_height - field_data.bottom_right[1];
        let left_margin = field_data.top_left[0];
        let right_margin = image_width - field_data.bottom_right[0];
        let margin = math::min(&[top_margin, bottom_margin, left_margin, right_margin]);
        let extended_field_width = field_width + margin * 2.0;
        let extended_field_height = field_height + margin * 2.0;
        let constrain_height =
            bounding_width / bounding_height > extended_field_width / extended_field_height;
        let base_image_scalar = if constrain_height {
            bounding_height / extended_field_height
        } else {
            bounding_width / extended_field_width
        };
        let image_scalar = base_image_scalar * self.zoom;

        if self.tracking_mode == CAMERA_UNLOCKED {
            let field_display_width = extended_field_width * image_scalar;
            let field_display_height = extended_field_height * image_scalar;

            let pan_offset_x = (math::sin(canvas_rotation) * visible_y_offset) / image_scalar;
            let pan_offset_y = (math::cos(canvas_rotation) * visible_y_offset) / image_scalar;

            let margin_x =
                math::max(&[0.0, (field_display_width - bounding_width) * 0.5]) / image_scalar;
            let min_pan_x = -margin_x + pan_offset_x;
            let max_pan_x = margin_x + pan_offset_x;

            let margin_y =
                math::max(&[0.0, (field_display_height - bounding_height) * 0.5]) / image_scalar;
            let min_pan_y = -margin_y + pan_offset_y;
            let max_pan_y = margin_y + pan_offset_y;

            pan_field_x = math::max(&[min_pan_x, math::min(&[max_pan_x, pan_field_x])]);
            pan_field_y = math::max(&[min_pan_y, math::min(&[max_pan_y, pan_field_y])]);

            field_center_x = target_field_x + pan_field_x;
            field_center_y = target_field_y + pan_field_y;

            self.pan = [
                (pan_field_x * math::cos(canvas_rotation)
                    - pan_field_y * math::sin(canvas_rotation))
                .to_js(),
                (pan_field_x * math::sin(canvas_rotation)
                    + pan_field_y * math::cos(canvas_rotation))
                .to_js(),
            ];
        }

        self.last_field_center = Some([field_center_x, field_center_y]);
        self.aspect_ratio = if is_vertical {
            field_height / field_width
        } else {
            field_width / field_height
        };
    }
}

impl<C: TabCx + ?Sized> TabRenderer<C> for Field2dRenderer {
    fn save_state(&self, _cx: &C) -> JsValue {
        let mut o = JsObject::new();
        o.insert("zoom", self.zoom.to_js());
        o.insert("pan", JsValue::Array(self.pan.to_vec()));
        o.insert("trackingMode", self.tracking_mode.to_js());
        o.insert("isFTC", self.is_ftc.to_js());
        JsValue::Object(o)
    }

    fn restore_state(&mut self, _cx: &mut C, state: &JsValue) -> Result<(), JsError> {
        if !matches!(state, JsValue::Object(_) | JsValue::Array(_)) {
            return Ok(());
        }
        if let Some(zoom) = state.get("zoom").and_then(JsValue::as_f64) {
            self.zoom = zoom;
        }
        if let Some(pan) = state.get("pan").and_then(JsValue::as_array)
            && pan.len() == 2
        {
            self.pan = [pan[0].clone(), pan[1].clone()];
        }
        if let Some(mode) = state.get("trackingMode").and_then(JsValue::as_f64) {
            self.set_tracking_mode(mode);
        }
        if let Some(is_ftc) = state.get("isFTC").and_then(JsValue::as_bool) {
            self.is_ftc = is_ftc;
        }
        Ok(())
    }

    fn aspect_ratio(&self, _cx: &C) -> Option<f64> {
        Some(self.aspect_ratio)
    }

    fn render(
        &mut self,
        cx: &mut C,
        command: &dyn asrs_model::tab_controller::TabCommand,
    ) -> Result<(), JsError> {
        let Some(command) = command.as_any().downcast_ref::<Field2dCommand>() else {
            return Err(JsError::type_error(
                "Cannot read properties of undefined (reading 'objects')",
            ));
        };
        self.render_command(cx, command);
        Ok(())
    }
}
