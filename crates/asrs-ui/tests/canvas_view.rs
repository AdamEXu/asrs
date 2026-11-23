use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use asrs_canvas::{Canvas2d, Pixmap, RasterState, ShadeCache};
use asrs_ui::canvas_view::geometry::device_snap;
use asrs_ui::canvas_view::{CanvasMetrics, CanvasState, canvas_view, rasterize};
use asrs_ui::capture::{Capture, HeadlessCapture};
use gpui_kit::{
    AnyWindowHandle, AppContext, Context, Entity, IntoElement, ParentElement, Render, RenderImage,
    Styled, Window, WindowHandle, div, point, px, rgb, size,
};

const ORIGIN: [f32; 2] = [12.0, 7.0];
const BOX: [f32; 2] = [180.0, 120.0];

#[derive(Clone, Copy, PartialEq)]
enum Scene {
    Opaque(u32),
    Translucent,
}

fn scene(kind: Scene, metrics: &CanvasMetrics) -> Canvas2d {
    let (bw, bh) = metrics.client_bitmap_size();
    let dpr = metrics.device_pixel_ratio;
    let (w, h) = (
        f64::from(metrics.client_width),
        f64::from(metrics.client_height),
    );
    let mut c = Canvas2d::new(bw, bh);
    c.scale(dpr, dpr);
    c.clear_rect(0.0, 0.0, w, h);
    let seed = match kind {
        Scene::Opaque(seed) => {
            c.set_fill_style("#222b33");
            c.fill_rect(0.0, 0.0, w, h);
            f64::from(seed)
        }
        Scene::Translucent => 0.0,
    };
    c.set_stroke_style("rgb(255,200,0)");
    c.set_line_width(1.5);
    c.begin_path();
    c.move_to(4.0, h - 4.0);
    for i in 1..12 {
        let x = f64::from(i) * w / 12.0;
        let y = h * 0.5 + (f64::from(i) * 7.0 + seed * 3.0) % (h * 0.4) - h * 0.2;
        c.line_to(x, y);
    }
    c.stroke();
    c.set_fill_style("rgba(0,160,255,0.45)");
    c.begin_path();
    c.arc(w * 0.3 + seed, h * 0.4, 25.0, 0.0, 6.0);
    c.fill();
    c.set_fill_style("rgba(255,64,64,0.8)");
    c.fill_rect(w * 0.6, 10.25, 33.5, 20.0);
    c.set_font("12px ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont");
    c.set_fill_style("#e0e0e0");
    c.fill_text(format!("Frame {seed}").as_str(), 8.0, 16.0);
    c
}

struct Host {
    state: Entity<CanvasState>,
    kind: Rc<Cell<Option<Scene>>>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        let kind = self.kind.clone();
        div().size_full().bg(rgb(0xffffff)).child(
            div()
                .absolute()
                .left(px(ORIGIN[0]))
                .top(px(ORIGIN[1]))
                .w(px(BOX[0]))
                .h(px(BOX[1]))
                .child(
                    canvas_view(self.state.clone(), move |metrics, _| {
                        kind.get().map(|k| scene(k, metrics))
                    })
                    .size_full(),
                ),
        )
    }
}

struct Rig {
    capture: HeadlessCapture,
    window: WindowHandle<Host>,
    state: Entity<CanvasState>,
    kind: Rc<Cell<Option<Scene>>>,
}

impl Rig {
    fn new(scale: f32) -> Rig {
        let mut capture = HeadlessCapture::new(Arc::new(())).expect("headless context");
        let kind = Rc::new(Cell::new(None));
        let host_kind = kind.clone();
        let window = capture
            .open_window(size(px(220.0), px(150.0)), scale, |_, cx| {
                let state = cx.new(CanvasState::new);
                cx.new(|_| Host {
                    state,
                    kind: host_kind,
                })
            })
            .expect("open window");
        let state = capture
            .context()
            .update_window(window.into(), |root, _, cx| {
                root.downcast::<Host>()
                    .expect("host")
                    .read(cx)
                    .state
                    .clone()
            })
            .expect("state");
        Rig {
            capture,
            window,
            state,
            kind,
        }
    }

    fn handle(&self) -> AnyWindowHandle {
        self.window.into()
    }

    fn shoot(&mut self, kind: Option<Scene>) -> Capture {
        self.kind.set(kind);
        self.capture.capture(self.handle()).expect("capture")
    }

    fn read<R>(&mut self, f: impl FnOnce(&CanvasState) -> R) -> R {
        let state = self.state.clone();
        self.capture.context().update(|cx| f(state.read(cx)))
    }

    fn image(&mut self) -> Option<Arc<RenderImage>> {
        self.read(|s| s.presenter().image().cloned())
    }

    fn source(&mut self, kind: Scene) -> Pixmap {
        let metrics = self.read(|s| s.metrics()).expect("laid out");
        let frame = scene(kind, &metrics);
        self.capture
            .context()
            .update(|cx| {
                let (mut state, mut shades) = (RasterState::default(), ShadeCache::default());
                rasterize(&frame, &HashMap::new(), &mut state, &mut shades, cx)
            })
            .expect("rasterized")
            .0
    }
}

