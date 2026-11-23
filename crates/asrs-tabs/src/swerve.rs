//! `src/hub/controllers/SwerveController.ts`.

use std::rc::Rc;

use asrs_js::math;
use asrs_js::number;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::geometry::{
    ModuleVelocitySample, RotationUnits, grab_module_velocities, grab_poses_auto,
    grab_robot_velocities, rotation_3d_to_2d,
};
use asrs_log::util::clamp_value;
use asrs_model::source_list::configs;
use asrs_model::source_list::{ListInput, ListOutput, SourceList};
use asrs_model::tab_controller::{Command, TabController};

use crate::cx::TabCx;
use crate::form::NumberInput;
use crate::list_cx::{self, SourceListCx, js_error, source_state, with_env};

/// `Orientation` in `Field2dRenderer.ts`: quarter turns, `DEG_90` by default.
const ORIENTATION_90: f64 = 1.0;

/// The numeric inputs by their saved-state key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwerveInput {
    MaxSpeed,
    SizeX,
    SizeY,
}

impl SwerveInput {
    const ALL: [SwerveInput; 3] = [
        SwerveInput::MaxSpeed,
        SwerveInput::SizeX,
        SwerveInput::SizeY,
    ];

    fn key(self) -> &'static str {
        match self {
            SwerveInput::MaxSpeed => "maxSpeed",
            SwerveInput::SizeX => "sizeX",
            SwerveInput::SizeY => "sizeY",
        }
    }
}

pub struct SwerveController {
    source_list: SourceList,
    max_speed: NumberInput,
    size_x: NumberInput,
    size_y: NumberInput,
    orientation: f64,
}

impl SwerveController {
    /// The source list draws one `createUUID()` after the controller's own.
    pub fn new<Cx: TabCx + ?Sized>(cx: &mut Cx) -> SwerveController {
        let uuid = asrs_js::time::create_uuid(cx.tab_rng());
        SwerveController {
            source_list: SourceList::new(configs::swerve(), uuid, false),
            max_speed: NumberInput::new("5"),
            size_x: NumberInput::new("0.65").with_step("0.01"),
            size_y: NumberInput::new("0.65").with_step("0.01"),
            orientation: ORIENTATION_90,
        }
    }

    pub fn source_list(&self) -> &SourceList {
        &self.source_list
    }

    pub fn source_list_mut(&mut self) -> &mut SourceList {
        &mut self.source_list
    }

    pub fn input(&self, input: SwerveInput) -> &NumberInput {
        match input {
            SwerveInput::MaxSpeed => &self.max_speed,
            SwerveInput::SizeX => &self.size_x,
            SwerveInput::SizeY => &self.size_y,
        }
    }

    pub fn input_mut(&mut self, input: SwerveInput) -> &mut NumberInput {
        match input {
            SwerveInput::MaxSpeed => &mut self.max_speed,
            SwerveInput::SizeX => &mut self.size_x,
            SwerveInput::SizeY => &mut self.size_y,
        }
    }

    /// The `change` listener every numeric input shares.
    pub fn input_changed(&mut self, input: SwerveInput) {
        let input = self.input_mut(input);
        if input.number() <= 0.0 {
            input.set_value(JsString::from("0.1"));
        }
    }

    /// `ORIENTATION_SWITCHER.children[0]` click: counter-clockwise.
    pub fn rotate_left(&mut self) {
        self.orientation += 1.0;
        if self.orientation > 3.0 {
            self.orientation = 0.0;
        }
    }

    /// `children[1]` click: clockwise.
    pub fn rotate_right(&mut self) {
        self.orientation -= 1.0;
        if self.orientation < 0.0 {
            self.orientation = 3.0;
        }
    }
}

fn uuid_arg(uuid: &JsValue) -> Option<String> {
    uuid.as_str().and_then(|s| s.as_string())
}

fn velocity_to_js(sample: &ModuleVelocitySample) -> JsValue {
    match sample {
        ModuleVelocitySample::Full(v) => {
            let mut o = JsObject::new();
            o.insert("velocity", v.velocity.to_js());
            o.insert("angle", v.angle.to_js());
            JsValue::Object(o)
        }
        ModuleVelocitySample::Partial { velocity, angle } => {
            let mut o = JsObject::new();
            o.insert("velocity", velocity.map_or(JsValue::Null, |x| x.to_js()));
            o.insert("angle", angle.map_or(JsValue::Null, |x| x.to_js()));
            JsValue::Object(o)
        }
        ModuleVelocitySample::Undefined => JsValue::Undefined,
    }
}

