#![allow(clippy::disallowed_methods)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use asrs_ui::capture::{Capture, HeadlessCapture};
use asrs_ui::text::{
    CACHE_BYTES, CACHE_ENTRIES, CacheStats, FontSpec, LineHeight, Text, TextStyle, TextSystem,
};
use gpui_kit::{
    AnyWindowHandle, AppContext, Context, IntoElement, ParentElement, Render, Styled,
    UniformListScrollHandle, Window, div, point, px, rgb, size, uniform_list,
};

const ROWS: usize = 10_000;
const ROW_HEIGHT: f32 = 30.0;
const VIEWPORT: f32 = 650.0;
const SCALE: f32 = 2.0;
/// One frame at 60 Hz.
const FRAME_BUDGET: Duration = Duration::from_micros(16_667);
/// Two rows per frame, a brisk trackpad scroll.
const SCROLL_STEP: f32 = 60.0;
const SCROLL_FRAMES: usize = 1500;
/// Wall-clock timing shares the machine with other work, so a pass over budget is retried.
const ATTEMPTS: usize = 3;

struct List {
    scroll: UniformListScrollHandle,
    label: TextStyle,
    value: TextStyle,
}

impl Render for List {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        let (label, value) = (self.label, self.value);
        div().size_full().bg(rgb(0xffffff)).child(
            uniform_list("rows", ROWS, move |range, _, _| {
                range
                    .map(|i| {
                        let name = format!("/RealOutputs/Module{}/Signal{i}", i % 4);
                        div()
                            .h(px(ROW_HEIGHT))
                            .w_full()
                            .relative()
                            .child(
                                div()
                                    .absolute()
                                    .left(px(35.0))
                                    .top(px(5.0))
                                    .child(Text::new(name, label).color(0x222222)),
                            )
                            .child(
                                div().absolute().right(px(12.0)).top(px(0.0)).child(
                                    Text::new(format!("{:.3}", i as f64 * 0.137), value)
                                        .color(0x222222),
                                ),
                            )
                    })
                    .collect()
            })
            .track_scroll(&self.scroll)
            .size_full(),
        )
    }
}

fn open(capture: &mut HeadlessCapture, scroll: &UniformListScrollHandle) -> AnyWindowHandle {
    let label = TextStyle::new(FontSpec::parse("15px system-ui").unwrap());
    let mut value = TextStyle::new(FontSpec::parse("15px Courier, monospace").unwrap());
    value.line_height = LineHeight::Px(30.0);
    let scroll = scroll.clone();
    capture
        .open_window(size(px(1100.0), px(VIEWPORT)), SCALE, move |_, cx| {
            cx.new(|_| List {
                scroll,
                label,
                value,
            })
        })
        .expect("open window")
        .into()
}

fn scroll_to(scroll: &UniformListScrollHandle, y: f32) {
    scroll
        .0
        .borrow()
        .base_handle
        .set_offset(point(px(0.0), px(-y)));
}

fn draw(capture: &mut HeadlessCapture, window: AnyWindowHandle) -> Duration {
    capture
        .context()
        .update_window(window, |_, window, cx| {
            window.refresh();
            let start = Instant::now();
            window.draw(cx).clear(cx);
            start.elapsed()
        })
        .expect("draw")
}

fn stats(capture: &mut HeadlessCapture, window: AnyWindowHandle) -> CacheStats {
    let stats = capture
        .context()
        .update(|cx| TextSystem::stats(cx, window.window_id()));
    assert!(stats.entries <= CACHE_ENTRIES, "{stats:?}");
    assert!(stats.bytes <= CACHE_BYTES, "{stats:?}");
    stats
}

fn shot_at(y: f32) -> Capture {
    let mut capture = HeadlessCapture::new(Arc::new(())).expect("headless context");
    let scroll = UniformListScrollHandle::new();
    let window = open(&mut capture, &scroll);
    scroll_to(&scroll, y);
    capture.capture(window).expect("capture")
}

fn percentile(sorted: &[Duration], q: f64) -> Duration {
    sorted[((sorted.len() - 1) as f64 * q) as usize]
}

fn main() {
    if asrs_parity::ci::skip_in_ci("text scroll", "a GPU") {
        return;
    }
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        println!("text scroll: skipped, no headless renderer on this platform");
        return;
    }
    let mut capture = HeadlessCapture::new(Arc::new(())).expect("headless context");
    let scroll = UniformListScrollHandle::new();
    let window = open(&mut capture, &scroll);
    capture.capture(window).expect("first frame");

    let end = ROWS as f32 * ROW_HEIGHT - VIEWPORT;
    let mut y = 0.0;
    let mut jumps = Vec::new();
    while y < end {
        y = (y + VIEWPORT).min(end);
        scroll_to(&scroll, y);
        jumps.push(draw(&mut capture, window));
        stats(&mut capture, window);
    }
    let swept = stats(&mut capture, window);
    assert!(swept.evicted > 0, "10k rows evicted nothing: {swept:?}");
    assert!(swept.rasterized >= 2 * ROWS as u64 - 100, "{swept:?}");
    let after = capture.capture(window).expect("capture after eviction");
    assert!(
        after == shot_at(y),
        "a frame drawn after eviction differs from a fresh one"
    );
    jumps.sort();
    println!(
        "text scroll: a page per frame over {ROWS} rows, draw p50 {:?} p95 {:?}; cache {} runs, {} KiB, {} rasterized, {} evicted",
        percentile(&jumps, 0.5),
        percentile(&jumps, 0.95),
        swept.entries,
        swept.bytes / 1024,
        swept.rasterized,
        swept.evicted
    );

    if cfg!(debug_assertions) {
        println!("text scroll: frame budget not checked in a debug build");
        return;
    }
    let mut best: Option<Vec<Duration>> = None;
    for attempt in 0..ATTEMPTS {
        let mut times = Vec::with_capacity(SCROLL_FRAMES);
        let start = (attempt * SCROLL_FRAMES) as f32 * SCROLL_STEP;
        for frame in 0..SCROLL_FRAMES {
            scroll_to(&scroll, start + frame as f32 * SCROLL_STEP);
            times.push(draw(&mut capture, window));
            stats(&mut capture, window);
        }
        times.sort();
        let p95 = percentile(&times, 0.95);
        if best.as_ref().is_none_or(|b| p95 < percentile(b, 0.95)) {
            best = Some(times);
        }
        if p95 <= FRAME_BUDGET {
            break;
        }
    }
    let times = best.expect("a timed pass");
    let (p50, p95, max) = (
        percentile(&times, 0.5),
        percentile(&times, 0.95),
        percentile(&times, 1.0),
    );
    println!(
        "text scroll: {SCROLL_FRAMES} frames at {SCROLL_STEP} px, draw p50 {p50:?} p95 {p95:?} max {max:?}"
    );
    assert!(
        p95 <= FRAME_BUDGET,
        "draw p95 {p95:?} over {FRAME_BUDGET:?}"
    );
}
