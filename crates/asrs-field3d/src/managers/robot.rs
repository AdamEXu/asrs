use std::collections::VecDeque;
use std::sync::Arc;

use asrs_canvas::Canvas2d;
use asrs_js::math;
use asrs_js::string::JsString;
use asrs_log::geometry::{AnnotatedPose3d, ModuleVelocitySample, Pose3d};
use asrs_log::log::mechanism_state::{MechanismLine, MechanismState};
use asrs_log::{units, util};
use asrs_model::assets::{AdvantageScopeAssets, Config3dRobot};

use super::{
    CanvasTexture, InstanceSource, ManagerContext, ManagerError, ObjectManager,
    ResizableInstancedMesh, poses, quaternion_to_rotation3d, rotation3d_to_quaternion,
};
use crate::glb::{LoadOptions, OptimizedMesh, three_quaternion};
use crate::render::{
    FatLine, Geometry, LineMaterial, LineSegmentsGeometry, Materials, NodeKind, PhongMaterial,
    Scene, Side,
};
use crate::three::primitives::{box_geometry, plane_geometry};
use crate::three::{Box3, BufferGeometry, Color, Euler, Graph, ObjectId, Vector3};

const SWERVE_CANVAS_PX: f64 = 1000.0;
const SWERVE_CANVAS_METERS: f64 = 3.0;
const SWERVE_BUMPER_OFFSET: f64 = 0.15;
const MECHANISM_WIDTH_PER_WEIGHT: f64 = 0.01;

/// `type` of `Field3dRendererCommand_RobotObj` and `_GhostObj`; only a ghost has a color.
#[derive(Clone, Debug, PartialEq)]
pub enum RobotKind {
    Robot,
    Ghost { color: JsString },
}

/// `mechanisms` of a robot object.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RobotMechanisms {
    pub xz: Option<MechanismState>,
    pub yz: Option<MechanismState>,
}

/// One entry of `swerveModuleVelocities`.
#[derive(Clone, Debug, PartialEq)]
pub struct SwerveModuleSet {
    pub values: Vec<ModuleVelocitySample>,
    pub color: JsString,
}

/// `Field3dRendererCommand_RobotObj` and `_GhostObj`.
#[derive(Clone, Debug, PartialEq)]
pub struct RobotObj {
    pub kind: RobotKind,
    pub model: JsString,
    pub poses: Vec<AnnotatedPose3d>,
    pub components: Vec<AnnotatedPose3d>,
    pub mechanisms: RobotMechanisms,
    pub vision_targets: Vec<AnnotatedPose3d>,
    pub swerve_module_velocities: Vec<SwerveModuleSet>,
}

impl RobotObj {
    fn is_ghost(&self) -> bool {
        matches!(self.kind, RobotKind::Ghost { .. })
    }
}

/// The `loadRobot` worker request a model change starts. Whoever owns the load queue runs it
/// (`glb::spawn_load_robot`) and hands a result back with `RobotManager::load_finished`; a load
/// that fails never answers, as upstream's worker never posts.
#[derive(Clone, Debug)]
pub struct RobotLoadRequest {
    pub config: Config3dRobot,
    pub is_ftc: bool,
    pub options: LoadOptions,
    counter: u64,
    ghost: bool,
}

struct MechanismLineData {
    mesh: ResizableInstancedMesh,
    geometry: BufferGeometry,
    scale: Vector3,
    translation: Vector3,
    material: Arc<PhongMaterial>,
}

#[derive(Clone, Copy)]
enum Plane {
    Xz,
    Yz,
}

fn type_error(property: &str) -> ManagerError {
    ManagerError(format!(
        "TypeError: Cannot read properties of undefined (reading '{property}')"
    ))
}

/// `array[i]` in arithmetic: a missing element is `undefined` (NaN), a `null` one is 0.
fn coordinate(values: &[Option<f64>], i: usize) -> f64 {
    match values.get(i) {
        None => f64::NAN,
        Some(value) => value.unwrap_or(0.0),
    }
}

fn line_length(line: &MechanismLine) -> f64 {
    math::hypot(&[
        coordinate(&line.end, 1) - coordinate(&line.start, 1),
        coordinate(&line.end, 0) - coordinate(&line.start, 0),
    ])
}

