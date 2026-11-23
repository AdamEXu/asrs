//! `www/loadingAnimation.css`: five bars that stretch one after the other, once a second.

use asrs_model::scroll_sensor::ease_in_out;
use asrs_ui::gpui::{Div, ParentElement, Styled, div, px, rgb};

const BARS: usize = 5;
const BAR_WIDTH: f64 = 4.0;
const BAR_HEIGHT: f64 = 18.0;
const BAR_RADIUS: f32 = 2.0;
const COLORS: [u32; 2] = [0x303dc9, 0xa8afd3];
/// The space between two inline blocks, and their line's height.
const SPACE: f64 = 4.1875;
const LINE_HEIGHT: f64 = 21.5;

const PERIOD_MS: f64 = 1000.0;
const DELAY_MS: f64 = 90.0;
const PEAK: f64 = 2.2;

/// The vertical scale of each bar `elapsed_ms` after the animation started.
pub fn scales(elapsed_ms: f64) -> [f64; BARS] {
    std::array::from_fn(|i| {
        let active = elapsed_ms - i as f64 * DELAY_MS;
        if active < 0.0 {
            return 1.0;
        }
        match (active / PERIOD_MS).fract() {
            p if p < 0.2 => 1.0 + (PEAK - 1.0) * ease_in_out(p / 0.2),
            p if p < 0.4 => PEAK + (1.0 - PEAK) * ease_in_out((p - 0.2) / 0.2),
            _ => 1.0,
        }
    })
}

/// `[x, y, width, height]` of `div.loading` in a list of this size.
pub fn container(width: f64, height: f64) -> [f64; 4] {
    let total = BARS as f64 * BAR_WIDTH + (BARS - 1) as f64 * SPACE;
    let (left, top) = ((width - total) / 2.0, (height - LINE_HEIGHT) / 2.0);
    [left, top, total, LINE_HEIGHT]
}

/// Each bar's unscaled `[x, y, width, height]` in a list of this size.
pub fn bars(width: f64, height: f64) -> [[f64; 4]; BARS] {
    let [left, top, ..] = container(width, height);
    std::array::from_fn(|i| {
        let x = left + i as f64 * (BAR_WIDTH + SPACE);
        [x, top, BAR_WIDTH, BAR_HEIGHT]
    })
}

/// The animation, which starts over each time the element stops being hidden.
#[derive(Default)]
pub struct Loading {
    since_ms: Option<f64>,
}

impl Loading {
    pub fn stop(&mut self) {
        self.since_ms = None;
    }

    pub fn bars(&mut self, width: f64, height: f64, now_ms: f64) -> Div {
        let since = *self.since_ms.get_or_insert(now_ms);
        let scales = scales(now_ms - since);
        let mut out = div().absolute().size_full().top_0().left_0();
        for (i, ([x, y, w, h], scale)) in bars(width, height).into_iter().zip(scales).enumerate() {
            let stretched = h * scale;
            out = out.child(
                div()
                    .absolute()
                    .left(px(x as f32))
                    .top(px((y - (stretched - h) / 2.0) as f32))
                    .w(px(w as f32))
                    .h(px(stretched as f32))
                    .rounded(px(BAR_RADIUS))
                    .bg(rgb(COLORS[i % COLORS.len()])),
            );
        }
        out
    }
}
