//! The GLB pipeline in the oracle's `genGlb` record format: typed arrays as
//! `{type, count, sha256}`, materials and objects as the `toJSON()` keys that differ from a fresh
//! `MeshPhongMaterial` / `Mesh`.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use asrs_field3d::glb::{self, LoadError, LoadOptions, LoadedField, OptimizedMesh};
use asrs_field3d::three::{ArrayData, BufferGeometry};
use asrs_js::string::JsString;
use asrs_model::assets::{self, Parsed};
use asrs_model::preferences::Field3dMode;
use asrs_parity::{Utf16String, Value};
use sha2::{Digest, Sha256};

pub type Result<T> = std::result::Result<T, String>;

fn bytes_of(array: &ArrayData) -> Vec<u8> {
    match array {
        ArrayData::Float32(a) => a.iter().flat_map(|v| v.to_le_bytes()).collect(),
        ArrayData::Float64(a) => a.iter().flat_map(|v| v.to_le_bytes()).collect(),
        ArrayData::Uint8(a) => a.clone(),
        ArrayData::Int8(a) => a.iter().flat_map(|v| v.to_le_bytes()).collect(),
        ArrayData::Uint16(a) => a.iter().flat_map(|v| v.to_le_bytes()).collect(),
        ArrayData::Int16(a) => a.iter().flat_map(|v| v.to_le_bytes()).collect(),
        ArrayData::Uint32(a) => a.iter().flat_map(|v| v.to_le_bytes()).collect(),
        ArrayData::Int32(a) => a.iter().flat_map(|v| v.to_le_bytes()).collect(),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn array_value(array: &ArrayData) -> Value {
    Value::object([
        ("type", Value::string(array.kind().name())),
        ("count", Value::number(array.len() as f64)),
        ("sha256", Value::string(&hex(&Sha256::digest(bytes_of(array))))),
    ])
}

fn pair(key: &str, value: Value) -> Value {
    Value::Array(vec![Value::string(key), value])
}

fn geometry_value(g: &BufferGeometry) -> Value {
    let attributes = g
        .attributes()
        .map(|(name, a)| {
            Value::Array(vec![
                Value::string(name),
                Value::number(a.item_size as f64),
                Value::Bool(a.normalized),
                array_value(&a.array),
                Value::Array(Vec::new()),
            ])
        })
        .collect();
    Value::object([
        ("type", Value::string("BufferGeometry")),
        ("index", g.index.as_ref().map_or(Value::Null, |i| array_value(&i.array))),
        ("attributes", Value::Array(attributes)),
        ("other", Value::Array(Vec::new())),
    ])
}

const DEFAULT_SPECULAR_HEX: f64 = 1118481.0;
const DEFAULT_SHININESS: f64 = 30.0;

pub fn mesh_value(mesh: &OptimizedMesh) -> Value {
    let mut object = vec![pair("name", Value::string(mesh.name))];
    if mesh.cast_shadow {
        object.push(pair("castShadow", Value::Bool(true)));
    }
    if mesh.receive_shadow {
        object.push(pair("receiveShadow", Value::Bool(true)));
    }
    let m = &mesh.material;
    let mut material = Vec::new();
    let specular = m.specular.get_hex();
    if specular != DEFAULT_SPECULAR_HEX {
        material.push(pair("specular", Value::number(specular)));
    }
    if m.shininess.to_bits() != DEFAULT_SHININESS.to_bits() {
        material.push(pair("shininess", Value::number(m.shininess)));
    }
    material.push(pair("side", Value::number(2.0)));
    material.push(pair("vertexColors", Value::Bool(true)));
    if m.opacity < 1.0 {
        material.push(pair("opacity", Value::number(m.opacity)));
    }
    if m.transparent {
        material.push(pair("transparent", Value::Bool(true)));
    }
    if !m.depth_write {
        material.push(pair("depthWrite", Value::Bool(false)));
    }
    Value::object([
        ("type", Value::string("Mesh")),
        ("object", Value::Array(object)),
        ("geometry", geometry_value(&mesh.geometry)),
        ("material", Value::Array(material)),
    ])
}

fn group_value(meshes: &[OptimizedMesh]) -> Value {
    Value::object([
        ("type", Value::string("Group")),
        ("object", Value::Array(Vec::new())),
        ("children", Value::Array(meshes.iter().map(mesh_value).collect())),
    ])
}

pub fn field_value(field: &LoadedField) -> Value {
    Value::object([
        ("field", group_value(&field.field)),
        ("fieldStagedPieces", group_value(&field.staged_pieces)),
        (
            "fieldPieces",
            Value::Array(
                field
                    .pieces
                    .iter()
                    .map(|(name, mesh)| Value::Array(vec![Value::String(Utf16String::from_units(name.units().to_vec())), mesh_value(mesh)]))
                    .collect(),
            ),
        ),
    ])
}

pub fn robot_value(scenes: &[Vec<OptimizedMesh>]) -> Value {
    Value::Array(scenes.iter().map(|s| Value::Array(s.iter().map(mesh_value).collect())).collect())
}

pub fn mode(text: &str) -> Result<Field3dMode> {
    Field3dMode::from_js_str(&JsString::from(text)).ok_or_else(|| format!("unknown mode {text}"))
}

/// What a worker run produced: its converted result, a worker that never answers, or a config
/// the asset parser does not accept.
pub enum Outcome {
    Value(Value),
    Never(LoadError),
    Config(String),
}

impl Outcome {
    /// The record's `out` field, as the oracle writes it.
    pub fn encode(&self) -> String {
        match self {
            Outcome::Value(v) => v.encode(),
            Outcome::Never(_) => "{\"never\":true}".to_owned(),
            Outcome::Config(c) => format!("{{\"config\":{}}}", json_string(c)),
        }
    }
}

pub fn json_string(text: &str) -> String {
    let mut out = String::new();
    asrs_parity::json::write_string(&text.encode_utf16().collect::<Vec<_>>(), &mut out);
    out
}

pub const MODEL_DIR: &str = "/asset/";

/// Runs the `kind` worker on `config.json` text with `config.path` set to
/// `/asset/model.glb`; `files` maps file names in that directory to their bytes.
pub fn run_asset(kind: &str, config: &str, mode: Field3dMode, files: &HashMap<String, Vec<u8>>) -> Result<Outcome> {
    let raw = asrs_js::json::parse_str(config).map_err(|e| e.message.to_string_lossy())?;
    let fetch = |url: &JsString| -> Option<Vec<u8>> {
        let text = url.to_string_lossy();
        let name = text.strip_prefix(MODEL_DIR)?;
        let name = asrs_js::string::uri::decode_uri_component(&JsString::from(name)).ok()?;
        files.get(&name.to_string_lossy()).cloned()
    };
    let fetch: &glb::Fetch<'_> = &fetch;
    let options = LoadOptions::for_mode(mode);
    let path = JsString::from(format!("{MODEL_DIR}model.glb"));
    let result = match kind {
        "robot" => match assets::parse_robot(&raw).map_err(|e| e.message.to_string_lossy())? {
            Parsed::Config(mut c) => {
                c.path = path;
                let is_ftc = c.is_ftc;
                glb::load_robot(&c, is_ftc, &options, fetch).map(|r| robot_value(&r))
            }
            Parsed::Invalid => return Ok(Outcome::Config("invalid".to_owned())),
            Parsed::Skip => return Ok(Outcome::Config("skip".to_owned())),
        },
        "field" => match assets::parse_field_3d(&raw).map_err(|e| e.message.to_string_lossy())? {
            Parsed::Config(mut c) => {
                c.path = path;
                glb::load_field(&c, &options, fetch).map(|r| field_value(&r))
            }
            Parsed::Invalid => return Ok(Outcome::Config("invalid".to_owned())),
            Parsed::Skip => return Ok(Outcome::Config("skip".to_owned())),
        },
        other => return Err(format!("unknown kind {other}")),
    };
    Ok(match result {
        Ok(v) => Outcome::Value(v),
        Err(e) => Outcome::Never(e),
    })
}

/// `asrs-cli glb release <dir> [--jobs N]`: the same lines as the oracle's
/// `genGlb release`, for every `Field3d_*` and `Robot_*` folder of an extracted asset tree.
pub fn run(args: &[String]) -> Result<()> {
    let [command, dir, rest @ ..] = args else {
        return Err("usage: asrs-cli glb release <asset dir> [--jobs N]".to_owned());
    };
    if command != "release" {
        return Err(format!("unknown glb command {command}"));
    }
    let jobs = match rest {
        [flag, n] if flag == "--jobs" => n.parse().map_err(|_| "bad --jobs")?,
        _ => 1usize,
    };
    let dir = Path::new(dir);
    let mut folders: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with("Field3d_") || n.starts_with("Robot_"))
        .collect();
    folders.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    let modes = ["low-power", "standard", "cinematic"];
    let tasks: Vec<(String, &str)> = folders.iter().flat_map(|f| modes.iter().map(move |m| (f.clone(), *m))).collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: std::sync::Mutex<Vec<Option<String>>> = std::sync::Mutex::new(vec![None; tasks.len()]);
    let written = std::sync::Mutex::new(0usize);
    let error: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..jobs.max(1) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some((folder, mode_name)) = tasks.get(i) else { break };
                    let line = release_line(&dir.join(folder), folder, mode_name);
                    let line = match line {
                        Ok(l) => l,
                        Err(e) => {
                            *error.lock().expect("error lock") = Some(e);
                            break;
                        }
                    };
                    let mut results = results.lock().expect("results lock");
                    results[i] = Some(line);
                    let mut written = written.lock().expect("written lock");
                    let mut out = std::io::stdout().lock();
                    while *written < results.len()
                        && let Some(line) = results[*written].take()
                    {
                        let _ = writeln!(out, "{line}");
                        *written += 1;
                    }
                }
            });
        }
    });
    match error.into_inner().expect("error lock") {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn release_line(asset: &Path, folder: &str, mode_name: &str) -> Result<String> {
    let config = std::fs::read_to_string(asset.join("config.json")).map_err(|e| format!("{}: {e}", asset.display()))?;
    let mut files = HashMap::new();
    for entry in std::fs::read_dir(asset).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".glb") {
            files.insert(name, std::fs::read(entry.path()).map_err(|e| e.to_string())?);
        }
    }
    let kind = if folder.starts_with("Robot_") { "robot" } else { "field" };
    let started = std::time::Instant::now();
    let outcome = run_asset(kind, &config, mode(mode_name)?, &files)?;
    if let Outcome::Never(e) = &outcome {
        eprintln!("{folder} {mode_name}: never completes ({e})");
    }
    eprintln!("{folder} {mode_name}: {} ms", started.elapsed().as_millis());
    Ok(format!(
        "{{\"case\":{},\"kind\":\"{kind}\",\"mode\":\"{mode_name}\",\"out\":{}}}",
        json_string(folder),
        outcome.encode()
    ))
}
