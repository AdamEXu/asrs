use std::sync::Arc;

use asrs_field3d::render::{
    Frame, FrameOptions, Geometry, Mesh, NodeKind, PhongMaterial, Scene, Side, build_frame,
};
use asrs_field3d::three::{Color, ObjectId, PerspectiveCamera, Vector3, primitives};
use asrs_js::math;

const OPTIONS: FrameOptions = FrameOptions {
    width: 200,
    height: 100,
    samples: 4,
    viewport: [200.0, 100.0],
    shadow_map: false,
};

fn camera() -> PerspectiveCamera {
    let mut camera = PerspectiveCamera::new(50.0, 2.0, 0.15, 1000.0);
    camera.object.position = Vector3::new(0.0, 0.0, 10.0);
    camera.look_at(Vector3::new(0.0, 0.0, 0.0));
    camera
}

fn cube() -> Arc<Geometry> {
    Geometry::new(primitives::box_geometry(1.0, 1.0, 1.0, 1.0, 1.0, 1.0))
}

fn add(scene: &mut Scene, material: &Arc<PhongMaterial>, z: f64) -> ObjectId {
    let root = scene.root;
    let id = scene.add_mesh(root, Mesh::new(cube(), material.clone()));
    scene.graph.object_mut(id).position = Vector3::new(0.0, 0.0, z);
    id
}

fn order(frame: &Frame) -> Vec<(ObjectId, Side)> {
    frame.draws.iter().map(|d| (d.object, d.side)).collect()
}

#[test]
fn opaque_sorts_by_material_then_front_to_back() {
    let mut scene = Scene::new();
    let first = Arc::new(PhongMaterial::new());
    let second = Arc::new(PhongMaterial::new());
    let far_second = add(&mut scene, &second, -5.0);
    let near_second = add(&mut scene, &second, 2.0);
    let far_first = add(&mut scene, &first, -3.0);
    let frame = build_frame(&mut scene, &mut camera(), OPTIONS);
    assert_eq!(
        order(&frame),
        vec![
            (far_first, Side::Front),
            (near_second, Side::Front),
            (far_second, Side::Front)
        ]
    );
}

#[test]
fn transparent_sorts_back_to_front_after_opaque_with_two_passes() {
    let mut scene = Scene::new();
    let glass = Arc::new(PhongMaterial {
        transparent: true,
        side: Side::Double,
        ..PhongMaterial::new()
    });
    let opaque = Arc::new(PhongMaterial::new());
    let near = add(&mut scene, &glass, 3.0);
    let far = add(&mut scene, &glass, -3.0);
    let solid = add(&mut scene, &opaque, 0.0);
    let frame = build_frame(&mut scene, &mut camera(), OPTIONS);
    assert_eq!(
        order(&frame),
        vec![
            (solid, Side::Front),
            (far, Side::Back),
            (far, Side::Front),
            (near, Side::Back),
            (near, Side::Front)
        ]
    );
}

#[test]
fn render_order_beats_depth_and_group_order_beats_render_order() {
    let mut scene = Scene::new();
    let glass = Arc::new(PhongMaterial {
        transparent: true,
        ..PhongMaterial::new()
    });
    let overlay = add(&mut scene, &glass, 3.0);
    scene.node_mut(overlay).render_order = 999.0;
    let back = add(&mut scene, &glass, -3.0);
    let root = scene.root;
    let group = scene.add_group(root);
    scene.node_mut(group).render_order = 1000.0;
    let grouped = scene.add_mesh(group, Mesh::new(cube(), glass.clone()));
    let frame = build_frame(&mut scene, &mut camera(), OPTIONS);
    assert_eq!(
        order(&frame),
        vec![
            (back, Side::Front),
            (overlay, Side::Front),
            (grouped, Side::Front)
        ]
    );
}

#[test]
fn invisible_objects_and_materials_are_skipped() {
    let mut scene = Scene::new();
    let root = scene.root;
    let material = Arc::new(PhongMaterial::new());
    let hidden_group = scene.add_group(root);
    scene.node_mut(hidden_group).visible = false;
    scene.add_mesh(hidden_group, Mesh::new(cube(), material.clone()));
    let hidden_material = Arc::new(PhongMaterial {
        visible: false,
        ..PhongMaterial::new()
    });
    add(&mut scene, &hidden_material, 0.0);
    let shown = add(&mut scene, &material, 0.0);
    let frame = build_frame(&mut scene, &mut camera(), OPTIONS);
    assert_eq!(order(&frame), vec![(shown, Side::Front)]);
}