fn line_width(line: &MechanismLine) -> f64 {
    line.weight.unwrap_or(0.0) * MECHANISM_WIDTH_PER_WEIGHT
}

fn vector_equals(a: Vector3, b: Vector3) -> bool {
    a.x == b.x && a.y == b.y && a.z == b.z
}

/// `dummyRobotPose`, `dummyUserPose` and `dummyConfigPose`, each the parent of the next.
struct DummyPoses {
    graph: Graph<()>,
    robot: ObjectId,
    user: ObjectId,
    config: ObjectId,
}

impl DummyPoses {
    fn new() -> Self {
        let mut graph = Graph::new();
        let robot = graph.create("", ());
        let user = graph.create("", ());
        let config = graph.create("", ());
        graph.add(robot, user);
        graph.add(user, config);
        DummyPoses {
            graph,
            robot,
            user,
            config,
        }
    }

    fn set_from_pose(&mut self, id: ObjectId, pose: &Pose3d) {
        let object = self.graph.object_mut(id);
        object.set_rotation_from_quaternion(&rotation3d_to_quaternion(&pose.rotation));
        let [x, y, z] = pose.translation;
        object.position = Vector3::new(x, y, z);
    }

    fn set_euler(&mut self, id: ObjectId, x: f64, y: f64, z: f64) {
        let object = self.graph.object_mut(id);
        let order = object.rotation().order;
        object.set_rotation(Euler::new(x, y, z, order));
    }

    fn world_pose(&mut self, id: ObjectId) -> Pose3d {
        let position = self.graph.world_position(id);
        let quaternion = self.graph.world_quaternion(id);
        Pose3d {
            translation: position.to_array(),
            rotation: quaternion_to_rotation3d(&quaternion),
        }
    }
}

/// `objectManagers/RobotManager.ts`, desktop path: models load through the `loadRobot` worker
/// (XR's direct `GLTFLoader` path is out of scope).
pub struct RobotManager {
    ctx: ManagerContext,
    assets: Box<dyn Fn() -> Option<Arc<AdvantageScopeAssets>>>,
    loading_start: Box<dyn FnMut()>,
    loading_end: Box<dyn FnMut()>,

    meshes: Vec<ResizableInstancedMesh>,
    dimensions: [f64; 4],
    ghost_material: Arc<PhongMaterial>,
    vision_lines: VecDeque<ObjectId>,
    mechanism_lines_xz: VecDeque<MechanismLineData>,
    mechanism_lines_yz: VecDeque<MechanismLineData>,

    swerve: Option<CanvasTexture>,
    swerve_material: Option<Arc<PhongMaterial>>,

    loading_counter: u64,
    should_load_new_model: bool,
    is_loading: bool,
    load_request: Option<RobotLoadRequest>,
    dummy: DummyPoses,
    assets_override: Option<Arc<AdvantageScopeAssets>>,
    hide_robot_models: bool,
    has_new_assets: bool,
    last_model: JsString,
    last_color: JsString,
    last_had_swerve_module_velocities: bool,
    last_hide_robot_models: bool,
}

impl RobotManager {
    /// `assets` reads `window.assets`; `loading_start` and `loading_end` are the renderer's
    /// loading counter callbacks.
    pub fn new(
        ctx: ManagerContext,
        assets: Box<dyn Fn() -> Option<Arc<AdvantageScopeAssets>>>,
        loading_start: Box<dyn FnMut()>,
        loading_end: Box<dyn FnMut()>,
    ) -> Self {
        let ghost_material = Arc::new(PhongMaterial {
            transparent: true,
            opacity: 0.35,
            specular: ctx.material_specular,
            shininess: ctx.material_shininess,
            depth_write: false,
            ..PhongMaterial::new()
        });
        let swerve = (!ctx.is_xr).then(|| {
            let size = SWERVE_CANVAS_PX as u32;
            CanvasTexture::new(size, size)
        });
        RobotManager {
            ctx,
            assets,
            loading_start,
            loading_end,
            meshes: Vec::new(),
            dimensions: [0.0; 4],
            ghost_material,
            vision_lines: VecDeque::new(),
            mechanism_lines_xz: VecDeque::new(),
            mechanism_lines_yz: VecDeque::new(),
            swerve,
            swerve_material: None,
            loading_counter: 0,
            should_load_new_model: false,
            is_loading: false,
            load_request: None,
            dummy: DummyPoses::new(),
            assets_override: None,
            hide_robot_models: false,
            has_new_assets: false,
            last_model: JsString::new(),
            last_color: JsString::new(),
            last_had_swerve_module_velocities: false,
            last_hide_robot_models: false,
        }
    }

