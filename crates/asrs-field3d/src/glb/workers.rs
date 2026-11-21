// Ported from AdvantageScope (BSD-3-Clause, Copyright 2021-2026 Littleton Robotics). See
// THIRD_PARTY_NOTICES.md.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use asrs_js::string::JsString;
use asrs_js::value::JsObject;
use asrs_model::assets::{self, Config3dField, Config3dRobot};
use asrs_model::preferences::Field3dMode;

use super::loader::{GltfError, GltfScene, load_gltf};
use super::optimize::{OptimizeError, OptimizedMesh, optimize_geometries};
use super::{ObjectKind, SceneObject, object_by_name};
use crate::three::{Color, Quaternion, Vector3};

/// Reads the bytes behind a model URL (`config.path` and the paths derived from it), the way
/// the worker's `fetch` does; `None` is a failed fetch.
pub type Fetch<'a> = dyn Fn(&JsString) -> Option<Vec<u8>> + Send + Sync + 'a;

/// The worker payload besides the asset config: `mode`, `materialSpecular` (a
/// `Color.toArray()`) and `materialShininess`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoadOptions {
    pub mode: Field3dMode,
    pub material_specular: [f64; 3],
    pub material_shininess: f64,
}

impl LoadOptions {
    /// What `Field3dRendererImpl` sends for a render mode.
    pub fn for_mode(mode: Field3dMode) -> Self {
        let (specular, shininess) = match mode {
            Field3dMode::Cinematic => (Color::hex(f64::from(0x666666)), 100.0),
            _ => (Color::hex(0.0), 0.0),
        };
        LoadOptions {
            mode,
            material_specular: specular.to_array(),
            material_shininess: shininess,
        }
    }
}

/// A worker that never posts its result: a model failed to load or upstream threw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    Fetch(JsString),
    Gltf(GltfError),
    Optimize(OptimizeError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Fetch(url) => write!(f, "failed to fetch {}", url.to_string_lossy()),
            LoadError::Gltf(e) => e.fmt(f),
            LoadError::Optimize(e) => e.fmt(f),
        }
    }
}

impl From<GltfError> for LoadError {
    fn from(e: GltfError) -> Self {
        LoadError::Gltf(e)
    }
}

impl From<OptimizeError> for LoadError {
    fn from(e: OptimizeError) -> Self {
        LoadError::Optimize(e)
    }
}

/// The `loadField` worker's result.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadedField {
    pub field: Vec<OptimizedMesh>,
    pub staged_pieces: Vec<OptimizedMesh>,
    /// `fieldPieces` in JS property order.
    pub pieces: Vec<(JsString, OptimizedMesh)>,
}

/// A URL with a `file://` prefix or a plain path, percent-encoded per component as upstream's
/// asset loader stores `config.path`, read from disk.
pub fn read_file_url(url: &JsString) -> Option<Vec<u8>> {
    let text = url.to_string_lossy();
    let path = text.strip_prefix("file://").unwrap_or(&text);
    let decoded = asrs_js::string::uri::decode_uri_component(&JsString::from(path)).ok()?;
    std::fs::read(decoded.as_string()?).ok()
}

fn model_urls(path: &JsString, extra: usize) -> Vec<JsString> {
    let mut urls = vec![path.clone()];
    let stem = path.slice(0.0, Some(-4.0));
    for i in 0..extra {
        let mut url = stem.clone();
        url.push_str(&JsString::from(format!("_{i}.glb")));
        urls.push(url);
    }
    urls
}

/// `Promise.all(urls.map(gltfLoader.loadAsync))`.
fn load_all(urls: &[JsString], fetch: &Fetch<'_>) -> Result<Vec<GltfScene>, LoadError> {
    urls.iter()
        .map(|url| {
            let bytes = fetch(url).ok_or_else(|| LoadError::Fetch(url.clone()))?;
            Ok(load_gltf(&bytes, url, fetch)?)
        })
        .collect()
}

fn three_quaternion(rotations: &[assets::Config3dRotation]) -> Quaternion {
    let q = assets::rotation_sequence_to_quaternion(rotations);
    Quaternion::new(q.x, q.y, q.z, q.w)
}

fn place(scene: &mut GltfScene, rotations: &[assets::Config3dRotation], position: [f64; 3]) {
    let object = scene.graph.object_mut(scene.scene);
    object.set_rotation_from_quaternion(&three_quaternion(rotations));
    object.position = Vector3::new(position[0], position[1], position[2]);
}

