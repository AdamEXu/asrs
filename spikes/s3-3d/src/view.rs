use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use glam::DVec3;
use gpui_kit::{
    App, AppContext, Bounds, Context, Corners, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render, RenderImage, ScrollDelta, ScrollEvent,
    Styled, Window, WindowBounds, WindowOptions, canvas, div, px, rgb, size,
};
use smallvec::smallvec;

use crate::render::{Camera, Renderer};

struct Rendered {
    seq: u64,
    bgra: Vec<u8>,
    w: u32,
    h: u32,
}

#[derive(Default)]
struct ThreadStats {
    render_ms: Vec<f64>,
    gpu_ms: Vec<f64>,
}

struct Request {
    cam: Camera,
    w: u32,
    h: u32,
}

fn spawn_render_thread(samples: u32) -> (Sender<Request>, Arc<Mutex<Option<Rendered>>>, Arc<Mutex<ThreadStats>>, Arc<Mutex<Vec<Vec<u8>>>>) {
    let (tx, rx): (Sender<Request>, Receiver<Request>) = channel();
    let latest: Arc<Mutex<Option<Rendered>>> = Arc::new(Mutex::new(None));
    let stats: Arc<Mutex<ThreadStats>> = Default::default();
    let pool: Arc<Mutex<Vec<Vec<u8>>>> = Default::default();
    let (l2, s2, p2) = (latest.clone(), stats.clone(), pool.clone());
    std::thread::Builder::new()
        .name("field3d-render".into())
        .spawn(move || {
            let (scene, _) = crate::scene::build_standard(&crate::scene_config()).unwrap();
            let mut r = Renderer::new(samples).unwrap();
            r.upload(&scene);
            let mut seq = 0;
            while let Ok(mut req) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    req = newer;
                }
                let mut buf = p2.lock().unwrap().pop().unwrap_or_default();
                let t = Instant::now();
                let tm = r.render(&scene, &req.cam, req.w, req.h, &mut buf);
                {
                    let mut s = s2.lock().unwrap();
                    s.render_ms.push(t.elapsed().as_secs_f64() * 1e3);
                    s.gpu_ms.push(tm.gpu_and_map_ms);
                }
                seq += 1;
                let old = l2.lock().unwrap().replace(Rendered {
                    seq,
                    bgra: buf,
                    w: req.w,
                    h: req.h,
                });
                if let Some(old) = old {
                    p2.lock().unwrap().push(old.bgra);
                }
            }
        })
        .unwrap();
    (tx, latest, stats, pool)
}

fn rss_mb() -> f64 {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map(|kb| kb / 1024.0)
        .unwrap_or(f64::NAN)
}

pub struct Field3dView {
    tx: Sender<Request>,
    latest: Arc<Mutex<Option<Rendered>>>,
    thread_stats: Arc<Mutex<ThreadStats>>,
    pool: Arc<Mutex<Vec<Vec<u8>>>>,
    image: Option<Arc<RenderImage>>,
    shown_seq: u64,
    azimuth: f64,
    polar: f64,
    radius: f64,
    target: DVec3,
    drag: Option<Point<Pixels>>,
    auto_secs: Option<f64>,
    start: Instant,
    last_new: Option<Instant>,
    new_intervals: Vec<f64>,
    upload_ms: Vec<f64>,
    paint_ms: Arc<Mutex<Vec<f64>>>,
    rss: Vec<f64>,
    frames: u64,
    done: bool,
}

impl Field3dView {
    fn camera(&self, dark: bool) -> Camera {
        let offset = DVec3::new(
            self.radius * self.polar.sin() * self.azimuth.sin(),
            self.radius * self.polar.cos(),
            self.radius * self.polar.sin() * self.azimuth.cos(),
        );
        Camera {
            position: self.target + offset,
            target: self.target,
            fov: 50.0,
            dark,
        }
    }

    fn report(&self) {
        let ts = self.thread_stats.lock().unwrap();
        let skip = |v: &[f64]| v[v.len().min(10)..].to_vec();
        println!("window run {:.1}s, scale {}", self.start.elapsed().as_secs_f64(), 2);
        println!(
            "new frames presented: {} => {:.1} fps",
            self.new_intervals.len() + 1,
            (self.new_intervals.len() + 1) as f64 / self.start.elapsed().as_secs_f64()
        );
        println!("{}", crate::summary("interval between new frames", &skip(&self.new_intervals)));
        println!("{}", crate::summary("render thread: render+readback", &skip(&ts.render_ms)));
        println!("{}", crate::summary("render thread: gpu+map wait", &skip(&ts.gpu_ms)));
        println!("{}", crate::summary("fg: RenderImage build", &skip(&self.upload_ms)));
        println!("{}", crate::summary("fg: paint_image", &skip(&self.paint_ms.lock().unwrap())));
        println!(
            "rss MB: first={:.1} last={:.1} max={:.1} n={}",
            self.rss.first().copied().unwrap_or(f64::NAN),
            self.rss.last().copied().unwrap_or(f64::NAN),
            self.rss.iter().cloned().fold(0.0, f64::max),
            self.rss.len()
        );
    }
}

