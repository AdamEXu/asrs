// Ported from camera-controls 2.9.0 (MIT, Copyright (c) 2017 @yomotsu). See THIRD_PARTY_NOTICES.md.

//! The part of camera-controls 2.9.0 that upstream's `Field3dRendererImpl` drives: mouse
//! rotate, truck and dolly, wheel dolly with `infinityDolly`, the programmatic moves
//! (`rotate`, `dolly`, `dollyInFixed`, `truck`, `elevate`, `moveTo`, `setLookAt`, `setTarget`)
//! with and without transition, and the smooth-damped `update`.
//!
//! Left out because upstream never reaches them: touch and pointer lock, `dollyToCursor`,
//! zoom and focal-offset controls, collider meshes, `setBoundary` (the boundary stays
//! infinite), `fitTo*`, `reset`/`saveState`, `toJSON`, and reassigning `mouseButtons`.
//! Every arithmetic step keeps the library's operation order, so camera state stays
//! bit-identical to it.

#![deny(clippy::disallowed_methods)]

use asrs_js::math;

use crate::three::math_utils::clamp;
use crate::three::{Box3, PerspectiveCamera, Quaternion, Spherical, Vector3};

const PI_2: f64 = math::PI * 2.0;
const EPSILON: f64 = 1e-5;
const DEG2RAD: f64 = math::PI / 180.0;
const TOUCH_DOLLY_FACTOR: f64 = 1.0 / 8.0;

pub mod mouse_button {
    pub const LEFT: u32 = 1;
    pub const RIGHT: u32 = 2;
    pub const MIDDLE: u32 = 4;
}

pub mod action {
    pub const NONE: u32 = 0;
    pub const ROTATE: u32 = 1;
    pub const TRUCK: u32 = 2;
    pub const DOLLY: u32 = 8;
}

const LEFT_ACTION: u32 = action::ROTATE;
const MIDDLE_ACTION: u32 = action::DOLLY;
const RIGHT_ACTION: u32 = action::TRUCK;

const DOLLY_DIRECTION_NONE: f64 = 0.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlsEvent {
    ControlStart,
    Control,
    ControlEnd,
    TransitionStart,
    Update,
    Wake,
    Rest,
    Sleep,
}

impl ControlsEvent {
    pub fn name(self) -> &'static str {
        match self {
            ControlsEvent::ControlStart => "controlstart",
            ControlsEvent::Control => "control",
            ControlsEvent::ControlEnd => "controlend",
            ControlsEvent::TransitionStart => "transitionstart",
            ControlsEvent::Update => "update",
            ControlsEvent::Wake => "wake",
            ControlsEvent::Rest => "rest",
            ControlsEvent::Sleep => "sleep",
        }
    }
}

/// The element's `getBoundingClientRect()` at the time the library reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ElementRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A mouse `PointerEvent` (`pointerType === "mouse"`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerInput {
    pub pointer_id: i32,
    pub buttons: u32,
    pub client_x: f64,
    pub client_y: f64,
}

