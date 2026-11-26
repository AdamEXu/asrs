use asrs_field3d::three::primitives::{
    box_geometry, cone_geometry, cylinder_geometry, plane_geometry, sphere_geometry,
};
use asrs_field3d::three::{
    ArrayData, ArrayKind, BufferAttribute, BufferGeometry, Color, Euler, EulerOrder, Graph,
    Matrix3, Matrix4, ObjectId, Quaternion, Vector3, merge_geometries,
};
use asrs_js::math;
use asrs_js::string::JsString;
use asrs_parity::{Json, Record, Value, diff, record_vectors};

type Result<T> = std::result::Result<T, String>;

fn value(record: &Record, key: &str) -> Result<Value> {
    record.value(key).map_err(|e| format!("{e}"))
}

fn number(v: &Value) -> Result<f64> {
    v.as_f64()
        .ok_or_else(|| format!("expected a number, got {}", v.kind()))
}

fn items(v: &Value) -> Result<&[Value]> {
    v.as_array()
        .ok_or_else(|| format!("expected an array, got {}", v.kind()))
}

fn text(v: &Value) -> Result<String> {
    v.as_str()
        .and_then(|s| s.as_string())
        .ok_or_else(|| format!("expected a string, got {}", v.kind()))
}

fn numbers(v: &Value) -> Result<Vec<f64>> {
    items(v)?.iter().map(number).collect()
}

fn vec3(v: &Value) -> Result<Vector3> {
    let n = numbers(v)?;
    Ok(Vector3::new(n[0], n[1], n[2]))
}

fn quat(v: &Value) -> Result<Quaternion> {
    let n = numbers(v)?;
    Ok(Quaternion::new(n[0], n[1], n[2], n[3]))
}

fn mat4(v: &Value) -> Result<Matrix4> {
    let n = numbers(v)?;
    let mut e = [0.0; 16];
    e.copy_from_slice(&n);
    Ok(Matrix4::from_array(e))
}

fn order(v: &Value) -> Result<EulerOrder> {
    let name = text(v)?;
    EulerOrder::from_name(&name).ok_or_else(|| format!("unknown order {name}"))
}

/// Argument `i` of a JS call, or `default` when it is missing or `undefined`.
fn arg(args: &[Value], i: usize, default: f64) -> Result<f64> {
    match args.get(i) {
        None | Some(Value::Undefined) => Ok(default),
        Some(v) => number(v),
    }
}

fn is_false_or_missing(args: &[Value], i: usize) -> bool {
    matches!(
        args.get(i),
        Some(Value::Bool(false)) | None | Some(Value::Undefined)
    )
}

fn num(x: f64) -> Value {
    Value::number(x)
}

fn nums(xs: &[f64]) -> Value {
    Value::Array(xs.iter().map(|&x| num(x)).collect())
}

fn array_value(array: &ArrayData) -> Value {
    if let ArrayData::Uint8(bytes) = array {
        return Value::Bytes(bytes.clone());
    }
    let values = (0..array.len()).map(|i| num(array.get(i))).collect();
    Value::TypedArray(array.kind().name().to_owned(), values)
}

fn geometry_value(g: &BufferGeometry) -> Value {
    Value::Array(vec![
        g.index
            .as_ref()
            .map_or(Value::Null, |index| array_value(&index.array)),
        Value::Array(
            g.attributes()
                .map(|(name, a)| {
                    Value::Array(vec![
                        Value::string(name),
                        num(a.item_size as f64),
                        Value::Bool(a.normalized),
                        array_value(&a.array),
                    ])
                })
                .collect(),
        ),
        Value::Array(
            g.groups
                .iter()
                .map(|group| {
                    nums(&[
                        group.start as f64,
                        group.count as f64,
                        group.material_index as f64,
                    ])
                })
                .collect(),
        ),
        g.bounding_box.map_or(Value::Null, |b| {
            Value::Array(vec![nums(&b.min.to_array()), nums(&b.max.to_array())])
        }),
        g.bounding_sphere.map_or(Value::Null, |s| {
            Value::Array(vec![nums(&s.center.to_array()), num(s.radius)])
        }),
    ])
}

