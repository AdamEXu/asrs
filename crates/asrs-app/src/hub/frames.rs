//! When the page's animation frames run. They are the window's own frames while it has any, and
//! run on a timer at the display's rate otherwise: upstream opens hubs with
//! `backgroundThrottling: false`, so a hub that is covered, minimized or not shown yet keeps its
//! `requestAnimationFrame` rate (Electron 39, macOS), where GPUI's platforms stop asking for
//! frames.

use std::time::{Duration, Instant};

use asrs_ui::gpui::{AnyWindowHandle, Context, Subscription, Task, WeakEntity, Window};

use super::Hub;

/// The rate of a display that does not report one, and of a page without a window.
pub(super) const FRAME_PERIOD: Duration = Duration::from_micros(16_667);

/// A window that says it is visible has stopped giving frames when one has been due for two of
/// these. One alone can end late, ahead of the frame, after the foreground was busy.
const WATCH_PERIOD: Duration = Duration::from_millis(100);

pub(super) struct Frames {
    window: Option<AnyWindowHandle>,
    period: Duration,
    hidden: bool,
    starved: bool,
    /// A frame was requested from the window and has not come.
    requested: bool,
    running: bool,
    watches: u32,
    last: Instant,
    due: Instant,
    timer: Option<Task<()>>,
    watch: Option<Task<()>>,
    _observers: Vec<Subscription>,
}

impl Frames {
    pub(super) fn new(window: Option<&Window>, cx: &mut Context<'_, Hub>) -> Self {
        let now = cx.background_executor().now();
        let mut frames = Frames {
            window: window.map(Window::window_handle),
            period: FRAME_PERIOD,
            hidden: true,
            starved: false,
            requested: false,
            running: false,
            watches: 0,
            last: now,
            due: now + FRAME_PERIOD,
            timer: None,
            watch: None,
            _observers: Vec::new(),
        };
        if let Some(window) = window {
            let this = cx.weak_entity();
            let display = this.clone();
            frames._observers = vec![
                window.observe_window_visibility(move |visibility, _, cx| {
                    let _ = this.update(cx, |hub, cx| {
                        hub.window_visibility(visibility.is_visible(), cx)
                    });
                }),
                window.observe_window_display(move |window, cx| {
                    let period = window.refresh_interval(cx);
                    let _ = display.update(cx, |hub, _| {
                        hub.host.frames.period = period.unwrap_or(FRAME_PERIOD)
                    });
                }),
            ];
            frames.period = window.refresh_interval(cx).unwrap_or(FRAME_PERIOD);
            frames.hidden = !window.is_visible();
            if !frames.hidden {
                frames.watch = Some(watch(cx));
            }
        }
        frames
    }

    /// Arranges for the page's next frame.
    pub(super) fn schedule(&mut self, cx: &mut Context<'_, Hub>) {
        if self.running {
            return;
        }
        if !self.hidden {
            self.request(cx);
        }
        if self.hidden || self.starved {
            self.arm(cx);
        }
    }

    fn request(&mut self, cx: &mut Context<'_, Hub>) {
        let Some(window) = self.window else {
            return;
        };
        if std::mem::replace(&mut self.requested, true) {
            return;
        }
        let this = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, _| {
                window.on_next_frame(move |_, cx| {
                    let _ = this.update(cx, |hub, cx| hub.window_frame(cx));
                });
            });
        });
    }

    /// Like a display's, frames missed while the foreground was busy are skipped, so ones that
    /// take longer than their period still leave time for other tasks.
    fn arm(&mut self, cx: &Context<'_, Hub>) {
        let executor = cx.background_executor();
        let now = executor.now();
        if self.due <= now {
            self.due = next_frame(self.due, now, self.period);
        }
        let timer = executor.timer(self.due - now);
        self.timer = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |hub, cx| hub.timer_frame(cx));
        }));
    }

    /// Takes the frames over from the window.
    fn start_timer(&mut self, cx: &Context<'_, Hub>) {
        self.due = self.last + self.period;
        self.arm(cx);
    }
}

/// The first frame after `now` of those every `period` from `due`.
fn next_frame(due: Instant, now: Instant, period: Duration) -> Instant {
    let late = now.saturating_duration_since(due).as_nanos() % period.as_nanos();
    now + period - Duration::from_nanos(late as u64)
}

fn watch(cx: &Context<'_, Hub>) -> Task<()> {
    cx.spawn(async move |this: WeakEntity<Hub>, cx| {
        loop {
            cx.background_executor().timer(WATCH_PERIOD).await;
            if this.update(cx, |hub, cx| hub.watch_frames(cx)).is_err() {
                break;
            }
        }
    })
}

impl Hub {
    fn window_frame(&mut self, cx: &mut Context<'_, Self>) {
        let now = cx.background_executor().now();
        let frames = &mut self.host.frames;
        frames.requested = false;
        frames.watches = 0;
        if frames.hidden {
            return;
        }
        if std::mem::take(&mut frames.starved) {
            frames.timer = None;
            if now.saturating_duration_since(frames.last) < frames.period / 2 {
                frames.request(cx);
                return;
            }
        }
        self.run_frame(now, true, cx);
    }

    fn run_frame(&mut self, now: Instant, drawn: bool, cx: &mut Context<'_, Self>) {
        self.host.frames.last = now;
        self.host.frames.running = true;
        self.frame(cx);
        self.host.frames.running = false;
        if self.host.frame_requested() {
            self.host.frames.schedule(cx);
        }
        let changed = self.model.take_frame_changed();
        self.moving = changed || self.repaint.settling();
        let Some(window) = self.host.frames.window else {
            cx.notify();
            return;
        };
        let this = cx.weak_entity();
        // After the frame's other callbacks: what they invalidate is drawn in this frame.
        cx.defer(move |cx| {
            let dirty = window.update(cx, |_, window, _| window.is_dirty());
            let changed = changed || drawn && dirty.unwrap_or(false);
            let _ = this.update(cx, |hub, cx| {
                if hub.repaint.frame(changed) {
                    cx.notify();
                }
            });
        });
    }

    fn timer_frame(&mut self, cx: &mut Context<'_, Self>) {
        let now = cx.background_executor().now();
        self.host.frames.timer = None;
        self.run_frame(now, false, cx);
    }

    fn window_visibility(&mut self, visible: bool, cx: &mut Context<'_, Self>) {
        let waiting = self.host.frame_requested();
        if visible {
            self.repaint.changed();
        }
        let frames = &mut self.host.frames;
        frames.hidden = !visible;
        frames.starved = false;
        frames.watches = 0;
        frames.timer = None;
        frames.watch = visible.then(|| watch(cx));
        match (waiting, visible) {
            (true, true) => frames.request(cx),
            (true, false) => frames.start_timer(cx),
            (false, _) => {}
        }
    }

    fn watch_frames(&mut self, cx: &mut Context<'_, Self>) {
        let frames = &mut self.host.frames;
        if frames.starved || !frames.requested {
            return;
        }
        frames.watches += 1;
        if frames.watches > 1 {
            frames.starved = true;
            frames.start_timer(cx);
        }
    }
}