/// A `WheelEvent`; `delta_mode` 1 is `DOM_DELTA_LINE`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WheelInput {
    pub delta_y: f64,
    pub delta_mode: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pointer {
    pub pointer_id: i32,
    pub client_x: f64,
    pub client_y: f64,
    pub mouse_button: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CameraControls {
    camera: PerspectiveCamera,

    pub min_polar_angle: f64,
    pub max_polar_angle: f64,
    pub min_azimuth_angle: f64,
    pub max_azimuth_angle: f64,
    pub min_distance: f64,
    pub max_distance: f64,
    pub infinity_dolly: bool,
    pub smooth_time: f64,
    pub dragging_smooth_time: f64,
    pub max_speed: f64,
    pub azimuth_rotate_speed: f64,
    pub polar_rotate_speed: f64,
    pub dolly_speed: f64,
    pub dolly_drag_inverted: bool,
    pub truck_speed: f64,
    pub boundary_friction: f64,
    pub rest_threshold: f64,

    enabled: bool,
    state: u32,
    has_rested: bool,
    needs_update: bool,
    updated_last_time: bool,
    element_rect: ElementRect,
    is_dragging: bool,
    drag_needs_update: bool,
    active_pointers: Vec<Pointer>,
    document_listening: bool,
    is_user_controlling_rotate: bool,
    is_user_controlling_dolly: bool,
    is_user_controlling_truck: bool,
    last_dolly_direction: f64,
    changed_dolly: f64,

    theta_velocity: f64,
    phi_velocity: f64,
    radius_velocity: f64,
    target_velocity: Vector3,
    zoom_velocity: f64,

    y_axis_up_space: Quaternion,
    y_axis_up_space_inverse: Quaternion,
    target: Vector3,
    target_end: Vector3,
    spherical: Spherical,
    spherical_end: Spherical,
    last_distance: f64,
    zoom: f64,
    zoom_end: f64,
    last_zoom: f64,
    boundary: Box3,

    drag_start_position: [f64; 2],
    last_drag_position: [f64; 2],
    dolly_start: [f64; 2],

    wheel_delta_y_factor: f64,
    events: Vec<ControlsEvent>,
}

fn approx_zero(number: f64, error: f64) -> bool {
    math::abs(number) < error
}

fn approx_equals(a: f64, b: f64, error: f64) -> bool {
    approx_zero(a - b, error)
}

fn approx_equals_vec3(a: Vector3, b: Vector3, error: f64) -> bool {
    approx_equals(a.x, b.x, error)
        && approx_equals(a.y, b.y, error)
        && approx_equals(a.z, b.z, error)
}

fn clamp_vec3(v: Vector3, min: Vector3, max: Vector3) -> Vector3 {
    Vector3::new(
        math::max2(min.x, math::min2(max.x, v.x)),
        math::max2(min.y, math::min2(max.y, v.y)),
        math::max2(min.z, math::min2(max.z, v.z)),
    )
}

/// Unity's `Mathf.SmoothDamp`, as camera-controls writes it.
fn smooth_damp(
    current: f64,
    target: f64,
    velocity: &mut f64,
    smooth_time: f64,
    max_speed: f64,
    delta_time: f64,
) -> f64 {
    let smooth_time = math::max2(0.0001, smooth_time);
    let omega = 2.0 / smooth_time;
    let x = omega * delta_time;
    let exp = 1.0 / (1.0 + x + 0.48 * x * x + 0.235 * x * x * x);
    let mut change = current - target;
    let original_to = target;
    let max_change = max_speed * smooth_time;
    change = clamp(change, -max_change, max_change);
    let target = current - change;
    let temp = (*velocity + omega * change) * delta_time;
    *velocity = (*velocity - omega * temp) * exp;
    let mut output = target + (change + temp) * exp;
    if (original_to - current > 0.0) == (output > original_to) {
        output = original_to;
        *velocity = (output - original_to) / delta_time;
    }
    output
}

/// `smoothDampVec3(current, target, velocity, ..., out)` with `out === current`, the only way
/// camera-controls calls it. The overshoot test reads `current` after `out` overwrote it, so it
/// compares the new value against itself and never fires for finite input.
fn smooth_damp_vec3_in_place(
    current: &mut Vector3,
    target: Vector3,
    velocity: &mut Vector3,
    smooth_time: f64,
    max_speed: f64,
    delta_time: f64,
) {
    let smooth_time = math::max2(0.0001, smooth_time);
    let omega = 2.0 / smooth_time;
    let x = omega * delta_time;
    let exp = 1.0 / (1.0 + x + 0.48 * x * x + 0.235 * x * x * x);
    let mut change_x = current.x - target.x;
    let mut change_y = current.y - target.y;
    let mut change_z = current.z - target.z;
    let original_to = target;
    let max_change = max_speed * smooth_time;
    let max_change_sq = max_change * max_change;
    let magnitude_sq = change_x * change_x + change_y * change_y + change_z * change_z;
    if magnitude_sq > max_change_sq {
        let magnitude = math::sqrt(magnitude_sq);
        change_x = change_x / magnitude * max_change;
        change_y = change_y / magnitude * max_change;
        change_z = change_z / magnitude * max_change;
    }
    let target_x = current.x - change_x;
    let target_y = current.y - change_y;
    let target_z = current.z - change_z;
    let temp_x = (velocity.x + omega * change_x) * delta_time;
    let temp_y = (velocity.y + omega * change_y) * delta_time;
    let temp_z = (velocity.z + omega * change_z) * delta_time;
    velocity.x = (velocity.x - omega * temp_x) * exp;
    velocity.y = (velocity.y - omega * temp_y) * exp;
    velocity.z = (velocity.z - omega * temp_z) * exp;
    current.x = target_x + (change_x + temp_x) * exp;
    current.y = target_y + (change_y + temp_y) * exp;
    current.z = target_z + (change_z + temp_z) * exp;
    let orig_minus_current = original_to - *current;
    let out_minus_orig = *current - original_to;
    if orig_minus_current.x * out_minus_orig.x
        + orig_minus_current.y * out_minus_orig.y
        + orig_minus_current.z * out_minus_orig.z
        > 0.0
    {
        *current = original_to;
        velocity.x = (current.x - original_to.x) / delta_time;
        velocity.y = (current.y - original_to.y) / delta_time;
        velocity.z = (current.z - original_to.z) / delta_time;
    }
}

fn average_client_coord(pointers: &[Pointer]) -> [f64; 2] {
    let mut out = [0.0, 0.0];
    for pointer in pointers {
        out[0] += pointer.client_x;
        out[1] += pointer.client_y;
    }
    let count = pointers.len() as f64;
    out[0] /= count;
    out[1] /= count;
    out
}

impl CameraControls {
    /// `new CameraControls(camera, domElement)`. `element_rect` is the element's bounding rect
    /// at construction; `mac` is the library's `/Mac/.test(navigator.platform)`, which picks
    /// the wheel scale.
    pub fn new(camera: PerspectiveCamera, element_rect: ElementRect, mac: bool) -> Self {
        let y_axis_up_space =
            Quaternion::from_unit_vectors(camera.object.up, Vector3::new(0.0, 1.0, 0.0));
        let y_axis_up_space_inverse = y_axis_up_space.invert();
        let spherical =
            Spherical::from_vector3(camera.object.position.apply_quaternion(&y_axis_up_space));
        let zoom = camera.zoom;
        let mut controls = CameraControls {
            camera,
            min_polar_angle: 0.0,
            max_polar_angle: math::PI,
            min_azimuth_angle: f64::NEG_INFINITY,
            max_azimuth_angle: f64::INFINITY,
            min_distance: f64::EPSILON,
            max_distance: f64::INFINITY,
            infinity_dolly: false,
            smooth_time: 0.25,
            dragging_smooth_time: 0.125,
            max_speed: f64::INFINITY,
            azimuth_rotate_speed: 1.0,
            polar_rotate_speed: 1.0,
            dolly_speed: 1.0,
            dolly_drag_inverted: false,
            truck_speed: 2.0,
            boundary_friction: 0.0,
            rest_threshold: 0.01,
            enabled: true,
            state: action::NONE,
            has_rested: true,
            needs_update: true,
            updated_last_time: false,
            element_rect,
            is_dragging: false,
            drag_needs_update: true,
            active_pointers: Vec::new(),
            document_listening: false,
            is_user_controlling_rotate: false,
            is_user_controlling_dolly: false,
            is_user_controlling_truck: false,
            last_dolly_direction: DOLLY_DIRECTION_NONE,
            changed_dolly: 0.0,
            theta_velocity: 0.0,
            phi_velocity: 0.0,
            radius_velocity: 0.0,
            target_velocity: Vector3::default(),
            zoom_velocity: 0.0,
            y_axis_up_space,
            y_axis_up_space_inverse,
            target: Vector3::default(),
            target_end: Vector3::default(),
            spherical,
            spherical_end: spherical,
            last_distance: spherical.radius,
            zoom,
            zoom_end: zoom,
            last_zoom: zoom,
            boundary: Box3 {
                min: Vector3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
                max: Vector3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
            },
            drag_start_position: [0.0, 0.0],
            last_drag_position: [0.0, 0.0],
            dolly_start: [0.0, 0.0],
            wheel_delta_y_factor: if mac { -1.0 } else { -3.0 },
            events: Vec::new(),
        };
        controls.update(0.0);
        controls
    }

    pub fn camera(&self) -> &PerspectiveCamera {
        &self.camera
    }

    pub fn camera_mut(&mut self) -> &mut PerspectiveCamera {
        &mut self.camera
    }

    pub fn take_events(&mut self) -> Vec<ControlsEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.cancel();
        }
    }

    pub fn active(&self) -> bool {
        !self.has_rested
    }

    pub fn current_action(&self) -> u32 {
        self.state
    }

    pub fn distance(&self) -> f64 {
        self.spherical.radius
    }

    pub fn azimuth_angle(&self) -> f64 {
        self.spherical.theta
    }

    pub fn polar_angle(&self) -> f64 {
        self.spherical.phi
    }

    /// The document-level `pointermove`/`pointerup` listeners are attached only while a drag
    /// is in progress; events outside that window never reach the controls.
    pub fn is_listening_to_document(&self) -> bool {
        self.document_listening
    }

    fn create_on_rest_promise(&mut self, resolve_immediately: bool) {
        if resolve_immediately {
            return;
        }
        self.has_rested = false;
        self.events.push(ControlsEvent::TransitionStart);
    }

    pub fn rotate(&mut self, azimuth_angle: f64, polar_angle: f64, enable_transition: bool) {
        self.rotate_to(
            self.spherical_end.theta + azimuth_angle,
            self.spherical_end.phi + polar_angle,
            enable_transition,
        );
    }

    pub fn rotate_to(&mut self, azimuth_angle: f64, polar_angle: f64, enable_transition: bool) {
        self.is_user_controlling_rotate = false;
        let theta = clamp(
            azimuth_angle,
            self.min_azimuth_angle,
            self.max_azimuth_angle,
        );
        let phi = clamp(polar_angle, self.min_polar_angle, self.max_polar_angle);
        self.spherical_end.theta = theta;
        self.spherical_end.phi = phi;
        self.spherical_end.make_safe();
        self.needs_update = true;
        if !enable_transition {
            self.spherical.theta = self.spherical_end.theta;
            self.spherical.phi = self.spherical_end.phi;
        }
        let resolve_immediately = !enable_transition
            || approx_equals(
                self.spherical.theta,
                self.spherical_end.theta,
                self.rest_threshold,
            ) && approx_equals(
                self.spherical.phi,
                self.spherical_end.phi,
                self.rest_threshold,
            );
        self.create_on_rest_promise(resolve_immediately);
    }

    pub fn dolly(&mut self, distance: f64, enable_transition: bool) {
        self.dolly_to(self.spherical_end.radius - distance, enable_transition);
    }

    pub fn dolly_to(&mut self, distance: f64, enable_transition: bool) {
        self.is_user_controlling_dolly = false;
        self.last_dolly_direction = DOLLY_DIRECTION_NONE;
        self.changed_dolly = 0.0;
        self.dolly_to_no_clamp(
            clamp(distance, self.min_distance, self.max_distance),
            enable_transition,
        );
    }

    fn dolly_to_no_clamp(&mut self, distance: f64, enable_transition: bool) {
        self.spherical_end.radius = distance;
        self.needs_update = true;
        if !enable_transition {
            self.spherical.radius = self.spherical_end.radius;
        }
        let resolve_immediately = !enable_transition
            || approx_equals(
                self.spherical.radius,
                self.spherical_end.radius,
                self.rest_threshold,
            );
        self.create_on_rest_promise(resolve_immediately);
    }

    pub fn dolly_in_fixed(&mut self, distance: f64, enable_transition: bool) {
        self.target_end = self.target_end + self.camera_direction().multiply_scalar(distance);
        if !enable_transition {
            self.target = self.target_end;
        }
        let resolve_immediately = !enable_transition
            || approx_equals_vec3(self.target, self.target_end, self.rest_threshold);
        self.create_on_rest_promise(resolve_immediately);
    }

    pub fn truck(&mut self, x: f64, y: f64, enable_transition: bool) {
        self.camera.update_matrix();
        let x_column =
            Vector3::from_matrix_column(&self.camera.object.matrix, 0).multiply_scalar(x);
        let y_column =
            Vector3::from_matrix_column(&self.camera.object.matrix, 1).multiply_scalar(-y);
        let offset = x_column + y_column;
        let to = self.target_end + offset;
        self.move_to(to.x, to.y, to.z, enable_transition);
    }

    pub fn elevate(&mut self, height: f64, enable_transition: bool) {
        let v = self.camera.object.up.multiply_scalar(height);
        self.move_to(
            self.target_end.x + v.x,
            self.target_end.y + v.y,
            self.target_end.z + v.z,
            enable_transition,
        );
    }

    pub fn move_to(&mut self, x: f64, y: f64, z: f64, enable_transition: bool) {
        self.is_user_controlling_truck = false;
        let offset = Vector3::new(x, y, z) - self.target_end;
        self.target_end = self.enclose_to_boundary(self.target_end, offset, self.boundary_friction);
        self.needs_update = true;
        if !enable_transition {
            self.target = self.target_end;
        }
        let resolve_immediately = !enable_transition
            || approx_equals_vec3(self.target, self.target_end, self.rest_threshold);
        self.create_on_rest_promise(resolve_immediately);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_look_at(
        &mut self,
        position_x: f64,
        position_y: f64,
        position_z: f64,
        target_x: f64,
        target_y: f64,
        target_z: f64,
        enable_transition: bool,
    ) {
        self.is_user_controlling_rotate = false;
        self.is_user_controlling_dolly = false;
        self.is_user_controlling_truck = false;
        self.last_dolly_direction = DOLLY_DIRECTION_NONE;
        self.changed_dolly = 0.0;
        let target = Vector3::new(target_x, target_y, target_z);
        let position = Vector3::new(position_x, position_y, position_z);
        self.target_end = target;
        self.spherical_end =
            Spherical::from_vector3((position - target).apply_quaternion(&self.y_axis_up_space));
        self.normalize_rotations();
        self.needs_update = true;
        if !enable_transition {
            self.target = self.target_end;
            self.spherical = self.spherical_end;
        }
        let resolve_immediately = !enable_transition
            || approx_equals_vec3(self.target, self.target_end, self.rest_threshold)
                && approx_equals(
                    self.spherical.theta,
                    self.spherical_end.theta,
                    self.rest_threshold,
                )
                && approx_equals(
                    self.spherical.phi,
                    self.spherical_end.phi,
                    self.rest_threshold,
                )
                && approx_equals(
                    self.spherical.radius,
                    self.spherical_end.radius,
                    self.rest_threshold,
                );
        self.create_on_rest_promise(resolve_immediately);
    }

    pub fn set_position(&mut self, x: f64, y: f64, z: f64, enable_transition: bool) {
        let t = self.target_end;
        self.set_look_at(x, y, z, t.x, t.y, t.z, enable_transition);
    }

    pub fn set_target(&mut self, x: f64, y: f64, z: f64, enable_transition: bool) {
        let pos = self.get_position(true);
        self.set_look_at(pos.x, pos.y, pos.z, x, y, z, enable_transition);
        self.spherical_end.phi = clamp(
            self.spherical_end.phi,
            self.min_polar_angle,
            self.max_polar_angle,
        );
    }

    pub fn get_target(&self, receive_end_value: bool) -> Vector3 {
        if receive_end_value {
            self.target_end
        } else {
            self.target
        }
    }

    pub fn get_position(&self, receive_end_value: bool) -> Vector3 {
        let (spherical, target) = if receive_end_value {
            (self.spherical_end, self.target_end)
        } else {
            (self.spherical, self.target)
        };
        spherical
            .to_vector3()
            .apply_quaternion(&self.y_axis_up_space_inverse)
            + target
    }

    pub fn get_spherical(&self, receive_end_value: bool) -> Spherical {
        if receive_end_value {
            self.spherical_end
        } else {
            self.spherical
        }
    }

    pub fn normalize_rotations(&mut self) {
        self.spherical_end.theta = math::rem(self.spherical_end.theta, PI_2);
        if self.spherical_end.theta < 0.0 {
            self.spherical_end.theta += PI_2;
        }
        self.spherical.theta +=
            PI_2 * math::round((self.spherical_end.theta - self.spherical.theta) / PI_2);
    }

    pub fn stop(&mut self) {
        self.target = self.target_end;
        self.spherical = self.spherical_end;
        self.zoom = self.zoom_end;
    }

    pub fn update(&mut self, delta: f64) -> bool {
        let delta_theta = self.spherical_end.theta - self.spherical.theta;
        let delta_phi = self.spherical_end.phi - self.spherical.phi;
        let delta_radius = self.spherical_end.radius - self.spherical.radius;
        let delta_target = self.target_end - self.target;
        let delta_zoom = self.zoom_end - self.zoom;

        if approx_zero(delta_theta, EPSILON) {
            self.theta_velocity = 0.0;
            self.spherical.theta = self.spherical_end.theta;
        } else {
            let smooth_time = self.rotate_smooth_time();
            self.spherical.theta = smooth_damp(
                self.spherical.theta,
                self.spherical_end.theta,
                &mut self.theta_velocity,
                smooth_time,
                f64::INFINITY,
                delta,
            );
            self.needs_update = true;
        }

        if approx_zero(delta_phi, EPSILON) {
            self.phi_velocity = 0.0;
            self.spherical.phi = self.spherical_end.phi;
        } else {
            let smooth_time = self.rotate_smooth_time();
            self.spherical.phi = smooth_damp(
                self.spherical.phi,
                self.spherical_end.phi,
                &mut self.phi_velocity,
                smooth_time,
                f64::INFINITY,
                delta,
            );
            self.needs_update = true;
        }

        if approx_zero(delta_radius, EPSILON) {
            self.radius_velocity = 0.0;
            self.spherical.radius = self.spherical_end.radius;
        } else {
            let smooth_time = if self.is_user_controlling_dolly {
                self.dragging_smooth_time
            } else {
                self.smooth_time
            };
            self.spherical.radius = smooth_damp(
                self.spherical.radius,
                self.spherical_end.radius,
                &mut self.radius_velocity,
                smooth_time,
                self.max_speed,
                delta,
            );
            self.needs_update = true;
        }

        if approx_zero(delta_target.x, EPSILON)
            && approx_zero(delta_target.y, EPSILON)
            && approx_zero(delta_target.z, EPSILON)
        {
            self.target_velocity = Vector3::default();
            self.target = self.target_end;
        } else {
            let smooth_time = if self.is_user_controlling_truck {
                self.dragging_smooth_time
            } else {
                self.smooth_time
            };
            smooth_damp_vec3_in_place(
                &mut self.target,
                self.target_end,
                &mut self.target_velocity,
                smooth_time,
                self.max_speed,
                delta,
            );
            self.needs_update = true;
        }

        if approx_zero(delta_zoom, EPSILON) {
            self.zoom_velocity = 0.0;
            self.zoom = self.zoom_end;
        } else {
            self.zoom = smooth_damp(
                self.zoom,
                self.zoom_end,
                &mut self.zoom_velocity,
                self.smooth_time,
                f64::INFINITY,
                delta,
            );
        }

        if self.camera.zoom != self.zoom {
            self.camera.zoom = self.zoom;
            self.camera.update_projection_matrix();
            self.needs_update = true;
        }

        self.drag_needs_update = true;

        self.spherical.radius = math::min2(self.spherical.radius, f64::INFINITY);
        self.spherical.make_safe();
        self.camera.object.position = self
            .spherical
            .to_vector3()
            .apply_quaternion(&self.y_axis_up_space_inverse)
            + self.target;
        self.camera.look_at(self.target);

        let updated = self.needs_update;
        if updated && !self.updated_last_time {
            self.has_rested = false;
            self.events.push(ControlsEvent::Wake);
            self.events.push(ControlsEvent::Update);
        } else if updated {
            self.events.push(ControlsEvent::Update);
            let rest = self.rest_threshold;
            if approx_zero(delta_theta, rest)
                && approx_zero(delta_phi, rest)
                && approx_zero(delta_radius, rest)
                && approx_zero(delta_target.x, rest)
                && approx_zero(delta_target.y, rest)
                && approx_zero(delta_target.z, rest)
                && approx_zero(delta_zoom, rest)
                && !self.has_rested
            {
                self.has_rested = true;
                self.events.push(ControlsEvent::Rest);
            }
        } else if self.updated_last_time {
            self.events.push(ControlsEvent::Sleep);
        }

        self.last_distance = self.spherical.radius;
        self.last_zoom = self.zoom;
        self.updated_last_time = updated;
        self.needs_update = false;
        updated
    }

    fn rotate_smooth_time(&self) -> f64 {
        if self.is_user_controlling_rotate {
            self.dragging_smooth_time
        } else {
            self.smooth_time
        }
    }

    fn camera_direction(&self) -> Vector3 {
        -self
            .spherical
            .to_vector3()
            .divide_scalar(self.spherical.radius)
            .apply_quaternion(&self.y_axis_up_space_inverse)
    }

    fn enclose_to_boundary(&self, position: Vector3, offset: Vector3, friction: f64) -> Vector3 {
        let offset_length2 = offset.length_sq();
        if offset_length2 == 0.0 {
            return position;
        }
        let new_target = offset + position;
        let clamped_target = clamp_vec3(new_target, self.boundary.min, self.boundary.max);
        let delta_clamped_target = clamped_target - new_target;
        let delta_clamped_target_length2 = delta_clamped_target.length_sq();
        if delta_clamped_target_length2 == 0.0 {
            position + offset
        } else if delta_clamped_target_length2 == offset_length2 {
            position
        } else if friction == 0.0 {
            position + offset + delta_clamped_target
        } else {
            let offset_factor =
                1.0 + friction * delta_clamped_target_length2 / offset.dot(delta_clamped_target);
            position
                + offset.multiply_scalar(offset_factor)
                + delta_clamped_target.multiply_scalar(1.0 - friction)
        }
    }

    fn truck_internal(&mut self, delta_x: f64, delta_y: f64) {
        let offset = self.camera.object.position - self.target;
        let fov = self.camera.effective_fov() * DEG2RAD;
        let target_distance = offset.length() * math::tan(fov * 0.5);
        let truck_x = self.truck_speed * delta_x * target_distance / self.element_rect.height;
        let pedestal_y = self.truck_speed * delta_y * target_distance / self.element_rect.height;
        self.truck(truck_x, pedestal_y, true);
    }

    fn rotate_internal(&mut self, delta_x: f64, delta_y: f64) {
        let theta = PI_2 * self.azimuth_rotate_speed * delta_x / self.element_rect.height;
        let phi = PI_2 * self.polar_rotate_speed * delta_y / self.element_rect.height;
        self.rotate(theta, phi, true);
    }

    fn dolly_internal(&mut self, delta: f64) {
        let dolly_scale = math::pow(0.95, -delta * self.dolly_speed);
        let distance = self.spherical_end.radius * dolly_scale;
        let clamped_distance = clamp(distance, self.min_distance, self.max_distance);
        let overflowed_distance = clamped_distance - distance;
        if self.infinity_dolly {
            self.dolly_in_fixed(overflowed_distance, true);
        }
        self.dolly_to_no_clamp(clamped_distance, true);
        self.last_dolly_direction = math::sign(-delta);
    }

    fn find_pointer_by_id(&self, pointer_id: i32) -> Option<usize> {
        self.active_pointers
            .iter()
            .position(|p| p.pointer_id == pointer_id)
    }

    fn mouse_state(buttons: u32) -> u32 {
        let mut state = action::NONE;
        if buttons & mouse_button::LEFT == mouse_button::LEFT {
            state |= LEFT_ACTION;
        }
        if buttons & mouse_button::MIDDLE == mouse_button::MIDDLE {
            state |= MIDDLE_ACTION;
        }
        if buttons & mouse_button::RIGHT == mouse_button::RIGHT {
            state |= RIGHT_ACTION;
        }
        state
    }

    /// `pointerdown` on the element. `element_rect` is its bounding rect at that moment.
    pub fn pointer_down(&mut self, event: PointerInput, element_rect: ElementRect) {
        if !self.enabled {
            return;
        }
        let mouse_button = [
            mouse_button::LEFT,
            mouse_button::MIDDLE,
            mouse_button::RIGHT,
        ]
        .into_iter()
        .find(|&bit| event.buttons & bit == bit);
        if let Some(button) = mouse_button
            && let Some(i) = self
                .active_pointers
                .iter()
                .position(|p| p.mouse_button == Some(button))
        {
            self.active_pointers.remove(i);
        }
        self.active_pointers.push(Pointer {
            pointer_id: event.pointer_id,
            client_x: event.client_x,
            client_y: event.client_y,
            mouse_button,
        });
        self.document_listening = true;
        self.is_dragging = true;
        self.start_dragging(event.buttons, element_rect);
    }

    /// `pointermove` on the document; ignored unless a drag attached the listener.
    pub fn pointer_move(&mut self, event: PointerInput) {
        if !self.document_listening {
            return;
        }
        let Some(i) = self.find_pointer_by_id(event.pointer_id) else {
            return;
        };
        self.active_pointers[i].client_x = event.client_x;
        self.active_pointers[i].client_y = event.client_y;
        self.state = if self.is_dragging {
            Self::mouse_state(event.buttons)
        } else {
            action::NONE
        };
        self.dragging();
    }

    /// `pointerup` on the document; ignored unless a drag attached the listener.
    pub fn pointer_up(&mut self, event: PointerInput) {
        if self.document_listening {
            self.on_pointer_up(event);
        }
    }

    /// `pointercancel` on the element, which is always listened to.
    pub fn pointer_cancel(&mut self, event: PointerInput) {
        self.on_pointer_up(event);
    }

    fn on_pointer_up(&mut self, event: PointerInput) {
        if let Some(i) = self.find_pointer_by_id(event.pointer_id) {
            self.active_pointers.remove(i);
        }
        self.state = action::NONE;
        self.end_dragging();
    }

    pub fn wheel(&mut self, event: WheelInput) {
        if !self.enabled {
            return;
        }
        let delta = if event.delta_mode == 1 {
            event.delta_y / self.wheel_delta_y_factor
        } else {
            event.delta_y / (self.wheel_delta_y_factor * 10.0)
        };
        self.dolly_internal(-delta);
        self.is_user_controlling_dolly = true;
        self.events.push(ControlsEvent::Control);
    }

    pub fn cancel(&mut self) {
        if self.state == action::NONE {
            return;
        }
        self.state = action::NONE;
        self.active_pointers.clear();
        self.end_dragging();
    }

    fn start_dragging(&mut self, buttons: u32, element_rect: ElementRect) {
        if !self.enabled {
            return;
        }
        let v2 = average_client_coord(&self.active_pointers);
        self.element_rect = element_rect;
        self.drag_start_position = v2;
        self.last_drag_position = v2;
        if self.active_pointers.len() >= 2 {
            let dx = v2[0] - self.active_pointers[1].client_x;
            let dy = v2[1] - self.active_pointers[1].client_y;
            let distance = math::sqrt(dx * dx + dy * dy);
            self.dolly_start = [0.0, distance];
            let x = (self.active_pointers[0].client_x + self.active_pointers[1].client_x) * 0.5;
            let y = (self.active_pointers[0].client_y + self.active_pointers[1].client_y) * 0.5;
            self.last_drag_position = [x, y];
        }
        self.state = Self::mouse_state(buttons);
        if self.state & action::ROTATE == action::ROTATE {
            self.spherical_end.theta = self.spherical.theta;
            self.spherical_end.phi = self.spherical.phi;
            self.theta_velocity = 0.0;
            self.phi_velocity = 0.0;
        }
        if self.state & action::TRUCK == action::TRUCK {
            self.target_end = self.target;
            self.target_velocity = Vector3::default();
        }
        if self.state & action::DOLLY == action::DOLLY {
            self.spherical_end.radius = self.spherical.radius;
            self.radius_velocity = 0.0;
        }
        self.events.push(ControlsEvent::ControlStart);
    }

    fn dragging(&mut self) {
        if !self.enabled || !self.drag_needs_update {
            return;
        }
        self.drag_needs_update = false;
        let v2 = average_client_coord(&self.active_pointers);
        let delta_x = self.last_drag_position[0] - v2[0];
        let delta_y = self.last_drag_position[1] - v2[1];
        self.last_drag_position = v2;
        if self.state & action::ROTATE == action::ROTATE {
            self.rotate_internal(delta_x, delta_y);
            self.is_user_controlling_rotate = true;
        }
        if self.state & action::DOLLY == action::DOLLY {
            let dolly_direction = if self.dolly_drag_inverted { -1.0 } else { 1.0 };
            self.dolly_internal(dolly_direction * delta_y * TOUCH_DOLLY_FACTOR);
            self.is_user_controlling_dolly = true;
        }
        if self.state & action::TRUCK == action::TRUCK {
            self.truck_internal(delta_x, delta_y);
            self.is_user_controlling_truck = true;
        }
        self.events.push(ControlsEvent::Control);
    }

    fn end_dragging(&mut self) {
        self.last_drag_position = average_client_coord(&self.active_pointers);
        self.drag_needs_update = false;
        if self.active_pointers.is_empty() {
            self.is_dragging = false;
            self.document_listening = false;
            self.events.push(ControlsEvent::ControlEnd);
        }
    }

    pub fn debug_state(&self) -> ControlsDebugState {
        ControlsDebugState {
            spherical: self.spherical,
            spherical_end: self.spherical_end,
            target: self.target,
            target_end: self.target_end,
            velocities: [
                self.theta_velocity,
                self.phi_velocity,
                self.radius_velocity,
                self.zoom_velocity,
            ],
            target_velocity: self.target_velocity,
            state: self.state,
            enabled: self.enabled,
            is_dragging: self.is_dragging,
            drag_needs_update: self.drag_needs_update,
            needs_update: self.needs_update,
            updated_last_time: self.updated_last_time,
            has_rested: self.has_rested,
            user_controlling: [
                self.is_user_controlling_rotate,
                self.is_user_controlling_dolly,
                self.is_user_controlling_truck,
            ],
            last_dolly_direction: self.last_dolly_direction,
            changed_dolly: self.changed_dolly,
            last_distance: self.last_distance,
            zoom: [self.zoom, self.zoom_end, self.last_zoom],
            element_rect: self.element_rect,
            active_pointers: self.active_pointers.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ControlsDebugState {
    pub spherical: Spherical,
    pub spherical_end: Spherical,
    pub target: Vector3,
    pub target_end: Vector3,
    pub velocities: [f64; 4],
    pub target_velocity: Vector3,
    pub state: u32,
    pub enabled: bool,
    pub is_dragging: bool,
    pub drag_needs_update: bool,
    pub needs_update: bool,
    pub updated_last_time: bool,
    pub has_rested: bool,
    pub user_controlling: [bool; 3],
    pub last_dolly_direction: f64,
    pub changed_dolly: f64,
    pub last_distance: f64,
    pub zoom: [f64; 3],
    pub element_rect: ElementRect,
    pub active_pointers: Vec<Pointer>,
}