fn fnv1a64(text: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for unit in text.encode_utf16() {
        h ^= u64::from(unit);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

const CANONICAL_NAN: u64 = 0x7ff8_0000_0000_0000;

/// NaN sign bits are left out of the comparison: V8 and LLVM both rewrite `(-a) * b` as
/// `-(a * b)` when they choose to, so the sign of a NaN that passes through a negation is
/// not reproducible, and no NaN in this module reaches a byte-exact output.
fn canonical(v: &Value) -> Value {
    match v {
        Value::Number(bits) if f64::from_bits(*bits).is_nan() => Value::Number(CANONICAL_NAN),
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        Value::TypedArray(name, items) => {
            Value::TypedArray(name.clone(), items.iter().map(canonical).collect())
        }
        other => other.clone(),
    }
}

fn compare(expected: &Value, actual: &Value) -> Result<()> {
    let (expected, actual) = (canonical(expected), canonical(actual));
    if expected.encode() == actual.encode() {
        return Ok(());
    }
    Err(diff::report(&expected, &actual).unwrap_or_else(|| "encodings differ".to_owned()))
}

/// Compares a recorded geometry, which is either the full tree or a digest of its JSON text.
fn compare_geometry(record: &Record, actual: Option<&BufferGeometry>) -> Result<()> {
    let json = record.field("out").ok_or("record has no out")?;
    if let Some(digest) = json.get("digest") {
        let Some(actual) = actual else {
            return Err("expected a geometry, got null".to_owned());
        };
        let encoded = geometry_value(actual).encode();
        let length = json.get("length").and_then(Json::as_f64).unwrap_or(-1.0);
        let digest = digest.as_str().map(|s| s.to_string_lossy());
        return if digest.as_deref() == Some(fnv1a64(&encoded).as_str())
            && length == encoded.encode_utf16().count() as f64
        {
            Ok(())
        } else {
            Err(format!(
                "digest differs (actual text has {} units)",
                encoded.len()
            ))
        };
    }
    let expected = value(record, "out")?;
    let actual = actual.map_or(Value::Null, geometry_value);
    compare(&expected, &actual)
}

fn build(spec: &Value) -> Result<BufferGeometry> {
    let ctor = text(spec.get("ctor").ok_or("spec has no ctor")?)?;
    let args = items(spec.get("args").ok_or("spec has no args")?)?;
    let tau = math::PI * 2.0;
    let mut g = match ctor.as_str() {
        "BoxGeometry" => box_geometry(
            arg(args, 0, 1.0)?,
            arg(args, 1, 1.0)?,
            arg(args, 2, 1.0)?,
            arg(args, 3, 1.0)?,
            arg(args, 4, 1.0)?,
            arg(args, 5, 1.0)?,
        ),
        "PlaneGeometry" => plane_geometry(
            arg(args, 0, 1.0)?,
            arg(args, 1, 1.0)?,
            arg(args, 2, 1.0)?,
            arg(args, 3, 1.0)?,
        ),
        "CylinderGeometry" => cylinder_geometry(
            arg(args, 0, 1.0)?,
            arg(args, 1, 1.0)?,
            arg(args, 2, 1.0)?,
            arg(args, 3, 32.0)?,
            arg(args, 4, 1.0)?,
            !is_false_or_missing(args, 5),
            arg(args, 6, 0.0)?,
            arg(args, 7, tau)?,
        ),
        "ConeGeometry" => cone_geometry(
            arg(args, 0, 1.0)?,
            arg(args, 1, 1.0)?,
            arg(args, 2, 32.0)?,
            arg(args, 3, 1.0)?,
            !is_false_or_missing(args, 4),
            arg(args, 5, 0.0)?,
            arg(args, 6, tau)?,
        ),
        "SphereGeometry" => sphere_geometry(
            arg(args, 0, 1.0)?,
            arg(args, 1, 32.0)?,
            arg(args, 2, 16.0)?,
            arg(args, 3, 0.0)?,
            arg(args, 4, tau)?,
            arg(args, 5, 0.0)?,
            arg(args, 6, math::PI)?,
        ),
        "Points" => {
            let points = args
                .iter()
                .map(|p| {
                    let n = numbers(p)?;
                    Ok(Vector3::new(
                        n[0],
                        n[1],
                        n.get(2).copied().unwrap_or(f64::NAN),
                    ))
                })
                .collect::<Result<Vec<_>>>()?;
            let mut g = BufferGeometry::new();
            g.set_from_points(&points);
            g
        }
        other => return Err(format!("unknown ctor {other}")),
    };
    for op in items(spec.get("ops").ok_or("spec has no ops")?)? {
        let op = items(op)?;
        let name = text(&op[0])?;
        let a = &op[1..];
        let n = |i: usize| number(&a[i]);
        match name.as_str() {
            "translate" => {
                g.translate(n(0)?, n(1)?, n(2)?);
            }
            "scale" => {
                g.scale(n(0)?, n(1)?, n(2)?);
            }
            "rotateX" => {
                g.rotate_x(n(0)?);
            }
            "rotateY" => {
                g.rotate_y(n(0)?);
            }
            "rotateZ" => {
                g.rotate_z(n(0)?);
            }
            "applyMatrix4" => {
                g.apply_matrix4(&mat4(&a[0])?);
            }
            "applyQuaternion" => {
                g.apply_quaternion(&Quaternion::new(n(0)?, n(1)?, n(2)?, n(3)?));
            }
            "deleteAttribute" => g.delete_attribute(&text(&a[0])?),
            "clone" => g = g.clone(),
            "toNonIndexed" => g = g.to_non_indexed(),
            "computeBoundingBox" => g.compute_bounding_box(),
            "computeBoundingSphere" => g.compute_bounding_sphere(),
            "color" => {
                let rgb = [n(0)?, n(1)?, n(2)?];
                let count = g.attribute("position").map_or(0, BufferAttribute::count);
                let values: Vec<f64> = (0..count * 3).map(|i| rgb[i % 3]).collect();
                g.set_attribute(
                    "color",
                    BufferAttribute::new(
                        ArrayData::from_numbers(ArrayKind::Uint8, &values),
                        3,
                        true,
                    ),
                );
            }
            "attribute" => {
                let kind = match text(&a[1])?.as_str() {
                    "Float32Array" => ArrayKind::Float32,
                    "Float64Array" => ArrayKind::Float64,
                    "Int16Array" => ArrayKind::Int16,
                    other => return Err(format!("unsupported array {other}")),
                };
                let values = numbers(&a[4])?;
                let normalized = a[3].as_bool().unwrap_or(false);
                g.set_attribute(
                    &text(&a[0])?,
                    BufferAttribute::new(
                        ArrayData::from_numbers(kind, &values),
                        n(2)? as usize,
                        normalized,
                    ),
                );
            }
            other => return Err(format!("unknown geometry op {other}")),
        }
    }
    Ok(g)
}

fn check_geometry(record: &Record) -> Result<()> {
    let spec = value(record, "spec")?;
    compare_geometry(record, Some(&build(&spec)?))
}

fn check_merge(record: &Record) -> Result<()> {
    let specs = value(record, "specs")?;
    let geometries = items(&specs)?
        .iter()
        .map(build)
        .collect::<Result<Vec<_>>>()?;
    let use_groups = matches!(record.field("useGroups"), Some(Json::Bool(true)));
    let merged = merge_geometries(geometries, use_groups).ok();
    compare_geometry(record, merged.as_ref())
}

fn apply_object_op(graph: &mut Graph<()>, id: ObjectId, op: &[Value]) -> Result<()> {
    let name = text(&op[0])?;
    let a = &op[1..];
    let n = |i: usize| number(&a[i]);
    let o = graph.object_mut(id);
    match name.as_str() {
        "position.set" => o.position = Vector3::new(n(0)?, n(1)?, n(2)?),
        "scale.set" => o.scale = Vector3::new(n(0)?, n(1)?, n(2)?),
        "rotation.set" => {
            let order = o.rotation().order;
            o.set_rotation(Euler::new(n(0)?, n(1)?, n(2)?, order));
        }
        "rotation.order" => {
            let r = o.rotation();
            o.set_rotation(Euler::new(r.x, r.y, r.z, order(&a[0])?));
        }
        "rotation.setFromQuaternion" => {
            o.set_rotation_from_quaternion(&Quaternion::new(n(0)?, n(1)?, n(2)?, n(3)?));
        }
        "quaternion.copy" => o.set_quaternion(Quaternion::new(n(0)?, n(1)?, n(2)?, n(3)?)),
        "setRotationFromAxisAngle" => {
            o.set_rotation_from_axis_angle(Vector3::new(n(0)?, n(1)?, n(2)?), n(3)?);
        }
        "rotateX" => o.rotate_x(n(0)?),
        "rotateY" => o.rotate_y(n(0)?),
        "rotateZ" => o.rotate_z(n(0)?),
        "translateX" => o.translate_x(n(0)?),
        "translateY" => o.translate_y(n(0)?),
        "translateZ" => o.translate_z(n(0)?),
        "rotateOnAxis" => o.rotate_on_axis(Vector3::new(n(0)?, n(1)?, n(2)?), n(3)?),
        "rotateOnWorldAxis" => {
            o.rotate_on_world_axis(Vector3::new(n(0)?, n(1)?, n(2)?), n(3)?);
        }
        "applyQuaternion" => o.apply_quaternion(&Quaternion::new(n(0)?, n(1)?, n(2)?, n(3)?)),
        "applyMatrix4" => o.apply_matrix4(&mat4(&a[0])?),
        "updateMatrix" => o.update_matrix(),
        "lookAt" => graph.look_at(id, Vector3::new(n(0)?, n(1)?, n(2)?)),
        other => return Err(format!("unknown object op {other}")),
    }
    Ok(())
}

fn object_state(graph: &Graph<()>, id: ObjectId) -> Value {
    let o = graph.object(id);
    let r = o.rotation();
    Value::Array(vec![
        nums(&o.position.to_array()),
        Value::Array(vec![
            num(r.x),
            num(r.y),
            num(r.z),
            Value::string(r.order.name()),
        ]),
        nums(&o.quaternion().to_array()),
        nums(&o.scale.to_array()),
        nums(&o.matrix.elements),
        nums(&o.matrix_world.elements),
    ])
}

fn check_object(record: &Record) -> Result<()> {
    let mut graph = Graph::new();
    let id = graph.create("", ());
    graph.object_mut(id).is_camera_or_light =
        matches!(record.field("camera"), Some(Json::Bool(true)));
    for op in items(&value(record, "ops")?)? {
        apply_object_op(&mut graph, id, items(op)?)?;
    }
    let expected = value(record, "out")?;
    compare(&expected, &object_state(&graph, id))
}

fn check_graph(record: &Record) -> Result<()> {
    let mut graph = Graph::new();
    let mut ids = Vec::new();
    for node in items(&value(record, "nodes")?)? {
        let node = items(node)?;
        let id = graph.create("", ());
        if let Some(parent) = node[0].as_f64() {
            graph.add(ids[parent as usize], id);
        }
        for op in items(&node[1])? {
            apply_object_op(&mut graph, id, items(op)?)?;
        }
        ids.push(id);
    }
    if let Value::Array(pair) = value(record, "attach")? {
        let child = ids[number(&pair[0])? as usize];
        let parent = ids[number(&pair[1])? as usize];
        graph.attach(parent, child);
    }
    graph.update_matrix_world(ids[0], false);
    let leaf = *ids.last().ok_or("empty graph")?;
    let world_position = graph.world_position(leaf);
    let world_quaternion = graph.world_quaternion(leaf);
    let actual = Value::Array(vec![
        Value::Array(
            ids.iter()
                .map(|&id| {
                    let o = graph.object(id);
                    Value::Array(vec![
                        nums(&o.matrix_world.elements),
                        nums(&o.position.to_array()),
                        nums(&o.quaternion().to_array()),
                    ])
                })
                .collect(),
        ),
        nums(&world_position.to_array()),
        nums(&world_quaternion.to_array()),
    ]);
    let expected = value(record, "out")?;
    compare(&expected, &actual)
}

fn euler_args(args: &[Value]) -> Result<Euler> {
    Ok(Euler::new(
        number(&args[0])?,
        number(&args[1])?,
        number(&args[2])?,
        order(&args[3])?,
    ))
}

fn check_math(record: &Record) -> Result<()> {
    let name = record.string("fn").ok_or("record has no fn")?;
    let args_value = value(record, "args")?;
    let args = items(&args_value)?;
    let n = |i: usize| number(&args[i]);
    let actual = match name.as_str() {
        "Quaternion.setFromEuler" => nums(&Quaternion::from_euler(&euler_args(args)?).to_array()),
        "Matrix4.makeRotationFromEuler" => {
            nums(&Matrix4::make_rotation_from_euler(&euler_args(args)?).elements)
        }
        "Euler.setFromQuaternion" => {
            let q = Quaternion::new(n(0)?, n(1)?, n(2)?, n(3)?);
            let e = Euler::from_quaternion(&q, order(&args[4])?);
            nums(&[e.x, e.y, e.z])
        }
        "Euler.setFromRotationMatrix" => {
            let e = Euler::from_rotation_matrix(&mat4(&args[0])?, order(&args[1])?);
            nums(&[e.x, e.y, e.z])
        }
        "Quaternion.setFromRotationMatrix" => {
            nums(&Quaternion::from_rotation_matrix(&mat4(&args[0])?).to_array())
        }
        "Matrix4.invert" => nums(&mat4(&args[0])?.invert().elements),
        "Matrix4.determinant" => num(mat4(&args[0])?.determinant()),
        "Matrix4.decompose" => {
            let (p, q, s) = mat4(&args[0])?.decompose();
            Value::Array(vec![
                nums(&p.to_array()),
                nums(&q.to_array()),
                nums(&s.to_array()),
            ])
        }
        "Matrix3.getNormalMatrix" => nums(&Matrix3::normal_matrix(&mat4(&args[0])?).elements),
        "Matrix4.multiplyMatrices" => {
            nums(&Matrix4::multiply_matrices(&mat4(&args[0])?, &mat4(&args[1])?).elements)
        }
        "Vector3.applyMatrix4" => nums(&vec3(&args[0])?.apply_matrix4(&mat4(&args[1])?).to_array()),
        "Vector3.applyQuaternion" => nums(
            &vec3(&args[0])?
                .apply_quaternion(&quat(&args[1])?)
                .to_array(),
        ),
        "Vector3.transformDirection" => nums(
            &vec3(&args[0])?
                .transform_direction(&mat4(&args[1])?)
                .to_array(),
        ),
        "Vector3.angleTo" => num(vec3(&args[0])?.angle_to(vec3(&args[1])?)),
        "Quaternion.setFromAxisAngle" => {
            nums(&Quaternion::from_axis_angle(vec3(&args[0])?, n(1)?).to_array())
        }
        "Matrix4.makeRotationAxis" => {
            nums(&Matrix4::make_rotation_axis(vec3(&args[0])?, n(1)?).elements)
        }
        "Quaternion.slerp" => nums(&quat(&args[0])?.slerp(quat(&args[1])?, n(2)?).to_array()),
        "Quaternion.multiply" => nums(&quat(&args[0])?.multiply(quat(&args[1])?).to_array()),
        "Quaternion.angleTo" => num(quat(&args[0])?.angle_to(quat(&args[1])?)),
        "Quaternion.rotateTowards" => nums(
            &quat(&args[0])?
                .rotate_towards(quat(&args[1])?, n(2)?)
                .to_array(),
        ),
        "Quaternion.setFromUnitVectors" => {
            nums(&Quaternion::from_unit_vectors(vec3(&args[0])?, vec3(&args[1])?).to_array())
        }
        "Matrix4.lookAt" => nums(
            &Matrix4::IDENTITY
                .look_at(vec3(&args[0])?, vec3(&args[1])?, vec3(&args[2])?)
                .elements,
        ),
        "Matrix4.makePerspective" => {
            nums(&Matrix4::make_perspective(n(0)?, n(1)?, n(2)?, n(3)?, n(4)?, n(5)?).elements)
        }
        "Matrix4.makeOrthographic" => {
            nums(&Matrix4::make_orthographic(n(0)?, n(1)?, n(2)?, n(3)?, n(4)?, n(5)?).elements)
        }
        "PerspectiveCamera.projectionMatrix" => {
            let m = Matrix4::perspective_camera(n(0)?, n(1)?, n(2)?, n(3)?, n(4)?);
            Value::Array(vec![nums(&m.elements), nums(&m.invert().elements)])
        }
        other => return Err(format!("unknown fn {other}")),
    };
    let expected = value(record, "out")?;
    compare(&expected, &actual).map_err(|e| format!("{name}: {e}"))
}

fn check_color(record: &Record) -> Result<()> {
    let arg = value(record, "arg")?;
    let c = match &arg {
        Value::String(s) => Color::style(&JsString::from_units(s.units().to_vec())),
        other => Color::hex(number(other)?),
    };
    let expected = value(record, "out")?;
    compare(&expected, &nums(&[c.r, c.g, c.b, c.get_hex()]))
        .map_err(|e| format!("{}: {e}", arg.encode()))
}

fn check(record: &Record) -> Result<()> {
    let kind = record.string("kind").ok_or("record has no kind")?;
    let case = record.string("case").unwrap_or_default();
    let result = match kind.as_str() {
        "geometry" => check_geometry(record),
        "merge" => check_merge(record),
        "object" => check_object(record),
        "graph" => check_graph(record),
        "math" => check_math(record),
        "color" => check_color(record),
        other => Err(format!("unknown kind {other}")),
    };
    result.map_err(|e| format!("{kind} {case}: {e}"))
}

record_vectors!(three_matches_three_js, "field3d/three", check);
