//! Replays `parity/vectors/field3d/fields` (oracle/src/genField3dFields.ts): each case builds one
//! procedural field with the port and compares every object under its root with the objects
//! upstream's builder returned. `glb_procedural_scenes` also checks `fields::to_scene_graph`
//! against the procedural scenes `field3d/glb-procedural` recorded before optimizing them.

use std::collections::{BTreeSet, HashMap};

use asrs_field3d::fields::{
    make_axes_field, make_axes_template, make_evergreen_field, to_scene_graph,
};
use asrs_field3d::glb::{MeshMaterial, ObjectKind, SceneGraph};
use asrs_field3d::render::{LineBasicMaterial, Materials, NodeKind, PhongMaterial, Scene, Side};
use asrs_field3d::three::{ArrayData, BufferGeometry, Color, ObjectId, Vector3};
use asrs_model::assets::CoordinateSystem;
use asrs_parity::vectors::{self, Record, check_records, load};
use asrs_parity::{Json, Value, diff};

type Result<T> = std::result::Result<T, String>;

fn get<'a>(v: &'a Value, key: &str) -> Result<&'a Value> {
    v.get(key).ok_or_else(|| format!("missing {key}"))
}

fn number(v: &Value) -> Result<f64> {
    v.as_f64()
        .ok_or_else(|| format!("expected a number, got {}", v.encode()))
}

fn boolean(v: &Value) -> Result<bool> {
    v.as_bool()
        .ok_or_else(|| format!("expected a boolean, got {}", v.encode()))
}

fn text(v: &Value) -> Result<String> {
    v.as_str()
        .map(|s| s.to_string_lossy())
        .ok_or_else(|| format!("expected a string, got {}", v.encode()))
}

fn num(x: f64) -> Value {
    Value::number(x)
}

fn nums(xs: &[f64]) -> Value {
    Value::Array(xs.iter().copied().map(num).collect())
}

fn coordinate_system(name: &str) -> Result<CoordinateSystem> {
    Ok(match name {
        "wall-alliance" => CoordinateSystem::WallAlliance,
        "wall-blue" => CoordinateSystem::WallBlue,
        "center-rotated" => CoordinateSystem::CenterRotated,
        "center-red" => CoordinateSystem::CenterRed,
        other => return Err(format!("unknown coordinate system {other:?}")),
    })
}

