use std::cmp::Ordering;
use std::sync::Arc;

use asrs_js::math;

use super::scene::{
    Geometry, LineBasicMaterial, LineMaterial, LineSegmentsGeometry, Materials, Mesh, NodeKind,
    PhongMaterial, Scene, Side, SpotLight, matrix_to_f32, sphere_apply_matrix4,
};
use crate::three::math_utils::RAD2DEG;
use crate::three::{Color, Group, Matrix3, Matrix4, ObjectId, PerspectiveCamera, Sphere, Vector3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameOptions {
    /// Drawing-buffer size in device pixels: `floor(clientSize * pixelRatio)`.
    pub width: u32,
    pub height: u32,
    /// 4 with `antialias: true`, otherwise 1.
    pub samples: u32,
    /// `renderer.getViewport()` size: the `setSize` size in CSS pixels, which `LineSegments2`
    /// copies into its material's `resolution` before every draw.
    pub viewport: [f64; 2],
    /// `renderer.shadowMap.enabled` (cinematic mode), always with `PCFSoftShadowMap`.
    pub shadow_map: bool,
}

impl FrameOptions {
    /// Options for `setSize(width, height)` at `setPixelRatio(pixel_ratio)`.
    pub fn new(width: f64, height: f64, pixel_ratio: f64, antialias: bool) -> Self {
        FrameOptions {
            width: math::floor(width * pixel_ratio) as u32,
            height: math::floor(height * pixel_ratio) as u32,
            samples: if antialias { 4 } else { 1 },
            viewport: [width, height],
            shadow_map: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HemisphereUniform {
    pub direction: [f32; 3],
    pub sky_color: [f32; 3],
    pub ground_color: [f32; 3],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PointUniform {
    pub position: [f32; 3],
    pub color: [f32; 3],
    pub distance: f32,
    pub decay: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpotUniform {
    pub position: [f32; 3],
    pub direction: [f32; 3],
    pub color: [f32; 3],
    pub distance: f32,
    pub decay: f32,
    pub cone_cos: f32,
    pub penumbra_cos: f32,
}

/// `spotLightShadows[i]` plus `spotLightMatrix[i]` (`LightShadow.matrix`, world to shadow-map
/// texture space).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpotShadowUniform {
    pub matrix: [f32; 16],
    pub intensity: f32,
    pub bias: f32,
    pub normal_bias: f32,
    pub radius: f32,
    pub map_size: [f32; 2],
}

/// The light uniforms `WebGLLights` produces, in view space. `spot_shadow[i]` belongs to
/// `spot[i]`: shadow-casting lights sort first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lights {
    pub hemisphere: Vec<HemisphereUniform>,
    pub point: Vec<PointUniform>,
    pub spot: Vec<SpotUniform>,
    pub spot_shadow: Vec<SpotShadowUniform>,
}

/// What one draw renders, with the material it renders with.
#[derive(Clone, Debug)]
pub enum DrawKind {
    /// `group` is the geometry group a material-array mesh draws, which limits the vertex range.
    Mesh {
        geometry: Arc<Geometry>,
        material: Arc<PhongMaterial>,
        instances: Option<Arc<Vec<[f32; 16]>>>,
        group: Option<Group>,
    },
    FatLine {
        geometry: Arc<LineSegmentsGeometry>,
        material: Arc<LineMaterial>,
        resolution: [f32; 2],
    },
    Line {
        geometry: Arc<Geometry>,
        material: Arc<LineBasicMaterial>,
        segments: bool,
    },
}

impl DrawKind {
    pub fn transparent(&self) -> bool {
        match self {
            DrawKind::Mesh { material, .. } => material.transparent,
            DrawKind::FatLine { material, .. } => material.transparent,
            DrawKind::Line { material, .. } => material.transparent,
        }
    }

    pub fn depth_test(&self) -> bool {
        match self {
            DrawKind::Mesh { material, .. } => material.depth_test,
            DrawKind::FatLine { material, .. } => material.depth_test,
            DrawKind::Line { material, .. } => material.depth_test,
        }
    }

    pub fn depth_write(&self) -> bool {
        match self {
            DrawKind::Mesh { material, .. } => material.depth_write,
            DrawKind::FatLine { material, .. } => material.depth_write,
            DrawKind::Line { material, .. } => material.depth_write,
        }
    }
}

/// One `renderBufferDirect` call. A transparent double-sided material produces two, the
/// `BackSide` pass first.
#[derive(Clone, Debug)]
pub struct Draw {
    pub object: ObjectId,
    pub kind: DrawKind,
    pub side: Side,
    pub front_face_cw: bool,
    pub model: [f32; 16],
    pub model_view: [f32; 16],
    pub normal_matrix: [f32; 9],
    pub receive_shadow: bool,
}

/// One light's pass of `WebGLShadowMap.render`: `MeshDepthMaterial` with RGBA depth packing
/// into a cleared RGBA8 target, objects in scene order, shadow side per material.
#[derive(Clone, Debug)]
pub struct ShadowPass {
    pub map_size: [u32; 2],
    pub projection: [f32; 16],
    pub draws: Vec<Draw>,
}

/// Everything the render thread needs for one frame. It owns its data, so it can be sent across
/// threads while the scene keeps changing. `shadows[i]` is the map of `lights.spot_shadow[i]`.
#[derive(Clone, Debug)]
pub struct Frame {
    pub options: FrameOptions,
    pub clear: [f64; 4],
    pub projection: [f32; 16],
    pub view: [f32; 16],
    pub lights: Lights,
    pub draws: Vec<Draw>,
    pub shadows: Vec<ShadowPass>,
}

#[derive(Clone, Debug)]
struct RenderItem {
    object: ObjectId,
    group_order: f64,
    render_order: f64,
    material_id: u64,
    z: f64,
    group: Option<Group>,
}

fn compare(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

/// `painterSortStable`.
fn painter_sort(a: &RenderItem, b: &RenderItem) -> Ordering {
    if a.group_order != b.group_order {
        compare(a.group_order, b.group_order)
    } else if a.render_order != b.render_order {
        compare(a.render_order, b.render_order)
    } else if a.material_id != b.material_id {
        a.material_id.cmp(&b.material_id)
    } else if a.z != b.z {
        compare(a.z, b.z)
    } else {
        a.object.cmp(&b.object)
    }
}

/// `reversePainterSortStable`.
fn reverse_painter_sort(a: &RenderItem, b: &RenderItem) -> Ordering {
    if a.group_order != b.group_order {
        compare(a.group_order, b.group_order)
    } else if a.render_order != b.render_order {
        compare(a.render_order, b.render_order)
    } else if a.z != b.z {
        compare(b.z, a.z)
    } else {
        a.object.cmp(&b.object)
    }
}

/// `_vector4.copy(center).applyMatrix4(matrixWorld).applyMatrix4(projScreenMatrix).z`: a
/// `Vector4` with `w = 1`, so the sort key is clip-space z without a perspective divide.
fn sort_z(center: Vector3, world: &Matrix4, proj_screen: &Matrix4) -> f64 {
    let apply = |v: [f64; 4], m: &Matrix4| {
        let e = &m.elements;
        let [x, y, z, w] = v;
        [
            e[0] * x + e[4] * y + e[8] * z + e[12] * w,
            e[1] * x + e[5] * y + e[9] * z + e[13] * w,
            e[2] * x + e[6] * y + e[10] * z + e[14] * w,
            e[3] * x + e[7] * y + e[11] * z + e[15] * w,
        ]
    };
    let v = apply([center.x, center.y, center.z, 1.0], world);
    apply(v, proj_screen)[2]
}

/// `Frustum.setFromProjectionMatrix` (WebGL clip space) and `intersectsSphere`.
struct Frustum {
    planes: [(Vector3, f64); 6],
}

impl Frustum {
    fn from_projection(m: &Matrix4) -> Self {
        let e = &m.elements;
        let plane = |x: f64, y: f64, z: f64, w: f64| {
            let normal = Vector3::new(x, y, z);
            let inverse_normal_length = 1.0 / normal.length();
            (
                normal.multiply_scalar(inverse_normal_length),
                w * inverse_normal_length,
            )
        };
        Frustum {
            planes: [
                plane(e[3] - e[0], e[7] - e[4], e[11] - e[8], e[15] - e[12]),
                plane(e[3] + e[0], e[7] + e[4], e[11] + e[8], e[15] + e[12]),
                plane(e[3] + e[1], e[7] + e[5], e[11] + e[9], e[15] + e[13]),
                plane(e[3] - e[1], e[7] - e[5], e[11] - e[9], e[15] - e[13]),
                plane(e[3] - e[2], e[7] - e[6], e[11] - e[10], e[15] - e[14]),
                plane(e[3] + e[2], e[7] + e[6], e[11] + e[10], e[15] + e[14]),
            ],
        }
    }

    fn intersects_sphere(&self, sphere: Sphere) -> bool {
        let neg_radius = -sphere.radius;
        !self
            .planes
            .iter()
            .any(|(normal, constant)| normal.dot(sphere.center) + constant < neg_radius)
    }
}

fn color_times(color: Color, intensity: f64) -> [f32; 3] {
    [
        (color.r * intensity) as f32,
        (color.g * intensity) as f32,
        (color.b * intensity) as f32,
    ]
}

fn vec3_f32(v: Vector3) -> [f32; 3] {
    [v.x as f32, v.y as f32, v.z as f32]
}

struct Collected {
    opaque: Vec<RenderItem>,
    transparent: Vec<RenderItem>,
    lights: Vec<ObjectId>,
}

/// One render-list entry `projectObject` pushes: material id, transparency and geometry group.
type ItemMaterial = (u64, bool, Option<Group>);

/// The bounding sphere `projectObject` culls and sorts by (an `InstancedMesh`'s own, computed
/// once, otherwise its geometry's) and the entries it pushes: one per visible material.
fn object_items(kind: &mut NodeKind) -> Option<(Sphere, Vec<ItemMaterial>)> {
    let visible = |id: u64, visible: bool, transparent: bool, group: Option<Group>| {
        visible.then_some((id, transparent, group))
    };
    match kind {
        NodeKind::Mesh(mesh) => {
            let sphere = match &mut mesh.instances {
                Some(instances) => {
                    if instances.bounding_sphere().is_none() {
                        instances.compute_bounding_sphere(&mesh.geometry);
                    }
                    instances.bounding_sphere().unwrap_or_default()
                }
                None => mesh.geometry.bounding_sphere(),
            };
            let items = mesh
                .material_groups()
                .into_iter()
                .filter_map(|(m, group)| visible(m.id, m.visible, m.transparent, group))
                .collect();
            Some((sphere, items))
        }
        NodeKind::FatLine(line) => Some((
            line.geometry.bounding_sphere(),
            visible(
                line.material.id,
                line.material.visible,
                line.material.transparent,
                None,
            )
            .into_iter()
            .collect(),
        )),
        NodeKind::Line(line) => Some((
            line.geometry.bounding_sphere(),
            visible(
                line.material.id,
                line.material.visible,
                line.material.transparent,
                None,
            )
            .into_iter()
            .collect(),
        )),
        _ => None,
    }
}

/// `projectObject`: depth first in child order, skipping invisible subtrees, with a group's
/// `renderOrder` passed down as `groupOrder`.
fn project(
    scene: &mut Scene,
    id: ObjectId,
    group_order: f64,
    proj_screen: &Matrix4,
    frustum: &Frustum,
    out: &mut Collected,
) {
    if !scene.node(id).visible {
        return;
    }
    let mut group_order = group_order;
    let world = scene.graph.object(id).matrix_world;
    let node = &mut scene.graph.node_mut(id).data;
    let (render_order, frustum_culled) = (node.render_order, node.frustum_culled);
    match &mut node.kind {
        NodeKind::Object3D => {}
        NodeKind::Group => {
            if id != scene.root {
                group_order = render_order;
            }
        }
        NodeKind::HemisphereLight(_) | NodeKind::PointLight(_) | NodeKind::SpotLight(_) => {
            out.lights.push(id);
        }
        kind => {
            if let Some((sphere, items)) = object_items(kind) {
                let in_view = !frustum_culled
                    || frustum.intersects_sphere(sphere_apply_matrix4(sphere, &world));
                if in_view {
                    let z = sort_z(sphere.center, &world, proj_screen);
                    for (material_id, transparent, group) in items {
                        let item = RenderItem {
                            object: id,
                            group_order,
                            render_order,
                            material_id,
                            z,
                            group,
                        };
                        if transparent {
                            out.transparent.push(item);
                        } else {
                            out.opaque.push(item);
                        }
                    }
                }
            }
        }
    }
    for child in scene.graph.node(id).children().to_vec() {
        project(scene, child, group_order, proj_screen, frustum, out);
    }
}

fn cast_shadow(kind: &NodeKind) -> bool {
    match kind {
        NodeKind::PointLight(l) => l.cast_shadow,
        NodeKind::SpotLight(l) => l.cast_shadow,
        _ => false,
    }
}

/// `SpotLightShadow.updateMatrices`: the shadow camera at the light looking at its target, and
/// `LightShadow.matrix`, which maps world space to shadow-map texture space.
fn spot_shadow_camera(
    scene: &Scene,
    id: ObjectId,
    light: &SpotLight,
) -> (PerspectiveCamera, Matrix4) {
    let mut camera = PerspectiveCamera::new(50.0, 1.0, 0.5, 500.0);
    let fov = RAD2DEG * 2.0 * light.angle * light.shadow.focus;
    let aspect = light.shadow.map_size[0] / light.shadow.map_size[1];
    let far = if light.distance != 0.0 && !light.distance.is_nan() {
        light.distance
    } else {
        camera.far
    };
    if fov != camera.fov || aspect != camera.aspect || far != camera.far {
        camera.fov = fov;
        camera.aspect = aspect;
        camera.far = far;
        camera.update_projection_matrix();
    }
    camera.object.position = Vector3::from_matrix_position(&scene.graph.object(id).matrix_world);
    camera.look_at(Vector3::from_matrix_position(
        &scene.graph.object(light.target).matrix_world,
    ));
    camera.update_matrix_world(false);
    let proj_screen =
        Matrix4::multiply_matrices(&camera.projection_matrix, &camera.matrix_world_inverse);
    let bias = Matrix4::from_rows([
        0.5, 0.0, 0.0, 0.5, //
        0.0, 0.5, 0.0, 0.5, //
        0.0, 0.0, 0.5, 0.5, //
        0.0, 0.0, 0.0, 1.0,
    ]);
    (camera, Matrix4::multiply_matrices(&bias, &proj_screen))
}

/// `WebGLLights.setup` and `setupView`, after `shadowCastingAndTexturingLightsFirst`, and with
/// the shadow map enabled, `WebGLShadowMap.render`'s shadow cameras for the shadow-casting spot
/// lights in the same order.
fn light_uniforms(
    scene: &Scene,
    mut lights: Vec<ObjectId>,
    view: &Matrix4,
    shadow_map: bool,
) -> (Lights, Vec<(PerspectiveCamera, [u32; 2])>) {
    lights.sort_by_key(|&id| !cast_shadow(&scene.node(id).kind));
    let mut out = Lights::default();
    let mut cameras = Vec::new();
    for id in lights {
        let world = &scene.graph.object(id).matrix_world;
        let position = Vector3::from_matrix_position(world);
        match &scene.node(id).kind {
            NodeKind::HemisphereLight(light) => out.hemisphere.push(HemisphereUniform {
                direction: vec3_f32(position.transform_direction(view)),
                sky_color: color_times(light.color, light.intensity),
                ground_color: color_times(light.ground_color, light.intensity),
            }),
            NodeKind::PointLight(light) => out.point.push(PointUniform {
                position: vec3_f32(position.apply_matrix4(view)),
                color: color_times(light.color, light.intensity),
                distance: light.distance as f32,
                decay: light.decay as f32,
            }),
            NodeKind::SpotLight(light) => {
                let target =
                    Vector3::from_matrix_position(&scene.graph.object(light.target).matrix_world);
                out.spot.push(SpotUniform {
                    position: vec3_f32(position.apply_matrix4(view)),
                    direction: vec3_f32((position - target).transform_direction(view)),
                    color: color_times(light.color, light.intensity),
                    distance: light.distance as f32,
                    decay: light.decay as f32,
                    cone_cos: math::cos(light.angle) as f32,
                    penumbra_cos: math::cos(light.angle * (1.0 - light.penumbra)) as f32,
                });
                if shadow_map && light.cast_shadow {
                    let (camera, matrix) = spot_shadow_camera(scene, id, light);
                    let shadow = &light.shadow;
                    out.spot_shadow.push(SpotShadowUniform {
                        matrix: matrix_to_f32(&matrix),
                        intensity: shadow.intensity as f32,
                        bias: shadow.bias as f32,
                        normal_bias: shadow.normal_bias as f32,
                        radius: shadow.radius as f32,
                        map_size: shadow.map_size.map(|v| v as f32),
                    });
                    cameras.push((camera, shadow.map_size.map(|v| v as u32)));
                }
            }
            _ => {}
        }
    }
    (out, cameras)
}

struct MeshPart<'a> {
    mesh: &'a Mesh,
    material: &'a Arc<PhongMaterial>,
    group: Option<Group>,
}

fn mesh_draw(
    id: ObjectId,
    part: &MeshPart<'_>,
    side: Side,
    world: &Matrix4,
    view: &Matrix4,
    receive_shadow: bool,
) -> Draw {
    let model_view = Matrix4::multiply_matrices(view, world);
    Draw {
        object: id,
        kind: DrawKind::Mesh {
            geometry: part.mesh.geometry.clone(),
            material: part.material.clone(),
            instances: part.mesh.instances.as_ref().map(|i| i.matrices.clone()),
            group: part.group,
        },
        side,
        front_face_cw: world.determinant() < 0.0,
        model: matrix_to_f32(world),
        model_view: matrix_to_f32(&model_view),
        normal_matrix: Matrix3::normal_matrix(&model_view)
            .elements
            .map(|v| v as f32),
        receive_shadow,
    }
}

/// `WebGLShadowMap`'s `renderObject` walk for one light: visible shadow casters in scene order,
/// each with `MeshDepthMaterial` on the side `shadowSide` maps the material's side to.
fn shadow_draws(scene: &Scene, id: ObjectId, view: &Matrix4, out: &mut Vec<Draw>) {
    let node = scene.node(id);
    if !node.visible {
        return;
    }
    if let NodeKind::Mesh(mesh) = &node.kind
        && mesh.cast_shadow
    {
        let world = scene.graph.object(id).matrix_world;
        for (material, group) in mesh.material_groups() {
            if !material.visible {
                continue;
            }
            let side = match material.side {
                Side::Front => Side::Back,
                Side::Back => Side::Front,
                Side::Double => Side::Double,
            };
            let part = MeshPart {
                mesh,
                material,
                group,
            };
            out.push(mesh_draw(id, &part, side, &world, view, false));
        }
    }
    for &child in scene.graph.node(id).children() {
        shadow_draws(scene, child, view, out);
    }
}

/// What `WebGLRenderer.render(scene, camera)` does on the CPU before issuing draws: world
/// matrices, the render lists and their sorts, light uniforms, shadow passes, and per-object
/// model-view and normal matrices.
pub fn build_frame(
    scene: &mut Scene,
    camera: &mut PerspectiveCamera,
    options: FrameOptions,
) -> Frame {
    let root = scene.root;
    scene.graph.update_matrix_world(root, false);
    camera.update_matrix_world(false);
    let view = camera.matrix_world_inverse;
    let proj_screen = Matrix4::multiply_matrices(&camera.projection_matrix, &view);
    let frustum = Frustum::from_projection(&proj_screen);

    let mut collected = Collected {
        opaque: Vec::new(),
        transparent: Vec::new(),
        lights: Vec::new(),
    };
    project(scene, root, 0.0, &proj_screen, &frustum, &mut collected);
    collected.opaque.sort_by(painter_sort);
    collected.transparent.sort_by(reverse_painter_sort);

    let (lights, shadow_cameras) =
        light_uniforms(scene, collected.lights, &view, options.shadow_map);
    let shadows = shadow_cameras
        .into_iter()
        .map(|(camera, map_size)| {
            let mut draws = Vec::new();
            shadow_draws(scene, root, &camera.matrix_world_inverse, &mut draws);
            ShadowPass {
                map_size,
                projection: matrix_to_f32(&camera.projection_matrix),
                draws,
            }
        })
        .collect();

    let resolution = options.viewport.map(|v| v as f32);
    let mut draws = Vec::new();
    for item in collected.opaque.iter().chain(&collected.transparent) {
        let world = scene.graph.object(item.object).matrix_world;
        match &scene.node(item.object).kind {
            NodeKind::Mesh(mesh) => {
                let material = match (&mesh.material, item.group) {
                    (Materials::Single(material), _) => material,
                    (Materials::Array(materials), Some(group)) => &materials[group.material_index],
                    (Materials::Array(_), None) => continue,
                };
                let part = MeshPart {
                    mesh,
                    material,
                    group: item.group,
                };
                let sides: &[Side] = if material.transparent && material.side == Side::Double {
                    &[Side::Back, Side::Front]
                } else {
                    std::slice::from_ref(&material.side)
                };
                for &side in sides {
                    draws.push(mesh_draw(
                        item.object,
                        &part,
                        side,
                        &world,
                        &view,
                        mesh.receive_shadow,
                    ));
                }
            }
            NodeKind::FatLine(line) => draws.push(line_draw(
                item.object,
                DrawKind::FatLine {
                    geometry: line.geometry.clone(),
                    material: line.material.clone(),
                    resolution,
                },
                &world,
                &view,
            )),
            NodeKind::Line(line) => draws.push(line_draw(
                item.object,
                DrawKind::Line {
                    geometry: line.geometry.clone(),
                    material: line.material.clone(),
                    segments: line.segments,
                },
                &world,
                &view,
            )),
            _ => {}
        }
    }

    let clear = match scene.background {
        Some(color) => {
            let [r, g, b] = color.get_srgb();
            [r, g, b, 1.0]
        }
        None => [0.0, 0.0, 0.0, 0.0],
    };
    Frame {
        options,
        clear,
        projection: matrix_to_f32(&camera.projection_matrix),
        view: matrix_to_f32(&view),
        lights,
        draws,
        shadows,
    }
}

fn line_draw(id: ObjectId, kind: DrawKind, world: &Matrix4, view: &Matrix4) -> Draw {
    let model_view = Matrix4::multiply_matrices(view, world);
    Draw {
        object: id,
        kind,
        side: Side::Front,
        front_face_cw: world.determinant() < 0.0,
        model: matrix_to_f32(world),
        model_view: matrix_to_f32(&model_view),
        normal_matrix: [0.0; 9],
        receive_shadow: false,
    }
}
