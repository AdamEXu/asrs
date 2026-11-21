//! `hub/controllers/Field3dController.ts`.

use std::rc::Rc;

use asrs_js::math;
use asrs_js::number;
use asrs_js::ops;
use asrs_js::string::{JsError, JsString};
use asrs_js::time;
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::geometry::{
    self, AnnotatedPose3d, GeometryError, HeatmapTimeRange, ModuleVelocitySample, Pose3d,
    PoseAnnotations, RotationUnits,
};
use asrs_log::log::log_util::get_or_default;
use asrs_log::log::mechanism_state::{get_mechanism_state, merge_mechanism_states, MechanismState};
use asrs_log::log::robot_state::{self, ALLIANCE_KEYS, DRIVER_STATION_KEYS};
use asrs_model::assets::{built_in_3d_fields, Config3dField, CoordinateSystem as FieldCoordinates};
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::preferences::CoordinateSystemPreference;
use asrs_model::source_list::{configs, SourceList};
use asrs_model::source_list_config::{
    SourceListConfig, SourceListItemState, SourceListOptionValueConfig,
};
use asrs_model::tab_controller::{Command, TabCommand, TabController};

use crate::cx::TabCx;
use crate::list_cx::{self, with_env, SourceListCx};

const FRC_APRIL_TAG_VARIANTS: [(&str, &str); 2] =
    [("36h11-6.5in", "36h11"), ("16h5-6in", "16h5")];
const FTC_APRIL_TAG_VARIANTS: [(&str, &str); 6] = [
    ("36h11-2in", "2 in"),
    ("36h11-3in", "3 in"),
    ("36h11-3.25in", "3.25 in"),
    ("36h11-4in", "4 in"),
    ("36h11-5in", "5 in"),
    ("36h11-6.5in", "6.5 in"),
];

#[derive(Clone, Copy, PartialEq)]
enum CoordinateAlert {
    None,
    Frc,
    Ftc,
}

/// A `swerveModuleVelocities` child: the grabbed samples plus the child's color
/// option (`undefined` when absent).
struct SwerveVelocities {
    values: Vec<ModuleVelocitySample>,
    color: Option<JsString>,
}

/// `PoseMechanisms` (`{xz, yz}`); each side is `null` when no mechanism child
/// produced a state for it.
#[derive(Default)]
struct PoseMechanisms {
    xz: Option<MechanismState>,
    yz: Option<MechanismState>,
}

/// `Field3dRendererCommand`.
pub struct Field3dCommand {
    field: JsString,
    is_red_alliance: bool,
    coordinate_system: JsString,
    /// Pre-serialized so every `Field3dRendererCommand_AnyObj` variant keeps its
    /// own key order.
    objects: Vec<JsValue>,
    /// `cameraOverride`: `AnnotatedPose3d | null` upstream, `undefined` where
    /// `poses[0]` read past the end, so the serialized value is stored.
    camera_override: JsValue,
    auto_driver_station: f64,
    all_robot_models: Vec<JsValue>,
}

impl TabCommand for Field3dCommand {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("field", JsValue::String(self.field.clone()));
        o.insert("isRedAlliance", JsValue::Bool(self.is_red_alliance));
        o.insert(
            "coordinateSystem",
            JsValue::String(self.coordinate_system.clone()),
        );
        o.insert("objects", JsValue::Array(self.objects.clone()));
        o.insert("cameraOverride", self.camera_override.clone());
        o.insert("autoDriverStation", JsValue::Number(self.auto_driver_station));
        o.insert(
            "allRobotModels",
            JsValue::Array(self.all_robot_models.clone()),
        );
        JsValue::Object(o)
    }
}

fn number(value: f64) -> JsValue {
    JsValue::Number(value)
}

fn option(options: &JsObject<JsString>, key: &str) -> JsValue {
    options
        .get(key)
        .map_or(JsValue::Undefined, |v| JsValue::String(v.clone()))
}

fn pose_js(pose: &Pose3d) -> JsValue {
    let mut o = JsObject::new();
    o.insert(
        "translation",
        JsValue::Array(pose.translation.iter().map(|&x| number(x)).collect()),
    );
    o.insert(
        "rotation",
        JsValue::Array(pose.rotation.iter().map(|&x| number(x)).collect()),
    );
    JsValue::Object(o)
}

