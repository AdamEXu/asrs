use std::cmp::Ordering;
use std::sync::Arc;

use asrs_js::math;

use super::scene::{Geometry, NodeKind, PhongMaterial, Scene, Side, matrix_to_f32};
use crate::three::{Color, Matrix3, Matrix4, ObjectId, PerspectiveCamera, Vector3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameOptions {
    /// Drawing-buffer size in device pixels: `floor(clientSize * pixelRatio)`.
    pub width: u32,
    pub height: u32,
    /// 4 with `antialias: true`, otherwise 1.
    pub samples: u32,
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

/// The light uniforms `WebGLLights` produces, in view space.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lights {
    pub hemisphere: Vec<HemisphereUniform>,
    pub point: Vec<PointUniform>,
    pub spot: Vec<SpotUniform>,
}

/// One `renderBufferDirect` call. A transparent double-sided material produces two, the
/// `BackSide` pass first.
#[derive(Clone, Debug)]
pub struct Draw {
    pub object: ObjectId,
    pub geometry: Arc<Geometry>,
    pub material: Arc<PhongMaterial>,
    pub side: Side,
    pub front_face_cw: bool,
    pub model_view: [f32; 16],
    pub normal_matrix: [f32; 9],
    pub instances: Option<Arc<Vec<[f32; 16]>>>,
}

impl Draw {
    pub fn transparent(&self) -> bool {
        self.material.transparent
    }
}

/// Everything the render thread needs for one frame. It owns its data, so it can be sent across
/// threads while the scene keeps changing.
#[derive(Clone, Debug)]
pub struct Frame {
    pub options: FrameOptions,
    pub clear: [f64; 4],
    pub projection: [f32; 16],
    pub lights: Lights,
    pub draws: Vec<Draw>,
}

#[derive(Clone, Debug)]
struct RenderItem {
    object: ObjectId,
    group_order: f64,
    render_order: f64,
    material_id: u64,
    z: f64,
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

/// `projectObject`: depth first in child order, skipping invisible subtrees, with a group's
/// `renderOrder` passed down as `groupOrder`.
fn project(
    scene: &mut Scene,
    id: ObjectId,
    group_order: f64,
    proj_screen: &Matrix4,
    out: &mut Collected,
) {
    if !scene.node(id).visible {
        return;
    }
    let mut group_order = group_order;
    let world = scene.graph.object(id).matrix_world;
    let render_order = scene.node(id).render_order;
    match &mut scene.graph.node_mut(id).data.kind {
        NodeKind::Group => {
            if id != scene.root {
                group_order = render_order;
            }
        }
        NodeKind::HemisphereLight(_) | NodeKind::PointLight(_) | NodeKind::SpotLight(_) => {
            out.lights.push(id);
        }
        NodeKind::Mesh(mesh) => {
            let center = match &mut mesh.instances {
                Some(instances) => {
                    if instances.bounding_sphere().is_none() {
                        instances.compute_bounding_sphere(&mesh.geometry);
                    }
                    instances.bounding_sphere().unwrap_or_default().center
                }
                None => mesh.geometry.bounding_sphere().center,
            };
            if mesh.material.visible {
                let item = RenderItem {
                    object: id,
                    group_order,
                    render_order,
                    material_id: mesh.material.id,
                    z: sort_z(center, &world, proj_screen),
                };
                if mesh.material.transparent {
                    out.transparent.push(item);
                } else {
                    out.opaque.push(item);
                }
            }
        }
    }
    for child in scene.graph.node(id).children().to_vec() {
        project(scene, child, group_order, proj_screen, out);
    }
}

fn cast_shadow(kind: &NodeKind) -> bool {
    match kind {
        NodeKind::PointLight(l) => l.cast_shadow,
        NodeKind::SpotLight(l) => l.cast_shadow,
        _ => false,
    }
}

/// `WebGLLights.setup` and `setupView`, after `shadowCastingAndTexturingLightsFirst`.
fn light_uniforms(scene: &Scene, mut lights: Vec<ObjectId>, view: &Matrix4) -> Lights {
    lights.sort_by_key(|&id| !cast_shadow(&scene.node(id).kind));
    let mut out = Lights::default();
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
                })
            }
            _ => {}
        }
    }
    out
}

/// What `WebGLRenderer.render(scene, camera)` does on the CPU before issuing draws: world
/// matrices, the render lists and their sorts, light uniforms, and per-object model-view and
/// normal matrices.
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

    let mut collected = Collected {
        opaque: Vec::new(),
        transparent: Vec::new(),
        lights: Vec::new(),
    };
    project(scene, root, 0.0, &proj_screen, &mut collected);
    collected.opaque.sort_by(painter_sort);
    collected.transparent.sort_by(reverse_painter_sort);

    let lights = light_uniforms(scene, collected.lights, &view);

    let mut draws = Vec::new();
    for item in collected.opaque.iter().chain(&collected.transparent) {
        let world = scene.graph.object(item.object).matrix_world;
        let NodeKind::Mesh(mesh) = &scene.node(item.object).kind else {
            continue;
        };
        let model_view = Matrix4::multiply_matrices(&view, &world);
        let normal_matrix = Matrix3::normal_matrix(&model_view);
        let front_face_cw = world.determinant() < 0.0;
        let sides: &[Side] = if mesh.material.transparent && mesh.material.side == Side::Double {
            &[Side::Back, Side::Front]
        } else {
            std::slice::from_ref(&mesh.material.side)
        };
        for &side in sides {
            draws.push(Draw {
                object: item.object,
                geometry: mesh.geometry.clone(),
                material: mesh.material.clone(),
                side,
                front_face_cw,
                model_view: matrix_to_f32(&model_view),
                normal_matrix: normal_matrix.elements.map(|v| v as f32),
                instances: mesh.instances.as_ref().map(|i| i.matrices.clone()),
            });
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
        lights,
        draws,
    }
}
