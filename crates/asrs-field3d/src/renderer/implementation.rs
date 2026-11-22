// Ported from AdvantageScope (BSD-3-Clause, Copyright 2021-2026 Littleton Robotics) and three.js
// r168 (MIT, Copyright 2010-2024 three.js authors). See THIRD_PARTY_NOTICES.md.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use asrs_js::json;
use asrs_js::math;
use asrs_js::number;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue};
use asrs_log::geometry::{self, Config3dRotation};
use asrs_log::units;
use asrs_model::assets::{
    self, AdvantageScopeAssets, Config3dField, Config3dRobot, CoordinateSystem, built_in_3d_fields,
};
use asrs_model::preferences::{CoordinateSystemPreference, Field3dMode};

use super::command::{AnyObj, CameraOverride, Field3dRendererCommand, ObjType};
use super::{Field3dDom, Field3dEnv, LoadRequest, RenderCall, TextureLoad};
use crate::camera_controls::{CameraControls, ElementRect};
use crate::fields::{make_axes_field, make_evergreen_field};
use crate::glb::{LoadOptions, LoadedField, OptimizedMesh};
use crate::managers::{
    AprilTagManager, AxesManager, ConeManager, GamePieceManager, GamePieces, HeatmapField,
    HeatmapManager, InstanceSource, ManagerContext, ManagerError, ObjectManager, RobotKind,
    RobotLoadRequest, RobotManager, RobotMechanisms, RobotObj, TextureRequest, TrajectoryManager,
    rotation3d_to_quaternion,
};
use crate::render::{Frame, FrameOptions, Line, NodeKind, Scene, SpotLight, Texture, build_frame};
use crate::three::{Color, Euler, Object3D, ObjectId, PerspectiveCamera, Quaternion, Vector3};

const LOWER_POWER_MAX_FPS: f64 = 30.0;
const MAX_ORBIT_FOV: f64 = 160.0;
const MIN_ORBIT_FOV: f64 = 10.0;
const ORBIT_FIELD_FRC_DEFAULT_TARGET: [f64; 3] = [0.0, 0.5, 0.0];
const ORBIT_FIELD_FTC_DEFAULT_TARGET: [f64; 3] = [0.0, 0.2, 0.0];
const ORBIT_ROBOT_FRC_DEFAULT_TARGET: [f64; 3] = [0.0, 0.5, 0.0];
const ORBIT_ROBOT_FTC_DEFAULT_TARGET: [f64; 3] = [0.0, 0.25, 0.0];
const ORBIT_FIELD_FRC_DEFAULT_POSITION: [f64; 3] = [0.0, 6.0, -12.0];
const ORBIT_FIELD_FTC_DEFAULT_POSITION: [f64; 3] = [0.0, 2.0, -4.0];
const ORBIT_ROBOT_FRC_DEFAULT_POSITION: [f64; 3] = [2.0, 1.0, 1.0];
const ORBIT_ROBOT_FTC_DEFAULT_POSITION: [f64; 3] = [1.0, 0.5, 0.5];
const DS_CAMERA_OFFSET_FRC: f64 = 1.5;
const DS_CAMERA_TARGET_FTC: [f64; 3] = [0.0, -0.2, 0.0];
const CONTROLS_MIN_DISTANCE_FRC: f64 = 1.0;
const CONTROLS_MIN_DISTANCE_FTC: f64 = 0.5;
const SPOT_LIGHT_POSITIONS_FRC: [[f64; 6]; 3] = [
    [0.0, 1.0, 8.0, 0.0, -2.0, 0.0],
    [6.0, -3.0, 8.0, 6.0, 2.0, 0.0],
    [-6.0, -3.0, 8.0, -6.0, 2.0, 0.0],
];
const SPOT_LIGHT_POSITIONS_FTC: [[f64; 6]; 3] = [
    [0.0, 0.0, 4.0, 0.0, 0.0, 0.0],
    [2.0, -1.5, 4.0, 0.0, 1.0, 0.0],
    [-2.0, 1.5, 4.0, 0.0, -1.0, 0.0],
];
const SPOT_LIGHT_INTENSITY_FRC: f64 = 150.0;
const SPOT_LIGHT_INTENSITY_FTC: f64 = 30.0;
const FTC_GRID_COLOR: u32 = 0x888888;

const ORBIT_FIELD: f64 = -1.0;
const ORBIT_ROBOT: f64 = -2.0;
const DS_AUTO: f64 = -3.0;

fn ds_camera_height() -> f64 {
    units::convert(62.0, "inches", "meters").expect("inches and meters are units")
}

fn inches_to_meters(inches: f64) -> f64 {
    units::convert(inches, "inches", "meters").expect("inches and meters are units")
}

fn sequence(rotations: &[(&str, f64)]) -> Quaternion {
    let rotations: Vec<Config3dRotation> = rotations
        .iter()
        .map(|&(axis, degrees)| Config3dRotation {
            axis: JsString::from(axis),
            degrees,
        })
        .collect();
    let q = geometry::rotation_sequence_to_quaternion(&rotations);
    Quaternion::new(q.x, q.y, q.z, q.w)
}

fn config_sequence(rotations: &[assets::Config3dRotation]) -> Quaternion {
    let q = assets::rotation_sequence_to_quaternion(rotations);
    Quaternion::new(q.x, q.y, q.z, q.w)
}

fn wpilib_rotation() -> Quaternion {
    sequence(&[("x", -90.0), ("y", 180.0)])
}

fn camera_rotation() -> Quaternion {
    sequence(&[("z", -90.0), ("y", -90.0)])
}

fn vec3(v: [f64; 3]) -> Vector3 {
    Vector3::new(v[0], v[1], v[2])
}

/// `cameras[index]`: only a whole, in-range index finds an element.
fn camera_at(robot: &Config3dRobot, index: f64) -> Option<&assets::Config3dRobotCamera> {
    if index >= 0.0 && index.fract() == 0.0 {
        robot.cameras.get(index as usize)
    } else {
        None
    }
}

/// Tickets are unique across implementations, so an answer for a replaced one finds nothing.
fn next_ticket() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn manager_error(e: ManagerError) -> JsError {
    let text = e.0;
    match text.split_once(": ") {
        Some(("TypeError", message)) => JsError::type_error(message),
        Some(("RangeError", message)) => JsError::range(message),
        _ => JsError::new("Error", text),
    }
}

/// `THREE.Clock` reading `performance.now()`.
#[derive(Default)]
struct Clock {
    running: bool,
    old_time: f64,
}

impl Clock {
    fn get_delta(&mut self, now: f64) -> f64 {
        if !self.running {
            self.running = true;
            self.old_time = now;
            return 0.0;
        }
        let diff = (now - self.old_time) / 1000.0;
        self.old_time = now;
        diff
    }
}

#[allow(clippy::large_enum_variant)]
enum AnyManager {
    Robot(RobotManager),
    GamePiece(GamePieceManager),
    Trajectory(TrajectoryManager),
    Heatmap(HeatmapManager),
    AprilTag(AprilTagManager),
    Axes(AxesManager),
    Cone(ConeManager),
}

