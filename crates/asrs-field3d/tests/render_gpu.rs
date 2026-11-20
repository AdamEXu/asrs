use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use asrs_field3d::render::{
    FrameOptions, Geometry, Mesh, PhongMaterial, RenderThread, Renderer, Scene, build_frame,
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
