use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use asrs_field3d::render::{
    FatLine, FrameOptions, Geometry, Line, LineMaterial, LineSegmentsGeometry, Mesh, NodeKind,
    PhongMaterial, RenderThread, Renderer, Scene, build_frame,
};
use asrs_field3d::three::{Color, PerspectiveCamera, Vector3, primitives};

const W: u32 = 64;
const H: u32 = 48;

fn scene_with_box_at(y: f64) -> (Scene, PerspectiveCamera) {
    let mut scene = Scene::new();
    scene.background = Some(Color::style(&"#222222".into()));
    let root = scene.root;
    let material = PhongMaterial {
        color: Color::hex(0.0),
        emissive: Color::hex(16711680.0),
        ..PhongMaterial::new()
    };
    let geometry = Geometry::new(primitives::box_geometry(1.0, 1.0, 1.0, 1.0, 1.0, 1.0));
    let id = scene.add_mesh(root, Mesh::new(geometry, Arc::new(material)));
    scene.graph.object_mut(id).position = Vector3::new(0.0, y, 0.0);
    let mut camera = PerspectiveCamera::new(50.0, f64::from(W) / f64::from(H), 0.15, 1000.0);
    camera.object.position = Vector3::new(0.0, 0.0, 6.0);
    camera.look_at(Vector3::new(0.0, 0.0, 0.0));
    (scene, camera)
}

fn pixel(bgra: &[u8], x: u32, y: u32) -> [u8; 4] {
    let o = ((y * W + x) * 4) as usize;
    [bgra[o], bgra[o + 1], bgra[o + 2], bgra[o + 3]]
}

fn renderer() -> Option<Renderer> {
    match Renderer::new() {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("skipping: {e}");
            None
        }
    }
}

#[test]
fn readback_is_top_row_first_with_the_background_clear() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let (mut scene, mut camera) = scene_with_box_at(1.2);
    let options = FrameOptions {
        width: W,
        height: H,
        samples: 4,
        viewport: [f64::from(W), f64::from(H)],
        shadow_map: false,
    };
    let frame = build_frame(&mut scene, &mut camera, options);
    let mut bgra = Vec::new();
    renderer.render(&frame, &mut bgra);
    assert_eq!(bgra.len(), (W * H * 4) as usize);
    assert_eq!(pixel(&bgra, 0, 0), [0x22, 0x22, 0x22, 0xff]);
    assert_eq!(pixel(&bgra, W / 2, H / 4), [0, 0, 0xff, 0xff]);
    assert_eq!(pixel(&bgra, W / 2, H * 3 / 4), [0x22, 0x22, 0x22, 0xff]);
}

#[test]
fn render_thread_delivers_the_latest_frame_and_wakes() {
    if renderer().is_none() {
        return;
    }
    let (tx, rx) = mpsc::channel();
    let thread = RenderThread::spawn(move || {
        let _ = tx.send(());
    })
    .expect("render thread");
    let options = FrameOptions {
        width: W,
        height: H,
        samples: 1,
        viewport: [f64::from(W), f64::from(H)],
        shadow_map: false,
    };
    let (mut scene, mut camera) = scene_with_box_at(-1.2);
    thread.submit(build_frame(&mut scene, &mut camera, options));
    rx.recv_timeout(Duration::from_secs(10)).expect("woken");
    let frame = thread.take().expect("a frame");
    assert_eq!((frame.width, frame.height), (W, H));
    assert_eq!(pixel(&frame.bgra, W / 2, H * 3 / 4), [0, 0, 0xff, 0xff]);
    assert!(thread.take().is_none());
    thread.recycle(frame.bgra);
}

#[test]
fn an_instanced_mesh_with_no_instances_draws_nothing() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let (mut scene, mut camera) = scene_with_box_at(1.2);
    let root = scene.root;
    let geometry = Geometry::new(primitives::box_geometry(1.0, 1.0, 1.0, 1.0, 1.0, 1.0));
    scene.add_mesh(
        root,
        Mesh::instanced(geometry, Arc::new(PhongMaterial::new()), 0),
    );
    let options = FrameOptions {
        width: W,
        height: H,
        samples: 4,
        viewport: [f64::from(W), f64::from(H)],
        shadow_map: false,
    };
    let frame = build_frame(&mut scene, &mut camera, options);
    let mut bgra = Vec::new();
    renderer.render(&frame, &mut bgra);
    assert_eq!(pixel(&bgra, W / 2, H / 4), [0, 0, 0xff, 0xff]);
    assert_eq!(pixel(&bgra, W / 2, H * 3 / 4), [0x22, 0x22, 0x22, 0xff]);
}

#[test]
fn a_shadowed_frame_with_lines_renders_and_dumps_its_shadow_map() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let mut scene = Scene::new();
    scene.background = Some(Color::style(&"#222222".into()));
    let root = scene.root;
    let floor = Geometry::new(primitives::plane_geometry(6.0, 6.0, 1.0, 1.0));
    let floor = scene.add_mesh(root, Mesh::new(floor, Arc::new(PhongMaterial::new())));
    scene.graph.object_mut(floor).position = Vector3::new(0.0, 0.0, -1.0);
    let cube = Geometry::new(primitives::box_geometry(1.0, 1.0, 1.0, 1.0, 1.0, 1.0));
    let cube = scene.add_mesh(root, Mesh::new(cube, Arc::new(PhongMaterial::new())));
    scene.mesh_mut(cube).unwrap().cast_shadow = true;
    scene.mesh_mut(floor).unwrap().receive_shadow = true;
    let (light, target) = scene.create_spot_light(Color::hex(16777215.0), 40.0, 0.0, 0.8, 0.2, 2.0);
    if let NodeKind::SpotLight(l) = &mut scene.node_mut(light).kind {
        l.cast_shadow = true;
    }
    scene.graph.object_mut(light).position = Vector3::new(0.5, 0.5, 5.0);
    scene.graph.add(root, light);
    scene.graph.add(root, target);
    scene.add(
        root,
        NodeKind::FatLine(FatLine {
            geometry: LineSegmentsGeometry::from_points(&[-2.0, -2.0, 0.6, 2.0, -2.0, 0.6]),
            material: Arc::new(LineMaterial::new(Color::hex(16747520.0), 4.0)),
        }),
    );
    scene.add(
        root,
        NodeKind::Line(Line::grid_helper(
            4.0,
            4.0,
            Color::hex(16711680.0),
            Color::hex(65280.0),
        )),
    );
    let (_, mut camera) = scene_with_box_at(0.0);
    let options = FrameOptions {
        shadow_map: true,
        ..FrameOptions::new(f64::from(W), f64::from(H), 1.0, true)
    };
    let frame = build_frame(&mut scene, &mut camera, options);
    assert_eq!(frame.shadows.len(), 1);
    let mut bgra = Vec::new();
    renderer.render(&frame, &mut bgra);
    let (w, h, texels) = renderer.shadow_map(0).expect("shadow map");
    assert_eq!((w, h, texels.len()), (512, 512, 512 * 512 * 4));
    assert_eq!(&texels[..4], &[255, 255, 255, 255]);
    assert!(texels.chunks(4).any(|t| t != [255, 255, 255, 255]));
    assert!(renderer.shadow_map(1).is_none());
    let aux = renderer.render_aux(&frame);
    assert_eq!(aux.opaque_ids.len(), (W * H) as usize);
}
