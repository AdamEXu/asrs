use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{
    App, AppContext, Bounds, Context, Corners, IntoElement, ParentElement, Render, RenderImage,
    Styled, Window, WindowBounds, WindowOptions, canvas, div, px, rgb, size,
};
use smallvec::smallvec;

#[derive(Default)]
struct Stats {
    intervals: Vec<f64>,
    build: Vec<f64>,
    swizzle: Vec<f64>,
    paint: Vec<f64>,
    render_total: Vec<f64>,
    rss_mb: Vec<f64>,
}

pub struct Frames {
    w: u32,
    h: u32,
    base: Vec<u8>,
    frame_no: u64,
    prev: Option<Arc<RenderImage>>,
    drop_old: bool,
    last: Option<Instant>,
    start: Instant,
    secs: f64,
    stats: Rc<RefCell<Stats>>,
    done: bool,
    logical: (f32, f32),
}

fn rss_mb() -> f64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok();
    out.and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map(|kb| kb / 1024.0)
        .unwrap_or(f64::NAN)
}

fn pct(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[((s.len() - 1) as f64 * p).round() as usize]
}

fn summary(name: &str, v: &[f64]) -> String {
    let mean = v.iter().sum::<f64>() / v.len().max(1) as f64;
    format!(
        "{name}: n={} mean={mean:.3}ms p50={:.3} p95={:.3} p99={:.3} max={:.3}",
        v.len(),
        pct(v, 0.5),
        pct(v, 0.95),
        pct(v, 0.99),
        pct(v, 1.0)
    )
}

impl Render for Frames {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let now = Instant::now();
        if let Some(last) = self.last {
            self.stats
                .borrow_mut()
                .intervals
                .push((now - last).as_secs_f64() * 1e3);
        }
        self.last = Some(now);
        if self.frame_no % 60 == 0 {
            self.stats.borrow_mut().rss_mb.push(rss_mb());
        }

        let (w, h) = (self.w as usize, self.h as usize);
        let t0 = Instant::now();
        let shift = (self.frame_no as usize * 7) % w;
        let mut rgba = vec![0u8; w * h * 4];
        for y in 0..h {
            let src = &self.base[(y * 2 * w + shift) * 4..(y * 2 * w + shift + w) * 4];
            rgba[y * w * 4..(y + 1) * w * 4].copy_from_slice(src);
        }
        let t1 = Instant::now();
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        let t2 = Instant::now();
        let buf = image::ImageBuffer::from_raw(self.w, self.h, rgba).unwrap();
        let img = Arc::new(RenderImage::new(smallvec![image::Frame::new(buf)]));
        if let Some(prev) = self.prev.replace(img.clone()) {
            if self.drop_old {
                let _ = window.drop_image(prev);
            }
        }
        {
            let mut s = self.stats.borrow_mut();
            s.build.push((t1 - t0).as_secs_f64() * 1e3);
            s.swizzle.push((t2 - t1).as_secs_f64() * 1e3);
        }
        self.frame_no += 1;

        let elapsed = self.start.elapsed().as_secs_f64();
        if elapsed > self.secs && !self.done {
            self.done = true;
            let s = self.stats.borrow();
            let skip = 30.min(s.intervals.len());
            let iv = &s.intervals[skip..];
            println!(
                "frames: image {}x{} drawn at {:?} logical, scale {}, drop_old={}",
                self.w,
                self.h,
                self.logical,
                window.scale_factor(),
                self.drop_old
            );
            println!("frames rendered={} in {elapsed:.2}s => {:.1} fps", self.frame_no, self.frame_no as f64 / elapsed);
            println!("{}", summary("frame interval", iv));
            println!("{}", summary("cpu build (copy rows)", &s.build));
            println!("{}", summary("cpu swizzle RGBA->BGRA", &s.swizzle));
            println!("{}", summary("paint_image (atlas insert+upload)", &s.paint));
            println!("{}", summary("render() total", &s.render_total));
            println!(
                "rss MB samples: first={:.1} last={:.1} max={:.1} (n={})",
                s.rss_mb.first().copied().unwrap_or(f64::NAN),
                s.rss_mb.last().copied().unwrap_or(f64::NAN),
                s.rss_mb.iter().cloned().fold(0.0, f64::max),
                s.rss_mb.len()
            );
            cx.defer(|cx| cx.quit());
        }
        window.request_animation_frame();

        let stats = self.stats.clone();
        let stats2 = self.stats.clone();
        let (lw, lh) = self.logical;
        let render_start = t0;
        div()
            .size_full()
            .bg(rgb(0x222222))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds: Bounds<gpui_kit::Pixels>, _, window, _| {
                        let t = Instant::now();
                        window
                            .paint_image(bounds, bounds, Corners::default(), img, 0, false)
                            .unwrap();
                        stats.borrow_mut().paint.push(t.elapsed().as_secs_f64() * 1e3);
                        stats2
                            .borrow_mut()
                            .render_total
                            .push(render_start.elapsed().as_secs_f64() * 1e3);
                    },
                )
                .w(px(lw))
                .h(px(lh)),
            )
    }
}

pub fn run(w: u32, h: u32, secs: f64, drop_old: bool) {
    gpui_kit::application().run(move |cx: &mut App| {
        gpui_kit::init(cx);
        let mut base = vec![0u8; (w as usize * 2) * h as usize * 4];
        for y in 0..h as usize {
            for x in 0..w as usize * 2 {
                let o = (y * w as usize * 2 + x) * 4;
                base[o] = (x % 256) as u8;
                base[o + 1] = (y % 256) as u8;
                base[o + 2] = ((x / 7 + y / 3) % 256) as u8;
                base[o + 3] = 255;
            }
        }
        let logical = (1100.0f32, 650.0f32);
        let bounds = Bounds::centered(None, size(px(logical.0), px(logical.1)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            move |_, cx| {
                cx.new(|_| Frames {
                    w,
                    h,
                    base,
                    frame_no: 0,
                    prev: None,
                    drop_old,
                    last: None,
                    start: Instant::now(),
                    secs,
                    stats: Default::default(),
                    done: false,
                    logical,
                })
            },
        )
        .unwrap();
        cx.activate(true);
        let _ = Duration::ZERO;
    });
}
