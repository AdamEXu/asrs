#![allow(clippy::disallowed_methods)]

#[path = "../../asrs-cli/src/glb.rs"]
#[allow(dead_code)]
mod cli_glb;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use asrs_field3d::glb::{
    self, MeshMaterial, ObjectKind, Optimized, OptimizedMesh, SceneGraph, SceneObject,
    SourceMaterial,
};
use asrs_field3d::three::{
    ArrayData, BufferAttribute, BufferGeometry, Color, ObjectId, Quaternion, Vector3,
};
use asrs_js::string::JsString;
use asrs_parity::vectors::{load, workspace_root};
use asrs_parity::{Json, Record, Value, diff};
use cli_glb::{Outcome, Result, mesh_value, mode};

fn optimized_value(o: &Optimized) -> Value {
    let list = |l: &[OptimizedMesh]| Value::Array(l.iter().map(mesh_value).collect());
    Value::object([
        ("normal", list(&o.normal)),
        ("transparent", list(&o.transparent)),
        ("carpet", list(&o.carpet)),
    ])
}

fn json_numbers(json: &Json) -> Result<Vec<f64>> {
    json.as_array()
        .ok_or("expected an array")?
        .iter()
        .map(|j| {
            Value::from_json(j)
                .ok()
                .and_then(|v| v.as_f64())
                .or_else(|| j.as_f64())
                .ok_or_else(|| "expected a number".to_owned())
        })
        .collect()
}

fn json_text(json: &Json) -> Result<String> {
    json.as_str()
        .map(|s| s.to_string_lossy())
        .ok_or_else(|| "expected a string".to_owned())
}

fn typed_from_hex(kind: &str, text: &str) -> Result<ArrayData> {
    let bytes: Vec<u8> = (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string()))
        .collect::<Result<_>>()?;
    Ok(match kind {
        "Float32Array" => ArrayData::Float32(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect(),
        ),
        "Uint16Array" => ArrayData::Uint16(
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes(*b))
                .collect(),
        ),
        "Uint32Array" => ArrayData::Uint32(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| u32::from_le_bytes(*b))
                .collect(),
        ),
        "Uint8Array" => ArrayData::Uint8(bytes),
        other => return Err(format!("unexpected array type {other}")),
    })
}

fn other_type(name: &str) -> &'static str {
    match name {
        "LineSegments" => "LineSegments",
        "Line" => "Line",
        "LineLoop" => "LineLoop",
        "Points" => "Points",
        "InstancedMesh" => "InstancedMesh",
        _ => "Other",
    }
}

/// Rebuilds a procedural scene from the oracle's `sceneDump`.
fn scene_from_dump(nodes: &Json) -> Result<(SceneGraph, ObjectId)> {
    let mut graph = SceneGraph::new();
    let mut ids = Vec::new();
    for node in nodes.as_array().ok_or("scene nodes")? {
        let field = |k: &str| node.get(k).ok_or_else(|| format!("scene node without {k}"));
        let name = json_text(field("name")?)?;
        let type_name = json_text(field("type")?)?;
        let kind = match type_name.as_str() {
            "Mesh" => {
                let g = field("geometry")?;
                let mut geometry = BufferGeometry::new();
                for a in g
                    .get("attributes")
                    .and_then(Json::as_array)
                    .ok_or("attributes")?
                {
                    let a = a.as_array().ok_or("attribute")?;
                    let item_size = a[1]
                        .as_f64()
                        .or_else(|| Value::from_json(&a[1]).ok()?.as_f64())
                        .ok_or("itemSize")? as usize;
                    let normalized = matches!(a[2], Json::Bool(true));
                    let array = typed_from_hex(&json_text(&a[3])?, &json_text(&a[4])?)?;
                    geometry.set_attribute(
                        &json_text(&a[0])?,
                        BufferAttribute::new(array, item_size, normalized),
                    );
                }
                if let Some(index) = g.get("index").and_then(Json::as_array) {
                    geometry.index = Some(BufferAttribute::new(
                        typed_from_hex(&json_text(&index[0])?, &json_text(&index[1])?)?,
                        1,
                        false,
                    ));
                }
                let m = field("material")?;
                let material = if m.get("array").is_some() {
                    MeshMaterial::Array
                } else {
                    let color = match m.get("color") {
                        Some(Json::Null) | None => None,
                        Some(c) => {
                            let c = json_numbers(c)?;
                            Some([c[0], c[1], c[2]])
                        }
                    };
                    let opacity = m
                        .get("opacity")
                        .and_then(|j| Value::from_json(j).ok())
                        .and_then(|v| v.as_f64())
                        .ok_or("opacity")?;
                    MeshMaterial::Single(Arc::new(SourceMaterial {
                        color,
                        transparent: matches!(m.get("transparent"), Some(Json::Bool(true))),
                        opacity,
                    }))
                };
                ObjectKind::Mesh {
                    geometry: Arc::new(geometry),
                    material,
                }
            }
            "Group" => ObjectKind::Group,
            "Object3D" => ObjectKind::Object3D,
            other => ObjectKind::Other(other_type(other)),
        };
        let id = graph.create("", SceneObject::new(JsString::from(name), kind));
        let p = json_numbers(field("position")?)?;
        let q = json_numbers(field("quaternion")?)?;
        let s = json_numbers(field("scale")?)?;
        let object = graph.object_mut(id);
        object.position = Vector3::new(p[0], p[1], p[2]);
        object.set_quaternion(Quaternion::new(q[0], q[1], q[2], q[3]));
        object.scale = Vector3::new(s[0], s[1], s[2]);
        if let Some(parent) = field("parent")?.as_f64() {
            graph.add(ids[parent as usize], id);
        }
        ids.push(id);
    }
    Ok((graph, *ids.first().ok_or("empty scene")?))
}

