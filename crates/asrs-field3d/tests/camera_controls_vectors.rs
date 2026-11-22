use asrs_field3d::camera_controls::{
    CameraControls, ControlsEvent, ElementRect, PointerInput, WheelInput,
};
use asrs_field3d::three::{Object3D, PerspectiveCamera, Quaternion, Spherical, Vector3};
use asrs_parity::{Record, Value, diff, record_vectors, vectors};

type Result<T> = std::result::Result<T, String>;

fn value(record: &Record, key: &str) -> Result<Value> {
    record.value(key).map_err(|e| format!("{e}"))
}

fn items(v: &Value) -> Result<&[Value]> {
    v.as_array()
        .ok_or_else(|| format!("expected an array, got {}", v.kind()))
}

fn number(v: &Value) -> Result<f64> {
    v.as_f64()
        .ok_or_else(|| format!("expected a number, got {}", v.kind()))
}

fn boolean(v: &Value) -> Result<bool> {
    v.as_bool()
        .ok_or_else(|| format!("expected a bool, got {}", v.kind()))
}

fn text(v: &Value) -> Result<String> {
    v.as_str()
        .and_then(|s| s.as_string())
        .ok_or_else(|| format!("expected a string, got {}", v.kind()))
}

fn numbers(v: &Value) -> Result<Vec<f64>> {
    items(v)?.iter().map(number).collect()
}

fn num(x: f64) -> Value {
    Value::number(x)
}

fn nums(xs: &[f64]) -> Value {
    Value::Array(xs.iter().map(|&x| num(x)).collect())
}

fn vec3(v: Vector3) -> Value {
    nums(&v.to_array())
}

fn sph(s: Spherical) -> Value {
    nums(&[s.radius, s.phi, s.theta])
}

const CANONICAL_NAN: u64 = 0x7ff8_0000_0000_0000;