fn geometry_tree(g: &BufferGeometry) -> Value {
    let array = |a: &ArrayData| {
        if let ArrayData::Uint8(bytes) = a {
            return Value::Bytes(bytes.clone());
        }
        let values = (0..a.len()).map(|i| num(a.get(i))).collect();
        Value::TypedArray(a.kind().name().to_owned(), values)
    };
    Value::Array(vec![
        g.index.as_ref().map_or(Value::Null, |i| array(&i.array)),
        Value::Array(
            g.attributes()
                .map(|(name, a)| {
                    Value::Array(vec![
                        Value::string(name),
                        num(a.item_size as f64),
                        Value::Bool(a.normalized),
                        array(&a.array),
                    ])
                })
                .collect(),
        ),
        Value::Array(
            g.groups
                .iter()
                .map(|g| nums(&[g.start as f64, g.count as f64, g.material_index as f64]))
                .collect(),
        ),
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

fn side(s: Side) -> f64 {
    match s {
        Side::Front => 0.0,
        Side::Back => 1.0,
        Side::Double => 2.0,
    }
}

fn ranks(keys: impl IntoIterator<Item = u64>) -> HashMap<u64, usize> {
    let sorted: BTreeSet<u64> = keys.into_iter().collect();
    sorted
        .into_iter()
        .enumerate()
        .map(|(rank, key)| (key, rank))
        .collect()
}

fn phong(m: &PhongMaterial, rank: usize) -> Value {
    Value::Array(vec![
        num(rank as f64),
        Value::string("MeshPhongMaterial"),
        nums(&m.color.to_array()),
        nums(&m.specular.to_array()),
        num(m.shininess),
        nums(&m.emissive.to_array()),
        num(m.emissive_intensity),
        num(m.opacity),
        Value::Bool(m.transparent),
        num(side(m.side)),
        Value::Bool(m.depth_test),
        Value::Bool(m.depth_write),
        Value::Bool(m.vertex_colors),
        Value::Bool(m.visible),
        Value::Bool(m.map.is_none()),
    ])
}

fn line_basic(m: &LineBasicMaterial, rank: usize) -> Value {
    Value::Array(vec![
        num(rank as f64),
        Value::string("LineBasicMaterial"),
        nums(&m.color.to_array()),
        num(m.opacity),
        Value::Bool(m.transparent),
        Value::Bool(m.vertex_colors),
        Value::Bool(m.depth_test),
        Value::Bool(m.depth_write),
        Value::Bool(m.visible),
    ])
}

/// The port's objects under `root` in the generator's node layout (see
/// oracle/src/lazyHelpers/field3dFieldsRun.ts).
fn field_nodes(scene: &mut Scene, root: ObjectId) -> Result<Vec<Value>> {
    if let Some(parent) = scene.graph.node(root).parent() {
        return Err(format!(
            "the field root is attached to {parent:?}; upstream returns it detached"
        ));
    }
    scene.graph.update_matrix_world(root, true);
    let ids = scene.graph.descendants(root);

    let mut geometry_keys = Vec::new();
    let mut material_ids = Vec::new();
    for &id in &ids {
        match &scene.node(id).kind {
            NodeKind::Mesh(mesh) => {
                geometry_keys.push(mesh.geometry.key());
                match &mesh.material {
                    Materials::Single(m) => material_ids.push(m.id),
                    Materials::Array(_) => {
                        return Err(format!("{id:?} has a material array"));
                    }
                }
            }
            NodeKind::Line(line) => {
                geometry_keys.push(line.geometry.key());
                material_ids.push(line.material.id);
            }
            _ => {}
        }
    }
    let geometry_ranks = ranks(geometry_keys);
    let material_ranks = ranks(material_ids);

    ids.iter()
        .map(|&id| {
            let node = scene.graph.node(id);
            let parent = match node.parent() {
                None => Value::Null,
                Some(p) => num(ids
                    .iter()
                    .position(|&i| i == p)
                    .ok_or_else(|| format!("{id:?} has a parent outside the field"))?
                    as f64),
            };
            let data = &node.data;
            let (type_name, cast, receive, geometry, material) = match &data.kind {
                NodeKind::Object3D => ("Object3D", false, false, Value::Null, Value::Null),
                NodeKind::Group => ("Group", false, false, Value::Null, Value::Null),
                NodeKind::Mesh(mesh) => {
                    if mesh.instances.is_some() {
                        return Err(format!("{id:?} is an InstancedMesh"));
                    }
                    let Materials::Single(m) = &mesh.material else {
                        return Err(format!("{id:?} has a material array"));
                    };
                    (
                        "Mesh",
                        mesh.cast_shadow,
                        mesh.receive_shadow,
                        Value::Array(vec![
                            num(geometry_ranks[&mesh.geometry.key()] as f64),
                            Value::string(&fnv1a64(
                                &geometry_tree(&mesh.geometry.geometry).encode(),
                            )),
                        ]),
                        phong(m, material_ranks[&m.id]),
                    )
                }
                NodeKind::Line(line) => (
                    if line.segments {
                        "LineSegments"
                    } else {
                        "Line"
                    },
                    false,
                    false,
                    Value::Array(vec![
                        num(geometry_ranks[&line.geometry.key()] as f64),
                        Value::string(&fnv1a64(&geometry_tree(&line.geometry.geometry).encode())),
                    ]),
                    line_basic(&line.material, material_ranks[&line.material.id]),
                ),
                other => {
                    return Err(format!(
                        "{id:?} is a {}, which no procedural field contains",
                        match other {
                            NodeKind::FatLine(_) => "Line2",
                            NodeKind::HemisphereLight(_) => "HemisphereLight",
                            NodeKind::PointLight(_) => "PointLight",
                            NodeKind::SpotLight(_) => "SpotLight",
                            _ => "node",
                        }
                    ));
                }
            };
            let object = &node.object;
            Ok(Value::Array(vec![
                parent,
                Value::string(type_name),
                Value::string(&node.name),
                Value::Bool(data.visible),
                Value::Bool(cast),
                Value::Bool(receive),
                Value::Bool(data.frustum_culled),
                num(data.render_order),
                nums(&object.position.to_array()),
                nums(&object.quaternion().to_array()),
                nums(&object.scale.to_array()),
                nums(&object.matrix_world.elements),
                geometry,
                material,
            ]))
        })
        .collect()
}

fn build(scene: &mut Scene, field: &str, args: &Value) -> Result<ObjectId> {
    let specular = Color::hex(number(get(args, "specular")?)?);
    let shininess = number(get(args, "shininess")?)?;
    Ok(match field {
        "evergreen" => {
            make_evergreen_field(scene, specular, shininess, boolean(get(args, "isFTC")?)?)
        }
        "axes" => make_axes_field(
            scene,
            specular,
            shininess,
            coordinate_system(&text(get(args, "coordinateSystem")?)?)?,
            boolean(get(args, "isFTC")?)?,
        ),
        "template" => make_axes_template(scene, specular, shininess),
        other => return Err(format!("unknown field {other:?}")),
    })
}

fn check_field(record: &Record, compared: &mut usize) -> Result<()> {
    let field = record.string("field").ok_or("record has no field")?;
    let args = record.value("args").map_err(|e| format!("{e}"))?;
    let expected = record.value("out").map_err(|e| format!("{e}"))?;
    let expected = expected.as_array().ok_or("out is not an array")?;
    let mut scene = Scene::new();
    let root = build(&mut scene, &field, &args)?;
    let actual = field_nodes(&mut scene, root)?;
    for (i, (expected, actual)) in expected.iter().zip(&actual).enumerate() {
        if !vectors::values_match(expected, actual) {
            let report =
                diff::report(expected, actual).unwrap_or_else(|| "encodings differ".to_owned());
            return Err(format!("node {i}: {report}"));
        }
        *compared += 1;
    }
    if actual.len() != expected.len() {
        return Err(format!(
            "upstream has {} objects, the port {}",
            expected.len(),
            actual.len()
        ));
    }
    Ok(())
}

#[test]
fn fields_match_upstream() {
    let mut compared = 0;
    let mut recorded = 0;
    check_records("field3d/fields", |record| {
        recorded += record
            .value("out")
            .ok()
            .and_then(|v| v.as_array().map(<[Value]>::len))
            .unwrap_or(0);
        check_field(record, &mut compared)
    });
    assert_eq!(compared, recorded, "every recorded object is compared");
}

fn hex<T: Copy, const N: usize>(values: &[T], bytes: impl Fn(T) -> [u8; N]) -> String {
    values
        .iter()
        .flat_map(|&v| bytes(v))
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn typed_hex(a: &ArrayData) -> Value {
    let hex = match a {
        ArrayData::Float32(v) => hex(v, f32::to_le_bytes),
        ArrayData::Float64(v) => hex(v, f64::to_le_bytes),
        ArrayData::Uint8(v) => hex(v, u8::to_le_bytes),
        ArrayData::Int8(v) => hex(v, i8::to_le_bytes),
        ArrayData::Uint16(v) => hex(v, u16::to_le_bytes),
        ArrayData::Int16(v) => hex(v, i16::to_le_bytes),
        ArrayData::Uint32(v) => hex(v, u32::to_le_bytes),
        ArrayData::Int32(v) => hex(v, i32::to_le_bytes),
    };
    Value::Array(vec![Value::string(a.kind().name()), Value::string(&hex)])
}

const CVE1_TAGS: [&str; 17] = [
    "n", "u", "bi", "hole", "o", "c", "b", "ta", "ab", "dv", "d", "map", "set", "err", "box", "fn",
    "sym",
];

/// A `sceneDump` node list: plain JSON objects and numbers holding CVE-1 trees.
fn dump_value(json: &Json) -> Result<Value> {
    match json {
        Json::Number(x) => Ok(num(*x)),
        Json::Array(items) => Ok(Value::Array(
            items.iter().map(dump_value).collect::<Result<_>>()?,
        )),
        Json::Object(pairs)
            if !(pairs.len() == 1
                && CVE1_TAGS.contains(&pairs[0].0.to_string_lossy().as_str())) =>
        {
            Ok(Value::Object(
                pairs
                    .iter()
                    .map(|(k, v)| Ok((k.clone(), dump_value(v)?)))
                    .collect::<Result<_>>()?,
            ))
        }
        _ => Value::from_json(json).map_err(|e| format!("{e}")),
    }
}

/// `sceneDump` in oracle/src/genGlb.ts, over the port's scene graph.
fn scene_dump(graph: &SceneGraph, root: ObjectId) -> Value {
    let ids = graph.descendants(root);
    let nodes = ids.iter().map(|&id| {
        let node = graph.node(id);
        let parent = if id == root {
            Value::Null
        } else {
            node.parent()
                .and_then(|p| ids.iter().position(|&i| i == p))
                .map_or(Value::Undefined, |p| num(p as f64))
        };
        let object = &node.object;
        let mut fields = vec![
            ("parent", parent),
            ("type", Value::string(node.data.type_name())),
            (
                "name",
                Value::String(node.data.name.units().to_vec().into()),
            ),
            ("position", nums(&object.position.to_array())),
            ("quaternion", nums(&object.quaternion().to_array())),
            ("scale", nums(&object.scale.to_array())),
        ];
        if let ObjectKind::Mesh { geometry, material } = &node.data.kind {
            let attributes = geometry
                .attributes()
                .map(|(name, a)| {
                    let Value::Array(typed) = typed_hex(&a.array) else {
                        unreachable!()
                    };
                    Value::Array(vec![
                        Value::string(name),
                        num(a.item_size as f64),
                        Value::Bool(a.normalized),
                        typed[0].clone(),
                        typed[1].clone(),
                    ])
                })
                .collect();
            fields.push((
                "geometry",
                Value::object([
                    (
                        "index",
                        geometry
                            .index
                            .as_ref()
                            .map_or(Value::Null, |i| typed_hex(&i.array)),
                    ),
                    ("attributes", Value::Array(attributes)),
                ]),
            ));
            fields.push((
                "material",
                match material {
                    MeshMaterial::Array => Value::object([("array", Value::Bool(true))]),
                    MeshMaterial::Single(m) => Value::object([
                        ("transparent", Value::Bool(m.transparent)),
                        ("opacity", num(m.opacity)),
                        ("color", m.color.map_or(Value::Null, |c| nums(&c))),
                    ]),
                },
            ));
        }
        Value::object(fields)
    });
    Value::Array(nodes.collect())
}

fn procedural_scene(case: &str, mode: &str) -> Result<(SceneGraph, ObjectId)> {
    let (specular, shininess) = match mode {
        "cinematic" => (Color::hex(f64::from(0x666666)), 100.0),
        "standard" | "low-power" => (Color::hex(0.0), 0.0),
        other => return Err(format!("unknown mode {other:?}")),
    };
    let name = case.split('/').next().unwrap_or_default();
    let mut scene = Scene::new();
    let root = match name {
        "evergreen-frc" => make_evergreen_field(&mut scene, specular, shininess, false),
        "evergreen-ftc" => make_evergreen_field(&mut scene, specular, shininess, true),
        "axes-template" => make_axes_template(&mut scene, specular, shininess),
        "axes-manager" => {
            let axes = make_axes_template(&mut scene, specular, shininess);
            scene.graph.object_mut(axes).scale = Vector3::new(0.25, 0.25, 0.25);
            axes
        }
        _ => {
            let (is_ftc, system) = if let Some(system) = name.strip_prefix("axes-frc-") {
                (false, system)
            } else if let Some(system) = name.strip_prefix("axes-ftc-") {
                (true, system)
            } else {
                return Err(format!("unknown procedural scene {name:?}"));
            };
            make_axes_field(
                &mut scene,
                specular,
                shininess,
                coordinate_system(system)?,
                is_ftc,
            )
        }
    };
    Ok(to_scene_graph(&scene, root))
}

#[test]
fn glb_procedural_scenes() {
    let records = load("field3d/glb-procedural").unwrap_or_else(|e| panic!("{e}"));
    let mut scenes: HashMap<String, Value> = HashMap::new();
    let mut failures = Vec::new();
    let (mut compared, mut recorded) = (0, 0);
    for record in &records {
        match record.string("kind").as_deref() {
            Some("scene") => {
                let id = record.string("scene").expect("scene id");
                let nodes = dump_value(record.field("nodes").expect("nodes"))
                    .unwrap_or_else(|e| panic!("line {}: {e}", record.line));
                scenes.insert(id, nodes);
            }
            Some("optimize") => {
                recorded += 1;
                let result = (|| -> Result<()> {
                    let case = record.string("case").ok_or("case")?;
                    let mode = record.string("mode").ok_or("mode")?;
                    let expected = scenes
                        .get(&record.string("scene").ok_or("scene")?)
                        .ok_or("unknown scene")?;
                    let (graph, root) = procedural_scene(&case, &mode)?;
                    let actual = scene_dump(&graph, root);
                    if !vectors::values_match(expected, &actual) {
                        return Err(diff::report(expected, &actual)
                            .unwrap_or_else(|| "encodings differ".to_owned()));
                    }
                    compared += 1;
                    Ok(())
                })();
                if let Err(e) = result {
                    failures.push(format!("line {}: {e}", record.line));
                }
            }
            other => failures.push(format!("line {}: unexpected kind {other:?}", record.line)),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {recorded} scenes differ\n\n{}",
        failures.len(),
        failures
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n")
    );
    assert_eq!(
        compared, recorded,
        "every optimize record's scene is compared"
    );
}