impl AnyManager {
    fn dispose(&mut self, scene: &mut Scene) {
        match self {
            AnyManager::Robot(m) => m.dispose(scene),
            AnyManager::GamePiece(m) => m.dispose(scene),
            AnyManager::Trajectory(m) => m.dispose(scene),
            AnyManager::Heatmap(m) => m.dispose(scene),
            AnyManager::AprilTag(m) => m.dispose(scene),
            AnyManager::Axes(m) => m.dispose(scene),
            AnyManager::Cone(m) => m.dispose(scene),
        }
    }

    fn set_resolution(&mut self, resolution: [f64; 2]) {
        match self {
            AnyManager::Robot(m) => m.set_resolution(resolution),
            AnyManager::GamePiece(m) => m.set_resolution(resolution),
            AnyManager::Trajectory(m) => m.set_resolution(resolution),
            AnyManager::Heatmap(m) => m.set_resolution(resolution),
            AnyManager::AprilTag(m) => m.set_resolution(resolution),
            AnyManager::Axes(m) => m.set_resolution(resolution),
            AnyManager::Cone(m) => m.set_resolution(resolution),
        }
    }

    fn set_object_data(&mut self, scene: &mut Scene, object: &AnyObj) -> Result<(), ManagerError> {
        match (self, object) {
            (AnyManager::Robot(m), AnyObj::Robot(o)) => m.set_object_data(scene, o),
            (AnyManager::GamePiece(m), AnyObj::GamePiece(o)) => m.set_object_data(scene, o),
            (AnyManager::Trajectory(m), AnyObj::Trajectory(o)) => m.set_object_data(scene, o),
            (AnyManager::Heatmap(m), AnyObj::Heatmap(o)) => m.set_object_data(scene, o),
            (AnyManager::AprilTag(m), AnyObj::AprilTag(o) | AnyObj::AprilTagBuiltIn(o)) => {
                m.set_object_data(scene, o)
            }
            (AnyManager::Axes(m), AnyObj::Axes(o)) => m.set_object_data(scene, o),
            (AnyManager::Cone(m), AnyObj::Cone(o)) => m.set_object_data(scene, o),
            _ => unreachable!("managers only receive objects of their own type"),
        }
    }

    fn robot(&mut self) -> Option<&mut RobotManager> {
        match self {
            AnyManager::Robot(m) => Some(m),
            _ => None,
        }
    }
}

struct ManagerEntry {
    id: u64,
    kind: ObjType,
    manager: AnyManager,
    active: bool,
}

#[allow(clippy::large_enum_variant)]
enum Pending {
    Field,
    Robot {
        manager: u64,
        request: RobotLoadRequest,
    },
}

/// `Field3dRendererImpl.ts` with the WebGL renderer replaced by a scene, a camera and the
/// drawing-buffer state the GPU renderer reads.
pub struct Field3dRendererImpl {
    mode: Field3dMode,
    use_aa: bool,
    material_specular: Color,
    material_shininess: f64,

    should_reset_camera: bool,
    clock: Clock,
    scene: Scene,
    controls: CameraControls,
    wpilib_coordinate_group: ObjectId,
    wpilib_field_coordinate_group: ObjectId,
    field: Option<ObjectId>,
    field_staged_pieces: Option<ObjectId>,
    field_pieces: Arc<GamePieces>,
    primary_robot_group: ObjectId,
    fixed_camera_obj: ObjectId,
    fixed_camera_override_obj: ObjectId,
    ds_camera_group: ObjectId,
    ds_camera_obj: ObjectId,
    spot_lights: Vec<ObjectId>,
    ftc_grid: Option<ObjectId>,

    object_managers: Vec<ManagerEntry>,
    next_manager_id: u64,
    orphans: Vec<(u64, RobotManager)>,

    should_render: bool,
    camera_index: f64,
    orbit_fov: f64,
    primary_robot_model: JsString,
    resolution: [f64; 2],
    field_config_cache: Option<Config3dField>,
    heatmap_field: Rc<Cell<Option<HeatmapField>>>,
    robot_loading_count: Rc<Cell<i64>>,
    should_load_new_field: bool,
    is_field_loading: bool,
    aspect_ratio: Option<f64>,
    last_camera_index: f64,
    last_auto_driver_station: f64,
    last_frame_time: f64,
    last_width: Option<f64>,
    last_height: Option<f64>,
    last_device_pixel_ratio: Option<f64>,
    last_is_dark: Option<bool>,
    last_command_string: String,
    last_assets: Option<Arc<AdvantageScopeAssets>>,
    last_field_id: JsString,
    last_is_ftc: Option<bool>,
    last_coordinate_system: Option<JsString>,
    keys_pressed: BTreeSet<String>,
    right_click_start: Option<[f64; 2]>,

    window_assets: Rc<RefCell<Option<Arc<AdvantageScopeAssets>>>>,
    coordinate_preference: Rc<Cell<Option<CoordinateSystemPreference>>>,

    canvas_size: [u32; 2],
    gl_size: [f64; 2],
    pixel_ratio: f64,
    renders: Vec<RenderCall>,

    pending: HashMap<u64, Pending>,
    load_requests: Vec<LoadRequest>,
    texture_requests: Vec<TextureLoad>,
    texture_targets: HashMap<u64, (u64, TextureRequest)>,
}

