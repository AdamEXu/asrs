//! The 3D presentation path end to end: the render thread renders offscreen, wakes the
//! foreground through a channel whose receiver calls `cx.notify`, and the view shows the
//! readback as a `RenderImage`, dropping the previous one. The camera orbits so every presented
//! frame is new; after `frames` new frames it prints the frame-time statistics and quits.
//! Usage: `field3d_window [frames]` (default 1200).

#![allow(clippy::disallowed_methods)]

use std::sync::Arc;
use std::time::Instant;

use asrs_field3d::render::{
    FrameOptions, Geometry, Mesh, PhongMaterial, RenderThread, Scene, Side, build_frame,
};
use asrs_field3d::three::{Color, Euler, EulerOrder, Object3D, PerspectiveCamera, Vector3, primitives};
use asrs_ui::gpui::{
    App, AppContext, Bounds, Context, Corners, IntoElement, ParentElement, Pixels, Render,
    RenderImage, Styled, Window, WindowBounds, WindowOptions, application, canvas, div, px, size,
};

fn scene() -> Scene {
    let mut scene = Scene::new();
    scene.background = Some(Color::hex(0x222222 as f64));
    let root = scene.root;
    scene.add_hemisphere_light(root, Color::hex(0xffffff as f64), Color::hex(0x444444 as f64), 2.0);
    let field = scene.add_group(root);
    scene.graph.object_mut(field).set_rotation(Euler::new(
        -std::f64::consts::FRAC_PI_2,
        0.0,
        std::f64::consts::PI,
        EulerOrder::Xyz,
    ));
    let light = scene.add_point_light(field, Color::hex(0xffffff as f64), 0.5);
    scene.graph.object_mut(light).position = Vector3::new(0.0, 0.0, 10.0);
    let carpet = PhongMaterial {
        color: Color::hex(0x888888 as f64),
        side: Side::Double,
        ..PhongMaterial::new()
    };
    let plane = Geometry::new(primitives::plane_geometry(20.5, 9.2, 1.0, 1.0));
    scene.add_mesh(field, Mesh::new(plane, Arc::new(carpet)));
    let post = Geometry::new(primitives::cylinder_geometry(
        0.02,
        0.02,
        0.5,
        12.0,
        1.0,
        false,
        0.0,
        std::f64::consts::TAU,
    ));
    let rail = Arc::new(PhongMaterial {
        color: Color::hex(0xdddddd as f64),
        ..PhongMaterial::new()
    });
    for side in [-1.0, 1.0] {
        for i in 0..28 {
            let id = scene.add_mesh(field, Mesh::new(post.clone(), rail.clone()));
            let object = scene.graph.object_mut(id);
            object.position = Vector3::new(-8.2 + f64::from(i) * 0.6, side * 4.1, 0.25);
            object.set_rotation(Euler::new(std::f64::consts::FRAC_PI_2, 0.0, 0.0, EulerOrder::Xyz));
        }
    }
    let robot = Geometry::new(primitives::sphere_geometry(
        0.4,
        64.0,
        52.0,
        0.0,
        std::f64::consts::TAU,
        0.0,
        std::f64::consts::PI,
    ));
    let mut mesh = Mesh::instanced(
        robot,
        Arc::new(PhongMaterial {
            color: Color::hex(0x6379a6 as f64),
            ..PhongMaterial::new()
        }),
        4,
    );
    if let Some(instances) = mesh.instances.as_mut() {
        for i in 0..4 {
            let mut dummy = Object3D::new();
            dummy.position = Vector3::new(-3.0 + i as f64 * 2.0, i as f64 - 1.5, 0.4);
            dummy.update_matrix();
            instances.set_matrix_at(i, &dummy.matrix);
        }
    }
    scene.add_mesh(field, mesh);
    scene
}

fn percentiles(values: &[f64]) -> String {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let at = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
    format!(
        "n={} p50={:.3} p95={:.3} p99={:.3} max={:.3} ms",
        v.len(),
        at(0.5),
        at(0.95),
        at(0.99),
        at(1.0)
    )
}

