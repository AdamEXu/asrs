//! The renderer's timer model, so live sources keep upstream's `setTimeout`/`setInterval`
//! cadence and ordering.
//!
//! It follows Blink's `DOMTimer`/`TimerBase` as measured in Electron 39's renderer:
//! - The delay is a WebIDL `long`, so it is truncated toward zero (`1000 / 60` runs every
//!   16 ms, measured mean 16.01 ms) and negative delays become 0.
//! - Timers due at the same time run in creation order. An interval keeps the position of its
//!   `setInterval` call on every repeat, because its repeats stay on the grid of its first due
//!   time, which real time puts before any timer created later with the same nominal due time.
//! - A repeating timer is rescheduled before its callback runs, drift free: when it fires late
//!   it fires once and the next fire returns to its grid, at least a 20th of its period later
//!   (one of 250 ms fired 235 ms late fires again after 15 ms, 240 ms late after 260 ms).
//!
//! - Timers carry the HTML timer nesting level. A timer created while a timer task runs at
//!   level n gets level n + 1 (one created by any other task gets 1), an interval's level rises
//!   by one per repeat, and a delay under 4 ms is raised to 4 ms when the creating task's level
//!   is above 5. In a `setInterval` callback a `setTimeout(0)` therefore runs at once on the
//!   first five repeats and 4 ms later from the sixth on (`PhoenixDiagnosticsSource` relies on
//!   this). Intervals repeat no faster than every 4 ms regardless of level.
//! - `AbortSignal.timeout`, `requestIdleCallback` and `requestAnimationFrame` are delayed tasks,
//!   not `DOMTimer`s: [`TimerQueue::post_delayed_task`], never clamped, run at level 0.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use asrs_js::math::min2;
use asrs_js::number::to_int32;
use asrs_js::time::Clock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TimerId(u32);

impl TimerId {
    pub fn get(self) -> u32 {
        self.0
    }
}

struct Timer {
    id: TimerId,
    due: f64,
    repeat: Option<f64>,
    level: u32,
}

pub struct TimerQueue {
    clock: Rc<dyn Clock>,
    next_id: u32,
    timers: Vec<Timer>,
    running_level: u32,
    idle_period: Option<f64>,
}

const MIN_DELAY_MS: f64 = 4.0;
const MAX_UNCLAMPED_LEVEL: u32 = 5;

fn webidl_delay(delay: f64) -> f64 {
    f64::from(to_int32(delay).max(0))
}

impl TimerQueue {
    /// `clock` is the queue's time base, in milliseconds. Blink schedules on a monotonic clock;
    /// scripted runs share one [`ManualClock`] between the queue and `Date.now()`.
    pub fn new(clock: Rc<dyn Clock>) -> Self {
        Self {
            clock,
            next_id: 1,
            timers: Vec::new(),
            running_level: 0,
            idle_period: None,
        }
    }

    /// The page has an idle period this often. Without one, idle callbacks run at their timeout.
    pub fn set_idle_period(&mut self, period: Option<f64>) {
        self.idle_period = period;
    }

    pub fn now(&self) -> f64 {
        self.clock.now()
    }

    fn add(&mut self, delay: f64, repeat: bool, nested: bool) -> TimerId {
        let id = TimerId(self.next_id);
        self.next_id += 1;
        let mut delay = webidl_delay(delay);
        let level = if nested {
            if self.running_level > MAX_UNCLAMPED_LEVEL && delay < MIN_DELAY_MS {
                delay = MIN_DELAY_MS;
            }
            self.running_level + 1
        } else {
            0
        };
        self.timers.push(Timer {
            id,
            due: self.clock.now() + delay,
            repeat: repeat.then_some(if delay < MIN_DELAY_MS {
                MIN_DELAY_MS
            } else {
                delay
            }),
            level,
        });
        id
    }

    pub fn set_timeout(&mut self, delay: f64) -> TimerId {
        self.add(delay, false, true)
    }

    pub fn set_interval(&mut self, delay: f64) -> TimerId {
        self.add(delay, true, true)
    }

    /// A one-shot delayed task that is not a `DOMTimer`: never clamped, and its task runs at
    /// nesting level 0. Shares the timer id space and the due-time order.
    pub fn post_delayed_task(&mut self, delay: f64) -> TimerId {
        self.add(delay, false, false)
    }

