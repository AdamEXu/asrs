//! `src/hub/ScrollSensor.ts` without its DOM. Upstream turns wheel input into scroll deltas
//! by letting Chromium scroll a hidden 1000000 px overlay and reading back how far it moved on
//! each `scroll` event. Here the overlay is just its scroll position: wheel deltas (Chromium's
//! `WebMouseWheelEvent` pixels, from the platform wheel mapping) move it the way Chromium
//! moves a scroller, and the frame's scroll step reports the movement to the callback.
//!
//! What follows Chromium, measured on Electron 39 (macOS): the offset accumulates unsnapped
//! and clamps to the overlay's range, and `scrollLeft`/`scrollTop` read it snapped to the
//! device pixel grid; precise (trackpad) deltas move the
//! position at once, and every change within one frame reaches the page as one `scroll`
//! event; a notched wheel tick moves at most the overlay's client size along each axis and
//! animates to its target with an ease-in-out curve. The animation's per-frame steps depend on
//! real compositor timing upstream, so only where it settles is reproducible
//! (`oracle/tools/wheelCalibration.cjs`, `tests/wheel_calibration.rs`).
//!
//! Every `now` here is on one monotonic frame clock (ms), a wheel event's arrival and each
//! frame's step alike: cc runs the animation on frame time, and the idle reset, which upstream
//! times with `Date`, reads the same clock so an animation and the reset cannot disagree.
//!
//! The drag pan (`mouseControl`) is not here: it needs no overlay, and each owner (the
//! timeline, line graph and 2D field) handles it with its own pointer events.

use asrs_js::math;

/// The overlay child's width and height (`SIZE_PX`).
pub const SIZE_PX: f64 = 1_000_000.0;
const RESET_MS: f64 = 1000.0;

/// `cc::ScrollOffsetAnimationCurve`'s inverse-delta duration, in 60 Hz frames: 12 frames up
/// to a 120 px delta, falling linearly to 6 frames at 480 px.
const RAMP_START_PX: f64 = 120.0;
const RAMP_END_PX: f64 = 480.0;
const MIN_FRAMES: f64 = 6.0;
const MAX_FRAMES: f64 = 12.0;

/// One wheel event in Chromium's `WebMouseWheelEvent` terms. `delta` is `delta_x`, `delta_y`:
/// positive scrolls content toward the start, so the overlay's `scrollLeft` and `scrollTop`
/// move by its negation (the DOM `deltaX`, `deltaY`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wheel {
    pub delta: [f64; 2],
    /// `hasPreciseScrollingDeltas`: a trackpad or other continuous device.
    pub precise: bool,
}

/// The `dx`, `dy` of one non-pan callback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollDelta {
    pub dx: f64,
    pub dy: f64,
}

#[derive(Clone, Copy, Debug)]
struct Animation {
    from: [f64; 2],
    to: [f64; 2],
    start: f64,
    duration: f64,
}

/// The `scrollLeft` or `scrollTop` a scroller ends up at when `value` is assigned or scrolled
/// to: clamped to `[0, max]`, then snapped to the device pixel grid (halves round up).
pub fn assign_scroll_offset(value: f64, max: f64, device_pixel_ratio: f64) -> f64 {
    let clamped = math::min2(math::max2(value, 0.0), math::max2(max, 0.0));
    math::round(clamped * device_pixel_ratio) / device_pixel_ratio
}

fn duration_ms(delta: [f64; 2]) -> f64 {
    let distance = math::max2(math::abs(delta[0]), math::abs(delta[1]));
    let slope = (MIN_FRAMES - MAX_FRAMES) / (RAMP_END_PX - RAMP_START_PX);
    let offset = MAX_FRAMES - RAMP_START_PX * slope;
    let frames = math::min2(
        math::max2(offset + distance * slope, MIN_FRAMES),
        MAX_FRAMES,
    );
    frames / 60.0 * 1000.0
}

/// `cubic-bezier(0.42, 0, 0.58, 1)` at `t`.
fn ease_in_out(t: f64) -> f64 {
    let (x1, y1, x2, y2) = (0.42, 0.0, 0.58, 1.0);
    let bezier = |a: f64, b: f64, s: f64| {
        let r = 1.0 - s;
        3.0 * r * r * s * a + 3.0 * r * s * s * b + s * s * s
    };
    let (mut lo, mut hi) = (0.0, 1.0);
    let mut s = t;
    for _ in 0..64 {
        let x = bezier(x1, x2, s);
        if math::abs(x - t) < 1e-9 {
            break;
        }
        if x < t {
            lo = s;
        } else {
            hi = s;
        }
        s = (lo + hi) / 2.0;
    }
    bezier(y1, y2, s)
}

