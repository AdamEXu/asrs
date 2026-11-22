//! `src/hub/controllers/Field2dController.ts` on `configs::field_2d`: the field/size/orientation
//! controls and the per-frame command built from the source list. The renderer is in
//! [`renderer`].

mod command;
mod renderer;

pub use command::{Field2dCommand, Object, Orientation, SwerveModules};
pub use renderer::{
    CAMERA_ROBOT, CAMERA_ROBOT_AND_ROTATION, CAMERA_UNLOCKED, CameraMode, CanvasStyle, Field2dDom,
    Field2dRenderer, HEATMAP_IMAGE, ScrollArgs,
};

use std::collections::HashMap;
use std::rc::Rc;

use asrs_js::math;
use asrs_js::string::{JsError, JsString};
use asrs_js::time::create_uuid;
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::geometry::{
    self, Alliance, AnnotatedPose2d, AnnotatedPose3d, HeatmapTimeRange, RotationUnits,
    annotated_pose_3d_to_2d, convert_from_coordinate_system, grab_heatmap_data,
    grab_module_velocities, grab_poses_auto, rotation_3d_to_2d,
};
use asrs_log::log::robot_state::{ALLIANCE_KEYS, get_is_red_alliance};
use asrs_log::units;
use asrs_model::assets::{AdvantageScopeAssets, Config2d};
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::preferences::CoordinateSystemPreference;
use asrs_model::source_list::{SourceList, configs};
use asrs_model::source_list_config::SourceListItemState;
use asrs_model::tab_controller::{Command, TabController};

use crate::cx::TabCx;
use crate::list_cx::{self, SourceListCx, js_error, with_env};

const TRAIL_LENGTH_SECS: f64 = 3.0;
const TRAIL_DT: f64 = 0.1;
const HEATMAP_DT: f64 = 0.25;

fn js_eq(s: &JsString, lit: &str) -> bool {
    s.units().iter().copied().eq(lit.encode_utf16())
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

/// `Field2dController`. Its `UUID` belongs to `Tabs` (drawn before construction), so the
/// constructor only draws the `SourceList`'s.
pub struct Field2dController {
    list: SourceList,
    /// The `FIELD_SELECT`/settings DOM state this controller mutates.
    pub controls: Field2dControls,
    orientation: Orientation,
    size: SizeSetting,
    last_coordinate_system_alert: CoordAlert,
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
        let prefers_ftc = cx.tab_preferences().is_some_and(|p| p.prefers_ftc);
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
            if let Some(preferences) = cx.tab_preferences_mut() {
                preferences.prefers_ftc = is_ftc;
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
            list_cx::set_source_state(&mut self.list, cx, sources)?;
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
                    )?;
                }
            } else {
                pose3ds = grab_heatmap_data(
                    &log,
                    &source.log_key,
                    &source.log_type,
                    heatmap_time_range(&source),
                    heatmap_range.get(&i).copied().flatten(),
                    uuid_str.as_deref(),
                )?;
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
                    let mut trail_poses: Vec<AnnotatedPose2d> = grab_poses_auto(
                        &log,
                        &source.log_key,
                        &source.log_type,
                        sample,
                        uuid_str.as_deref(),
                        RotationUnits::Radians,
                    )?
                    .into_iter()
                    .map(annotated_pose_3d_to_2d)
                    .collect();
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
                    )?;
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
                    )?;
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