struct Field3dWindow {
    thread: RenderThread,
    scene: Scene,
    camera: PerspectiveCamera,
    image: Option<Arc<RenderImage>>,
    shown: u64,
    target: usize,
    last_new: Option<Instant>,
    intervals: Vec<f64>,
    render_ms: Vec<f64>,
    present_ms: Vec<f64>,
    done: bool,
}

impl Field3dWindow {
    fn submit(&mut self, width: u32, height: u32) {
        let angle = self.shown as f64 * 0.01;
        self.camera.aspect = f64::from(width) / f64::from(height);
        self.camera.update_projection_matrix();
        self.camera.object.position = Vector3::new(12.37 * angle.sin(), 6.0, -12.37 * angle.cos());
        self.camera.look_at(Vector3::new(0.0, 0.5, 0.0));
        let options = FrameOptions {
            width,
            height,
            samples: 4,
        };
        self.thread.submit(build_frame(&mut self.scene, &mut self.camera, options));
    }
}

impl Render for Field3dWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let scale = window.scale_factor();
        let viewport = window.viewport_size();
        let width = (f32::from(viewport.width) * scale) as u32;
        let height = (f32::from(viewport.height) * scale) as u32;
        if let Some(frame) = self.thread.take() {
            let start = Instant::now();
            self.shown = frame.seq;
            if let Some(last) = self.last_new.replace(start) {
                self.intervals.push((start - last).as_secs_f64() * 1e3);
            }
            self.render_ms.push(frame.render_ms);
            let pixels = frame.bgra.clone();
            self.thread.recycle(frame.bgra);
            if let Some(buffer) = image::ImageBuffer::from_raw(frame.width, frame.height, pixels) {
                let image = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
                if let Some(previous) = self.image.replace(image) {
                    let _ = window.drop_image(previous);
                }
            }
            self.present_ms.push(start.elapsed().as_secs_f64() * 1e3);
            if self.intervals.len() >= self.target && !self.done {
                self.done = true;
                let active: f64 = self.intervals.iter().sum();
                println!(
                    "adapter {}: {} new frames in {:.2} s = {:.1} fps",
                    self.thread.adapter(),
                    self.intervals.len(),
                    active / 1e3,
                    self.intervals.len() as f64 / (active / 1e3)
                );
                println!("interval between new frames: {}", percentiles(&self.intervals[10..]));
                println!("render thread render+readback: {}", percentiles(&self.render_ms[10..]));
                println!("foreground RenderImage build: {}", percentiles(&self.present_ms[10..]));
                cx.defer(|cx| cx.quit());
            }
            self.submit(width, height);
        } else if self.image.is_none() && self.shown == 0 {
            self.submit(width, height);
        }
        let image = self.image.clone();
        div().size_full().child(
            canvas(
                |_, _, _| {},
                move |bounds: Bounds<Pixels>, _, window, _| {
                    if let Some(image) = image {
                        let _ = window.paint_image(bounds, bounds, Corners::default(), image, 0, false);
                    }
                },
            )
            .size_full(),
        )
    }
}

fn main() {
    let target = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(1200);
    application().run(move |cx: &mut App| {
        asrs_ui::init(cx);
        let bounds = Bounds::centered(None, size(px(1100.0), px(650.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };
        asrs_ui::open_window(options, cx, move |_, cx| {
            let (wake_tx, wake_rx) = async_channel::bounded::<()>(1);
            let thread = RenderThread::spawn(move || {
                let _ = wake_tx.try_send(());
            })
            .expect("render thread");
            let view = cx.new(|_| Field3dWindow {
                thread,
                scene: scene(),
                camera: PerspectiveCamera::new(50.0, 2.0, 0.15, 1000.0),
                image: None,
                shown: 0,
                target,
                last_new: None,
                intervals: Vec::new(),
                render_ms: Vec::new(),
                present_ms: Vec::new(),
                done: false,
            });
            let weak = view.downgrade();
            cx.spawn(async move |cx| {
                while wake_rx.recv().await.is_ok() {
                    if weak.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            })
            .detach();
            view
        })
        .expect("window");
        cx.activate(true);
    });
}