/// How Chromium turns wheel input over a scroller into scroll offsets.
#[derive(Clone, Debug, Default)]
pub struct WheelScroll(Option<Animation>);

impl WheelScroll {
    pub fn animating(&self) -> bool {
        self.0.is_some()
    }

    pub fn stop(&mut self) {
        self.0 = None;
    }

    /// A wheel event at `now` (ms) over a scroller at `offset` with a `client_size` box: the
    /// offset a precise event scrolls to at once, or nothing when a notched one started an
    /// animation.
    pub fn wheel(
        &mut self,
        wheel: Wheel,
        now: f64,
        offset: [f64; 2],
        client_size: [f64; 2],
    ) -> Option<[f64; 2]> {
        let delta = [-wheel.delta[0], -wheel.delta[1]];
        if wheel.precise {
            self.0 = None;
            return Some([offset[0] + delta[0], offset[1] + delta[1]]);
        }
        let capped = [0, 1].map(|axis| {
            let size = client_size[axis];
            math::min2(math::max2(delta[axis], -size), size)
        });
        let base = self.0.map_or(offset, |a| a.to);
        let to = [base[0] + capped[0], base[1] + capped[1]];
        self.0 = Some(Animation {
            from: offset,
            to,
            start: now,
            duration: duration_ms([to[0] - offset[0], to[1] - offset[1]]),
        });
        None
    }

    /// Where the animation has the scroller at the frame at `now` (ms).
    pub fn animate(&mut self, now: f64) -> Option<[f64; 2]> {
        let a = self.0?;
        let t = if a.duration > 0.0 {
            math::min2(math::max2((now - a.start) / a.duration, 0.0), 1.0)
        } else {
            1.0
        };
        if t >= 1.0 {
            self.0 = None;
            return Some(a.to);
        }
        let p = ease_in_out(t);
        Some([
            a.from[0] + (a.to[0] - a.from[0]) * p,
            a.from[1] + (a.to[1] - a.from[1]) * p,
        ])
    }
}

#[derive(Clone, Debug)]
pub struct ScrollSensor {
    raw: [f64; 2],
    position: [f64; 2],
    client_size: [f64; 2],
    device_pixel_ratio: f64,
    animation: WheelScroll,
    scroll_event: bool,
    last_scroll_update: f64,
    reset_next: bool,
    last_scroll: [f64; 2],
}

impl Default for ScrollSensor {
    fn default() -> Self {
        ScrollSensor {
            raw: [0.0, 0.0],
            position: [0.0, 0.0],
            client_size: [0.0, 0.0],
            device_pixel_ratio: 1.0,
            animation: WheelScroll::default(),
            scroll_event: false,
            last_scroll_update: 0.0,
            reset_next: true,
            last_scroll: [0.0, 0.0],
        }
    }
}

impl ScrollSensor {
    /// The overlay's `clientWidth`, `clientHeight` and the window's `devicePixelRatio`, which
    /// bound and snap its scroll position.
    pub fn set_layout(&mut self, client_size: [f64; 2], device_pixel_ratio: f64) {
        self.client_size = client_size;
        self.device_pixel_ratio = device_pixel_ratio;
    }

    /// The overlay's `scrollLeft` and `scrollTop`.
    pub fn position(&self) -> [f64; 2] {
        self.position
    }

    /// Whether a notched wheel animation is still moving the overlay, so the host keeps
    /// producing frames.
    pub fn animating(&self) -> bool {
        self.animation.animating()
    }

    fn scroll_to(&mut self, target: [f64; 2]) {
        let mut next = self.position;
        for axis in 0..2 {
            let max = math::max2(SIZE_PX - self.client_size[axis], 0.0);
            self.raw[axis] = math::min2(math::max2(target[axis], 0.0), max);
            next[axis] = assign_scroll_offset(self.raw[axis], max, self.device_pixel_ratio);
        }
        if next != self.position {
            self.position = next;
            self.scroll_event = true;
        }
    }

    /// Assigns the overlay's `scrollLeft` and `scrollTop`, which stops any wheel animation.
    pub fn assign(&mut self, position: [f64; 2]) {
        self.animation.stop();
        self.scroll_to(position);
    }