    pub fn new_assets(&mut self) {
        self.has_new_assets = true;
    }

    pub fn set_assets_override(&mut self, assets: Option<Arc<AdvantageScopeAssets>>) {
        self.assets_override = assets;
    }

    pub fn set_hide_robot_models(&mut self, hide: bool) {
        self.hide_robot_models = hide;
    }

    pub fn get_model(&self) -> &JsString {
        &self.last_model
    }

    /// `[+x, +y, -x, -y]` extents of the loaded model, which place the swerve vectors.
    pub fn dimensions(&self) -> [f64; 4] {
        self.dimensions
    }

    /// The swerve overlay canvas, absent in XR.
    pub fn swerve_canvas(&self) -> Option<&CanvasTexture> {
        self.swerve.as_ref()
    }

    /// The worker request the last `set_object_data` started, once.
    pub fn take_load_request(&mut self) -> Option<RobotLoadRequest> {
        self.load_request.take()
    }

    /// The worker's answer to `request`: `loadRobot`'s meshes per model file. The meshes join
    /// the scene on the next `set_object_data`. `false` when a later model switch discarded it.
    pub fn load_finished(
        &mut self,
        request: &RobotLoadRequest,
        result: &[Vec<OptimizedMesh>],
    ) -> bool {
        if request.counter != self.loading_counter {
            return false;
        }
        self.meshes = Vec::new();
        self.dimensions = [0.0; 4];
        for (index, scene_meshes) in result.iter().enumerate() {
            let mut sources: Vec<InstanceSource> = Vec::new();
            for optimized in scene_meshes {
                let mesh = optimized.to_render_mesh();
                if index == 0
                    && let Some(position) = mesh.geometry.geometry.attribute("position")
                {
                    let bounds = Box3::from_buffer_attribute(position);
                    let d = &mut self.dimensions;
                    d[0] = math::max2(d[0], bounds.max.x);
                    d[1] = math::max2(d[1], bounds.max.y);
                    d[2] = math::max2(d[2], -bounds.min.x);
                    d[3] = math::max2(d[3], -bounds.min.y);
                }
                let material = if request.ghost {
                    Materials::Single(self.ghost_material.clone())
                } else {
                    mesh.material
                };
                sources.push(InstanceSource {
                    geometry: mesh.geometry,
                    material,
                });
            }
            if index == 0
                && let Some(swerve) = &self.swerve
            {
                let mut geometry =
                    plane_geometry(SWERVE_CANVAS_METERS, SWERVE_CANVAS_METERS, 1.0, 1.0);
                geometry.translate(0.0, 0.0, 0.1);
                let material = Arc::new(PhongMaterial {
                    map: Some(swerve.texture().clone()),
                    transparent: true,
                    side: Side::Double,
                    depth_test: false,
                    ..PhongMaterial::new()
                });
                self.swerve_material = Some(material.clone());
                sources.push(InstanceSource {
                    geometry: Geometry::new(geometry),
                    material: Materials::Single(material),
                });
            }
            let mut cast_shadow = vec![true; sources.len()];
            if let Some(last) = cast_shadow.last_mut() {
                *last = false;
            }
            self.meshes.push(ResizableInstancedMesh::new(
                self.ctx.root,
                sources,
                Some(&cast_shadow),
            ));
        }
        (self.loading_end)();
        self.is_loading = false;
        true
    }

    fn robot_config(&self, model: &JsString) -> Option<Config3dRobot> {
        let assets = self.assets_override.clone().or_else(|| (self.assets)())?;
        assets.robots.iter().find(|r| r.name == *model).cloned()
    }