impl<Cx: SourceListCx + ?Sized> TabController<Cx> for SwerveController {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        let mut o = JsObject::new();
        o.insert("sources", self.source_list.state().to_js());
        for input in SwerveInput::ALL {
            o.insert(input.key(), self.input(input).number().to_js());
        }
        o.insert("orientation", self.orientation.to_js());
        JsValue::Object(o)
    }

    fn restore_state(&mut self, cx: &mut Cx, state: &JsValue) -> Result<(), JsError> {
        let JsValue::Object(state) = state else {
            return Ok(());
        };
        if let Some(sources) = state.get("sources") {
            let items = source_state(sources)?;
            with_env(cx, |env| self.source_list.set_state(env, items)).map_err(js_error)?;
        }
        for input in SwerveInput::ALL {
            if let Some(n) = state.get(input.key()).and_then(JsValue::as_f64) {
                self.input_mut(input)
                    .set_value(JsString::from(number::to_string(n)));
            }
        }
        if let Some(o) = state.get("orientation").and_then(JsValue::as_f64)
            && [0.0, 1.0, 2.0, 3.0].contains(&o)
        {
            self.orientation = o;
        }
        Ok(())
    }

    fn refresh(&mut self, cx: &mut Cx) -> Result<(), JsError> {
        with_env(cx, |env| self.source_list.refresh(env)).map_err(js_error)
    }

    fn new_assets(&mut self, _cx: &mut Cx) -> Result<(), JsError> {
        Ok(())
    }

    fn active_fields(&self, _cx: &Cx) -> Vec<JsString> {
        self.source_list.active_fields()
    }

    fn show_timeline(&self, _cx: &Cx) -> bool {
        true
    }

    fn source_lists(&self) -> Vec<&SourceList> {
        vec![&self.source_list]
    }

    fn source_list_input(
        &mut self,
        cx: &mut Cx,
        input: ListInput<'_>,
        errors: &mut Vec<JsError>,
    ) -> ListOutput {
        list_cx::route_lists(&mut [&mut self.source_list], cx, input, errors)
    }

    fn command(&mut self, cx: &mut Cx, uuid: &JsValue) -> Result<Command, JsError> {
        let time = cx.tab_selection().get_render_time();
        let log = cx.tab_log();
        let time = time.unwrap_or_else(|| log.get_timestamp_range()[1]);
        let uuid = uuid_arg(uuid);
        let uuid = uuid.as_deref();
        let max_speed = self.max_speed.number();

        let mut rotation = 0.0_f64;
        let mut module_entries: Vec<JsValue> = Vec::new();
        let mut robot_entries: Vec<JsValue> = Vec::new();
        for source in self.source_list.displayed_state(&log) {
            let units = match source.options.get("units") {
                Some(u) if u == &JsString::from("degrees") => RotationUnits::Degrees,
                _ => RotationUnits::Radians,
            };
            match source.item_type.as_string().as_deref() {
                Some("moduleVelocities") => {
                    let mut velocities = grab_module_velocities(
                        &log,
                        &source.log_key,
                        time,
                        source.options.get("arrangement"),
                        uuid,
                    );
                    for sample in &mut velocities {
                        match sample {
                            ModuleVelocitySample::Full(v) => {
                                v.velocity = clamp_value(v.velocity / max_speed, -1.0, 1.0);
                            }
                            // `null / x` is `0 / x`; the write turns the slot into a number.
                            ModuleVelocitySample::Partial { velocity, .. } => {
                                *velocity = Some(clamp_value(
                                    velocity.unwrap_or(0.0) / max_speed,
                                    -1.0,
                                    1.0,
                                ));
                            }
                            ModuleVelocitySample::Undefined => {
                                return Err(JsError::type_error(
                                    "Cannot read properties of undefined (reading 'velocity')",
                                ));
                            }
                        }
                    }
                    let mut o = JsObject::new();
                    o.insert(
                        "values",
                        velocities
                            .iter()
                            .map(velocity_to_js)
                            .collect::<Vec<_>>()
                            .to_js(),
                    );
                    o.insert(
                        "color",
                        source
                            .options
                            .get("color")
                            .map_or(JsValue::Undefined, |c| c.clone().to_js()),
                    );
                    module_entries.push(JsValue::Object(o));
                }
                Some("robotVelocities") => {
                    let mut speeds = grab_robot_velocities(&log, &source.log_key, time, uuid);
                    let angle = math::atan2(speeds.vy, speeds.vx);
                    let mut length = math::hypot(&[speeds.vx, speeds.vy]);
                    length = clamp_value(length / max_speed, -1.0, 1.0);
                    speeds.vx = math::cos(angle) * length;
                    speeds.vy = math::sin(angle) * length;
                    let mut s = JsObject::new();
                    s.insert("vx", speeds.vx.to_js());
                    s.insert("vy", speeds.vy.to_js());
                    s.insert("omega", speeds.omega.to_js());
                    let mut o = JsObject::new();
                    o.insert("value", JsValue::Object(s));
                    o.insert(
                        "color",
                        source
                            .options
                            .get("color")
                            .map_or(JsValue::Undefined, |c| c.clone().to_js()),
                    );
                    robot_entries.push(JsValue::Object(o));
                }
                _ => {
                    let poses = grab_poses_auto(
                        &log,
                        &source.log_key,
                        &source.log_type,
                        time,
                        uuid,
                        units,
                    )?;
                    if let Some(first) = poses.first() {
                        rotation = rotation_3d_to_2d(first.pose.rotation);
                    }
                }
            }
        }
        rotation += self.orientation * (std::f64::consts::PI / 2.0);
        module_entries.reverse();
        robot_entries.reverse();

        let mut o = JsObject::new();
        o.insert("rotation", rotation.to_js());
        o.insert(
            "frameAspectRatio",
            (self.size_y.number() / self.size_x.number()).to_js(),
        );
        o.insert("moduleVelocities", module_entries.to_js());
        o.insert("robotVelocities", robot_entries.to_js());
        Ok(Rc::new(JsValue::Object(o)))
    }
}
