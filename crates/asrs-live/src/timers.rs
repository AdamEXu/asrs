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
//!   it fires once and the next fire returns to its grid.
//!
//! Blink's nesting clamp is reduced to one rule, that intervals repeat no faster than every
//! 4 ms; upstream's live sources never use delays below 16 ms.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

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
}

pub struct TimerQueue {
    clock: Rc<dyn Clock>,
    next_id: u32,
    timers: Vec<Timer>,
}

const MIN_REPEAT_MS: f64 = 4.0;

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
        }
    }

    pub fn now(&self) -> f64 {
        self.clock.now()
    }

    fn add(&mut self, delay: f64, repeat: bool) -> TimerId {
        let id = TimerId(self.next_id);
        self.next_id += 1;
        let delay = webidl_delay(delay);
        self.timers.push(Timer {
            id,
            due: self.clock.now() + delay,
            repeat: repeat.then_some(if delay < MIN_REPEAT_MS {
                MIN_REPEAT_MS
            } else {
                delay
            }),
        });
        id
    }

    pub fn set_timeout(&mut self, delay: f64) -> TimerId {
        self.add(delay, false)
    }

    pub fn set_interval(&mut self, delay: f64) -> TimerId {
        self.add(delay, true)
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
        match timer.repeat {
            Some(interval) => timer.due = now + (interval - (now - timer.due) % interval),
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