    /// A wheel event over the overlay at `now` (ms).
    pub fn wheel(&mut self, wheel: Wheel, now: f64) {
        if let Some(to) = self.animation.wheel(wheel, now, self.raw, self.client_size) {
            self.scroll_to(to);
        }
    }

    /// Advances a notched wheel animation to the frame at `now` (ms).
    pub fn animate(&mut self, now: f64) {
        if let Some(to) = self.animation.animate(now) {
            self.scroll_to(to);
        }
    }

    /// The frame's scroll step: the overlay's `scroll` event, if its position changed since
    /// the last one, run through `update()`. `visible` is whether the overlay has a box
    /// (`offsetWidth` or `offsetHeight` non-zero). Returns the callback's deltas when upstream
    /// calls it.
    pub fn scroll_step(&mut self, now: f64, visible: bool) -> Option<ScrollDelta> {
        if !std::mem::take(&mut self.scroll_event) {
            return None;
        }
        self.last_scroll_update = now;
        if self.reset_next {
            self.reset_next = false;
            self.reset();
        }
        if !visible {
            self.reset_next = true;
            return None;
        }
        let dx = self.position[0] - self.last_scroll[0];
        let dy = self.position[1] - self.last_scroll[1];
        self.last_scroll = self.position;
        Some(ScrollDelta { dx, dy })
    }

    /// `periodic()` at `now` (ms): recentres the overlay once scrolling has been idle for a
    /// second, and on the first call.
    pub fn periodic(&mut self, now: f64) {
        if self.reset_next || now - self.last_scroll_update > RESET_MS {
            self.reset_next = false;
            self.reset();
        }
    }

    fn reset(&mut self) {
        let middle = SIZE_PX / 2.0;
        self.animation.stop();
        self.scroll_to([middle, middle]);
        self.last_scroll = [middle, middle];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notch(lines: f64) -> Wheel {
        Wheel {
            delta: [0.0, lines * 40.0],
            precise: false,
        }
    }

    #[test]
    fn notched_tick_settles_on_its_target_through_eased_steps() {
        let mut s = ScrollSensor::default();
        s.set_layout([400.0, 40.0], 2.0);
        s.periodic(0.0);
        assert_eq!(
            s.scroll_step(0.0, true),
            Some(ScrollDelta { dx: 0.0, dy: 0.0 })
        );
        s.wheel(notch(-1.0), 0.0);
        let mut steps = Vec::new();
        let mut now = 0.0;
        while s.animating() {
            now += 1000.0 / 60.0;
            s.animate(now);
            if let Some(d) = s.scroll_step(now, true) {
                steps.push(d.dy);
            }
        }
        assert_eq!(steps.iter().sum::<f64>(), 40.0);
        assert!(steps.len() > 4);
        assert!(steps[0] < steps[steps.len() / 2]);
        assert!(steps.iter().all(|d| (d * 2.0).fract() == 0.0));
    }

    #[test]
    fn idle_reset_reports_a_zero_scroll_once() {
        let mut s = ScrollSensor::default();
        s.set_layout([400.0, 40.0], 1.0);
        s.periodic(0.0);
        assert!(s.scroll_step(16.0, true).is_some());
        s.wheel(
            Wheel {
                delta: [0.0, -10.0],
                precise: true,
            },
            20.0,
        );
        assert_eq!(
            s.scroll_step(32.0, true),
            Some(ScrollDelta { dx: 0.0, dy: 10.0 })
        );
        s.periodic(1032.0);
        assert_eq!(s.scroll_step(1048.0, true), None);
        s.periodic(1033.0);
        assert_eq!(
            s.scroll_step(1048.0, true),
            Some(ScrollDelta { dx: 0.0, dy: 0.0 })
        );
        s.periodic(1060.0);
        assert_eq!(s.scroll_step(1076.0, true), None);
    }

    #[test]
    fn hidden_overlay_drops_the_event_and_resets_on_the_next() {
        let mut s = ScrollSensor::default();
        s.set_layout([400.0, 40.0], 1.0);
        s.periodic(0.0);
        assert_eq!(s.scroll_step(16.0, false), None);
        s.wheel(
            Wheel {
                delta: [0.0, -5.0],
                precise: true,
            },
            20.0,
        );
        assert_eq!(
            s.scroll_step(32.0, true),
            Some(ScrollDelta { dx: 0.0, dy: 0.0 })
        );
    }
}