    /// Every write upstream makes to a shared material object: each mesh and line holding `old`
    /// sees `new`.
    fn replace_material(
        &mut self,
        scene: &mut Scene,
        old: &Arc<PhongMaterial>,
        new: Arc<PhongMaterial>,
    ) {
        let holds = |material: &Materials| match material {
            Materials::Single(m) => Arc::ptr_eq(m, old),
            Materials::Array(_) => false,
        };
        let lines = self
            .mechanism_lines_xz
            .iter_mut()
            .chain(self.mechanism_lines_yz.iter_mut());
        for line in lines {
            if Arc::ptr_eq(&line.material, old) {
                line.material = new.clone();
            }
            if holds(&line.mesh.sources()[0].material) {
                line.mesh
                    .set_material(scene, 0, Materials::Single(new.clone()));
            }
        }
        for instances in &mut self.meshes {
            let indices: Vec<usize> = (0..instances.sources().len())
                .filter(|&i| holds(&instances.sources()[i].material))
                .collect();
            for i in indices {
                instances.set_material(scene, i, Materials::Single(new.clone()));
            }
        }
        if Arc::ptr_eq(&self.ghost_material, old) {
            self.ghost_material = new.clone();
        }
        if self
            .swerve_material
            .as_ref()
            .is_some_and(|m| Arc::ptr_eq(m, old))
        {
            self.swerve_material = Some(new);
        }
    }

    fn set_material_color(
        &mut self,
        scene: &mut Scene,
        material: &Arc<PhongMaterial>,
        color: Color,
    ) {
        let new = Arc::new(PhongMaterial {
            color,
            ..(**material).clone()
        });
        self.replace_material(scene, material, new);
    }

    fn load_model(&mut self, scene: &mut Scene, object: &RobotObj, config: Option<&Config3dRobot>) {
        if object.model != self.last_model
            || self.hide_robot_models != self.last_hide_robot_models
            || self.has_new_assets
        {
            self.should_load_new_model = true;
            self.last_model = object.model.clone();
            self.has_new_assets = false;
            self.last_hide_robot_models = self.hide_robot_models;
        }
        if !self.should_load_new_model || self.is_loading {
            return;
        }
        self.should_load_new_model = false;
        for mesh in std::mem::take(&mut self.meshes) {
            mesh.dispose(scene);
        }
        let Some(config) = config else {
            return;
        };
        if self.hide_robot_models {
            return;
        }
        self.loading_counter += 1;
        (self.loading_start)();
        if self.is_loading {
            (self.loading_end)();
        }
        self.is_loading = true;
        self.load_request = Some(RobotLoadRequest {
            config: config.clone(),
            is_ftc: config.is_ftc,
            options: LoadOptions {
                mode: self.ctx.mode,
                material_specular: self.ctx.material_specular.to_array(),
                material_shininess: self.ctx.material_shininess,
            },
            counter: self.loading_counter,
            ghost: object.is_ghost(),
        });
    }

    fn component_pose(
        &mut self,
        i: usize,
        robot_pose: &Pose3d,
        object: &RobotObj,
        config: Option<&Config3dRobot>,
    ) -> Result<Pose3d, ManagerError> {
        let d = &mut self.dummy;
        d.set_from_pose(d.robot, robot_pose);

        let user_pose = match config {
            Some(_) if i - 1 < object.components.len() => Some(&object.components[i - 1].pose),
            _ => None,
        };
        match user_pose {
            None => {
                d.set_euler(d.user, 0.0, 0.0, 0.0);
                d.graph.object_mut(d.user).position = Vector3::new(0.0, 0.0, 0.0);
            }
            Some(pose) => d.set_from_pose(d.user, pose),
        }

        let config = config.ok_or_else(|| type_error("rotations"))?;
        let (rotations, position) = match user_pose {
            None => (&config.rotations, config.position),
            Some(_) => {
                let component = config
                    .components
                    .get(i - 1)
                    .ok_or_else(|| type_error("zeroedRotations"))?;
                (&component.zeroed_rotations, component.zeroed_position)
            }
        };
        let object = d.graph.object_mut(d.config);
        object.set_rotation_from_quaternion(&three_quaternion(rotations));
        object.position = Vector3::new(position[0], position[1], position[2]);
        Ok(d.world_pose(d.config))
    }

