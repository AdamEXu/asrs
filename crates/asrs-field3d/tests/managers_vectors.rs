//! Replays `parity/vectors/field3d/managers` (oracle/src/genField3dManagers.ts) and
//! `parity/vectors/field3d/robot` (oracle/src/genField3dRobot.ts): each case runs a port of one
//! upstream object manager through the recorded ops and compares the scene under the root after
//! every op with the state upstream's manager left.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use asrs_canvas::Raster;
use asrs_field3d::glb::{self, OptimizedMesh};
use asrs_field3d::managers::{
    AprilTagManager, AprilTagObj, AprilTagVariant, AxesManager, AxesObj, ConeManager, ConeObj,
    GamePieceManager, GamePieceObj, GamePieces, HeatmapField, HeatmapManager, HeatmapObj,
    InstanceSource, ManagerContext, ObjectManager, RobotKind, RobotLoadRequest, RobotManager,
    RobotMechanisms, RobotObj, SwerveModuleSet, TextureRequest, TrajectoryManager, TrajectoryObj,
    april_tag_texture_path, decode_png_texture,
};
use asrs_field3d::render::{
    Filter, Geometry, LineMaterial, Materials, MinFilter, NodeKind, PhongMaterial, Scene, Side,
    Texture,
};
use asrs_field3d::three::primitives::{box_geometry, cylinder_geometry, sphere_geometry};
use asrs_field3d::three::{ArrayData, BufferGeometry, Color, ObjectId};
use asrs_js::math;
use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_log::geometry::{
    AnnotatedPose3d, ModuleVelocity, ModuleVelocitySample, Pose3d, PoseAnnotations,
};
use asrs_log::log::mechanism_state::{MechanismLine, MechanismState};
use asrs_model::assets::{
    AdvantageScopeAssets, Config3dRobot, CoordinateSystem, Parsed, parse_robot,
};
use asrs_model::preferences::{CoordinateSystemPreference, Field3dMode};
use asrs_parity::vectors::{self, Record, check_records};
use asrs_parity::{Value, diff};

type Result<T> = std::result::Result<T, String>;

fn get<'a>(v: &'a Value, key: &str) -> Result<&'a Value> {
    v.get(key).ok_or_else(|| format!("missing {key}"))
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

fn js_string(v: &Value) -> Result<JsString> {
    v.as_str()
        .map(|s| JsString::from_units(s.units().to_vec()))
        .ok_or_else(|| format!("expected a string, got {}", v.kind()))
}

fn numbers<const N: usize>(v: &Value) -> Result<[f64; N]> {
    let values: Vec<f64> = items(v)?.iter().map(number).collect::<Result<_>>()?;
    values
        .try_into()
        .map_err(|v: Vec<f64>| format!("expected {N} numbers, got {}", v.len()))
}

fn num(x: f64) -> Value {
    Value::number(x)
}

fn nums(xs: &[f64]) -> Value {
    Value::Array(xs.iter().map(|&x| num(x)).collect())
}

fn float32(xs: impl IntoIterator<Item = f32>) -> Value {
    Value::TypedArray(
        "Float32Array".to_owned(),
        xs.into_iter().map(|x| num(f64::from(x))).collect(),
    )
}

fn from_js(value: &JsValue) -> Value {
    match value {
        JsValue::Undefined => Value::Undefined,
        JsValue::Null => Value::Null,
        JsValue::Bool(b) => Value::Bool(*b),
        JsValue::Number(x) => num(*x),
        JsValue::String(s) => Value::String(s.units().to_vec().into()),
        JsValue::Array(items) => Value::Array(items.iter().map(from_js).collect()),
        other => panic!("unexpected trace value {other:?}"),
    }
}

fn workspace_root() -> PathBuf {
    vectors::workspace_root()
}

fn poses(v: &Value) -> Result<Vec<AnnotatedPose3d>> {
    items(v)?
        .iter()
        .map(|p| {
            let pose = get(p, "pose")?;
            let annotation = get(p, "annotation")?;
            let april_tag_id = annotation.get("aprilTagId").map(number).transpose()?;
            let vision_color = annotation.get("visionColor").map(js_string).transpose()?;
            let vision_size = annotation.get("visionSize").map(js_string).transpose()?;
            Ok(AnnotatedPose3d {
                pose: Pose3d {
                    translation: numbers(get(pose, "translation")?)?,
                    rotation: numbers(get(pose, "rotation")?)?,
                },
                annotation: PoseAnnotations {
                    is_2d_source: boolean(get(annotation, "is2DSource")?)?,
                    has_april_tag_id: april_tag_id.is_some(),
                    april_tag_id,
                    vision_color,
                    vision_size,
                },
            })
        })
        .collect()
}