impl Field3dRendererImpl {
    /// `new Field3dRendererImpl(mode, useAA, ...)`. `mac` is camera-controls'
    /// `/Mac/.test(navigator.platform)`.
    pub fn new(mode: Field3dMode, use_aa: bool, mac: bool) -> Self {
        let cinematic = mode == Field3dMode::Cinematic;
        let (material_specular, material_shininess) = if cinematic {
            (Color::hex(f64::from(0x666666)), 100.0)
        } else {
            (Color::hex(0.0), 0.0)
        };
        let mut scene = Scene::new();
        let root = scene.root;

        let wpilib_coordinate_group = scene.add_group(root);
        scene
            .graph
            .object_mut(wpilib_coordinate_group)
            .set_rotation_from_quaternion(&wpilib_rotation());
        let wpilib_field_coordinate_group = scene.add_group(wpilib_coordinate_group);

        let camera = PerspectiveCamera::new(50.0, 2.0, 0.15, 1000.0);
        let mut controls = CameraControls::new(camera, ElementRect::default(), mac);
        controls.min_distance = CONTROLS_MIN_DISTANCE_FRC;
        controls.max_distance = f64::INFINITY;
        controls.smooth_time = 0.1;
        controls.dragging_smooth_time = 0.1;
        controls.infinity_dolly = true;
        controls.dolly_speed = 0.25;

        scene.add_hemisphere_light(
            root,
            Color::hex(f64::from(0xffffff)),
            Color::hex(f64::from(0x444444)),
            if cinematic { 0.5 } else { 2.0 },
        );
        let mut spot_lights = Vec::new();
        if !cinematic {
            let light = scene.add_point_light(
                wpilib_coordinate_group,
                Color::hex(f64::from(0xffffff)),
                0.5,
            );
            scene.graph.object_mut(light).position = Vector3::new(0.0, 0.0, 10.0);
        } else {
            for [x, y, z, tx, ty, tz] in SPOT_LIGHT_POSITIONS_FRC {
                let (light, target) = scene.create_spot_light(
                    Color::hex(f64::from(0xffffff)),
                    SPOT_LIGHT_INTENSITY_FRC,
                    0.0,
                    50.0 * (math::PI / 180.0),
                    0.2,
                    2.0,
                );
                scene.graph.object_mut(light).position = Vector3::new(x, y, z);
                scene.graph.object_mut(target).position = Vector3::new(tx, ty, tz);
                if let NodeKind::SpotLight(spot) = &mut scene.node_mut(light).kind {
                    spot.cast_shadow = true;
                    spot.shadow.map_size = [2048.0, 2048.0];
                    spot.shadow.bias = -0.0001;
                }
                scene.graph.add(wpilib_coordinate_group, light);
                scene.graph.add(wpilib_coordinate_group, target);
                spot_lights.push(light);
            }
            for (color, x) in [(0xff0000, -4.5), (0x0000ff, 4.5)] {
                let light = scene.add_point_light(
                    wpilib_coordinate_group,
                    Color::hex(f64::from(color)),
                    60.0,
                );
                scene.graph.object_mut(light).position = Vector3::new(x, 0.0, 5.0);
            }
        }

        let fixed_camera_obj = scene.create(NodeKind::Object3D);
        let primary_robot_group = scene.create(NodeKind::Group);
        scene.graph.add(primary_robot_group, fixed_camera_obj);
        scene.node_mut(primary_robot_group).visible = false;
        let fixed_camera_override_obj = scene.create(NodeKind::Object3D);
        scene.node_mut(fixed_camera_override_obj).visible = false;
        scene
            .graph
            .add(wpilib_field_coordinate_group, primary_robot_group);
        scene
            .graph
            .add(wpilib_field_coordinate_group, fixed_camera_override_obj);

        let ds_camera_obj = scene.create(NodeKind::Object3D);
        scene.graph.object_mut(ds_camera_obj).position =
            Vector3::new(-DS_CAMERA_OFFSET_FRC, 0.0, ds_camera_height());
        let ds_camera_group = scene.create(NodeKind::Group);
        scene.graph.add(ds_camera_group, ds_camera_obj);
        scene.graph.add(wpilib_coordinate_group, ds_camera_group);

        Field3dRendererImpl {
            mode,
            use_aa,
            material_specular,
            material_shininess,
            should_reset_camera: true,
            clock: Clock::default(),
            scene,
            controls,
            wpilib_coordinate_group,
            wpilib_field_coordinate_group,
            field: None,
            field_staged_pieces: None,
            field_pieces: Arc::new(GamePieces::new()),
            primary_robot_group,
            fixed_camera_obj,
            fixed_camera_override_obj,
            ds_camera_group,
            ds_camera_obj,
            spot_lights,
            ftc_grid: None,
            object_managers: Vec::new(),
            next_manager_id: 0,
            orphans: Vec::new(),
            should_render: false,
            camera_index: ORBIT_FIELD,
            orbit_fov: 50.0,
            primary_robot_model: JsString::new(),
            resolution: [0.0, 0.0],
            field_config_cache: None,
            heatmap_field: Rc::default(),
            robot_loading_count: Rc::default(),
            should_load_new_field: false,
            is_field_loading: false,
            aspect_ratio: None,
            last_camera_index: -1.0,
            last_auto_driver_station: -1.0,
            last_frame_time: 0.0,
            last_width: Some(0.0),
            last_height: Some(0.0),
            last_device_pixel_ratio: None,
            last_is_dark: None,
            last_command_string: String::new(),
            last_assets: None,
            last_field_id: JsString::new(),
            last_is_ftc: None,
            last_coordinate_system: None,
            keys_pressed: BTreeSet::new(),
            right_click_start: None,
            window_assets: Rc::default(),
            coordinate_preference: Rc::default(),
            canvas_size: [300, 150],
            gl_size: [300.0, 150.0],
            pixel_ratio: 1.0,
            renders: Vec::new(),
            pending: HashMap::new(),
            load_requests: Vec::new(),
            texture_requests: Vec::new(),
            texture_targets: HashMap::new(),
        }
    }

    pub fn mode(&self) -> Field3dMode {
        self.mode
    }

    pub fn use_aa(&self) -> bool {
        self.use_aa
    }

    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    pub fn controls(&self) -> &CameraControls {
        &self.controls
    }

    /// The controls, for the pointer and wheel events camera-controls listens to on the canvas.
    pub fn controls_mut(&mut self) -> &mut CameraControls {
        &mut self.controls
    }

    /// The drawing-buffer size (`canvas.width`, `canvas.height`).
    pub fn canvas_size(&self) -> [u32; 2] {
        self.canvas_size
    }

    /// The `renderer.render` calls since the last take.
    pub fn take_renders(&mut self) -> Vec<RenderCall> {
        std::mem::take(&mut self.renders)
    }

    /// The frame the last `renderer.render` call draws.
    pub fn build_frame(&mut self) -> Frame {
        let mut options = FrameOptions::new(
            self.gl_size[0],
            self.gl_size[1],
            self.pixel_ratio,
            self.use_aa,
        );
        options.shadow_map = self.mode == Field3dMode::Cinematic;
        build_frame(&mut self.scene, self.controls.camera_mut(), options)
    }

    /// The heatmap managers, in reconciliation order.
    pub fn heatmap_managers(&self) -> impl Iterator<Item = &HeatmapManager> {
        self.object_managers
            .iter()
            .filter_map(|e| match &e.manager {
                AnyManager::Heatmap(m) => Some(m),
                _ => None,
            })
    }

    /// The worker loads started since the last take, in request order.
    pub fn take_load_requests(&mut self) -> Vec<LoadRequest> {
        std::mem::take(&mut self.load_requests)
    }

    /// The `TextureLoader` loads started since the last take, in request order.
    pub fn take_texture_requests(&mut self) -> Vec<TextureLoad> {
        std::mem::take(&mut self.texture_requests)
    }

    pub fn save_state(&self) -> JsValue {
        let position = self.controls.get_position(true);
        let target = self.controls.get_target(true);
        let mut o = JsObject::new();
        o.insert("cameraIndex", JsValue::Number(self.camera_index));
        o.insert("orbitFov", JsValue::Number(self.orbit_fov));
        o.insert(
            "cameraPosition",
            JsValue::Array(position.to_array().map(JsValue::Number).to_vec()),
        );
        o.insert(
            "cameraTarget",
            JsValue::Array(target.to_array().map(JsValue::Number).to_vec()),
        );
        JsValue::Object(o)
    }

    pub fn restore_state(&mut self, state: &JsValue) {
        let JsValue::Object(object) = state else {
            if matches!(state, JsValue::Array(_)) {
                self.should_reset_camera = true;
                self.last_camera_index = self.camera_index;
                self.should_render = true;
            }
            return;
        };
        if let Some(JsValue::Number(index)) = object.get("cameraIndex") {
            self.camera_index = *index;
        }
        if let Some(JsValue::Number(fov)) = object.get("orbitFov") {
            self.orbit_fov = *fov;
        }
        let vector = |key: &str| -> Option<[f64; 3]> {
            let items = object.get(key)?.as_array()?;
            let numbers: Vec<f64> = items.iter().map(JsValue::as_f64).collect::<Option<_>>()?;
            if numbers.len() != 3 || numbers.iter().all(|&x| x == 0.0) {
                return None;
            }
            Some([numbers[0], numbers[1], numbers[2]])
        };
        if let (Some(p), Some(t)) = (vector("cameraPosition"), vector("cameraTarget")) {
            self.controls
                .set_look_at(p[0], p[1], p[2], t[0], t[1], t[2], false);
            self.should_reset_camera = false;
        } else {
            self.should_reset_camera = true;
        }
        self.last_camera_index = self.camera_index;
        self.should_render = true;
    }