    fn update_vision_lines(&mut self, scene: &mut Scene, object: &RobotObj) {
        let root = self.ctx.root;
        if object.poses.is_empty() {
            while let Some(line) = self.vision_lines.pop_front() {
                scene.graph.remove(root, line);
            }
            return;
        }
        while self.vision_lines.len() > object.vision_targets.len() {
            let line = self.vision_lines.pop_front().expect("non-empty");
            scene.graph.remove(root, line);
        }
        while self.vision_lines.len() < object.vision_targets.len() {
            let line = FatLine {
                geometry: LineSegmentsGeometry::from_segments(Vec::new()),
                material: Arc::new(LineMaterial::new(Color::WHITE, 1.0)),
            };
            self.vision_lines
                .push_back(scene.add(root, NodeKind::FatLine(line)));
        }
        let origin = object.poses[0].pose.translation;
        for (line, target) in self.vision_lines.iter().zip(&object.vision_targets) {
            let mut translation = target.pose.translation;
            if target.annotation.is_2d_source {
                translation[2] = 0.5;
            }
            let NodeKind::FatLine(fat) = &mut scene.node_mut(*line).kind else {
                unreachable!("vision lines are fat lines");
            };
            fat.geometry = LineSegmentsGeometry::from_points(&[
                origin[0],
                origin[1],
                origin[2] + 0.5,
                translation[0],
                translation[1],
                translation[2],
            ]);
            let annotation = &target.annotation;
            if annotation.vision_color.is_some() || annotation.vision_size.is_some() {
                let material = Arc::make_mut(&mut fat.material);
                if let Some(color) = &annotation.vision_color {
                    material.color = Color::style(color);
                }
                if let Some(size) = &annotation.vision_size {
                    material.linewidth = if size.as_string().as_deref() == Some("bold") {
                        3.0
                    } else {
                        1.0
                    };
                }
            }
        }
    }