impl Render for Field3dView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let scale = window.scale_factor();
        let vp = window.viewport_size();
        let (w, h) = ((f32::from(vp.width) * scale) as u32, (f32::from(vp.height) * scale) as u32);
        if let Some(secs) = self.auto_secs {
            self.azimuth = self.start.elapsed().as_secs_f64() * 0.8;
            if self.start.elapsed().as_secs_f64() > secs && !self.done {
                self.done = true;
                self.report();
                cx.defer(|cx| cx.quit());
            }
        }
        let dark = matches!(window.appearance(), gpui_kit::WindowAppearance::Dark | gpui_kit::WindowAppearance::VibrantDark);
        let _ = self.tx.send(Request {
            cam: self.camera(dark),
            w,
            h,
        });

        let fresh = self.latest.lock().unwrap().take();
        if let Some(frame) = fresh {
            if frame.seq != self.shown_seq {
                self.shown_seq = frame.seq;
                let now = Instant::now();
                if let Some(last) = self.last_new {
                    self.new_intervals.push((now - last).as_secs_f64() * 1e3);
                }
                self.last_new = Some(now);
                let t = Instant::now();
                let bytes = frame.bgra.clone();
                self.pool.lock().unwrap().push(frame.bgra);
                let buf = image::ImageBuffer::from_raw(frame.w, frame.h, bytes).unwrap();
                let img = Arc::new(RenderImage::new(smallvec![image::Frame::new(buf)]));
                self.upload_ms.push(t.elapsed().as_secs_f64() * 1e3);
                if let Some(prev) = self.image.replace(img) {
                    let _ = window.drop_image(prev);
                }
            }
        }
        self.frames += 1;
        if self.frames % 60 == 0 {
            self.rss.push(rss_mb());
        }
        window.request_animation_frame();

        let img = self.image.clone();
        let paint_ms = self.paint_ms.clone();
        div()
            .size_full()
            .bg(rgb(if dark { 0x222222 } else { 0xffffff }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, _| this.drag = Some(ev.position)),
            )
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, _| this.drag = None))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, _| {
                if let Some(start) = this.drag {
                    let dx = f32::from(ev.position.x - start.x) as f64;
                    let dy = f32::from(ev.position.y - start.y) as f64;
                    this.azimuth -= dx * 0.005;
                    this.polar = (this.polar - dy * 0.005).clamp(0.01, std::f64::consts::PI - 0.01);
                    this.drag = Some(ev.position);
                }
            }))
            .on_scroll(cx.listener(|this, ev: &ScrollEvent, _, _| {
                let dy = match ev.delta {
                    ScrollDelta::Pixels(p) => f32::from(p.y) as f64,
                    ScrollDelta::Lines(l) => l.y as f64 * 40.0,
                };
                this.radius = (this.radius * 1.001f64.powf(-dy)).max(1.0);
            }))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds: Bounds<Pixels>, _, window, _| {
                        if let Some(img) = img {
                            let t = Instant::now();
                            window.paint_image(bounds, bounds, Corners::default(), img, 0, false).unwrap();
                            paint_ms.lock().unwrap().push(t.elapsed().as_secs_f64() * 1e3);
                        }
                    },
                )
                .size_full(),
            )
    }
}

pub fn run(auto_secs: Option<f64>, samples: u32) {
    gpui_kit::application().run(move |cx: &mut App| {
        gpui_kit::init(cx);
        let bounds = Bounds::centered(None, size(px(1100.0), px(650.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                move |_, cx| {
                    let (tx, latest, thread_stats, pool) = spawn_render_thread(samples);
                    let offset = DVec3::new(0.0, 6.0, -12.0) - DVec3::new(0.0, 0.5, 0.0);
                    let radius = offset.length();
                    cx.new(|_| Field3dView {
                        tx,
                        latest,
                        thread_stats,
                        pool,
                        image: None,
                        shown_seq: 0,
                        azimuth: offset.x.atan2(offset.z),
                        polar: (offset.y / radius).acos(),
                        radius,
                        target: DVec3::new(0.0, 0.5, 0.0),
                        drag: None,
                        auto_secs,
                        start: Instant::now(),
                        last_new: None,
                        new_intervals: Vec::new(),
                        upload_ms: Vec::new(),
                        paint_ms: Default::default(),
                        rss: Vec::new(),
                        frames: 0,
                        done: false,
                    })
                },
            )
            .unwrap();
        window
            .update(cx, |_, window, _| window.resize(size(px(1100.0), px(650.0))))
            .ok();
        cx.activate(true);
    });
}