fn region(shot: &Capture, x: u32, y: u32, w: u32, h: u32) -> Vec<[u8; 4]> {
    (y..y + h)
        .flat_map(|py| (x..x + w).map(move |px| (px, py)))
        .map(|(px, py)| shot.pixel(px, py))
        .collect()
}

fn origin_device(scale: f32) -> (u32, u32) {
    (
        device_snap(ORIGIN[0], scale) as u32,
        device_snap(ORIGIN[1], scale) as u32,
    )
}

fn check_metrics(rig: &mut Rig, scale: f32) {
    let m = rig.read(|s| s.metrics()).expect("metrics");
    assert_eq!((m.client_width, m.client_height), (180, 120));
    assert_eq!(m.device_pixel_ratio, f64::from(scale));
    assert_eq!(m.client_rect, [12.0, 7.0, 180.0, 120.0]);
    assert_eq!(
        m.client_bitmap_size(),
        ((180.0 * scale) as u32, (120.0 * scale) as u32)
    );
    assert_eq!(m.pointer(point(px(50.75), px(30.0))), [38.0, 23.0]);
}

fn check_opaque_exact(rig: &mut Rig, scale: f32) {
    let shot = rig.shoot(Some(Scene::Opaque(1)));
    let source = rig.source(Scene::Opaque(1));
    assert!(source.pixels().iter().all(|p| p.alpha() == 255));
    let (x, y) = origin_device(scale);
    let got = region(&shot, x, y, source.width(), source.height());
    let want: Vec<[u8; 4]> = source.data().as_chunks::<4>().0.to_vec();
    let differing = got.iter().zip(&want).filter(|(a, b)| a != b).count();
    assert_eq!(differing, 0, "{differing} pixels differ at {scale}x");
    assert_eq!(shot.pixel(x - 1, y), [255; 4]);
    assert_eq!(shot.pixel(x, y - 1), [255; 4]);
    assert_eq!(shot.pixel(x + source.width(), y), [255; 4]);
    assert_eq!(shot.pixel(x, y + source.height()), [255; 4]);
}

fn check_translucent(rig: &mut Rig, scale: f32) -> u8 {
    let shot = rig.shoot(Some(Scene::Translucent));
    let source = rig.source(Scene::Translucent);
    let (x, y) = origin_device(scale);
    let got = region(&shot, x, y, source.width(), source.height());
    let mut worst = 0u8;
    for (g, p) in got.iter().zip(source.data().as_chunks::<4>().0) {
        let inv = 255 - u32::from(p[3]);
        for i in 0..3 {
            let want = u32::from(p[i]) + (255 * inv + 127) / 255;
            worst = worst.max(g[i].abs_diff(want as u8));
        }
    }
    worst
}

fn check_dirty_and_drop(rig: &mut Rig) {
    rig.shoot(Some(Scene::Opaque(2)));
    let first = rig.image().expect("image");
    let draws = rig.read(|s| s.draw_count());
    rig.shoot(Some(Scene::Opaque(2)));
    rig.shoot(None);
    assert_eq!(
        rig.read(|s| s.draw_count()),
        draws,
        "unchanged frame redrawn"
    );
    assert_eq!(rig.image().expect("image").id, first.id);

    let mut previous = first;
    for seed in 3..200 {
        rig.shoot(Some(Scene::Opaque(seed)));
        let current = rig.image().expect("image");
        assert_ne!(current.id, previous.id);
        assert_eq!(
            Arc::strong_count(&previous),
            1,
            "frame {seed}: previous image kept"
        );
        previous = current;
    }

    let handle = rig.handle();
    let state = rig.state.clone();
    rig.capture
        .context()
        .update_window(handle, |root, _, cx| {
            let host = root.downcast::<Host>().expect("host");
            host.update(cx, |host, cx| host.state = cx.new(CanvasState::new));
        })
        .expect("swap state");
    drop(state);
    rig.state = rig
        .capture
        .context()
        .update_window(handle, |root, _, cx| {
            root.downcast::<Host>()
                .expect("host")
                .read(cx)
                .state
                .clone()
        })
        .expect("state");
    rig.shoot(None);
    assert_eq!(
        Arc::strong_count(&previous),
        1,
        "released canvas kept its image"
    );
    assert!(rig.image().is_none());
}

fn main() {
    if asrs_parity::ci::skip_in_ci("canvas view", "a GPU") {
        return;
    }
    if !cfg!(target_os = "macos") {
        println!("canvas view: skipped, needs the macOS text backend and headless renderer");
        return;
    }
    for scale in [1.0f32, 2.0] {
        let mut rig = Rig::new(scale);
        rig.shoot(Some(Scene::Opaque(1)));
        check_metrics(&mut rig, scale);
        check_opaque_exact(&mut rig, scale);
        let worst = check_translucent(&mut rig, scale);
        assert!(worst <= 1, "translucent pixels off by {worst} at {scale}x");
        check_dirty_and_drop(&mut rig);
        println!("canvas view {scale}x: opaque exact, translucent within {worst}, one live image");
    }
}
