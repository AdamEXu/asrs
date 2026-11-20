//! Frame-time benchmark for the render thread, sized like S3's measurement: about 70 draw items
//! (a carpet, rails and posts, glass panes, a robot-sized instanced mesh) at 2200x1300 with
//! MSAA 4x, the camera orbiting so every frame is new. Usage:
//! `field3d_bench [width] [height] [frames] [samples]`.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use asrs_field3d::render::{
    FrameOptions, Geometry, Mesh, PhongMaterial, RenderThread, Scene, Side, build_frame,
};
use asrs_js::math;
use asrs_field3d::three::{Color, Euler, EulerOrder, PerspectiveCamera, Vector3, primitives};

fn build() -> Scene {
    let mut scene = Scene::new();
    scene.background = Some(Color::hex(0x222222 as f64));
    let root = scene.root;
    scene.add_hemisphere_light(root, Color::hex(0xffffff as f64), Color::hex(0x444444 as f64), 2.0);
    let field = scene.add_group(root);
    scene
        .graph
        .object_mut(field)
        .set_rotation(Euler::new(-std::f64::consts::FRAC_PI_2, 0.0, std::f64::consts::PI, EulerOrder::Xyz));
    let light = scene.add_point_light(field, Color::hex(0xffffff as f64), 0.5);
    scene.graph.object_mut(light).position = Vector3::new(0.0, 0.0, 10.0);

    let carpet = PhongMaterial {
        color: Color::hex(0x888888 as f64),
        side: Side::Double,
        ..PhongMaterial::new()
    };
    let plane = Geometry::new(primitives::plane_geometry(20.5, 9.2, 1.0, 1.0));
    scene.add_mesh(field, Mesh::new(plane, Arc::new(carpet)));

    let rail = Arc::new(PhongMaterial {
        color: Color::hex(0xdddddd as f64),
        specular: Color::hex(0.0),
        shininess: 0.0,
        ..PhongMaterial::new()
    });
    let post = Geometry::new(primitives::cylinder_geometry(
        0.02, 0.02, 0.5, 12.0, 1.0, false, 0.0, std::f64::consts::TAU,
    ));
    for side in [-1.0, 1.0] {
        for i in 0..28 {
            let id = scene.add_mesh(field, Mesh::new(post.clone(), rail.clone()));
            let object = scene.graph.object_mut(id);
            object.position = Vector3::new(-8.2 + i as f64 * 0.6, side * 4.1, 0.25);
            object.set_rotation(Euler::new(std::f64::consts::FRAC_PI_2, 0.0, 0.0, EulerOrder::Xyz));
        }
        let glass = PhongMaterial {
            color: Color::hex(0xffffff as f64),
            side: Side::Double,
            transparent: true,
            opacity: 0.25,
            ..PhongMaterial::new()
        };
        let pane = Geometry::new(primitives::plane_geometry(16.5, 0.5, 1.0, 1.0));
        let id = scene.add_mesh(field, Mesh::new(pane, Arc::new(glass)));
        let object = scene.graph.object_mut(id);
        object.position = Vector3::new(0.0, side * 4.1, 0.25);
        object.set_rotation(Euler::new(std::f64::consts::FRAC_PI_2, 0.0, 0.0, EulerOrder::Xyz));
    }

    let robot = Geometry::new(primitives::sphere_geometry(
        0.4, 64.0, 52.0, 0.0, std::f64::consts::TAU, 0.0, std::f64::consts::PI,
    ));
    let body = Arc::new(PhongMaterial {
        color: Color::hex(0x6379a6 as f64),
        side: Side::Double,
        ..PhongMaterial::new()
    });
    let mut mesh = Mesh::instanced(robot, body, 4);
    if let Some(instances) = mesh.instances.as_mut() {
        for i in 0..4 {
            let mut dummy = asrs_field3d::three::Object3D::new();
            dummy.position = Vector3::new(-3.0 + i as f64 * 2.0, (i as f64) - 1.5, 0.4);
            dummy.update_matrix();
            instances.set_matrix_at(i, &dummy.matrix);
        }
    }
    scene.add_mesh(field, mesh);
    scene
}

fn percentile(values: &mut [f64], p: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    values[((values.len() - 1) as f64 * p).round() as usize]
}

fn report(name: &str, mut values: Vec<f64>) {
    let mean = values.iter().sum::<f64>() / values.len().max(1) as f64;
    println!(
        "{name}: n={} mean={mean:.3} p50={:.3} p95={:.3} p99={:.3} max={:.3} ms",
        values.len(),
        percentile(&mut values, 0.5),
        percentile(&mut values, 0.95),
        percentile(&mut values, 0.99),
        percentile(&mut values, 1.0)
    );
}

fn main() -> Result<(), String> {
    let args: Vec<u32> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let width = args.first().copied().unwrap_or(2200);
    let height = args.get(1).copied().unwrap_or(1300);
    let frames = args.get(2).copied().unwrap_or(600) as usize;
    let samples = args.get(3).copied().unwrap_or(4);

    let (wake_tx, wake_rx) = mpsc::channel();
    let thread = RenderThread::spawn(move || {
        let _ = wake_tx.send(Instant::now());
    })
    .map_err(|e| e.to_string())?;
    let mut scene = build();
    let mut camera = PerspectiveCamera::new(50.0, width as f64 / height as f64, 0.15, 1000.0);
    let options = FrameOptions {
        width,
        height,
        samples,
    };
    let frame = build_frame(&mut scene, &mut camera, options);
    println!(
        "adapter {}, {}x{} msaa {samples}, {} draws",
        thread.adapter(),
        width,
        height,
        frame.draws.len()
    );

    let (mut build_ms, mut render_ms, mut gpu_ms, mut copy_ms, mut wake_ms, mut total_ms) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for i in 0..frames + 10 {
        let angle = i as f64 * 0.01;
        camera.object.position = Vector3::new(12.37 * math::sin(angle), 6.0, -12.37 * math::cos(angle));
        camera.look_at(Vector3::new(0.0, 0.5, 0.0));
        let start = Instant::now();
        let frame = build_frame(&mut scene, &mut camera, options);
        let built = start.elapsed();
        thread.submit(frame);
        let woke = wake_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|e| format!("render thread did not wake: {e}"))?;
        let rendered = thread.take().ok_or("woken without a frame")?;
        let done = Instant::now();
        if i >= 10 {
            build_ms.push(built.as_secs_f64() * 1e3);
            render_ms.push(rendered.render_ms);
            gpu_ms.push(rendered.timings.gpu_and_map_ms);
            copy_ms.push(rendered.timings.copy_ms);
            wake_ms.push((done - woke).as_secs_f64() * 1e3);
            total_ms.push((done - start).as_secs_f64() * 1e3);
        }
        thread.recycle(rendered.bgra);
    }
    report("foreground build_frame", build_ms);
    report("render thread render+readback", render_ms);
    report("render thread gpu+map wait", gpu_ms);
    report("render thread row copy", copy_ms);
    report("wake to take", wake_ms);
    report("submit to frame in hand", total_ms);
    Ok(())
}