fn optional_number(v: &Value) -> Result<Option<f64>> {
    match v {
        Value::Null => Ok(None),
        other => number(other).map(Some),
    }
}

fn mechanism(v: &Value) -> Result<Option<MechanismState>> {
    if matches!(v, Value::Null) {
        return Ok(None);
    }
    let coordinates = |v: &Value| items(v)?.iter().map(optional_number).collect::<Result<_>>();
    let lines = items(get(v, "lines")?)?
        .iter()
        .map(|line| {
            let color = match get(line, "color")? {
                Value::String(_) => Some(js_string(get(line, "color")?)?),
                other if number(other)? == 0.0 => None,
                other => return Err(format!("unmodeled mechanism line color {}", other.encode())),
            };
            Ok(MechanismLine {
                start: coordinates(get(line, "start")?)?,
                end: coordinates(get(line, "end")?)?,
                color,
                weight: optional_number(get(line, "weight")?)?,
            })
        })
        .collect::<Result<_>>()?;
    Ok(Some(MechanismState {
        background_color: js_string(get(v, "backgroundColor")?)?,
        dimensions: items(get(v, "dimensions")?)?
            .iter()
            .map(number)
            .collect::<Result<_>>()?,
        lines,
    }))
}

fn module_sample(v: &Value) -> Result<ModuleVelocitySample> {
    if matches!(v, Value::Undefined) {
        return Ok(ModuleVelocitySample::Undefined);
    }
    let velocity = optional_number(get(v, "velocity")?)?;
    let angle = optional_number(get(v, "angle")?)?;
    Ok(match (velocity, angle) {
        (Some(velocity), Some(angle)) => {
            ModuleVelocitySample::Full(ModuleVelocity { velocity, angle })
        }
        (velocity, angle) => ModuleVelocitySample::Partial { velocity, angle },
    })
}