    fn update_mechanism(
        &mut self,
        scene: &mut Scene,
        object: &RobotObj,
        state: Option<&MechanismState>,
        plane: Plane,
    ) {
        let mut lines = std::mem::take(match plane {
            Plane::Xz => &mut self.mechanism_lines_xz,
            Plane::Yz => &mut self.mechanism_lines_yz,
        });
        let Some(state) = state else {
            while let Some(line) = lines.pop_front() {
                line.mesh.dispose(scene);
            }
            self.put_lines(plane, lines);
            return;
        };

        let visible: Vec<&MechanismLine> = state
            .lines
            .iter()
            .filter(|line| line_length(line) >= 1e-3 && line_width(line) >= 1e-3)
            .collect();
        while lines.len() > visible.len() {
            let line = lines.pop_front().expect("non-empty");
            line.mesh.dispose(scene);
        }
        while lines.len() < visible.len() {
            let geometry = box_geometry(1.0, 1.0, 1.0, 1.0, 1.0, 1.0);
            let material = if object.is_ghost() {
                self.ghost_material.clone()
            } else {
                Arc::new(PhongMaterial {
                    specular: self.ctx.material_specular,
                    shininess: self.ctx.material_shininess,
                    ..PhongMaterial::new()
                })
            };
            let source = InstanceSource {
                geometry: Geometry::new(geometry.clone()),
                material: Materials::Single(material.clone()),
            };
            lines.push_back(MechanismLineData {
                mesh: ResizableInstancedMesh::new(self.ctx.root, vec![source], None),
                geometry,
                scale: Vector3::new(1.0, 1.0, 1.0),
                translation: Vector3::new(0.0, 0.0, 0.0),
                material,
            });
        }
        self.put_lines(plane, lines);

        let offset_x = state.dimensions.first().copied().unwrap_or(f64::NAN) / 2.0;
        for (i, line) in visible.iter().enumerate() {
            let length = line_length(line);
            let angle = math::atan2(
                coordinate(&line.end, 1) - coordinate(&line.start, 1),
                coordinate(&line.end, 0) - coordinate(&line.start, 0),
            );

            let width = line_width(line);
            let new_scale = Vector3::new(length, width, width);
            let new_translation = Vector3::new(length / 2.0, 0.0, 0.0);
            let entry = &mut self.lines_mut(plane)[i];
            if !vector_equals(new_scale, entry.scale)
                || !vector_equals(new_translation, entry.translation)
            {
                let (t, s) = (entry.translation, entry.scale);
                entry.geometry.translate(-t.x, -t.y, -t.z);
                entry.geometry.scale(1.0 / s.x, 1.0 / s.y, 1.0 / s.z);
                entry.geometry.scale(new_scale.x, new_scale.y, new_scale.z);
                entry
                    .geometry
                    .translate(new_translation.x, new_translation.y, new_translation.z);
                entry.scale = new_scale;
                entry.translation = new_translation;
                let geometry = Geometry::new(entry.geometry.clone());
                entry.mesh.set_geometry(scene, 0, geometry);
            }

            if !object.is_ghost() {
                let color = match &line.color {
                    Some(color) => Color::style(color),
                    None => Color::hex(0.0),
                };
                let material = self.lines_mut(plane)[i].material.clone();
                self.set_material_color(scene, &material, color);
            }

            let start_x = coordinate(&line.start, 0) - offset_x;
            let start_y = coordinate(&line.start, 1);
            let mut line_poses = Vec::with_capacity(object.poses.len());
            for robot_pose in &object.poses {
                let d = &mut self.dummy;
                d.set_from_pose(d.robot, &robot_pose.pose);
                if let Plane::Yz = plane {
                    d.graph.object_mut(d.robot).rotate_z(math::PI / 2.0);
                }
                d.graph.object_mut(d.user).position = Vector3::new(start_x, 0.0, start_y);
                d.set_euler(d.user, 0.0, -angle, 0.0);
                line_poses.push(d.world_pose(d.user));
            }
            self.lines_mut(plane)[i].mesh.set_poses(scene, &line_poses);
        }
    }

    fn lines_mut(&mut self, plane: Plane) -> &mut VecDeque<MechanismLineData> {
        match plane {
            Plane::Xz => &mut self.mechanism_lines_xz,
            Plane::Yz => &mut self.mechanism_lines_yz,
        }
    }

    fn put_lines(&mut self, plane: Plane, lines: VecDeque<MechanismLineData>) {
        *self.lines_mut(plane) = lines;
    }

    fn update_swerve(&mut self, scene: &mut Scene, object: &RobotObj) -> Result<(), ManagerError> {
        let dimensions = self.dimensions;
        let Some(swerve) = self.swerve.as_mut() else {
            return Ok(());
        };
        draw_swerve(
            swerve.canvas_mut(),
            dimensions,
            &object.swerve_module_velocities,
        )?;
        let has_swerve_module_velocities = !object.swerve_module_velocities.is_empty();
        let needs_update = has_swerve_module_velocities || self.last_had_swerve_module_velocities;
        self.last_had_swerve_module_velocities = has_swerve_module_velocities;
        if needs_update {
            swerve.update();
            let texture = swerve.texture().clone();
            if let Some(old) = self.swerve_material.clone() {
                let new = Arc::new(PhongMaterial {
                    map: Some(texture),
                    ..(*old).clone()
                });
                self.replace_material(scene, &old, new);
            }
        }
        Ok(())
    }
}