fn run_optimize(nodes: &Json, args: &Value) -> Result<Value> {
    let (mut graph, root) = scene_from_dump(nodes)?;
    let get = |k: &str| args.get(k).ok_or_else(|| format!("args without {k}"));
    let mode = mode(&get("mode")?.as_str().ok_or("mode")?.to_string_lossy())?;
    let specular = get("specular")?
        .as_array()
        .ok_or("specular")?
        .iter()
        .map(|v| v.as_f64().unwrap_or(f64::NAN))
        .collect::<Vec<_>>();
    let shininess = get("shininess")?.as_f64().ok_or("shininess")?;
    let simplify = get("enableSimplification")?
        .as_bool()
        .ok_or("enableSimplification")?;
    let is_ftc = get("isFTC")?.as_bool().ok_or("isFTC")?;
    let slicing = get("slicingSize")?.as_f64();
    let optimized = glb::optimize_geometries(
        &mut graph,
        root,
        mode,
        Color::from_array([specular[0], specular[1], specular[2]]),
        shininess,
        simplify,
        is_ftc,
        slicing,
    )
    .map_err(|e| e.to_string())?;
    Ok(optimized_value(&optimized))
}

fn upstream() -> PathBuf {
    workspace_root().join("reference/AdvantageScope")
}

fn hex_bytes(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).expect("hex"))
        .collect()
}

fn compare(record: &Record, actual: &Outcome) -> Result<()> {
    let expected = record.field("out").ok_or("record without out")?;
    if expected.get("never").is_some() {
        return match actual {
            Outcome::Never(_) => Ok(()),
            _ => Err("upstream never answers; the port did".to_owned()),
        };
    }
    let expected = Value::from_json(expected).map_err(|e| e.to_string())?;
    match actual {
        Outcome::Value(actual) => match diff::report(&expected, actual) {
            None => Ok(()),
            Some(report) => Err(report),
        },
        Outcome::Never(e) => Err(format!("the port never answers ({e}); upstream did")),
        Outcome::Config(c) => Err(format!("config parses as {c}")),
    }
}

/// Every robot and field case of vector `id`, with inputs from its `file` records and the
/// bundled assets.
fn check_assets(id: &str) {
    let records = load(id).unwrap_or_else(|e| panic!("{e}"));
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();
    let mut failures = Vec::new();
    let mut cases = 0;
    for record in &records {
        match record.string("kind").as_deref() {
            Some("file") => {
                let sha = record.string("sha256").expect("file sha256");
                files.insert(sha, hex_bytes(&record.string("hex").expect("file hex")));
            }
            Some(kind @ ("robot" | "field")) => {
                cases += 1;
                let result = (|| -> Result<()> {
                    let mut inputs = HashMap::new();
                    for file in record
                        .field("files")
                        .and_then(Json::as_array)
                        .ok_or("files")?
                    {
                        let pair = file.as_array().ok_or("file")?;
                        let name = json_text(&pair[0])?;
                        let bytes = if let Some(bundled) = pair[1].get("bundled") {
                            std::fs::read(upstream().join(json_text(bundled)?))
                                .map_err(|e| e.to_string())?
                        } else {
                            files
                                .get(&json_text(pair[1].get("sha256").ok_or("sha256")?)?)
                                .cloned()
                                .ok_or("unknown file")?
                        };
                        inputs.insert(name, bytes);
                    }
                    let mode = mode(&record.string("mode").ok_or("mode")?)?;
                    let outcome = cli_glb::run_asset(
                        kind,
                        &record.string("config").ok_or("config")?,
                        mode,
                        &inputs,
                    )?;
                    compare(record, &outcome)
                })();
                if let Err(e) = result {
                    failures.push(format!(
                        "line {} ({} {}): {e}",
                        record.line,
                        record.string("case").unwrap_or_default(),
                        record.string("mode").unwrap_or_default()
                    ));
                }
            }
            other => failures.push(format!("line {}: unexpected kind {other:?}", record.line)),
        }
    }
    finish(id, cases, failures);
}

fn finish(id: &str, cases: usize, failures: Vec<String>) {
    assert!(cases > 0, "vectors {id}: no cases");
    if !failures.is_empty() {
        panic!(
            "vectors {id}: {} of {cases} cases differ\n\n{}",
            failures.len(),
            failures
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n\n")
        );
    }
}

#[test]
fn glb_bundled() {
    check_assets("field3d/glb-bundled");
}

#[test]
fn glb_synthetic() {
    check_assets("field3d/glb-synthetic");
}