fn annotation_js(a: &PoseAnnotations) -> JsValue {
    let mut o = JsObject::new();
    // A `grab`-created `aprilTagId` sits before `is2DSource`; one assigned later
    // (aprilTagIDs pass, built-in tags) lands at the end.
    if a.has_april_tag_id {
        o.insert(
            "aprilTagId",
            a.april_tag_id.map_or(JsValue::Undefined, number),
        );
    }
    o.insert("is2DSource", JsValue::Bool(a.is_2d_source));
    if a.has_vision_color {
        o.insert(
            "visionColor",
            a.vision_color
                .as_ref()
                .map_or(JsValue::Undefined, |v| JsValue::String(v.clone())),
        );
    }
    if a.has_vision_size {
        o.insert(
            "visionSize",
            a.vision_size
                .as_ref()
                .map_or(JsValue::Undefined, |v| JsValue::String(v.clone())),
        );
    }
    if !a.has_april_tag_id && a.april_tag_id.is_some() {
        o.insert("aprilTagId", number(a.april_tag_id.unwrap()));
    }
    JsValue::Object(o)
}

fn annotated_pose_js(pose: &AnnotatedPose3d) -> JsValue {
    let mut o = JsObject::new();
    o.insert("pose", pose_js(&pose.pose));
    o.insert("annotation", annotation_js(&pose.annotation));
    JsValue::Object(o)
}

fn poses_js(poses: &[AnnotatedPose3d]) -> JsValue {
    JsValue::Array(poses.iter().map(annotated_pose_js).collect())
}

fn mechanism_js(state: &MechanismState) -> JsValue {
    let mut o = JsObject::new();
    o.insert(
        "backgroundColor",
        JsValue::String(state.background_color.clone()),
    );
    o.insert(
        "dimensions",
        JsValue::Array(state.dimensions.iter().map(|&x| number(x)).collect()),
    );
    o.insert(
        "lines",
        JsValue::Array(
            state
                .lines
                .iter()
                .map(|line| {
                    let mut l = JsObject::new();
                    let coords = |c: &[Option<f64>]| {
                        JsValue::Array(
                            c.iter().map(|x| x.map_or(JsValue::Null, number)).collect(),
                        )
                    };
                    l.insert("start", coords(&line.start));
                    l.insert("end", coords(&line.end));
                    // `getOrDefault(..., 0)`: a missing or non-string color is `0`.
                    l.insert(
                        "color",
                        line.color
                            .as_ref()
                            .map_or(JsValue::Number(0.0), |v| JsValue::String(v.clone())),
                    );
                    l.insert("weight", line.weight.map_or(JsValue::Null, number));
                    JsValue::Object(l)
                })
                .collect(),
        ),
    );
    JsValue::Object(o)
}

fn mechanisms_js(mechanisms: &PoseMechanisms) -> JsValue {
    let mut o = JsObject::new();
    o.insert(
        "xz",
        mechanisms.xz.as_ref().map_or(JsValue::Null, mechanism_js),
    );
    o.insert(
        "yz",
        mechanisms.yz.as_ref().map_or(JsValue::Null, mechanism_js),
    );
    JsValue::Object(o)
}

fn velocity_sample_js(sample: &ModuleVelocitySample) -> JsValue {
    match sample {
        ModuleVelocitySample::Full(v) => {
            let mut o = JsObject::new();
            o.insert("velocity", number(v.velocity));
            o.insert("angle", number(v.angle));
            JsValue::Object(o)
        }
        ModuleVelocitySample::Partial { velocity, angle } => {
            let mut o = JsObject::new();
            o.insert("velocity", velocity.map_or(JsValue::Null, number));
            o.insert("angle", angle.map_or(JsValue::Null, number));
            JsValue::Object(o)
        }
        ModuleVelocitySample::Undefined => JsValue::Undefined,
    }
}

fn swerve_js(sets: &[SwerveVelocities]) -> JsValue {
    JsValue::Array(
        sets.iter()
            .map(|set| {
                let mut o = JsObject::new();
                o.insert(
                    "values",
                    JsValue::Array(set.values.iter().map(velocity_sample_js).collect()),
                );
                o.insert(
                    "color",
                    set.color
                        .as_ref()
                        .map_or(JsValue::Undefined, |v| JsValue::String(v.clone())),
                );
                JsValue::Object(o)
            })
            .collect(),
    )
}

