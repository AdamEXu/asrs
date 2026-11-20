#![allow(clippy::disallowed_methods)]

use std::process::{Command, ExitCode};
use std::sync::Arc;
use std::time::Instant;

use asrs_canvas::Pixmap;
use asrs_ui::canvas_view::presenter::into_straight_bgra;
use asrs_ui::canvas_view::{CanvasState, canvas_view};
use asrs_ui::capture::HeadlessCapture;
use gpui_kit::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, Window, div, px, rgb,
    size,
};

const LOGICAL: (f32, f32) = (1100.0, 650.0);
const SCALE: f32 = 2.0;
const PRESENT_P95_MS: f64 = 4.0;
const RSS_GROWTH_MB: f64 = 64.0;

struct Host {
    state: Entity<CanvasState>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0xffffff))
            .child(canvas_view(self.state.clone(), |_, _| None).size_full())
    }
}

fn rss_mb() -> f64 {
    Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map_or(f64::NAN, |kb| kb / 1024.0)
}

fn frame(width: u32, height: u32, seed: u32) -> Pixmap {
    let mut pixmap = Pixmap::new(width, height).expect("pixmap");
    for (i, p) in pixmap
        .data_mut()
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .enumerate()
    {
        let (x, y) = (i as u32 % width, i as u32 / width);
        let a = if (x / 64 + y / 64 + seed).is_multiple_of(5) {
            128
        } else {
            255
        };
        let c = |v: u32| ((v % 256) * a / 255) as u8;
        *p = [c(x + seed * 13), c(y), c(x / 7 + y / 3 + seed), a as u8];
    }
    pixmap
}

fn percentile(values: &[f64], p: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[((sorted.len() - 1) as f64 * p) as usize]
}

fn main() -> ExitCode {
    let frames: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(10_000);
    let (width, height) = ((LOGICAL.0 * SCALE) as u32, (LOGICAL.1 * SCALE) as u32);
    let sources: Vec<Pixmap> = (0..4).map(|seed| frame(width, height, seed)).collect();

    let mut capture = HeadlessCapture::new(Arc::new(())).expect("headless context");
    let window = capture
        .open_window(size(px(LOGICAL.0), px(LOGICAL.1)), SCALE, |_, cx| {
            let state = cx.new(CanvasState::new);
            cx.new(|_| Host { state })
        })
        .expect("open window");
    let handle = window.into();
    let cx = capture.context();
    let state = cx
        .update_window(handle, |root, _, cx| {
            root.downcast::<Host>()
                .expect("host")
                .read(cx)
                .state
                .clone()
        })
        .expect("state");

    let mut swizzle = Vec::new();
    for source in &sources {
        for _ in 0..10 {
            let copy = source.clone();
            let t = Instant::now();
            std::hint::black_box(into_straight_bgra(copy));
            swizzle.push(t.elapsed().as_secs_f64() * 1e3);
        }
    }
    let mut present = Vec::with_capacity(frames);
    let mut convert = Vec::with_capacity(frames);
    let mut rss = Vec::new();
    for i in 0..frames {
        let source = sources[i % sources.len()].clone();
        let t0 = Instant::now();
        cx.update_window(handle, |_, window, cx| {
            state.update(cx, |s, cx| s.present_pixmap(source, window, cx));
            window.refresh();
        })
        .expect("present");
        let t1 = Instant::now();
        cx.run_until_parked();
        let t2 = Instant::now();
        convert.push((t1 - t0).as_secs_f64() * 1e3);
        present.push((t2 - t0).as_secs_f64() * 1e3);
        if i % 100 == 0 {
            rss.push(rss_mb());
        }
    }
    rss.push(rss_mb());

    let baseline = rss[0];
    let peak = rss.iter().copied().fold(baseline, f64::max);
    let last = *rss.last().expect("samples");
    let p50 = percentile(&present, 0.5);
    let p95 = percentile(&present, 0.95);
    println!("canvas frames: {frames} frames of {width}x{height} at {SCALE}x");
    println!(
        "unpremultiply + swizzle alone: p50 {:.3} ms, p95 {:.3} ms",
        percentile(&swizzle, 0.5),
        percentile(&swizzle, 0.95)
    );
    println!(
        "update (convert, replace, draw): p50 {:.3} ms, p95 {:.3} ms",
        percentile(&convert, 0.5),
        percentile(&convert, 0.95)
    );
    println!(
        "present (convert, replace, paint, atlas upload, draw): p50 {p50:.3} ms, p95 {p95:.3} ms"
    );
    println!("rss: after the first frame {baseline:.1} MB, peak {peak:.1} MB, last {last:.1} MB");
    let mut ok = true;
    if p95 >= PRESENT_P95_MS {
        println!("FAIL: present p95 {p95:.3} ms is not under {PRESENT_P95_MS} ms");
        ok = false;
    }
    if peak - baseline > RSS_GROWTH_MB {
        println!(
            "FAIL: rss grew {:.1} MB after the first frame (limit {RSS_GROWTH_MB})",
            peak - baseline
        );
        ok = false;
    }
    if ok {
        println!("ok");
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