#[test]
fn glb_procedural() {
    let records = load("field3d/glb-procedural").unwrap_or_else(|e| panic!("{e}"));
    let mut scenes: HashMap<String, Json> = HashMap::new();
    let mut failures = Vec::new();
    let mut cases = 0;
    for record in &records {
        match record.string("kind").as_deref() {
            Some("scene") => {
                scenes.insert(
                    record.string("scene").expect("scene id"),
                    record.field("nodes").expect("nodes").clone(),
                );
            }
            Some("optimize") => {
                cases += 1;
                let result = (|| -> Result<()> {
                    let nodes = scenes
                        .get(&record.string("scene").ok_or("scene")?)
                        .ok_or("unknown scene")?;
                    let args = record.value("args").map_err(|e| e.to_string())?;
                    compare(record, &Outcome::Value(run_optimize(nodes, &args)?))
                })();
                if let Err(e) = result {
                    failures.push(format!(
                        "line {} ({} {}): {e}",
                        record.line,
                        record.string("case").unwrap_or_default(),
                        record.string("mode").unwrap_or_default()
                    ));
                }
            }
            other => failures.push(format!("line {}: unexpected kind {other:?}", record.line)),
        }
    }
    finish("field3d/glb-procedural", cases, failures);
}

#[test]
fn load_job_matches_direct_load_and_converts_for_the_renderer() {
    use asrs_field3d::render::Side;
    use asrs_model::preferences::Field3dMode;

    let folder = upstream().join("bundledAssets/Robot_FRCKitBot");
    let text = std::fs::read_to_string(folder.join("config.json")).expect("bundled config");
    let raw = asrs_js::json::parse_str(&text).expect("config JSON");
    let asrs_model::assets::Parsed::Config(mut config) =
        asrs_model::assets::parse_robot(&raw).expect("parses")
    else {
        panic!("bundled robot config does not parse");
    };
    config.path = JsString::from(folder.join("model.glb").to_string_lossy().as_ref());
    let options = glb::LoadOptions::for_mode(Field3dMode::Cinematic);
    let direct =
        glb::load_robot(&config, config.is_ftc, &options, &glb::read_file_url).expect("loads");

    let (tx, rx) = std::sync::mpsc::channel();
    let job = glb::spawn_load_robot(
        config.clone(),
        config.is_ftc,
        options,
        Arc::new(glb::read_file_url),
        move || {
            let _ = tx.send(());
        },
    );
    rx.recv().expect("waker fires");
    let background = job.try_take().expect("result is ready").expect("loads");
    assert_eq!(background, direct);

    let mesh = background[0][0].to_render_mesh();
    let material = mesh.material.single().expect("one material");
    assert_eq!(material.specular.get_hex(), f64::from(0x666666));
    assert_eq!(material.shininess, 100.0);
    assert_eq!(material.side, Side::Double);
    assert!(material.vertex_colors && mesh.cast_shadow && !mesh.receive_shadow);
    let (rendered, source) = (&mesh.geometry.data(), &background[0][0].geometry);
    assert!(Arc::ptr_eq(rendered, source));
    for (index, mesh) in background.iter().flatten().enumerate() {
        let from = mesh
            .source
            .as_ref()
            .expect("a background load can load again");
        assert_eq!(from.index, index);
        assert_eq!(from.reload().expect("loads again"), mesh.geometry);
    }

    config.path = JsString::from("/nonexistent/model.glb");
    let failed =
        glb::spawn_load_robot(config, false, options, Arc::new(glb::read_file_url), || {}).wait();
    assert!(matches!(failed, Err(glb::LoadError::Fetch(_))));
}

#[test]
fn load_job_refuses_deep_trees_and_survives_panics() {
    let chain = |depth: usize| -> Vec<u8> {
        let nodes: Vec<String> = (0..depth)
            .map(|i| match i + 1 < depth {
                true => format!(r#"{{"name":"n{i}","children":[{}]}}"#, i + 1),
                false => format!(r#"{{"name":"n{i}","mesh":0}}"#),
            })
            .collect();
        format!(
            r#"{{"asset":{{"version":"2.0"}},"scenes":[{{"nodes":[0]}}],"nodes":[{}],"meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"NORMAL":0}}}}]}}],"accessors":[{{"componentType":5126,"count":3,"type":"VEC3"}}]}}"#,
            nodes.join(",")
        )
        .into_bytes()
    };
    let load = |depth: usize| {
        let data = chain(depth);
        glb::LoadJob::spawn(
            move || {
                let scene = glb::load_gltf(&data, &JsString::from("/a/model.glb"), &|_| None)?;
                Ok(scene.graph.descendants(scene.scene).len())
            },
            || {},
        )
        .wait()
    };
    assert_eq!(load(4096), Ok(4097));
    assert!(matches!(
        load(4097),
        Err(glb::LoadError::Gltf(glb::GltfError::Unmodeled(_)))
    ));

    let panicked = glb::LoadJob::<()>::spawn(|| panic!("boom"), || {}).wait();
    assert_eq!(panicked, Err(glb::LoadError::Panic("boom".to_owned())));
}