fn robot_obj(o: &Value) -> Result<RobotObj> {
    let kind = match js_string(get(o, "type")?)?.as_string().as_deref() {
        Some("robot") => RobotKind::Robot,
        Some("ghost") => RobotKind::Ghost {
            color: js_string(get(o, "color")?)?,
        },
        other => return Err(format!("not a robot object type: {other:?}")),
    };
    let mechanisms = get(o, "mechanisms")?;
    let swerve_module_velocities = items(get(o, "swerveModuleVelocities")?)?
        .iter()
        .map(|set| {
            Ok(SwerveModuleSet {
                values: items(get(set, "values")?)?
                    .iter()
                    .map(module_sample)
                    .collect::<Result<_>>()?,
                color: js_string(get(set, "color")?)?,
            })
        })
        .collect::<Result<_>>()?;
    Ok(RobotObj {
        kind,
        model: js_string(get(o, "model")?)?,
        poses: poses(get(o, "poses")?)?,
        components: poses(get(o, "components")?)?,
        mechanisms: RobotMechanisms {
            xz: mechanism(get(mechanisms, "xz")?)?,
            yz: mechanism(get(mechanisms, "yz")?)?,
        },
        vision_targets: poses(get(o, "visionTargets")?)?,
        swerve_module_velocities,
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

fn geometry_digest(g: &BufferGeometry) -> Value {
    Value::string(&fnv1a64(&geometry_tree(g).encode()))
}

fn min_filter(f: MinFilter) -> f64 {
    match f {
        MinFilter::Nearest => 1003.0,
        MinFilter::NearestMipmapNearest => 1004.0,
        MinFilter::NearestMipmapLinear => 1005.0,
        MinFilter::Linear => 1006.0,
        MinFilter::LinearMipmapNearest => 1007.0,
        MinFilter::LinearMipmapLinear => 1008.0,
    }
}

fn mag_filter(f: Filter) -> f64 {
    match f {
        Filter::Nearest => 1003.0,
        Filter::Linear => 1006.0,
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Source {
    Canvas(u64),
    Url(JsString),
}

/// First-seen ordinals of canvases, as the generator labels `texture.image`.
#[derive(Default)]
struct Labels(Vec<Source>);

impl Labels {
    fn label(&mut self, source: &Source) -> usize {
        match self.0.iter().position(|s| s == source) {
            Some(i) => i,
            None => {
                self.0.push(source.clone());
                self.0.len() - 1
            }
        }
    }
}

#[allow(clippy::large_enum_variant)]
enum Manager {
    GamePiece(GamePieceManager),
    AprilTag(AprilTagManager),
    Axes(AxesManager),
    Cone(ConeManager),
    Trajectory(TrajectoryManager),
    Heatmap(HeatmapManager),
    Robot(RobotManager),
}

/// The robot assets a case names: each one's config (its path is `<id>/model.glb`), whether
/// upstream's worker answers for it, and the files behind its URLs.
struct RobotAssets {
    ids: Vec<String>,
    configs: Vec<Config3dRobot>,
    never: Vec<bool>,
    files: HashMap<String, Vec<u8>>,
    results: HashMap<usize, Option<Vec<Vec<OptimizedMesh>>>>,
    window: Rc<RefCell<Option<Arc<AdvantageScopeAssets>>>>,
    loading: Rc<Cell<[u32; 2]>>,
    pending: Vec<RobotLoadRequest>,
    requested: Vec<RobotLoadRequest>,
    calls_seen: usize,
}

impl RobotAssets {
    fn new(setup: &Value) -> Result<RobotAssets> {
        let mut assets = RobotAssets {
            ids: Vec::new(),
            configs: Vec::new(),
            never: Vec::new(),
            files: HashMap::new(),
            results: HashMap::new(),
            window: Rc::default(),
            loading: Rc::default(),
            pending: Vec::new(),
            requested: Vec::new(),
            calls_seen: 0,
        };
        for asset in items(get(setup, "assets")?)? {
            let id = js_string(get(asset, "id")?)?.to_string_lossy();
            let text = js_string(get(asset, "config")?)?.to_string_lossy();
            let raw = asrs_js::json::parse_str(&text).map_err(|e| format!("{e:?}"))?;
            let Ok(Parsed::Config(mut config)) = parse_robot(&raw) else {
                return Err(format!("robot asset {id} does not parse"));
            };
            config.path = JsString::from(format!("{id}/model.glb").as_str());
            for file in items(get(asset, "files")?)? {
                let pair = items(file)?;
                let name = js_string(&pair[0])?.to_string_lossy();
                let bundled = js_string(get(&pair[1], "bundled")?)?.to_string_lossy();
                let bytes = std::fs::read(
                    workspace_root()
                        .join("reference/AdvantageScope")
                        .join(&bundled),
                )
                .map_err(|e| format!("{bundled}: {e}"))?;
                assets.files.insert(format!("{id}/{name}"), bytes);
            }
            assets.ids.push(id);
            assets.configs.push(config);
            assets.never.push(boolean(get(asset, "never")?)?);
        }
        assets.set_window(Some(&assets.ids.clone()))?;
        Ok(assets)
    }

    fn set_window(&self, ids: Option<&[String]>) -> Result<()> {
        let assets = ids
            .map(|ids| {
                let robots = ids
                    .iter()
                    .map(|id| {
                        self.ids
                            .iter()
                            .position(|i| i == id)
                            .map(|i| self.configs[i].clone())
                            .ok_or_else(|| format!("no robot asset {id}"))
                    })
                    .collect::<Result<_>>()?;
                Ok::<_, String>(Arc::new(AdvantageScopeAssets {
                    robots,
                    ..AdvantageScopeAssets::default()
                }))
            })
            .transpose()?;
        *self.window.borrow_mut() = assets;
        Ok(())
    }

    /// Runs the port's `loadRobot` for each pending request and hands back the answers, checking
    /// that the port answers exactly where upstream's worker does.
    fn answer(&mut self, manager: &mut RobotManager) -> Result<()> {
        for request in std::mem::take(&mut self.pending) {
            let index = self
                .configs
                .iter()
                .position(|c| c.path == request.config.path)
                .ok_or("a load request for no asset")?;
            if !self.results.contains_key(&index) {
                let files = &self.files;
                let fetch = |url: &JsString| files.get(&url.to_string_lossy()).cloned();
                let loaded = glb::with_load_stack(|| {
                    glb::load_robot(&request.config, request.is_ftc, &request.options, &fetch)
                });
                self.results.insert(index, loaded.ok());
            }
            match (&self.results[&index], self.never[index]) {
                (Some(meshes), false) => {
                    manager.load_finished(&request, meshes);
                }
                (None, true) => {}
                (Some(_), true) => {
                    return Err(format!("{} loads; upstream never answers", self.ids[index]));
                }
                (None, false) => {
                    return Err(format!(
                        "{} fails to load; upstream answers",
                        self.ids[index]
                    ));
                }
            }
        }
        Ok(())
    }
}

struct Case {
    scene: Scene,
    root: ObjectId,
    manager: Manager,
    labels: Labels,
    pending: Vec<TextureRequest>,
    texture_urls: HashMap<u64, JsString>,
    field: Rc<RefCell<Option<HeatmapField>>>,
    preference: Rc<RefCell<Option<CoordinateSystemPreference>>>,
    robot: Option<RobotAssets>,
    digests: HashMap<u64, Value>,
    checked_textures: HashSet<u64>,
}

fn field_of(v: &Value) -> Result<Option<HeatmapField>> {
    if matches!(v, Value::Null) {
        return Ok(None);
    }
    let system = js_string(get(v, "coordinateSystem")?)?;
    let coordinate_system = [
        CoordinateSystem::WallAlliance,
        CoordinateSystem::WallBlue,
        CoordinateSystem::CenterRotated,
        CoordinateSystem::CenterRed,
    ]
    .into_iter()
    .find(|c| system.as_string().as_deref() == Some(c.as_str()))
    .ok_or("unknown field coordinate system")?;
    Ok(Some(HeatmapField {
        width_inches: number(get(v, "widthInches")?)?,
        height_inches: number(get(v, "heightInches")?)?,
        coordinate_system,
    }))
}

fn preference_of(v: Option<&Value>) -> Result<Option<CoordinateSystemPreference>> {
    match v.and_then(|p| p.get("coordinateSystem")) {
        None | Some(Value::Undefined) => Ok(None),
        Some(system) => CoordinateSystemPreference::from_js_str(&js_string(system)?)
            .map(Some)
            .ok_or_else(|| "unknown coordinate system preference".to_owned()),
    }
}

fn piece(spec: &Value) -> Result<InstanceSource> {
    let geometry = items(get(spec, "geometry")?)?;
    let ctor = js_string(&geometry[0])?.to_string_lossy();
    let args: Vec<f64> = geometry[1..].iter().map(number).collect::<Result<_>>()?;
    let tau = math::PI * 2.0;
    let geometry = match ctor.as_str() {
        "BoxGeometry" => box_geometry(args[0], args[1], args[2], 1.0, 1.0, 1.0),
        "SphereGeometry" => sphere_geometry(args[0], args[1], args[2], 0.0, tau, 0.0, math::PI),
        "CylinderGeometry" => {
            cylinder_geometry(args[0], args[1], args[2], args[3], 1.0, false, 0.0, tau)
        }
        other => return Err(format!("no piece geometry {other}")),
    };
    let material = PhongMaterial {
        color: Color::hex(number(get(spec, "color")?)?),
        ..PhongMaterial::new()
    };
    Ok(InstanceSource {
        geometry: Geometry::new(geometry),
        material: Arc::new(material).into(),
    })
}

impl Case {
    fn new(kind: &str, setup: &Value) -> Result<Case> {
        let mut scene = Scene::new();
        let root = scene.add_group(scene.root);
        let mode = match js_string(get(setup, "mode")?)?.as_string().as_deref() {
            Some("standard") => Field3dMode::Standard,
            Some("cinematic") => Field3dMode::Cinematic,
            Some("low-power") => Field3dMode::LowPower,
            other => return Err(format!("unknown mode {other:?}")),
        };
        let ctx = ManagerContext {
            root,
            material_specular: Color::hex(number(get(setup, "specular")?)?),
            material_shininess: number(get(setup, "shininess")?)?,
            mode,
            is_xr: false,
        };
        let field = Rc::new(RefCell::new(match setup.get("field") {
            Some(v) => field_of(v)?,
            None => None,
        }));
        let preference = Rc::new(RefCell::new(preference_of(setup.get("preferences"))?));
        let robot = if kind == "robot" {
            Some(RobotAssets::new(setup)?)
        } else {
            None
        };
        let manager = match kind {
            "gamePiece" => {
                let mut pieces = GamePieces::new();
                let Value::Object(entries) = get(setup, "pieces")? else {
                    return Err("pieces is not an object".to_owned());
                };
                for (name, spec) in entries {
                    pieces.insert(JsString::from_units(name.units().to_vec()), piece(spec)?);
                }
                Manager::GamePiece(GamePieceManager::new(ctx, Arc::new(pieces)))
            }
            "aprilTag" => Manager::AprilTag(AprilTagManager::new(ctx)),
            "axes" => Manager::Axes(AxesManager::new(ctx)),
            "cone" => Manager::Cone(ConeManager::new(ctx)),
            "trajectory" => Manager::Trajectory(TrajectoryManager::new(ctx, &mut scene)),
            "heatmap" => {
                let f = field.clone();
                let p = preference.clone();
                Manager::Heatmap(HeatmapManager::new(
                    ctx,
                    Box::new(move || *f.borrow()),
                    Box::new(move || *p.borrow()),
                ))
            }
            "robot" => {
                let robot = robot.as_ref().expect("robot assets");
                let window = robot.window.clone();
                let (starts, ends) = (robot.loading.clone(), robot.loading.clone());
                Manager::Robot(RobotManager::new(
                    ctx,
                    Box::new(move || window.borrow().clone()),
                    Box::new(move || starts.set([starts.get()[0] + 1, starts.get()[1]])),
                    Box::new(move || ends.set([ends.get()[0], ends.get()[1] + 1])),
                ))
            }
            other => return Err(format!("unknown manager {other}")),
        };
        Ok(Case {
            scene,
            root,
            manager,
            labels: Labels::default(),
            pending: Vec::new(),
            texture_urls: HashMap::new(),
            field,
            preference,
            robot,
            digests: HashMap::new(),
            checked_textures: HashSet::new(),
        })
    }

    fn apply(&mut self, op: &Value) -> Result<std::result::Result<(), String>> {
        let op = items(op)?;
        let name = js_string(&op[0])?.to_string_lossy();
        let scene = &mut self.scene;
        let outcome = match (name.as_str(), &mut self.manager) {
            ("set", manager) => {
                let o = &op[1];
                let p = poses(get(o, "poses")?)?;
                match manager {
                    Manager::GamePiece(m) => m.set_object_data(
                        scene,
                        &GamePieceObj {
                            variant: js_string(get(o, "variant")?)?,
                            poses: p,
                        },
                    ),
                    Manager::AprilTag(m) => {
                        let variant = get(o, "variant")?;
                        m.set_object_data(
                            scene,
                            &AprilTagObj {
                                poses: p,
                                variant: AprilTagVariant {
                                    family: js_string(get(variant, "family")?)?,
                                    inches: number(get(variant, "inches")?)?,
                                },
                            },
                        )
                    }
                    Manager::Axes(m) => m.set_object_data(scene, &AxesObj { poses: p }),
                    Manager::Cone(m) => m.set_object_data(
                        scene,
                        &ConeObj {
                            color: js_string(get(o, "color")?)?,
                            position: js_string(get(o, "position")?)?,
                            poses: p,
                        },
                    ),
                    Manager::Trajectory(m) => m.set_object_data(
                        scene,
                        &TrajectoryObj {
                            color: js_string(get(o, "color")?)?,
                            size: js_string(get(o, "size")?)?,
                            poses: p,
                        },
                    ),
                    Manager::Heatmap(m) => m.set_object_data(scene, &HeatmapObj { poses: p }),
                    Manager::Robot(m) => m.set_object_data(scene, &robot_obj(o)?),
                }
                .map_err(|e| format!("{e}"))
            }
            ("loadTextures", Manager::AprilTag(m)) => {
                for request in std::mem::take(&mut self.pending) {
                    let Some(path) = april_tag_texture_path(
                        &workspace_root().join("reference/AdvantageScope"),
                        &request.url,
                    ) else {
                        continue;
                    };
                    let Ok(bytes) = std::fs::read(&path) else {
                        continue;
                    };
                    let texture = decode_png_texture(&bytes)
                        .ok_or_else(|| format!("{} does not decode", path.display()))?;
                    self.texture_urls.insert(texture.key(), request.url.clone());
                    m.texture_loaded(scene, &request, texture);
                }
                Ok(())
            }
            ("redAlliance", Manager::Heatmap(m)) => {
                m.set_is_red_alliance(boolean(&op[1])?);
                Ok(())
            }
            ("field", Manager::Heatmap(_)) => {
                *self.field.borrow_mut() = field_of(&op[1])?;
                Ok(())
            }
            ("preferences", Manager::Heatmap(_)) => {
                *self.preference.borrow_mut() = preference_of(op.get(1))?;
                Ok(())
            }
            ("resolution", manager) => {
                let resolution = [number(&op[1])?, number(&op[2])?];
                match manager {
                    Manager::GamePiece(m) => m.set_resolution(resolution),
                    Manager::AprilTag(m) => m.set_resolution(resolution),
                    Manager::Axes(m) => m.set_resolution(resolution),
                    Manager::Cone(m) => m.set_resolution(resolution),
                    Manager::Trajectory(m) => m.set_resolution(resolution),
                    Manager::Heatmap(m) => m.set_resolution(resolution),
                    Manager::Robot(m) => m.set_resolution(resolution),
                }
                Ok(())
            }
            ("dispose", manager) => {
                match manager {
                    Manager::GamePiece(m) => m.dispose(scene),
                    Manager::AprilTag(m) => m.dispose(scene),
                    Manager::Axes(m) => m.dispose(scene),
                    Manager::Cone(m) => m.dispose(scene),
                    Manager::Trajectory(m) => m.dispose(scene),
                    Manager::Heatmap(m) => m.dispose(scene),
                    Manager::Robot(m) => m.dispose(scene),
                }
                Ok(())
            }
            ("load", Manager::Robot(m)) => {
                self.robot.as_mut().expect("robot assets").answer(m)?;
                Ok(())
            }
            ("assets", Manager::Robot(_)) => {
                let ids = match &op[1] {
                    Value::Null => None,
                    v => Some(
                        items(v)?
                            .iter()
                            .map(|id| js_string(id).map(|s| s.to_string_lossy()))
                            .collect::<Result<Vec<_>>>()?,
                    ),
                };
                self.robot
                    .as_ref()
                    .expect("robot assets")
                    .set_window(ids.as_deref())?;
                Ok(())
            }
            ("newAssets", Manager::Robot(m)) => {
                m.new_assets();
                Ok(())
            }
            ("hide", Manager::Robot(m)) => {
                m.set_hide_robot_models(boolean(&op[1])?);
                Ok(())
            }
            (other, _) => return Err(format!("op {other} does not apply to this manager")),
        };
        match &mut self.manager {
            Manager::AprilTag(m) => self.pending.extend(m.take_texture_requests()),
            Manager::Robot(m) => {
                if let Some(request) = m.take_load_request() {
                    let robot = self.robot.as_mut().expect("robot assets");
                    robot.pending.push(request.clone());
                    robot.requested.push(request);
                }
            }
            _ => {}
        }
        Ok(outcome)
    }

    fn source(&self, texture: &Texture) -> Result<Source> {
        match &self.manager {
            Manager::Cone(m) => {
                if texture.key() == m.main_canvas().texture().key() {
                    Ok(Source::Canvas(0))
                } else if texture.key() == m.base_canvas().texture().key() {
                    Ok(Source::Canvas(1))
                } else {
                    Err("a cone texture from no current canvas".to_owned())
                }
            }
            Manager::Heatmap(m) => m
                .canvas()
                .map(Source::Canvas)
                .ok_or_else(|| "a heatmap texture before any canvas".to_owned()),
            Manager::Robot(m) => match m.swerve_canvas() {
                Some(swerve) if swerve.texture().key() == texture.key() => Ok(Source::Canvas(0)),
                _ => Err("a robot texture that is not the current swerve canvas".to_owned()),
            },
            _ => self
                .texture_urls
                .get(&texture.key())
                .map(|url| Source::Url(url.clone()))
                .ok_or_else(|| "a texture no load delivered".to_owned()),
        }
    }

    /// The recording names only a canvas texture's canvas, whose contents the cone traces and
    /// the heatmap store pin. Upstream's `needsUpdate` means the texture shows those contents.
    fn check_canvas_pixels(&mut self, id: u64, texture: &Texture) -> Result<()> {
        let current = match &mut self.manager {
            Manager::Cone(m) => {
                let canvas = if id == 0 {
                    m.main_canvas()
                } else {
                    m.base_canvas()
                }
                .canvas();
                let mut raster =
                    Raster::new(canvas.width, canvas.height).ok_or("cone canvas size")?;
                for call in &canvas.calls {
                    raster.apply(call);
                }
                let pixmap = raster.pixmap();
                asrs_canvas::pixels::read_rect(pixmap, 0, 0, pixmap.width(), pixmap.height())
                    .ok_or("whole-canvas read")?
                    .data
            }
            Manager::Heatmap(m) => match m.heatmap_mut().image_data() {
                Ok(Some(image)) => image.data,
                _ => return Ok(()),
            },
            Manager::Robot(m) => {
                if !self.checked_textures.insert(texture.key()) {
                    return Ok(());
                }
                let swerve = m.swerve_canvas().ok_or("no swerve canvas")?;
                let canvas = swerve.canvas();
                let mut raster =
                    Raster::new(canvas.width, canvas.height).ok_or("swerve canvas size")?;
                for call in &canvas.calls[..swerve.applied()] {
                    raster.apply(call);
                }
                let pixmap = raster.pixmap();
                asrs_canvas::pixels::read_rect(pixmap, 0, 0, pixmap.width(), pixmap.height())
                    .ok_or("whole-canvas read")?
                    .data
            }
            _ => return Ok(()),
        };
        if texture.rgba[..] == current[..] {
            Ok(())
        } else {
            Err(format!("canvas {id}'s texture is not its current pixels"))
        }
    }

    fn texture(&mut self, texture: Option<&Arc<Texture>>) -> Result<Value> {
        let Some(texture) = texture else {
            return Ok(Value::Null);
        };
        let source = match self.source(texture)? {
            Source::Canvas(id) => {
                self.check_canvas_pixels(id, texture)?;
                let label = self.labels.label(&Source::Canvas(id));
                vec![Value::string("canvas"), num(label as f64)]
            }
            Source::Url(url) => vec![
                Value::string("url"),
                Value::String(url.units().to_vec().into()),
            ],
        };
        Ok(Value::Array(
            source
                .into_iter()
                .chain([
                    num(min_filter(texture.min_filter)),
                    num(mag_filter(texture.mag_filter)),
                    Value::Bool(texture.flip_y),
                    Value::Bool(texture.generate_mipmaps),
                ])
                .collect(),
        ))
    }

    fn phong(&mut self, m: &PhongMaterial) -> Result<Value> {
        let side = match m.side {
            Side::Front => 0.0,
            Side::Back => 1.0,
            Side::Double => 2.0,
        };
        Ok(Value::Array(vec![
            Value::string("MeshPhongMaterial"),
            nums(&m.color.to_array()),
            nums(&m.specular.to_array()),
            num(m.shininess),
            nums(&m.emissive.to_array()),
            num(m.opacity),
            Value::Bool(m.transparent),
            num(side),
            Value::Bool(m.depth_test),
            Value::Bool(m.depth_write),
            Value::Bool(m.vertex_colors),
            Value::Bool(m.visible),
            self.texture(m.map.as_ref())?,
        ]))
    }

    fn line_material(m: &LineMaterial) -> Value {
        Value::Array(vec![
            Value::string("LineMaterial"),
            nums(&m.color.to_array()),
            num(m.linewidth),
            num(m.opacity),
            Value::Bool(m.transparent),
            Value::Bool(m.depth_test),
            Value::Bool(m.depth_write),
            Value::Bool(m.visible),
        ])
    }

    fn child(&mut self, id: ObjectId) -> Result<Value> {
        let node = self.scene.node(id).clone();
        let object = self.scene.graph.object(id).clone();
        let (kind, cast, receive, geometry, materials, instances) = match &node.kind {
            NodeKind::Mesh(mesh) => {
                let materials = match &mesh.material {
                    Materials::Single(m) => Value::Array(vec![
                        Value::string("single"),
                        Value::Array(vec![self.phong(m)?]),
                    ]),
                    Materials::Array(ms) => {
                        let dumps = ms.iter().map(|m| self.phong(m)).collect::<Result<_>>()?;
                        Value::Array(vec![Value::string("array"), Value::Array(dumps)])
                    }
                };
                let instances = mesh.instances.as_ref().map_or(Value::Null, |i| {
                    Value::Array(vec![
                        num(i.count() as f64),
                        float32(i.matrices.iter().flatten().copied()),
                    ])
                });
                (
                    "Mesh",
                    mesh.cast_shadow,
                    mesh.receive_shadow,
                    self.digests
                        .entry(mesh.geometry.key())
                        .or_insert_with(|| geometry_digest(&mesh.geometry.geometry))
                        .clone(),
                    materials,
                    instances,
                )
            }
            NodeKind::FatLine(line) => {
                let segments = &line.geometry.segments;
                let geometry = if segments.is_empty() {
                    Value::Null
                } else {
                    float32(segments.iter().flatten().copied())
                };
                let materials = Value::Array(vec![
                    Value::string("single"),
                    Value::Array(vec![Self::line_material(&line.material)]),
                ]);
                ("Line2", false, false, geometry, materials, Value::Null)
            }
            other => return Err(format!("unexpected child {other:?}")),
        };
        Ok(Value::Array(vec![
            Value::string(kind),
            Value::Bool(node.visible),
            Value::Bool(cast),
            Value::Bool(receive),
            Value::Bool(node.frustum_culled),
            nums(&object.position.to_array()),
            nums(&object.quaternion().to_array()),
            nums(&object.scale.to_array()),
            geometry,
            materials,
            instances,
        ]))
    }

    fn extra(&mut self, requested: &[TextureRequest]) -> Value {
        match &self.manager {
            Manager::AprilTag(_) => Value::Array(
                requested
                    .iter()
                    .map(|r| Value::String(r.url.units().to_vec().into()))
                    .collect(),
            ),
            Manager::Cone(m) => {
                let canvases: Vec<(usize, &Source)> = self
                    .labels
                    .0
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| matches!(s, Source::Canvas(_)))
                    .collect();
                Value::Array(
                    canvases
                        .into_iter()
                        .map(|(label, source)| {
                            let canvas = match source {
                                Source::Canvas(0) => m.main_canvas(),
                                _ => m.base_canvas(),
                            };
                            let calls = canvas
                                .canvas()
                                .calls
                                .iter()
                                .map(|c| from_js(&asrs_canvas::trace::call_to_value(c)))
                                .collect();
                            Value::Array(vec![num(label as f64), Value::Array(calls)])
                        })
                        .collect(),
                )
            }
            Manager::Heatmap(m) => {
                let heatmap = m.heatmap();
                let size = heatmap
                    .canvas_size()
                    .map_or(Value::Null, |(w, h)| nums(&[f64::from(w), f64::from(h)]));
                let store = heatmap.get_data().map_or(Value::Null, |store| {
                    Value::object([
                        ("min", num(store.min)),
                        ("max", num(store.max)),
                        (
                            "data",
                            Value::Array(
                                store
                                    .data
                                    .iter()
                                    .map(|p| {
                                        Value::object([
                                            ("x", Value::string(&p.x)),
                                            ("y", Value::string(&p.y)),
                                            ("radius", num(p.radius)),
                                            ("value", num(p.value)),
                                        ])
                                    })
                                    .collect(),
                            ),
                        ),
                    ])
                });
                Value::Array(vec![size, store])
            }
            Manager::Robot(m) => {
                let robot = self.robot.as_mut().expect("robot assets");
                let swerve = m
                    .swerve_canvas()
                    .expect("desktop robots have a swerve canvas");
                let calls = &swerve.canvas().calls;
                let drawn = calls[robot.calls_seen..]
                    .iter()
                    .map(|c| from_js(&asrs_canvas::trace::call_to_value(c)))
                    .collect();
                robot.calls_seen = calls.len();
                let [starts, ends] = robot.loading.get();
                let requests = std::mem::take(&mut robot.requested)
                    .into_iter()
                    .map(|r| {
                        Value::Array(vec![
                            Value::String(r.config.name.units().to_vec().into()),
                            Value::Bool(r.is_ftc),
                            Value::string(r.options.mode.as_str()),
                            nums(&r.options.material_specular),
                            num(r.options.material_shininess),
                        ])
                    })
                    .collect();
                Value::Array(vec![
                    Value::Array(drawn),
                    num(f64::from(swerve.version())),
                    nums(&[f64::from(starts), f64::from(ends)]),
                    Value::String(m.get_model().units().to_vec().into()),
                    nums(&m.dimensions()),
                    Value::Array(requests),
                ])
            }
            _ => Value::Null,
        }
    }

    fn state(
        &mut self,
        outcome: std::result::Result<(), String>,
        requested: &[TextureRequest],
    ) -> Result<Value> {
        let children = self.scene.graph.node(self.root).children().to_vec();
        let children = children
            .into_iter()
            .map(|id| self.child(id))
            .collect::<Result<_>>()?;
        let mut fields = Vec::new();
        if let Err(message) = outcome {
            let (name, message) = match message.strip_prefix("TypeError: ") {
                Some(rest) => ("TypeError", rest),
                None => ("Error", message.as_str()),
            };
            fields.push(("throw", Value::Error(name.into(), message.into())));
        }
        fields.push(("children", Value::Array(children)));
        fields.push(("extra", self.extra(requested)));
        Ok(Value::object(fields))
    }
}

fn check_case(record: &Record, compared: &mut usize) -> Result<()> {
    let kind = record.string("manager").ok_or("record has no manager")?;
    let setup = record.value("setup").map_err(|e| format!("{e}"))?;
    let ops = record.value("ops").map_err(|e| format!("{e}"))?;
    let expected = record.value("out").map_err(|e| format!("{e}"))?;
    let (ops, expected) = (items(&ops)?, items(&expected)?);
    if ops.len() != expected.len() {
        return Err(format!("{} ops but {} states", ops.len(), expected.len()));
    }
    let mut case = Case::new(&kind, &setup)?;
    for (i, (op, expected)) in ops.iter().zip(expected).enumerate() {
        let before = case.pending.len();
        let outcome = case.apply(op)?;
        let requested = case.pending[before.min(case.pending.len())..].to_vec();
        let actual = case.state(outcome, &requested)?;
        if !vectors::values_match(expected, &actual) {
            let report =
                diff::report(expected, &actual).unwrap_or_else(|| "encodings differ".to_owned());
            return Err(format!("op {i} {}: {report}", op.encode()));
        }
        *compared += 1;
    }
    Ok(())
}

fn check_vectors(id: &str) {
    let mut compared = 0;
    let mut recorded = 0;
    check_records(id, |record| {
        recorded += record
            .value("out")
            .ok()
            .and_then(|v| v.as_array().map(<[Value]>::len))
            .unwrap_or(0);
        check_case(record, &mut compared)
    });
    assert_eq!(compared, recorded, "every recorded state is compared");
}

#[test]
fn managers_match_upstream() {
    check_vectors("field3d/managers");
}

#[test]
fn robot_matches_upstream() {
    check_vectors("field3d/robot");
}