    /// `requestIdleCallback(callback, { timeout })`. A page that draws frames runs one that asks
    /// for itself again once per frame (Electron 39).
    pub fn request_idle_callback(&mut self, timeout: f64) -> TimerId {
        let delay = self.idle_period.map_or(timeout, |p| min2(p, timeout));
        self.post_delayed_task(delay)
    }

    /// Call before dispatching any task that is not a timer (a network event, an IPC message,
    /// a script step), so timers it creates start a new nesting chain.
    pub fn begin_task(&mut self) {
        self.running_level = 0;
    }

    /// `clearTimeout` and `clearInterval`, which share one id space.
    pub fn clear(&mut self, id: TimerId) {
        self.timers.retain(|t| t.id != id);
    }

    pub fn is_active(&self, id: TimerId) -> bool {
        self.timers.iter().any(|t| t.id == id)
    }

    pub fn is_empty(&self) -> bool {
        self.timers.is_empty()
    }

    fn earliest(&self) -> Option<usize> {
        self.timers
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| a.due.total_cmp(&b.due).then(a.id.cmp(&b.id)))
            .map(|(i, _)| i)
    }

    pub fn next_due(&self) -> Option<f64> {
        self.earliest().map(|i| self.timers[i].due)
    }

    /// Takes the earliest timer that is due at the clock's current time and returns its id for
    /// the caller to dispatch. Intervals are rescheduled first, so the callback may clear them.
    pub fn fire_next(&mut self) -> Option<TimerId> {
        let i = self.earliest()?;
        let now = self.clock.now();
        let timer = &mut self.timers[i];
        if timer.due > now {
            return None;
        }
        let id = timer.id;
        self.running_level = timer.level;
        match timer.repeat {
            Some(interval) => {
                let late = now - timer.due + interval / 20.0;
                timer.due += (late / interval).ceil() * interval;
                timer.level += 1;
            }
            None => {
                self.timers.remove(i);
            }
        }
        Some(id)
    }

    /// Scripted time: moves `clock` forward to `target`, stopping at every due time on the way
    /// so each callback sees the clock at its own due time, as on an idle renderer thread.
    pub fn advance_to(
        &mut self,
        clock: &ManualClock,
        target: f64,
        mut dispatch: impl FnMut(&mut Self, TimerId),
    ) {
        while let Some(due) = self.next_due().filter(|due| *due <= target) {
            if due > clock.now() {
                clock.set(due);
            }
            match self.fire_next() {
                Some(id) => dispatch(self, id),
                None => break,
            }
        }
        if target > clock.now() {
            clock.set(target);
        }
    }
}

/// A settable clock shared by everything that reads time in a scripted run.
#[derive(Clone, Default)]
pub struct ManualClock(Rc<Cell<f64>>);

impl ManualClock {
    pub fn new(start_ms: f64) -> Self {
        Self(Rc::new(Cell::new(start_ms)))
    }