    pub fn set_3d_camera(&mut self, index: f64) {
        self.camera_index = index;
        self.should_render = true;
    }

    pub fn set_fov(&mut self, fov: f64) {
        self.orbit_fov = math::min2(math::max2(fov, MIN_ORBIT_FOV), MAX_ORBIT_FOV);
        self.should_render = true;
    }

    pub fn get_aspect_ratio(&self) -> Option<f64> {
        self.aspect_ratio
    }

    /// The window `keydown` listener. `editable` is a target that is an input, a textarea or
    /// content-editable; `container_height` is the canvas container's `clientHeight`.
    pub fn key_down(
        &mut self,
        code: &str,
        command_modifier: bool,
        editable: bool,
        container_height: f64,
    ) {
        if command_modifier || editable || container_height == 0.0 {
            return;
        }
        self.keys_pressed.insert(code.to_owned());
    }

    pub fn key_up(&mut self, code: &str, editable: bool) {
        if editable {
            return;
        }
        self.keys_pressed.remove(code);
    }

    pub fn mouse_down(&mut self, button: f64, x: f64, y: f64) {
        if button == 2.0 {
            self.right_click_start = Some([x, y]);
        }
    }

    /// The canvas `mouseup` listener: the `ask-3d-camera` message data for a right click that
    /// did not move.
    pub fn mouse_up(
        &mut self,
        x: f64,
        y: f64,
        assets: Option<&AdvantageScopeAssets>,
    ) -> Option<JsValue> {
        let start = self.right_click_start.take();
        let start = start?;
        if x != start[0] || y != start[1] {
            return None;
        }
        let robot =
            assets.and_then(|a| a.robots.iter().find(|r| r.name == self.primary_robot_model));
        let options: Vec<JsValue> = robot.map_or(Vec::new(), |r| {
            r.cameras
                .iter()
                .map(|c| JsValue::String(c.name.clone()))
                .collect()
        });
        let selected = if self.camera_index >= options.len() as f64 {
            ORBIT_FIELD
        } else {
            self.camera_index
        };
        let mut o = JsObject::new();
        o.insert("options", JsValue::Array(options));
        o.insert(
            "position",
            JsValue::Array(vec![JsValue::Number(x), JsValue::Number(y)]),
        );
        o.insert("selectedIndex", JsValue::Number(selected));
        o.insert("fov", JsValue::Number(self.orbit_fov));
        o.insert(
            "isFTC",
            JsValue::Bool(self.field_config_cache.as_ref().is_some_and(|f| f.is_ftc)),
        );
        Some(JsValue::Object(o))
    }

    /// The `loadField` worker's answer.
    pub fn field_loaded(&mut self, ticket: u64, result: &LoadedField) {
        if !matches!(self.pending.remove(&ticket), Some(Pending::Field)) {
            return;
        }
        let field = self.scene.create(NodeKind::Group);
        for mesh in &result.field {
            let node = self.scene.create(NodeKind::Mesh(mesh.to_render_mesh()));
            self.scene.graph.add(field, node);
        }
        let staged = self.scene.create(NodeKind::Group);
        for mesh in &result.staged_pieces {
            let node = self.scene.create(NodeKind::Mesh(mesh.to_render_mesh()));
            self.scene.graph.add(staged, node);
        }
        self.field = Some(field);
        self.field_staged_pieces = Some(staged);
        let mut pieces = GamePieces::new();
        for (name, mesh) in &result.pieces {
            let mesh = mesh.to_render_mesh();
            pieces.insert(
                name.clone(),
                InstanceSource {
                    geometry: mesh.geometry,
                    material: mesh.material,
                },
            );
        }
        self.new_field_ready(pieces);
        self.is_field_loading = false;
    }

    /// The `loadRobot` worker's answer, delivered to the manager that asked even if it has
    /// since been disposed.
    pub fn robot_loaded(&mut self, ticket: u64, result: &[Vec<OptimizedMesh>]) {
        let Some(Pending::Robot { manager, request }) = self.pending.remove(&ticket) else {
            return;
        };
        let applied = if let Some(entry) = self.object_managers.iter_mut().find(|e| e.id == manager)
        {
            entry
                .manager
                .robot()
                .is_some_and(|m| m.load_finished(&request, result))
        } else if let Some(i) = self.orphans.iter().position(|(id, _)| *id == manager) {
            let (_, mut orphan) = self.orphans.remove(i);
            orphan.load_finished(&request, result)
        } else {
            false
        };
        if applied {
            self.should_render = true;
        }
    }

    /// A `TextureLoader` success callback.
    pub fn texture_loaded(&mut self, ticket: u64, texture: Texture) {
        let Some((manager, request)) = self.texture_targets.remove(&ticket) else {
            return;
        };
        self.should_render = true;
        if let Some(entry) = self.object_managers.iter_mut().find(|e| e.id == manager)
            && let AnyManager::AprilTag(m) = &mut entry.manager
        {
            m.texture_loaded(&mut self.scene, &request, texture);
        }
    }

    fn reset_camera(
        &mut self,
        command: &Field3dRendererCommand,
        is_ftc: bool,
        animate: bool,
        env: &Field3dEnv,
    ) -> Result<(), JsError> {
        let animate = animate && !env.reduced_motion;
        self.controls.min_distance = if is_ftc {
            CONTROLS_MIN_DISTANCE_FTC
        } else {
            CONTROLS_MIN_DISTANCE_FRC
        };
        let look = |controls: &mut CameraControls, p: [f64; 3], t: [f64; 3]| {
            controls.set_look_at(p[0], p[1], p[2], t[0], t[1], t[2], animate);
        };
        if self.camera_index == ORBIT_FIELD {
            if is_ftc {
                look(
                    &mut self.controls,
                    ORBIT_FIELD_FTC_DEFAULT_POSITION,
                    ORBIT_FIELD_FTC_DEFAULT_TARGET,
                );
            } else {
                look(
                    &mut self.controls,
                    ORBIT_FIELD_FRC_DEFAULT_POSITION,
                    ORBIT_FIELD_FRC_DEFAULT_TARGET,
                );
            }
        } else if self.camera_index == ORBIT_ROBOT {
            if is_ftc {
                look(
                    &mut self.controls,
                    ORBIT_ROBOT_FTC_DEFAULT_POSITION,
                    ORBIT_ROBOT_FTC_DEFAULT_TARGET,
                );
            } else {
                look(
                    &mut self.controls,
                    ORBIT_ROBOT_FRC_DEFAULT_POSITION,
                    ORBIT_ROBOT_FRC_DEFAULT_TARGET,
                );
            }
        } else if let Some(field_config) = self.get_field_config(command, env) {
            let driver_station = if self.camera_index < -3.0 {
                -4.0 - self.camera_index
            } else {
                command.auto_driver_station
            };
            if driver_station >= 0.0 && driver_station < field_config.driver_stations.len() as f64 {
                if driver_station.fract() != 0.0 {
                    return Err(JsError::type_error(
                        "Cannot read properties of undefined (reading '0')",
                    ));
                }
                let position = field_config.driver_stations[driver_station as usize];
                let group = self.scene.graph.object_mut(self.ds_camera_group);
                group.position = Vector3::new(position[0], position[1], 0.0);
                let order = group.rotation().order;
                group.set_rotation(Euler::new(
                    0.0,
                    0.0,
                    math::atan2(-position[1], -position[0]),
                    order,
                ));
                self.scene.graph.object_mut(self.ds_camera_obj).position = Vector3::new(
                    if is_ftc { 0.0 } else { -DS_CAMERA_OFFSET_FRC },
                    0.0,
                    ds_camera_height(),
                );
                let camera_position = self.scene.graph.world_position(self.ds_camera_obj);
                let target = if is_ftc {
                    DS_CAMERA_TARGET_FTC
                } else {
                    ORBIT_FIELD_FRC_DEFAULT_TARGET
                };
                look(&mut self.controls, camera_position.to_array(), target);
            }
        }
        Ok(())
    }