fn draw_swerve(
    context: &mut Canvas2d,
    dimensions: [f64; 4],
    sets: &[SwerveModuleSet],
) -> Result<(), ManagerError> {
    context.clear_rect(0.0, 0.0, SWERVE_CANVAS_PX, SWERVE_CANVAS_PX);
    let px_per_meter = SWERVE_CANVAS_PX / SWERVE_CANVAS_METERS;
    let d = dimensions;
    let module_x = (math::min2(d[0], d[2]) - SWERVE_BUMPER_OFFSET) * px_per_meter;
    let module_y = (math::min2(d[1], d[3]) - SWERVE_BUMPER_OFFSET) * px_per_meter;
    let center_px = [SWERVE_CANVAS_PX / 2.0, SWERVE_CANVAS_PX / 2.0];
    let arrow_scale = px_per_meter
        * units::convert(36.0, "inches", "meters").expect("inches and meters are units");
    for (index, corner) in [[1.0, 1.0], [1.0, -1.0], [-1.0, 1.0], [-1.0, -1.0]]
        .into_iter()
        .enumerate()
    {
        let module_center_px = [
            center_px[0] + module_x * corner[0],
            center_px[1] - module_y * corner[1],
        ];
        for set in sets {
            let Some(sample) = set.values.get(index) else {
                continue;
            };
            context.set_line_width(0.03 * px_per_meter);
            context.set_stroke_style(set.color.to_string_lossy().as_str());
            context.set_line_cap("round");
            context.set_line_join("round");

            let (velocity, angle) = match *sample {
                ModuleVelocitySample::Full(v) => (v.velocity, v.angle),
                ModuleVelocitySample::Partial { velocity, angle } => {
                    (velocity.unwrap_or(0.0), angle.unwrap_or(0.0))
                }
                ModuleVelocitySample::Undefined => return Err(type_error("velocity")),
            };
            if math::abs(velocity) <= 0.001 {
                continue;
            }
            let mut vector_speed = velocity / 5.0;
            let mut vector_rotation = angle;
            if velocity < 0.0 {
                vector_speed *= -1.0;
                vector_rotation += math::PI;
            }
            if vector_speed < 0.05 {
                continue;
            }
            let vector_length = arrow_scale * vector_speed;
            let at = |relative: [f64; 2]| {
                util::transform_px(module_center_px, vector_rotation, relative)
            };
            let back = at([0.0, 0.0]);
            let front = at([vector_length, 0.0]);
            let left = at([vector_length - px_per_meter * 0.1, px_per_meter * 0.1]);
            let right = at([vector_length - px_per_meter * 0.1, px_per_meter * -0.1]);
            context.begin_path();
            context.move_to(back[0], back[1]);
            context.line_to(front[0], front[1]);
            context.move_to(left[0], left[1]);
            context.line_to(front[0], front[1]);
            context.line_to(right[0], right[1]);
            context.stroke();
        }
    }
    Ok(())
}

impl ObjectManager for RobotManager {
    type Object = RobotObj;

    fn dispose(&mut self, scene: &mut Scene) {
        for mesh in &self.meshes {
            mesh.dispose(scene);
        }
        for line in self
            .mechanism_lines_xz
            .iter()
            .chain(&self.mechanism_lines_yz)
        {
            line.mesh.dispose(scene);
        }
        while let Some(line) = self.vision_lines.pop_front() {
            scene.graph.remove(self.ctx.root, line);
        }
    }

    fn set_object_data(
        &mut self,
        scene: &mut Scene,
        object: &RobotObj,
    ) -> Result<(), ManagerError> {
        let config = self.robot_config(&object.model);
        self.load_model(scene, object, config.as_ref());

        if let RobotKind::Ghost { color } = &object.kind
            && *color != self.last_color
        {
            self.last_color = color.clone();
            let ghost = self.ghost_material.clone();
            self.set_material_color(scene, &ghost, Color::style(color));
        }

        let robot_poses = poses(&object.poses);
        if let Some(first) = self.meshes.first_mut() {
            first.set_poses(scene, &robot_poses);
        }

        for i in 1..self.meshes.len() {
            let mut component_poses = Vec::with_capacity(robot_poses.len());
            for robot_pose in &robot_poses {
                component_poses.push(self.component_pose(
                    i,
                    robot_pose,
                    object,
                    config.as_ref(),
                )?);
            }
            self.meshes[i].set_poses(scene, &component_poses);
        }

        self.update_vision_lines(scene, object);
        self.update_mechanism(scene, object, object.mechanisms.xz.as_ref(), Plane::Xz);
        self.update_mechanism(scene, object, object.mechanisms.yz.as_ref(), Plane::Yz);
        self.update_swerve(scene, object)
    }
}