    pub fn set(&self, ms: f64) {
        self.0.set(ms);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> f64 {
        self.0.get()
    }
}

/// Milliseconds since construction on the OS monotonic clock: the app's timer base.
pub struct MonotonicClock(Instant);

impl MonotonicClock {
    pub fn new() -> Self {
        Self(Instant::now())
    }
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for MonotonicClock {
    fn now(&self) -> f64 {
        self.0.elapsed().as_secs_f64() * 1000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(start: f64) -> (ManualClock, TimerQueue) {
        let clock = ManualClock::new(start);
        let queue = TimerQueue::new(Rc::new(clock.clone()));
        (clock, queue)
    }

    fn run(clock: &ManualClock, q: &mut TimerQueue, target: f64) -> Vec<(u32, f64)> {
        let mut fired = Vec::new();
        q.advance_to(clock, target, |_, id| fired.push((id.get(), clock.now())));
        fired
    }

    #[test]
    fn interval_delay_is_truncated_to_whole_milliseconds() {
        let (clock, mut q) = queue(1000.0);
        q.set_interval(1000.0 / 60.0);
        let fired = run(&clock, &mut q, 1050.0);
        assert_eq!(fired, vec![(1, 1016.0), (1, 1032.0), (1, 1048.0)]);
        assert_eq!(clock.now(), 1050.0);
    }

    #[test]
    fn negative_and_nan_delays_run_immediately() {
        let (clock, mut q) = queue(0.0);
        q.set_timeout(-5.0);
        q.set_timeout(f64::NAN);
        assert_eq!(run(&clock, &mut q, 0.0), vec![(1, 0.0), (2, 0.0)]);
        assert!(q.is_empty());
    }

    #[test]
    fn equal_due_times_run_in_creation_order_and_intervals_keep_theirs() {
        let (clock, mut q) = queue(0.0);
        q.set_timeout(20.0);
        q.set_interval(10.0);
        q.set_timeout(20.0);
        q.set_timeout(25.0);
        let fired = run(&clock, &mut q, 25.0);
        assert_eq!(
            fired,
            vec![(2, 10.0), (1, 20.0), (2, 20.0), (3, 20.0), (4, 25.0)]
        );
    }

    #[test]
    fn late_interval_fires_once_and_returns_to_its_grid() {
        let (clock, mut q) = queue(0.0);
        let id = q.set_interval(16.0);
        clock.set(119.0);
        assert_eq!(q.fire_next(), Some(id));
        assert_eq!(q.fire_next(), None);
        assert_eq!(q.next_due(), Some(128.0));
    }

    /// Recorded in Electron 39 with a blocked renderer thread.
    #[test]
    fn interval_fired_at_the_end_of_its_period_skips_the_next_one() {
        for (period, late, next) in [
            (250.0, 0.0, 250.0),
            (250.0, 200.0, 250.0),
            (250.0, 235.0, 250.0),
            (250.0, 240.0, 500.0),
            (250.0, 248.0, 500.0),
            (250.0, 252.0, 500.0),
            (250.0, 480.0, 500.0),
            (250.0, 495.0, 750.0),
            (50.0, 46.0, 50.0),
            (50.0, 48.5, 100.0),
            (50.0, 60.0, 100.0),
        ] {
            let (clock, mut q) = queue(0.0);
            let id = q.set_interval(period);
            clock.set(period + late);
            assert_eq!(q.fire_next(), Some(id));
            assert_eq!(q.next_due(), Some(period + next), "{period}, {late} late");
        }
    }

    #[test]
    fn short_delays_clamp_to_4_ms_past_nesting_level_5() {
        let (clock, mut q) = queue(0.0);
        let interval = q.set_interval(50.0);
        let mut zero_delays = Vec::new();
        q.advance_to(&clock, 410.0, |q, id| {
            if id == interval {
                q.set_timeout(0.0);
            } else {
                zero_delays.push(clock.now() % 50.0);
            }
        });
        assert_eq!(zero_delays, vec![0.0, 0.0, 0.0, 0.0, 0.0, 4.0, 4.0, 4.0]);

        let (clock, mut q) = queue(0.0);
        q.set_timeout(0.0);
        let mut fired = Vec::new();
        q.advance_to(&clock, 100.0, |q, _| {
            fired.push(clock.now());
            if fired.len() < 8 {
                q.set_timeout(0.0);
            }
        });
        assert_eq!(fired, vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 4.0, 8.0]);
    }

    #[test]
    fn delayed_tasks_and_other_tasks_restart_the_nesting_chain() {
        let (clock, mut q) = queue(0.0);
        let interval = q.set_interval(10.0);
        let mut fired = Vec::new();
        let mut task = None;
        q.advance_to(&clock, 75.0, |q, id| {
            if id == interval && clock.now() == 70.0 {
                task = Some(q.post_delayed_task(0.0));
            } else if Some(id) == task {
                fired.push(clock.now());
                q.set_timeout(0.0);
                q.clear(interval);
            } else if id != interval {
                fired.push(clock.now());
            }
        });
        assert_eq!(fired, vec![70.0, 70.0]);

        let (clock, mut q) = queue(0.0);
        let interval = q.set_interval(10.0);
        q.advance_to(&clock, 60.0, |_, _| {});
        q.clear(interval);
        q.begin_task();
        q.set_timeout(0.0);
        assert_eq!(q.next_due(), Some(60.0));
    }

    #[test]
    fn clearing_inside_a_callback_stops_an_interval() {
        let (clock, mut q) = queue(0.0);
        q.set_interval(10.0);
        let mut fired = 0;
        q.advance_to(&clock, 100.0, |q, id| {
            fired += 1;
            q.clear(id);
        });
        assert_eq!(fired, 1);
        assert!(q.is_empty());
    }
}