fn april_tag_variant_js(options: &JsObject<JsString>) -> Result<JsValue, JsError> {
    let variant = options.get("variant").ok_or_else(|| {
        JsError::new(
            "TypeError",
            "Cannot read properties of undefined (reading 'split')",
        )
    })?;
    let parts = variant.split(Some(&JsString::from("-")), None);
    let family = parts.first().cloned().unwrap_or_default();
    let inches = parts
        .get(1)
        .map(|s| number::parse_float(s.units()))
        .unwrap_or(f64::NAN);
    let mut o = JsObject::new();
    o.insert("family", JsValue::String(family));
    o.insert("inches", number(inches));
    Ok(JsValue::Object(o))
}

/// `[...(assets?.field3ds ?? []), ...BuiltIn3dFields]`.
fn all_fields<'a>(cx: &'a impl TabCx) -> Vec<&'a Config3dField> {
    match cx.tab_assets() {
        Some(assets) => assets.field3ds.iter().chain(built_in_3d_fields()).collect(),
        None => built_in_3d_fields().iter().collect(),
    }
}

fn selected_field(controller: &Field3dController, cx: &impl TabCx) -> Option<Config3dField> {
    let value = controller.field_value();
    all_fields(cx).into_iter().find(|f| f.id == value).cloned()
}

fn geometry_error(error: GeometryError) -> JsError {
    JsError::new(error.name(), error.message())
}

/// `Field3dController`.
pub struct Field3dController {
    /// `UUID = createUUID()`; `Tabs` may overwrite it with a restored value.
    uuid: JsString,
    config: SourceListConfig,
    source_list: SourceList,
    /// `option.value` order inside each optgroup.
    frc_field_options: Vec<JsString>,
    ftc_field_options: Vec<JsString>,
    /// `FIELD_SELECT.value`; `None` reads back as `""`.
    field_value: Option<JsString>,
    xr_active: bool,
    last_coordinate_alert: CoordinateAlert,
}

impl Field3dController {
    /// The constructor cannot throw: the only fallible calls here are
    /// `setOptionValues`, which never reaches its error paths on an empty state.
    pub fn new(cx: &mut impl SourceListCx) -> Self {
        let controller_uuid = JsString::from(time::create_uuid(cx.tab_rng()));
        let list_uuid = time::create_uuid(cx.tab_rng());
        let config = configs::field_3d();
        let mut controller = Field3dController {
            uuid: controller_uuid,
            config: config.clone(),
            source_list: SourceList::new(config, list_uuid, false),
            frc_field_options: Vec::new(),
            ftc_field_options: Vec::new(),
            field_value: None,
            xr_active: false,
            last_coordinate_alert: CoordinateAlert::None,
        };
        // The state is empty here, so the `setOptionValues` calls inside cannot
        // reach their error paths.
        let _ = controller.update_field_options(cx);
        let _ = controller.update_robot_options(cx);
        controller
    }

    /// `controller.UUID` as drawn in the constructor.
    pub fn uuid(&self) -> &JsString {
        &self.uuid
    }

    /// `XR_BUTTON` click; `uuid` is the tab's `controller.UUID` (`Tabs` restores it).
    pub fn xr_click(&mut self, cx: &mut impl TabCx, uuid: &JsValue) {
        cx.send_main_message(NamedMessage::new(HubToMain::OpenXr, uuid.clone()));
    }

    /// `setXRActive`.
    pub fn set_xr_active(&mut self, active: bool) {
        self.xr_active = active;
    }

    pub fn xr_active(&self) -> bool {
        self.xr_active
    }

    pub fn source_list(&self) -> &SourceList {
        &self.source_list
    }

    /// `sourceList.addField(logKey)`; no supplemental suppliers.
    pub fn add_field(
        &mut self,
        cx: &mut impl SourceListCx,
        log_key: &JsString,
    ) -> Result<(), JsError> {
        with_env(cx, |env| {
            self.source_list.add_field(env, &[], log_key, None)
        })
        .map_err(list_cx::js_error)
    }