    fn get_field_config(
        &self,
        command: &Field3dRendererCommand,
        env: &Field3dEnv,
    ) -> Option<Config3dField> {
        let assets = env.assets.as_ref()?;
        assets
            .field3ds
            .iter()
            .chain(built_in_3d_fields())
            .find(|f| f.id == command.field)
            .cloned()
    }

    fn make_object_manager(&mut self, kind: ObjType) -> ManagerEntry {
        let ctx = |root| ManagerContext {
            root,
            material_specular: self.material_specular,
            material_shininess: self.material_shininess,
            mode: self.mode,
            is_xr: false,
        };
        let field_ctx = ctx(self.wpilib_field_coordinate_group);
        let wpilib_ctx = ctx(self.wpilib_coordinate_group);
        let mut manager = match kind {
            ObjType::Robot | ObjType::Ghost => {
                let assets = self.window_assets.clone();
                let (starts, ends) = (
                    self.robot_loading_count.clone(),
                    self.robot_loading_count.clone(),
                );
                AnyManager::Robot(RobotManager::new(
                    field_ctx,
                    Box::new(move || assets.borrow().clone()),
                    Box::new(move || starts.set(starts.get() + 1)),
                    Box::new(move || ends.set(ends.get() - 1)),
                ))
            }
            ObjType::GamePiece => {
                AnyManager::GamePiece(GamePieceManager::new(field_ctx, self.field_pieces.clone()))
            }
            ObjType::Trajectory => {
                AnyManager::Trajectory(TrajectoryManager::new(field_ctx, &mut self.scene))
            }
            ObjType::Heatmap => {
                let field = self.heatmap_field.clone();
                let preference = self.coordinate_preference.clone();
                AnyManager::Heatmap(HeatmapManager::new(
                    wpilib_ctx,
                    Box::new(move || field.get()),
                    Box::new(move || preference.get()),
                ))
            }
            ObjType::AprilTag => AnyManager::AprilTag(AprilTagManager::new(field_ctx)),
            ObjType::AprilTagBuiltIn => AnyManager::AprilTag(AprilTagManager::new(wpilib_ctx)),
            ObjType::Axes => AnyManager::Axes(AxesManager::new(field_ctx)),
            ObjType::Cone => AnyManager::Cone(ConeManager::new(field_ctx)),
        };
        manager.set_resolution(self.resolution);
        self.next_manager_id += 1;
        ManagerEntry {
            id: self.next_manager_id,
            kind,
            manager,
            active: true,
        }
    }

    fn set_object_data(&mut self, index: usize, object: &AnyObj) -> Result<(), JsError> {
        let entry = &mut self.object_managers[index];
        let result = entry.manager.set_object_data(&mut self.scene, object);
        let id = entry.id;
        let (robot_request, texture_requests) = match &mut entry.manager {
            AnyManager::Robot(m) => (m.take_load_request(), Vec::new()),
            AnyManager::AprilTag(m) => (None, m.take_texture_requests()),
            _ => (None, Vec::new()),
        };
        if let Some(request) = robot_request {
            let ticket = next_ticket();
            self.load_requests.push(LoadRequest::Robot {
                ticket,
                request: request.clone(),
            });
            self.pending.insert(
                ticket,
                Pending::Robot {
                    manager: id,
                    request,
                },
            );
        }
        for request in texture_requests {
            let ticket = next_ticket();
            self.texture_requests.push(TextureLoad {
                ticket,
                url: request.url.clone(),
            });
            self.texture_targets.insert(ticket, (id, request));
        }
        result.map_err(manager_error)
    }

    fn new_field_ready(&mut self, pieces: GamePieces) {
        if let Some(field) = self.field {
            self.scene.graph.add(self.wpilib_coordinate_group, field);
            if let Some(staged) = self.field_staged_pieces {
                self.scene.graph.add(self.wpilib_coordinate_group, staged);
            }
        }
        for entry in &mut self.object_managers {
            if entry.kind == ObjType::GamePiece {
                entry.manager.dispose(&mut self.scene);
            }
        }
        self.object_managers
            .retain(|e| e.kind != ObjType::GamePiece);
        self.field_pieces = Arc::new(pieces);
        self.should_render = true;
    }

    fn rotate(&mut self, x: f64, y: f64) {
        let camera = &self.controls.camera().object;
        let mut reference = Object3D::new();
        reference.position = camera.position;
        reference.set_rotation(camera.rotation());
        reference.rotate_y(x);
        let polar = self.controls.polar_angle();
        if polar + y > self.controls.min_polar_angle + 0.2
            && polar + y < self.controls.max_polar_angle - 0.2
        {
            reference.rotate_x(y);
        }
        reference.translate_z(-self.controls.distance());
        let p = reference.position;
        self.controls.set_target(p.x, p.y, p.z, false);
        self.controls.update(0.0);
    }

    fn assets_json(assets: Option<&Arc<AdvantageScopeAssets>>) -> Option<String> {
        assets.and_then(|a| json::stringify(&a.to_js()).ok().flatten())
    }

    fn part_json(assets: Option<&Arc<AdvantageScopeAssets>>, robots: bool) -> Option<String> {
        let assets = assets?;
        let js = assets.to_js();
        let part = js.get(if robots { "robots" } else { "field3ds" })?;
        json::stringify(part).ok().flatten()
    }

    fn same_assets(
        a: Option<&Arc<AdvantageScopeAssets>>,
        b: Option<&Arc<AdvantageScopeAssets>>,
        part: Option<bool>,
    ) -> bool {
        if let (Some(a), Some(b)) = (a, b)
            && Arc::ptr_eq(a, b)
        {
            return true;
        }
        match part {
            None => Self::assets_json(a) == Self::assets_json(b),
            Some(robots) => Self::part_json(a, robots) == Self::part_json(b, robots),
        }
    }