#[test]
fn mirrored_objects_flip_the_front_face() {
    let mut scene = Scene::new();
    let material = Arc::new(PhongMaterial::new());
    let plain = add(&mut scene, &material, 0.0);
    let mirrored = add(&mut scene, &material, 1.0);
    scene.graph.object_mut(mirrored).scale = Vector3::new(-1.0, 1.0, 1.0);
    let frame = build_frame(&mut scene, &mut camera(), OPTIONS);
    let cw = |id| {
        frame
            .draws
            .iter()
            .find(|d| d.object == id)
            .map(|d| d.front_face_cw)
    };
    assert_eq!(cw(plain), Some(false));
    assert_eq!(cw(mirrored), Some(true));
}

#[test]
fn light_uniforms_are_in_view_space() {
    let mut scene = Scene::new();
    let root = scene.root;
    scene.add_hemisphere_light(root, Color::hex(16777215.0), Color::hex(4473924.0), 2.0);
    let point = scene.add_point_light(root, Color::hex(16711680.0), 0.5);
    scene.graph.object_mut(point).position = Vector3::new(1.0, 2.0, 3.0);
    let (spot, target) = scene.create_spot_light(Color::hex(16777215.0), 4.0, 0.0, 0.5, 0.2, 2.0);
    scene.graph.add(root, spot);
    scene.graph.add(root, target);
    scene.graph.object_mut(target).position = Vector3::new(0.0, 0.0, 0.0);
    let frame = build_frame(&mut scene, &mut camera(), OPTIONS);
    let lights = &frame.lights;
    assert_eq!(lights.hemisphere.len(), 1);
    assert_eq!(lights.hemisphere[0].direction, [0.0, 1.0, 0.0]);
    assert_eq!(lights.hemisphere[0].sky_color, [2.0, 2.0, 2.0]);
    assert_eq!(lights.point[0].position, [1.0, 2.0, -7.0]);
    assert_eq!(lights.point[0].color, [0.5, 0.0, 0.0]);
    assert_eq!(lights.point[0].decay, 2.0);
    assert_eq!(lights.spot[0].direction, [0.0, 1.0, 0.0]);
    assert_eq!(lights.spot[0].cone_cos, math::cos(0.5) as f32);
    assert_eq!(
        lights.spot[0].penumbra_cos,
        math::cos(0.5 * (1.0 - 0.2)) as f32
    );
}

#[test]
fn instanced_bounding_sphere_is_computed_once() {
    let mut scene = Scene::new();
    let root = scene.root;
    let material = Arc::new(PhongMaterial {
        transparent: true,
        ..PhongMaterial::new()
    });
    let mut a = Mesh::instanced(cube(), material.clone(), 1);
    let mut b = Mesh::instanced(cube(), material.clone(), 1);
    let place = |mesh: &mut Mesh, z: f64| {
        let mut o = asrs_field3d::three::Object3D::new();
        o.position = Vector3::new(0.0, 0.0, z);
        o.update_matrix();
        mesh.instances.as_mut().unwrap().set_matrix_at(0, &o.matrix);
    };
    place(&mut a, -3.0);
    place(&mut b, 3.0);
    let a = scene.add_mesh(root, a);
    let b = scene.add_mesh(root, b);
    let mut camera = camera();
    let first = build_frame(&mut scene, &mut camera, OPTIONS);
    assert_eq!(order(&first), vec![(a, Side::Front), (b, Side::Front)]);
    if let Some(mesh) = scene.mesh_mut(a) {
        place(mesh, 5.0);
    }
    let second = build_frame(&mut scene, &mut camera, OPTIONS);
    assert_eq!(order(&second), vec![(a, Side::Front), (b, Side::Front)]);
    assert!(matches!(scene.node(a).kind, NodeKind::Mesh(_)));
}

#[test]
fn background_clear_is_the_srgb_colour() {
    let mut scene = Scene::new();
    scene.background = Some(Color::style(&"#222222".into()));
    let frame = build_frame(&mut scene, &mut camera(), OPTIONS);
    let level = math::round(frame.clear[0] * 255.0);
    assert_eq!(level, 34.0);
    assert_eq!(frame.clear[3], 1.0);
}