    /// `sourceList.setState`.
    pub fn set_source_state(
        &mut self,
        cx: &mut impl SourceListCx,
        value: &JsValue,
    ) -> Result<(), JsError> {
        let JsValue::Array(items) = value else {
            self.source_list.clear();
            if let JsValue::Bytes(_) = value {
                return Err(JsError::new("Unmodeled", "a typed array of sources"));
            }
            ops::get(value, "forEach")?;
            return Err(JsError::type_error("state.forEach is not a function"));
        };
        let items = items
            .iter()
            .map(|item| {
                SourceListItemState::from_js(item)
                    .ok_or_else(|| JsError::new("Unmodeled", "a source item of another shape"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        with_env(cx, |env| self.source_list.set_state(env, items))
            .map_err(list_cx::js_error)
    }

    /// `FIELD_SELECT.value = ...`; the assignment sticks only when an option matches.
    pub fn set_field_value(&mut self, value: &JsString) {
        self.field_value = if self
            .frc_field_options
            .iter()
            .chain(self.ftc_field_options.iter())
            .any(|v| v == value)
        {
            Some(value.clone())
        } else {
            None
        };
    }

    /// `FIELD_SELECT.value`; `""` when no option is selected.
    pub fn field_value(&self) -> JsString {
        self.field_value.clone().unwrap_or_default()
    }

    /// The `change` listener on `FIELD_SELECT`.
    pub fn field_select_change(&mut self, cx: &mut impl SourceListCx) -> Result<(), JsError> {
        if let Some(field) = selected_field(self, cx) {
            let is_ftc = field.is_ftc;
            if let Some(preferences) = cx.tab_preferences_mut() {
                preferences.prefers_ftc = is_ftc;
            }
            let mut o = JsObject::new();
            o.insert("prefersFTC", JsValue::Bool(is_ftc));
            cx.send_main_message(NamedMessage::new(
                HubToMain::UpdatePreferences,
                JsValue::Object(o),
            ));
        }
        self.update_field_dependent_controls(cx)
    }

    fn select_default_field(&mut self, cx: &impl TabCx) {
        let prefers_ftc = cx.tab_preferences().map(|p| p.prefers_ftc).unwrap_or(false);
        let (primary, secondary) = if prefers_ftc {
            (&self.ftc_field_options, &self.frc_field_options)
        } else {
            (&self.frc_field_options, &self.ftc_field_options)
        };
        self.field_value = primary.first().or_else(|| secondary.first()).cloned();
    }

    fn update_field_options(&mut self, cx: &mut impl SourceListCx) -> Result<(), JsError> {
        let value = self.field_value();
        self.frc_field_options.clear();
        self.ftc_field_options.clear();
        let mut options: Vec<JsString> = Vec::new();
        if cx.tab_assets().is_some() {
            for field in all_fields(cx) {
                options.push(field.id.clone());
                if field.is_ftc {
                    self.ftc_field_options.push(field.id.clone());
                } else {
                    self.frc_field_options.push(field.id.clone());
                }
            }
        }
        if options.iter().any(|v| *v == value) {
            self.field_value = Some(value);
        } else {
            self.select_default_field(cx);
        }
        self.update_field_dependent_controls(cx)
    }

    fn update_robot_options(&mut self, cx: &mut impl SourceListCx) -> Result<(), JsError> {
        if cx.tab_assets().is_none() {
            return Ok(());
        }
        let is_ftc = selected_field(self, cx).is_some_and(|f| f.is_ftc);
        let mut robots: Vec<JsString> = cx
            .tab_assets()
            .unwrap()
            .robots
            .iter()
            .filter(|r| r.is_ftc == is_ftc)
            .map(|r| r.name.clone())
            .collect();
        if robots.is_empty() {
            robots.push(JsString::from("KitBot"));
        }
        let values: Vec<SourceListOptionValueConfig> = robots
            .iter()
            .map(|name| SourceListOptionValueConfig {
                key: name.to_string_lossy(),
                display: name.to_string_lossy(),
            })
            .collect();
        with_env(cx, |env| {
            self.source_list
                .set_option_values(env, "robot", "model", values.clone())
                .and_then(|()| {
                    self.source_list
                        .set_option_values(env, "ghost", "model", values)
                })
        })
        .map_err(list_cx::js_error)
    }

    fn update_field_dependent_controls(
        &mut self,
        cx: &mut impl SourceListCx,
    ) -> Result<(), JsError> {
        if cx.tab_assets().is_none() {
            return Ok(());
        }
        let field = selected_field(self, cx);

        let mut game_pieces: Vec<JsString> = field
            .as_ref()
            .map(|f| f.game_pieces.iter().map(|g| g.name.clone()).collect())
            .unwrap_or_default();
        if game_pieces.is_empty() {
            game_pieces.push(JsString::from("None"));
        }
        let game_piece_values: Vec<SourceListOptionValueConfig> = game_pieces
            .iter()
            .map(|name| SourceListOptionValueConfig {
                key: name.to_string_lossy(),
                display: name.to_string_lossy(),
            })
            .collect();
        let variants: &[(&str, &str)] = if field.as_ref().is_some_and(|f| f.is_ftc) {
            &FTC_APRIL_TAG_VARIANTS
        } else {
            &FRC_APRIL_TAG_VARIANTS
        };
        let april_tag_values: Vec<SourceListOptionValueConfig> = variants
            .iter()
            .map(|(key, display)| SourceListOptionValueConfig {
                key: (*key).to_owned(),
                display: (*display).to_owned(),
            })
            .collect();
        with_env(cx, |env| {
            self.source_list
                .set_option_values(env, "gamePiece", "variant", game_piece_values)
                .and_then(|()| {
                    self.source_list
                        .set_option_values(env, "aprilTag", "variant", april_tag_values)
                })
        })
        .map_err(list_cx::js_error)?;
        self.update_robot_options(cx)
    }

    fn is_child(&self, item_type: &JsString) -> bool {
        let key = item_type.to_string_lossy();
        self.config
            .types
            .iter()
            .any(|t| t.key == key && t.child_of.is_some())
    }

    #[allow(clippy::type_complexity)]
    fn command_objects(
        &self,
        cx: &mut impl SourceListCx,
        uuid: Option<&str>,
        time: Option<f64>,
        field_data: Option<&Config3dField>,
    ) -> Result<(Vec<JsValue>, JsValue), JsError> {
        // `list_parts` borrows the log and selection together: `grabHeatmapData`
        // reads `getTimelineRange` mid-command.
        let parts = cx.list_parts();
        let log = &*parts.log;
        let mut objects: Vec<JsValue> = Vec::new();
        let mut camera_override = JsValue::Null;
        let sources = self.source_list.displayed_state(log);
        let mut i = 0;
        let mut seen_camera_override = false;
        while i < sources.len() {
            let source = &sources[i];
            if self.is_child(&source.item_type) {
                i += 1;
                continue;
            }
            let mut children: Vec<SourceListItemState> = Vec::new();
            while sources
                .get(i + 1)
                .is_some_and(|s| self.is_child(&s.item_type))
            {
                i += 1;
                children.push(sources[i].clone());
            }
            i += 1;

            let is_heatmap = source.item_type == JsString::from("heatmap");
            let mut poses: Vec<AnnotatedPose3d> = Vec::new();
            if is_heatmap {
                let time_range = match source
                    .options
                    .get("timeRange")
                    .map(|v| v.to_string_lossy())
                    .as_deref()
                {
                    Some("auto") => HeatmapTimeRange::Auto,
                    Some("teleop") => HeatmapTimeRange::Teleop,
                    Some("teleop-no-endgame") => HeatmapTimeRange::TeleopNoEndgame,
                    Some("full") => HeatmapTimeRange::Full,
                    Some("visible") => HeatmapTimeRange::Visible,
                    _ => HeatmapTimeRange::Enabled,
                };
                let timeline_range = if time_range == HeatmapTimeRange::Visible {
                    Some(parts.selection.get_timeline_range())
                } else {
                    None
                };
                poses = geometry::grab_heatmap_data(
                    log,
                    &source.log_key,
                    &source.log_type,
                    time_range,
                    timeline_range,
                    uuid,
                )
                .map_err(geometry_error)?;
            } else if let Some(t) = time {
                poses = geometry::grab_poses_auto(
                    log,
                    &source.log_key,
                    &source.log_type,
                    t,
                    uuid,
                    RotationUnits::Radians,
                )
                .map_err(geometry_error)?;
            }

            let mut components: Vec<AnnotatedPose3d> = Vec::new();
            let mut mechanisms_xz: Vec<MechanismState> = Vec::new();
            let mut mechanisms_yz: Vec<MechanismState> = Vec::new();
            let mut vision_targets: Vec<AnnotatedPose3d> = Vec::new();
            let mut swerve_velocities: Vec<SwerveVelocities> = Vec::new();
            if let Some(t) = time {
                for child in &children {
                    match child.item_type.as_string().as_deref() {
                        Some("component") => {
                            components.extend(
                                geometry::grab_poses_auto(
                                    log,
                                    &child.log_key,
                                    &child.log_type,
                                    t,
                                    uuid,
                                    RotationUnits::Radians,
                                )
                                .map_err(geometry_error)?,
                            );
                        }
                        Some("mechanism") => {
                            if let Some(state) = get_mechanism_state(log, &child.log_key, t) {
                                if child
                                    .options
                                    .get("plane")
                                    .is_some_and(|v| v == &JsString::from("yz"))
                                {
                                    mechanisms_yz.push(state);
                                } else {
                                    mechanisms_xz.push(state);
                                }
                            }
                        }
                        Some("rotationOverride") | Some("rotationOverrideLegacy") => {
                            let units = if child
                                .options
                                .get("units")
                                .is_some_and(|v| v == &JsString::from("degrees"))
                            {
                                RotationUnits::Degrees
                            } else {
                                RotationUnits::Radians
                            };
                            let rotations = geometry::grab_poses_auto(
                                log,
                                &child.log_key,
                                &child.log_type,
                                t,
                                uuid,
                                units,
                            )
                            .map_err(geometry_error)?;
                            if let Some(first) = rotations.first() {
                                let rotation = first.pose.rotation;
                                for pose in poses.iter_mut() {
                                    pose.pose.rotation = rotation;
                                }
                            }
                        }
                        Some("vision") => {
                            let mut new_targets = geometry::grab_poses_auto(
                                log,
                                &child.log_key,
                                &child.log_type,
                                t,
                                uuid,
                                RotationUnits::Radians,
                            )
                            .map_err(geometry_error)?;
                            for pose in &mut new_targets {
                                pose.annotation.has_vision_color = true;
                                pose.annotation.vision_color = child.options.get("color").cloned();
                                pose.annotation.has_vision_size = true;
                                pose.annotation.vision_size = child.options.get("size").cloned();
                            }
                            vision_targets.extend(new_targets);
                        }
                        Some("swerveModuleVelocities") => {
                            let states = geometry::grab_module_velocities(
                                log,
                                &child.log_key,
                                t,
                                child.options.get("arrangement"),
                                uuid,
                            );
                            swerve_velocities.push(SwerveVelocities {
                                values: states,
                                color: child.options.get("color").cloned(),
                            });
                        }
                        Some("aprilTagIDs") => {
                            let values: Vec<f64> =
                                get_or_default(log, &child.log_key, t, Vec::new(), uuid);
                            let tag_count = if source
                                .options
                                .get("variant")
                                .is_some_and(|v| v.starts_with(&JsString::from("16h5"), None))
                            {
                                geometry::APRIL_TAG_16H5_COUNT
                            } else {
                                geometry::APRIL_TAG_36H11_COUNT
                            };
                            for id in values {
                                let id = math::min2(
                                    math::max2(math::floor(id), 0.0),
                                    tag_count - 1.0,
                                );
                                if let Some(pose) = poses
                                    .iter_mut()
                                    .find(|p| p.annotation.april_tag_id.is_none())
                                {
                                    pose.annotation.april_tag_id = Some(id);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            let mechanisms = PoseMechanisms {
                xz: merge_mechanism_states(&mechanisms_xz),
                yz: merge_mechanism_states(&mechanisms_yz),
            };
            vision_targets.reverse();
            swerve_velocities.reverse();

            let mut o = JsObject::new();
            match source.item_type.as_string().as_deref() {
                Some("robot") => {
                    o.insert("type", "robot".to_js());
                    o.insert("model", option(&source.options, "model"));
                    o.insert("poses", poses_js(&poses));
                    o.insert("components", poses_js(&components));
                    o.insert("mechanisms", mechanisms_js(&mechanisms));
                    o.insert("visionTargets", poses_js(&vision_targets));
                    o.insert(
                        "swerveModuleVelocities",
                        swerve_js(&swerve_velocities),
                    );
                }
                Some("ghost") => {
                    o.insert("type", "ghost".to_js());
                    o.insert("color", option(&source.options, "color"));
                    o.insert("model", option(&source.options, "model"));
                    o.insert("poses", poses_js(&poses));
                    o.insert("components", poses_js(&components));
                    o.insert("mechanisms", mechanisms_js(&mechanisms));
                    o.insert("visionTargets", poses_js(&vision_targets));
                    o.insert(
                        "swerveModuleVelocities",
                        swerve_js(&swerve_velocities),
                    );
                }
                Some("gamePiece") => {
                    o.insert("type", "gamePiece".to_js());
                    o.insert("variant", option(&source.options, "variant"));
                    o.insert("poses", poses_js(&poses));
                }
                Some("trajectory") => {
                    o.insert("type", "trajectory".to_js());
                    o.insert("color", option(&source.options, "color"));
                    o.insert("size", option(&source.options, "size"));
                    o.insert("poses", poses_js(&poses));
                }
                Some("heatmap") => {
                    o.insert("type", "heatmap".to_js());
                    o.insert("poses", poses_js(&poses));
                }
                Some("aprilTag") => {
                    let variant = april_tag_variant_js(&source.options)?;
                    o.insert("type", "aprilTag".to_js());
                    o.insert("poses", poses_js(&poses));
                    o.insert("variant", variant);
                }
                Some("axes") => {
                    o.insert("type", "axes".to_js());
                    o.insert("poses", poses_js(&poses));
                }
                Some("cone") => {
                    let position = match source.options.get("position").map(|v| v.as_string()) {
                        Some(Some(p))
                            if p == "center" || p == "back" || p == "front" =>
                        {
                            JsValue::String(JsString::from(p))
                        }
                        _ => JsValue::String(JsString::from("center")),
                    };
                    o.insert("type", "cone".to_js());
                    o.insert("color", option(&source.options, "color"));
                    o.insert("position", position);
                    o.insert("poses", poses_js(&poses));
                }
                Some("cameraOverride") => {
                    if !seen_camera_override {
                        seen_camera_override = true;
                        camera_override = poses
                            .first()
                            .map_or(JsValue::Undefined, annotated_pose_js);
                    }
                    continue;
                }
                _ => continue,
            }
            objects.push(JsValue::Object(o));
        }

        // Built-in AprilTags, grouped by first-seen variant.
        if let Some(field) = field_data {
            let mut variants: Vec<JsString> = Vec::new();
            for tag in &field.april_tags {
                if !variants.contains(&tag.variant) {
                    variants.push(tag.variant.clone());
                }
            }
            for variant in variants {
                let parts = variant.split(Some(&JsString::from("-")), None);
                let family = parts.first().cloned().unwrap_or_default();
                let inches = parts
                    .get(1)
                    .map(|s| number::parse_float(s.units()))
                    .unwrap_or(f64::NAN);
                let mut variant_js = JsObject::new();
                variant_js.insert("family", JsValue::String(family));
                variant_js.insert("inches", number(inches));
                let mut o = JsObject::new();
                o.insert("type", "aprilTagBuiltIn".to_js());
                o.insert(
                    "poses",
                    JsValue::Array(
                        field
                            .april_tags
                            .iter()
                            .filter(|t| t.variant == variant)
                            .map(|tag| {
                                let rotations: Vec<geometry::Config3dRotation> = tag
                                    .rotations
                                    .iter()
                                    .map(|r| geometry::Config3dRotation {
                                        axis: JsString::from(r.axis.as_str()),
                                        degrees: r.degrees,
                                    })
                                    .collect();
                                let quaternion =
                                    geometry::rotation_sequence_to_quaternion(&rotations);
                                annotated_pose_js(&AnnotatedPose3d {
                                    pose: Pose3d {
                                        translation: tag.position,
                                        rotation: [
                                            quaternion.w,
                                            quaternion.x,
                                            quaternion.y,
                                            quaternion.z,
                                        ],
                                    },
                                    annotation: PoseAnnotations {
                                        is_2d_source: false,
                                        april_tag_id: Some(tag.id),
                                        ..PoseAnnotations::default()
                                    },
                                })
                            })
                            .collect(),
                    ),
                );
                o.insert("variant", JsValue::Object(variant_js));
                objects.push(JsValue::Object(o));
            }
        }

        Ok((objects, camera_override))
    }
}

impl<Cx: SourceListCx> TabController<Cx> for Field3dController {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        let mut o = JsObject::new();
        o.insert(
            "sources",
            JsValue::Array(
                self.source_list
                    .state()
                    .iter()
                    .map(ToJs::to_js)
                    .collect(),
            ),
        );
        o.insert("game", JsValue::String(self.field_value()));
        JsValue::Object(o)
    }

    fn restore_state(&mut self, cx: &mut Cx, state: &JsValue) -> Result<(), JsError> {
        let JsValue::Object(o) = state else {
            return Ok(());
        };
        self.update_field_options(cx)?;
        if let Some(sources) = o.get("sources") {
            self.set_source_state(cx, sources)?;
        }
        if let Some(JsValue::String(game)) = o.get("game") {
            self.set_field_value(game);
            if self.field_value.is_none() {
                self.select_default_field(cx);
            }
        }
        self.update_field_dependent_controls(cx)
    }

    fn refresh(&mut self, cx: &mut Cx) -> Result<(), JsError> {
        with_env(cx, |env| self.source_list.refresh(env)).map_err(list_cx::js_error)
    }

    fn new_assets(&mut self, cx: &mut Cx) -> Result<(), JsError> {
        self.update_field_options(cx)?;
        self.update_robot_options(cx)
    }

    fn active_fields(&self, _cx: &Cx) -> Vec<JsString> {
        let mut fields = self.source_list.active_fields();
        for key in ALLIANCE_KEYS.iter().chain(DRIVER_STATION_KEYS.iter()) {
            fields.push(JsString::from(*key));
        }
        fields
    }

    fn show_timeline(&self, _cx: &Cx) -> bool {
        true
    }

    fn command(&mut self, cx: &mut Cx, uuid: &JsValue) -> Result<Command, JsError> {
        let render_time = cx.tab_selection().get_render_time();
        let field_data = selected_field(self, cx);
        let coordinate_system: JsString = match cx.tab_preferences().map(|p| p.coordinate_system) {
            Some(CoordinateSystemPreference::Automatic) => field_data
                .as_ref()
                .map(|f| JsString::from(f.coordinate_system.as_str()))
                .unwrap_or_else(|| JsString::from("center-red")),
            Some(preference) => JsString::from(preference.as_str()),
            None => JsString::from("center-red"),
        };
        if let (Some(field), Some(preferences)) = (field_data.as_ref(), cx.tab_preferences()) {
            let alert = if !field.is_ftc && field.coordinate_system != FieldCoordinates::CenterRed {
                CoordinateAlert::Frc
            } else if field.is_ftc && field.coordinate_system == FieldCoordinates::CenterRed {
                CoordinateAlert::Ftc
            } else {
                CoordinateAlert::None
            };
            if alert != self.last_coordinate_alert {
                if preferences.coordinate_system == CoordinateSystemPreference::Automatic {
                    match alert {
                        CoordinateAlert::Frc => cx.send_main_message(NamedMessage::without_data(
                            HubToMain::FrcCoordinateSystemWarning,
                        )),
                        CoordinateAlert::Ftc => cx.send_main_message(NamedMessage::without_data(
                            HubToMain::FtcCoordinateSystemWarning,
                        )),
                        CoordinateAlert::None => {}
                    }
                }
                self.last_coordinate_alert = alert;
            }
        }

        let is_red_alliance = render_time.is_some_and(|t| {
            robot_state::get_is_red_alliance(&cx.tab_log(), t)
        });
        let uuid_str = uuid.as_str().map(|s| s.to_string_lossy());
        let (objects, camera_override) = self.command_objects(
            cx,
            uuid_str.as_deref(),
            render_time,
            field_data.as_ref(),
        )?;

        let mut all_robot_models: Vec<JsValue> = Vec::new();
        for source in self.source_list.state() {
            let model_type = source.item_type.as_string();
            if matches!(model_type.as_deref(), Some("robot") | Some("ghost")) {
                let model = option(&source.options, "model");
                if !all_robot_models.iter().any(|m| ops::same_value_zero(m, &model)) {
                    all_robot_models.push(model);
                }
            }
        }

        Ok(Rc::new(Field3dCommand {
            field: self.field_value(),
            is_red_alliance,
            coordinate_system,
            objects,
            camera_override,
            auto_driver_station: robot_state::get_driver_station(
                &cx.tab_log(),
                render_time.unwrap_or(f64::NAN),
            ),
            all_robot_models,
        }))
    }
}