/// NaN bits are left out, as in the three vectors: V8 and LLVM pick NaN signs and payloads
/// differently, and a NaN camera state only reaches saved layouts as JSON `null`.
fn canonical(v: &Value) -> Value {
    match v {
        Value::Number(bits) if f64::from_bits(*bits).is_nan() => Value::Number(CANONICAL_NAN),
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

fn compare(expected: &Value, actual: &Value) -> Result<()> {
    let (expected, actual) = (canonical(expected), canonical(actual));
    if vectors::pow_values_match(&expected, &actual) {
        return Ok(());
    }
    Err(diff::report(&expected, &actual).unwrap_or_else(|| "encodings differ".to_owned()))
}

fn rect(v: &[f64]) -> ElementRect {
    ElementRect {
        x: v[0],
        y: v[1],
        width: v[2],
        height: v[3],
    }
}

fn state(controls: &CameraControls) -> Value {
    let s = controls.debug_state();
    let camera = controls.camera();
    let rotation = camera.object.rotation();
    Value::Array(vec![
        sph(s.spherical),
        sph(s.spherical_end),
        vec3(s.target),
        vec3(s.target_end),
        nums(&s.velocities),
        vec3(s.target_velocity),
        num(f64::from(s.state)),
        Value::Bool(s.enabled),
        Value::Bool(s.is_dragging),
        Value::Bool(s.drag_needs_update),
        Value::Bool(s.needs_update),
        Value::Bool(s.updated_last_time),
        Value::Bool(s.has_rested),
        Value::Array(s.user_controlling.iter().map(|&b| Value::Bool(b)).collect()),
        num(s.last_dolly_direction),
        num(s.changed_dolly),
        num(s.last_distance),
        nums(&s.zoom),
        nums(&[
            s.element_rect.x,
            s.element_rect.y,
            s.element_rect.width,
            s.element_rect.height,
        ]),
        Value::Array(
            s.active_pointers
                .iter()
                .map(|p| {
                    Value::Array(vec![
                        num(f64::from(p.pointer_id)),
                        num(p.client_x),
                        num(p.client_y),
                        p.mouse_button.map_or(Value::Null, |b| num(f64::from(b))),
                    ])
                })
                .collect(),
        ),
        Value::Bool(controls.is_listening_to_document()),
        Value::Array(vec![
            num(controls.distance()),
            num(controls.azimuth_angle()),
            num(controls.polar_angle()),
            Value::Bool(controls.active()),
        ]),
        vec3(controls.get_position(true)),
        vec3(controls.get_position(false)),
        vec3(controls.get_target(false)),
        vec3(camera.object.position),
        nums(&camera.object.quaternion().to_array()),
        nums(&[rotation.x, rotation.y, rotation.z]),
        nums(&camera.object.matrix.elements),
        Value::Bool(camera.object.matrix_world_needs_update),
        num(camera.zoom),
    ])
}

fn events(list: Vec<ControlsEvent>) -> Value {
    Value::Array(list.into_iter().map(|e| Value::string(e.name())).collect())
}

fn set(controls: &mut CameraControls, key: &str, v: &Value) -> Result<()> {
    match key {
        "enabled" => controls.set_enabled(boolean(v)?),
        "infinityDolly" => controls.infinity_dolly = boolean(v)?,
        "dollyDragInverted" => controls.dolly_drag_inverted = boolean(v)?,
        _ => {
            let x = number(v)?;
            let field = match key {
                "minDistance" => &mut controls.min_distance,
                "maxDistance" => &mut controls.max_distance,
                "smoothTime" => &mut controls.smooth_time,
                "draggingSmoothTime" => &mut controls.dragging_smooth_time,
                "dollySpeed" => &mut controls.dolly_speed,
                "truckSpeed" => &mut controls.truck_speed,
                "azimuthRotateSpeed" => &mut controls.azimuth_rotate_speed,
                "polarRotateSpeed" => &mut controls.polar_rotate_speed,
                "minPolarAngle" => &mut controls.min_polar_angle,
                "maxPolarAngle" => &mut controls.max_polar_angle,
                "minAzimuthAngle" => &mut controls.min_azimuth_angle,
                "maxAzimuthAngle" => &mut controls.max_azimuth_angle,
                "maxSpeed" => &mut controls.max_speed,
                "restThreshold" => &mut controls.rest_threshold,
                "boundaryFriction" => &mut controls.boundary_friction,
                other => return Err(format!("unknown property {other}")),
            };
            *field = x;
        }
    }
    Ok(())
}

/// `Field3dRendererImpl.render`'s keyboard `rotate` helper.
fn key_rotate(controls: &mut CameraControls, x: f64, y: f64) {
    let camera = &controls.camera().object;
    let mut reference = Object3D::new();
    reference.position = camera.position;
    reference.set_rotation(camera.rotation());
    reference.rotate_y(x);
    if controls.polar_angle() + y > controls.min_polar_angle + 0.2
        && controls.polar_angle() + y < controls.max_polar_angle - 0.2
    {
        reference.rotate_x(y);
    }
    reference.translate_z(-controls.distance());
    let p = reference.position;
    controls.set_target(p.x, p.y, p.z, false);
    controls.update(0.0);
}

fn pointer(n: &[f64]) -> PointerInput {
    PointerInput {
        pointer_id: n[0] as i32,
        buttons: n[1] as u32,
        client_x: n[2],
        client_y: n[3],
    }
}

fn apply(
    controls: &mut CameraControls,
    live_rect: &mut ElementRect,
    op: &[Value],
) -> Result<Value> {
    let kind = text(&op[0])?;
    let args = &op[1..];
    let n = |i: usize| number(&args[i]);
    let b = |i: usize| boolean(&args[i]);
    match kind.as_str() {
        "update" => return Ok(Value::Bool(controls.update(n(0)?))),
        "rotate" => controls.rotate(n(0)?, n(1)?, b(2)?),
        "rotateTo" => controls.rotate_to(n(0)?, n(1)?, b(2)?),
        "dolly" => controls.dolly(n(0)?, b(1)?),
        "dollyTo" => controls.dolly_to(n(0)?, b(1)?),
        "dollyInFixed" => controls.dolly_in_fixed(n(0)?, b(1)?),
        "truck" => controls.truck(n(0)?, n(1)?, b(2)?),
        "elevate" => controls.elevate(n(0)?, b(1)?),
        "moveTo" => controls.move_to(n(0)?, n(1)?, n(2)?, b(3)?),
        "setLookAt" => controls.set_look_at(n(0)?, n(1)?, n(2)?, n(3)?, n(4)?, n(5)?, b(6)?),
        "setPosition" => controls.set_position(n(0)?, n(1)?, n(2)?, b(3)?),
        "setTarget" => controls.set_target(n(0)?, n(1)?, n(2)?, b(3)?),
        "normalizeRotations" => controls.normalize_rotations(),
        "stop" => controls.stop(),
        "cancel" => controls.cancel(),
        "set" => set(controls, &text(&args[0])?, &args[1])?,
        "cameraFov" => {
            let camera = controls.camera_mut();
            camera.fov = n(0)?;
            camera.update_projection_matrix();
        }
        "cameraAspect" => {
            let camera = controls.camera_mut();
            camera.aspect = n(0)?;
            camera.update_projection_matrix();
        }
        "cameraPose" => {
            let q = Quaternion::new(n(3)?, n(4)?, n(5)?, n(6)?);
            let object = &mut controls.camera_mut().object;
            object.position = Vector3::new(n(0)?, n(1)?, n(2)?);
            object.set_quaternion(q);
        }
        "rect" => *live_rect = rect(&numbers(&Value::Array(args.to_vec()))?),
        "keyRotate" => key_rotate(controls, n(0)?, n(1)?),
        "pointerdown" => {
            controls.pointer_down(pointer(&numbers(&Value::Array(args.to_vec()))?), *live_rect)
        }
        "pointermove" => controls.pointer_move(pointer(&numbers(&Value::Array(args.to_vec()))?)),
        "pointerup" => controls.pointer_up(pointer(&numbers(&Value::Array(args.to_vec()))?)),
        "pointercancel" => {
            controls.pointer_cancel(pointer(&numbers(&Value::Array(args.to_vec()))?))
        }
        "wheel" => controls.wheel(WheelInput {
            delta_y: n(0)?,
            delta_mode: n(1)? as u32,
        }),
        other => return Err(format!("unknown op {other}")),
    }
    Ok(Value::Null)
}

fn check(record: &Record, mac: bool) -> Result<()> {
    let case = record.string("case").unwrap_or_default();
    run(record, mac).map_err(|e| format!("{case}: {e}"))
}

fn run(record: &Record, mac: bool) -> Result<()> {
    let init = value(record, "init")?;
    let Value::Object(fields) = &init else {
        return Err("init is not an object".to_owned());
    };
    let field = |name: &str| {
        fields
            .iter()
            .find(|(k, _)| k.as_string().as_deref() == Some(name))
            .map(|(_, v)| v)
            .ok_or_else(|| format!("init has no {name}"))
    };
    let c = numbers(field("camera")?)?;
    let mut live_rect = rect(&numbers(field("rect")?)?);
    let mut camera = PerspectiveCamera::new(c[0], c[1], c[2], c[3]);
    camera.object.position = Vector3::new(c[4], c[5], c[6]);
    let mut controls = CameraControls::new(camera, live_rect, mac);
    controls.take_events();
    for entry in items(field("config")?)? {
        let entry = items(entry)?;
        set(&mut controls, &text(&entry[0])?, &entry[1])?;
    }

    for (i, step) in items(&value(record, "steps")?)?.iter().enumerate() {
        let step = items(step)?;
        let actual = if i == 0 {
            Value::Array(vec![
                Value::Null,
                events(controls.take_events()),
                state(&controls),
            ])
        } else {
            let result = apply(&mut controls, &mut live_rect, items(&step[0])?)?;
            Value::Array(vec![
                step[0].clone(),
                result,
                events(controls.take_events()),
                state(&controls),
            ])
        };
        let expected = Value::Array(step.to_vec());
        compare(&expected, &actual).map_err(|e| format!("step {i} {}: {e}", step[0].encode()))?;
    }

    let camera = controls.camera_mut();
    camera.update_matrix_world(false);
    let actual = Value::Array(vec![
        nums(&camera.object.matrix_world.elements),
        nums(&camera.matrix_world_inverse.elements),
        nums(&camera.projection_matrix.elements),
        nums(&camera.projection_matrix_inverse.elements),
    ]);
    compare(&value(record, "final")?, &actual).map_err(|e| format!("final: {e}"))
}

record_vectors!(
    camera_controls_match_mac,
    "field3d/camera-controls-mac",
    |r| { check(r, true) }
);
record_vectors!(
    camera_controls_match_other,
    "field3d/camera-controls-other",
    |r| check(r, false)
);