    pub fn render(
        &mut self,
        command: &Field3dRendererCommand,
        env: &Field3dEnv,
        dom: &mut Field3dDom,
    ) -> Result<(), JsError> {
        *self.window_assets.borrow_mut() = env.assets.clone();
        self.coordinate_preference
            .set(env.preferences.as_ref().map(|p| p.coordinate_system));

        let delta = self.clock.get_delta(env.performance_now);
        let mut controls_updated = false;
        if self.keys_pressed.contains("KeyW") {
            if self.controls.distance() <= self.controls.min_distance {
                self.controls.dolly_in_fixed(5.0 * delta, false);
            } else {
                self.controls.dolly(5.0 * delta, false);
            }
            self.controls.update(0.0);
            controls_updated = true;
        }
        if self.keys_pressed.contains("KeyS") {
            self.controls.dolly(-5.0 * delta, false);
            self.controls.update(0.0);
            controls_updated = true;
        }
        if self.keys_pressed.contains("KeyA") {
            self.controls.truck(-5.0 * delta, 0.0, false);
            self.controls.update(0.0);
            controls_updated = true;
        }
        if self.keys_pressed.contains("KeyD") {
            self.controls.truck(5.0 * delta, 0.0, false);
            self.controls.update(0.0);
            controls_updated = true;
        }
        if self.keys_pressed.contains("KeyQ") {
            self.controls.elevate(-5.0 * delta, false);
            self.controls.update(0.0);
            controls_updated = true;
        }
        if self.keys_pressed.contains("KeyE") {
            self.controls.elevate(5.0 * delta, false);
            self.controls.update(0.0);
            controls_updated = true;
        }
        if self.keys_pressed.contains("KeyI") {
            self.rotate(0.0, 2.5 * delta);
            controls_updated = true;
        }
        if self.keys_pressed.contains("KeyK") {
            self.rotate(0.0, -2.5 * delta);
            controls_updated = true;
        }
        if self.keys_pressed.contains("KeyJ") {
            self.rotate(2.5 * delta, 0.0);
            controls_updated = true;
        }
        if self.keys_pressed.contains("KeyL") {
            self.rotate(-2.5 * delta, 0.0);
            controls_updated = true;
        }
        controls_updated = self.controls.update(delta) || controls_updated;

        let assets = env.assets.as_ref();
        let last_assets = self.last_assets.as_ref();
        let new_assets = !Self::same_assets(assets, last_assets, None);
        let new_field_assets = !Self::same_assets(assets, last_assets, Some(false));
        let new_robot_assets = !Self::same_assets(assets, last_assets, Some(true));

        if Some(env.client_width) != self.last_width
            || Some(env.client_height) != self.last_height
            || Some(env.device_pixel_ratio) != self.last_device_pixel_ratio
            || Some(env.is_dark) != self.last_is_dark
            || command.field != self.last_field_id
            || command.json != self.last_command_string
            || new_assets
            || controls_updated
        {
            self.last_width = Some(env.client_width);
            self.last_height = Some(env.client_height);
            self.last_device_pixel_ratio = Some(env.device_pixel_ratio);
            self.last_is_dark = Some(env.is_dark);
            self.last_command_string = command.json.clone();
            self.last_assets = env.assets.clone();
            self.should_render = true;
        }

        if env.rect_width == 0.0 {
            return Ok(());
        }

        let now = env.date_now;
        if self.mode == Field3dMode::LowPower
            && now - self.last_frame_time < 1000.0 / LOWER_POWER_MAX_FPS
        {
            return Ok(());
        }

        let is_axes = command.field.to_string_lossy() == "FRC:Axes"
            || command.field.to_string_lossy() == "FTC:Axes";
        if self.last_coordinate_system.as_ref() != Some(&command.coordinate_system)
            && self.last_coordinate_system.is_some()
            && is_axes
        {
            self.should_load_new_field = true;
            self.should_render = true;
        }
        self.last_coordinate_system = Some(command.coordinate_system.clone());

        if !self.should_render {
            return Ok(());
        }
        self.last_frame_time = now;
        self.should_render = false;

        let field_id = command.field.clone();
        self.field_config_cache = self.get_field_config(command, env);
        self.heatmap_field
            .set(self.field_config_cache.as_ref().map(HeatmapField::from));
        let Some(field_config) = self.field_config_cache.clone() else {
            return Ok(());
        };

        if self.should_reset_camera {
            self.reset_camera(command, field_config.is_ftc, true, env)?;
            self.should_reset_camera = false;
        }

        let field_group = self.wpilib_field_coordinate_group;
        let z = Vector3::new(0.0, 0.0, 1.0);
        let half_width = inches_to_meters(field_config.width_inches / 2.0);
        let half_height = inches_to_meters(field_config.height_inches / 2.0);
        let placement = match command.coordinate_system.to_string_lossy().as_str() {
            "wall-alliance" => {
                let sign = if command.is_red_alliance { -1.0 } else { 1.0 };
                Some((
                    if command.is_red_alliance {
                        0.0
                    } else {
                        math::PI
                    },
                    Vector3::new(half_width * sign, half_height * sign, 0.0),
                ))
            }
            "wall-blue" => Some((math::PI, Vector3::new(half_width, half_height, 0.0))),
            "center-rotated" => Some((-math::PI / 2.0, Vector3::new(0.0, 0.0, 0.0))),
            "center-red" => Some((0.0, Vector3::new(0.0, 0.0, 0.0))),
            _ => None,
        };
        if let Some((angle, position)) = placement {
            let object = self.scene.graph.object_mut(field_group);
            object.set_rotation_from_axis_angle(z, angle);
            object.position = position;
        }

        if field_id != self.last_field_id || new_field_assets {
            self.should_load_new_field = true;
            self.last_field_id = field_id.clone();

            if (Some(field_config.is_ftc) != self.last_is_ftc && self.last_is_ftc.is_some())
                || self.camera_index < -2.0
            {
                self.reset_camera(command, field_config.is_ftc, true, env)?;
            }

            if self.mode == Field3dMode::Cinematic && Some(field_config.is_ftc) != self.last_is_ftc
            {
                let positions = if field_config.is_ftc {
                    SPOT_LIGHT_POSITIONS_FTC
                } else {
                    SPOT_LIGHT_POSITIONS_FRC
                };
                for (index, [x, y, z, tx, ty, tz]) in positions.into_iter().enumerate() {
                    let light = self.spot_lights[index];
                    self.scene.graph.object_mut(light).position = Vector3::new(x, y, z);
                    if let NodeKind::SpotLight(SpotLight {
                        target, intensity, ..
                    }) = &mut self.scene.node_mut(light).kind
                    {
                        *intensity = if field_config.is_ftc {
                            SPOT_LIGHT_INTENSITY_FTC
                        } else {
                            SPOT_LIGHT_INTENSITY_FRC
                        };
                        let target = *target;
                        self.scene.graph.object_mut(target).position = Vector3::new(tx, ty, tz);
                    }
                }
            }

            if let Some(grid) = self.ftc_grid {
                self.scene.graph.remove(self.wpilib_coordinate_group, grid);
            }
            if field_config.is_ftc && field_config.use_grid {
                let color = Color::hex(f64::from(FTC_GRID_COLOR));
                let grid = self.scene.create(NodeKind::Line(Line::grid_helper(
                    inches_to_meters(field_config.width_inches),
                    6.0,
                    color,
                    color,
                )));
                self.scene.graph.object_mut(grid).rotate_x(math::PI / 2.0);
                self.scene.graph.add(self.wpilib_coordinate_group, grid);
                self.ftc_grid = Some(grid);
            }

            self.last_is_ftc = Some(field_config.is_ftc);
        }
        if self.should_load_new_field && !self.is_field_loading {
            self.should_load_new_field = false;
            if let Some(field) = self.field {
                self.scene.graph.remove(self.wpilib_coordinate_group, field);
            }
            if let Some(staged) = self.field_staged_pieces {
                self.scene
                    .graph
                    .remove(self.wpilib_coordinate_group, staged);
            }

            let id = field_id.to_string_lossy();
            if id == "FRC:Evergreen" || id == "FTC:Evergreen" {
                self.is_field_loading = false;
                self.field = Some(make_evergreen_field(
                    &mut self.scene,
                    self.material_specular,
                    self.material_shininess,
                    id == "FTC:Evergreen",
                ));
                self.field_staged_pieces = Some(self.scene.create(NodeKind::Object3D));
                self.new_field_ready(GamePieces::new());
            } else if id == "FRC:Axes" || id == "FTC:Axes" {
                self.is_field_loading = false;
                let system = [
                    CoordinateSystem::WallAlliance,
                    CoordinateSystem::WallBlue,
                    CoordinateSystem::CenterRotated,
                    CoordinateSystem::CenterRed,
                ]
                .into_iter()
                .find(|c| command.coordinate_system.to_string_lossy() == c.as_str())
                .ok_or_else(|| {
                    JsError::new(
                        "Unmodeled",
                        "makeAxesField with an unknown coordinate system",
                    )
                })?;
                self.field = Some(make_axes_field(
                    &mut self.scene,
                    self.material_specular,
                    self.material_shininess,
                    system,
                    id == "FTC:Axes",
                ));
                self.field_staged_pieces = Some(self.scene.create(NodeKind::Object3D));
                self.new_field_ready(GamePieces::new());
            } else {
                self.is_field_loading = true;
                let ticket = next_ticket();
                self.load_requests.push(LoadRequest::Field {
                    ticket,
                    config: field_config.clone(),
                    options: LoadOptions {
                        mode: self.mode,
                        material_specular: self.material_specular.to_array(),
                        material_shininess: self.material_shininess,
                    },
                });
                self.pending.insert(ticket, Pending::Field);
            }
        }

        let primary = self.primary_robot_group;
        self.scene.node_mut(primary).visible = false;
        let first_robot = command.objects.iter().find_map(|o| match o {
            AnyObj::Robot(r) if r.kind == RobotKind::Robot => Some(r),
            _ => None,
        });
        if let Some(robot) = first_robot {
            self.primary_robot_model = robot.model.clone();
            if let Some(pose) = robot.poses.first() {
                let object = self.scene.graph.object_mut(primary);
                let [x, y, z] = pose.pose.translation;
                object.position = Vector3::new(x, y, z);
                object.set_rotation_from_quaternion(&rotation3d_to_quaternion(&pose.pose.rotation));
                self.scene.node_mut(primary).visible = true;
            }
        }

        let override_obj = self.fixed_camera_override_obj;
        self.scene.node_mut(override_obj).visible =
            !matches!(command.camera_override, CameraOverride::Null);
        match &command.camera_override {
            CameraOverride::Null => {}
            CameraOverride::Undefined => {
                return Err(JsError::type_error(
                    "Cannot read properties of undefined (reading 'pose')",
                ));
            }
            CameraOverride::Pose(pose) => {
                let object = self.scene.graph.object_mut(override_obj);
                let [x, y, z] = pose.pose.translation;
                object.position = Vector3::new(x, y, z);
                object.set_rotation_from_quaternion(
                    &rotation3d_to_quaternion(&pose.pose.rotation).multiply(camera_rotation()),
                );
            }
        }

        if let Some(staged) = self.field_staged_pieces {
            self.scene.node_mut(staged).visible = command
                .objects
                .iter()
                .all(|o| o.obj_type() != ObjType::GamePiece);
        }

        for entry in &mut self.object_managers {
            entry.active = false;
        }
        for object in &command.objects {
            let kind = object.obj_type();
            let model = match object {
                AnyObj::Robot(r) => Some(&r.model),
                _ => None,
            };
            let mut found = self.object_managers.iter_mut().position(|e| {
                !e.active
                    && e.kind == kind
                    && match model {
                        None => true,
                        Some(model) => e.manager.robot().is_some_and(|m| m.get_model() == model),
                    }
            });
            if found.is_none() {
                found = self
                    .object_managers
                    .iter()
                    .position(|e| !e.active && e.kind == kind);
            }
            let index = match found {
                Some(index) => {
                    self.object_managers[index].active = true;
                    index
                }
                None => {
                    let entry = self.make_object_manager(kind);
                    self.object_managers.push(entry);
                    self.object_managers.len() - 1
                }
            };
            let entry = &mut self.object_managers[index];
            if new_robot_assets && let Some(robot) = entry.manager.robot() {
                robot.new_assets();
            }
            if let AnyManager::Heatmap(heatmap) = &mut entry.manager {
                heatmap.set_is_red_alliance(command.is_red_alliance);
            }
            self.set_object_data(index, object)?;
        }
        for index in 0..self.object_managers.len() {
            let entry = &mut self.object_managers[index];
            if entry.active || !matches!(entry.kind, ObjType::Robot | ObjType::Ghost) {
                continue;
            }
            let Some(model) = entry.manager.robot().map(|m| m.get_model().clone()) else {
                continue;
            };
            if command.all_robot_models.contains(&model) {
                entry.active = true;
                let kind = if entry.kind == ObjType::Ghost {
                    RobotKind::Ghost {
                        color: JsString::from("#000000"),
                    }
                } else {
                    RobotKind::Robot
                };
                let object = AnyObj::Robot(RobotObj {
                    kind,
                    model,
                    poses: Vec::new(),
                    components: Vec::new(),
                    mechanisms: RobotMechanisms { xz: None, yz: None },
                    vision_targets: Vec::new(),
                    swerve_module_velocities: Vec::new(),
                });
                self.set_object_data(index, &object)?;
            }
        }
        let mut kept = Vec::new();
        for mut entry in std::mem::take(&mut self.object_managers) {
            if entry.active {
                kept.push(entry);
                continue;
            }
            entry.manager.dispose(&mut self.scene);
            let waiting = self
                .pending
                .values()
                .any(|p| matches!(p, Pending::Robot { manager, .. } if *manager == entry.id));
            if waiting && let AnyManager::Robot(robot) = entry.manager {
                self.orphans.push((entry.id, robot));
            }
        }
        self.object_managers = kept;

        let robot_config = env.assets.as_ref().and_then(|a| {
            a.robots
                .iter()
                .find(|r| r.name == self.primary_robot_model)
                .cloned()
        });
        if let Some(robot) = &robot_config
            && self.camera_index >= robot.cameras.len() as f64
        {
            self.camera_index = ORBIT_FIELD;
        }

        let orbital_camera = self.camera_index == ORBIT_FIELD || self.camera_index == ORBIT_ROBOT;
        let ds_camera = self.camera_index < ORBIT_ROBOT;
        if orbital_camera != self.controls.enabled() {
            self.controls.set_enabled(orbital_camera);
        }

        let mut fov = self.orbit_fov;
        self.aspect_ratio = None;
        let wpilib = self.wpilib_coordinate_group;
        if orbital_camera || ds_camera {
            dom.canvas_fixed = false;
            dom.container_fixed = false;
            dom.canvas_width = String::new();
            dom.canvas_height = String::new();

            let mut refs = None;
            if self.camera_index != self.last_camera_index {
                let position = self.scene.create(NodeKind::Object3D);
                let target = self.scene.create(NodeKind::Object3D);
                self.scene.graph.add(wpilib, position);
                self.scene.graph.add(wpilib, target);
                let p = self
                    .scene
                    .graph
                    .world_to_local(wpilib, self.controls.get_position(true));
                self.scene.graph.object_mut(position).position = p;
                let t = self
                    .scene
                    .graph
                    .world_to_local(wpilib, self.controls.get_target(true));
                self.scene.graph.object_mut(target).position = t;
                refs = Some((position, target));
            }

            if self.camera_index == ORBIT_FIELD || ds_camera {
                let object = self.scene.graph.object_mut(wpilib);
                object.position = Vector3::new(0.0, 0.0, 0.0);
                object.set_rotation_from_quaternion(&wpilib_rotation());
            } else if self.scene.node(primary).visible {
                {
                    let object = self.scene.graph.object_mut(wpilib);
                    object.position = Vector3::new(0.0, 0.0, 0.0);
                    object.set_rotation_from_quaternion(&Quaternion::new(0.0, 0.0, 0.0, 1.0));
                }
                let position = self.scene.graph.world_position(primary);
                let rotation = self
                    .scene
                    .graph
                    .world_quaternion(primary)
                    .multiply(wpilib_rotation());
                let position = Vector3::new(-position.x, -position.y, -position.z);
                let rotation = rotation.invert();
                let object = self.scene.graph.object_mut(wpilib);
                object.position = position.apply_quaternion(&rotation);
                object.set_rotation_from_quaternion(&rotation);
            }

            if let Some((position, target)) = refs {
                let p = self.scene.graph.world_position(position);
                let t = self.scene.graph.world_position(target);
                self.controls
                    .set_look_at(p.x, p.y, p.z, t.x, t.y, t.z, false);
                self.controls.update(0.0);
            }

            if self.camera_index != self.last_camera_index
                || (self.camera_index == DS_AUTO
                    && self.last_auto_driver_station != command.auto_driver_station)
            {
                let animate = self.last_camera_index < 0.0;
                self.reset_camera(command, field_config.is_ftc, animate, env)?;
            }
        } else {
            dom.canvas_fixed = true;
            dom.container_fixed = true;

            let camera_config = robot_config
                .as_ref()
                .and_then(|r| camera_at(r, self.camera_index))
                .cloned();
            let aspect_ratio = camera_config
                .as_ref()
                .map_or(4.0 / 3.0, |c| c.resolution[0] / c.resolution[1]);
            if let Some(c) = &camera_config {
                fov = c.fov / aspect_ratio;
            }
            self.aspect_ratio = Some(aspect_ratio);
            let parent_aspect_ratio = env.parent_size.map_or(aspect_ratio, |[w, h]| w / h);
            let percent = |x: f64| format!("{}%", number::to_string(x * 100.0));
            if aspect_ratio > parent_aspect_ratio {
                dom.canvas_width = "100%".to_owned();
                dom.canvas_height = percent(parent_aspect_ratio / aspect_ratio);
            } else {
                dom.canvas_width = percent(aspect_ratio / parent_aspect_ratio);
                dom.canvas_height = "100%".to_owned();
            }

            let mut reference = None;
            if self.scene.node(override_obj).visible {
                reference = Some(override_obj);
            } else if self.scene.node(primary).visible
                && let Some(c) = &camera_config
            {
                let object = self.scene.graph.object_mut(self.fixed_camera_obj);
                object.position = vec3(c.position);
                object.set_rotation_from_quaternion(
                    &config_sequence(&c.rotations).multiply(camera_rotation()),
                );
                reference = Some(self.fixed_camera_obj);
            }
            if let Some(reference) = reference {
                let position = self.scene.graph.world_position(reference);
                let quaternion = self.scene.graph.world_quaternion(reference);
                let camera = &mut self.controls.camera_mut().object;
                camera.position = position;
                camera.set_quaternion(quaternion);
            }
        }

        if self.camera_index == ORBIT_ROBOT {
            dom.alert_hidden = self.scene.node(primary).visible;
            dom.alert_html = "Robot pose not available</br>for camera \"Orbit Robot\".".to_owned();
        } else if self.camera_index == DS_AUTO {
            dom.alert_hidden = command.auto_driver_station >= 0.0;
            dom.alert_html = "Driver Station position</br>not available.".to_owned();
        } else if self.camera_index == ORBIT_FIELD || ds_camera {
            dom.alert_hidden = true;
        } else {
            dom.alert_hidden =
                self.scene.node(primary).visible || self.scene.node(override_obj).visible;
            let name = match &robot_config {
                None => "???".to_owned(),
                Some(robot) => camera_at(robot, self.camera_index)
                    .ok_or_else(|| {
                        JsError::type_error("Cannot read properties of undefined (reading 'name')")
                    })?
                    .name
                    .to_string_lossy(),
            };
            dom.alert_html = format!("Robot pose not available</br>for camera \"{name}\".");
        }

        if fov != self.controls.camera().fov {
            let camera = self.controls.camera_mut();
            camera.fov = fov;
            camera.update_projection_matrix();
        }

        self.last_camera_index = self.camera_index;
        self.last_auto_driver_station = command.auto_driver_station;

        if self.robot_loading_count.get() > 0 || self.is_field_loading {
            dom.spinner_visible = true;
            dom.spinner_animating = true;
        } else if dom.spinner_visible {
            dom.spinner_visible = false;
            dom.spinner_timeouts.push(env.performance_now + 250.0);
        }

        let device_pixel_ratio = env.device_pixel_ratio
            * if self.mode == Field3dMode::LowPower {
                0.75
            } else {
                1.0
            };
        let (client_width, client_height) = (env.client_width, env.client_height);
        if f64::from(self.canvas_size[0]) / device_pixel_ratio != client_width
            || f64::from(self.canvas_size[1]) / device_pixel_ratio != client_height
        {
            self.set_size(client_width, client_height);
            let camera = self.controls.camera_mut();
            camera.aspect = client_width / client_height;
            camera.update_projection_matrix();
            self.resolution = [client_width, client_height];
            for entry in &mut self.object_managers {
                entry.manager.set_resolution(self.resolution);
            }
        }
        let background = Color::hex(f64::from(if env.is_dark { 0x222222 } else { 0xffffff }));
        self.scene.background = Some(background);
        self.pixel_ratio = device_pixel_ratio;
        self.set_size(self.gl_size[0], self.gl_size[1]);

        let root = self.scene.root;
        self.scene.graph.update_matrix_world(root, false);
        self.controls.camera_mut().update_matrix_world(false);
        self.renders.push(RenderCall {
            width: self.canvas_size[0],
            height: self.canvas_size[1],
            pixel_ratio: self.pixel_ratio,
            background,
        });
        Ok(())
    }

    /// `WebGLRenderer.setSize(width, height, false)`: the canvas's `unsigned long` width and
    /// height attributes.
    fn set_size(&mut self, width: f64, height: f64) {
        self.gl_size = [width, height];
        self.canvas_size = [
            number::to_int32(math::floor(width * self.pixel_ratio)) as u32,
            number::to_int32(math::floor(height * self.pixel_ratio)) as u32,
        ];
    }
}