/// `workers/loadRobot.ts`: the robot model plus one model per component, each optimized to at
/// most one normal and one transparent mesh.
pub fn load_robot(
    config: &Config3dRobot,
    is_ftc: bool,
    options: &LoadOptions,
    fetch: &Fetch<'_>,
) -> Result<Vec<Vec<OptimizedMesh>>, LoadError> {
    let mut scenes = load_all(&model_urls(&config.path, config.components.len()), fetch)?;
    let specular = Color::from_array(options.material_specular);
    let mut meshes = Vec::new();
    for (index, scene) in scenes.iter_mut().enumerate() {
        if index == 0 {
            place(scene, &config.rotations, config.position);
        }
        let optimized = optimize_geometries(
            &mut scene.graph,
            scene.scene,
            options.mode,
            specular,
            options.material_shininess,
            !config.disable_simplification,
            is_ftc,
            None,
        )?;
        let mut scene_meshes = Vec::new();
        scene_meshes.extend(optimized.normal.into_iter().next());
        scene_meshes.extend(optimized.transparent.into_iter().next());
        meshes.push(scene_meshes);
    }
    Ok(meshes)
}

/// `workers/loadField.ts`: the field model with its staged game pieces pulled out, spatially
/// sliced, plus one model per game piece.
pub fn load_field(config: &Config3dField, options: &LoadOptions, fetch: &Fetch<'_>) -> Result<LoadedField, LoadError> {
    let mut scenes = load_all(&model_urls(&config.path, config.game_pieces.len()), fetch)?;
    let specular = Color::from_array(options.material_specular);
    let mut result = LoadedField::default();
    let mut pieces: JsObject<OptimizedMesh> = JsObject::new();
    for (index, scene) in scenes.iter_mut().enumerate() {
        if index == 0 {
            let graph = &mut scene.graph;
            let staged = graph.create("", SceneObject::new(JsString::new(), ObjectKind::Group));
            for piece in &config.game_pieces {
                for name in &piece.staged_objects {
                    let Some(object) = object_by_name(graph, scene.scene, name) else {
                        continue;
                    };
                    let rotation = graph.world_quaternion(object);
                    let position = graph.world_position(object);
                    graph.remove_from_parent(object);
                    let o = graph.object_mut(object);
                    o.set_rotation_from_quaternion(&rotation);
                    o.position = position;
                    graph.add(staged, object);
                }
            }
            let o = graph.object_mut(staged);
            o.set_rotation_from_quaternion(&three_quaternion(&config.rotations));
            o.position = Vector3::new(config.position[0], config.position[1], config.position[2]);
            let staged_meshes = optimize_geometries(
                graph,
                staged,
                options.mode,
                specular,
                options.material_shininess,
                false,
                config.is_ftc,
                None,
            )?;
            result.staged_pieces.extend(staged_meshes.normal.into_iter().next());
            result.staged_pieces.extend(staged_meshes.transparent.into_iter().next());
            result.staged_pieces.extend(staged_meshes.carpet.into_iter().next());

            place(scene, &config.rotations, config.position);
            let field = optimize_geometries(
                &mut scene.graph,
                scene.scene,
                options.mode,
                specular,
                options.material_shininess,
                true,
                config.is_ftc,
                Some(1.0),
            )?;
            result.field.extend(field.normal);
            result.field.extend(field.transparent);
            result.field.extend(field.carpet);
        } else {
            let piece = &config.game_pieces[index - 1];
            place(scene, &piece.rotations, piece.position);
            let meshes = optimize_geometries(
                &mut scene.graph,
                scene.scene,
                options.mode,
                specular,
                options.material_shininess,
                false,
                config.is_ftc,
                None,
            )?;
            if let Some(mesh) = meshes.normal.into_iter().next() {
                pieces.insert(piece.name.clone(), mesh);
            }
        }
    }
    result.pieces = pieces.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    Ok(result)
}

/// A load running on its own thread, as upstream runs each request in a fresh worker. The
/// waker fires once the result is ready; a load that fails yields its `LoadError`, which the
/// caller treats as a worker that never answers.
pub struct LoadJob<T> {
    rx: Receiver<Result<T, LoadError>>,
}

impl<T: Send + 'static> LoadJob<T> {
    pub fn spawn(
        work: impl FnOnce() -> Result<T, LoadError> + Send + 'static,
        waker: impl Fn() + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("asrs-model-load".to_owned())
            .spawn(move || {
                let _ = tx.send(work());
                waker();
            })
            .expect("spawning a model load thread");
        LoadJob { rx }
    }

    /// The result, once; `None` while the load is still running.
    pub fn try_take(&self) -> Option<Result<T, LoadError>> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    /// Blocks until the load finishes.
    pub fn wait(self) -> Result<T, LoadError> {
        self.rx.recv().expect("the load thread always sends a result")
    }
}

pub fn spawn_load_field(
    config: Config3dField,
    options: LoadOptions,
    fetch: Arc<Fetch<'static>>,
    waker: impl Fn() + Send + 'static,
) -> LoadJob<LoadedField> {
    LoadJob::spawn(move || load_field(&config, &options, &*fetch), waker)
}

pub fn spawn_load_robot(
    config: Config3dRobot,
    is_ftc: bool,
    options: LoadOptions,
    fetch: Arc<Fetch<'static>>,
    waker: impl Fn() + Send + 'static,
) -> LoadJob<Vec<Vec<OptimizedMesh>>> {
    LoadJob::spawn(move || load_robot(&config, is_ftc, &options, &*fetch), waker)
}
