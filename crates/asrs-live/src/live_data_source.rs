//! Port of `src/hub/dataSources/LiveDataSource.ts`.
//!
//! Upstream's subclasses extend the abstract class; here each source embeds a
//! [`LiveDataSourceBase`] and implements [`LiveDataSource`]. Timers come from the hub's
//! [`TimerQueue`], which hands fired ids back through [`LiveDataSource::handle_timer`].

use std::cell::RefCell;
use std::rc::Rc;

use asrs_js::math;
use asrs_js::time::{Rng, create_uuid};
use asrs_js::value::JsValue;
use asrs_log::log::Log;

use crate::live_data_tuner::LiveDataTuner;
use crate::timers::{TimerId, TimerQueue};

/// The source's `Log`, shared with the hub the way upstream hands the same object to
/// `window.log`. Only the foreground thread touches it.
pub type LiveLog = Rc<RefCell<Log>>;

/// Returns the current live timestamp in seconds.
pub type TimeSupplier = Rc<dyn Fn() -> f64>;

pub type StatusCallback = Box<dyn FnMut(LiveDataSourceStatus)>;

/// Receives the source's current `log` field, which upstream's subclasses only pass once set.
pub type OutputCallback = Box<dyn FnMut(Option<LiveLog>, TimeSupplier)>;

/// `1000 / 60`; the timer queue truncates it to 16 ms as Blink does.
pub const DISCARD_PERIOD_MS: f64 = 1000.0 / 60.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum LiveDataSourceStatus {
    Waiting,
    Connecting,
    Active,
    Error,
    Stopped,
}

impl LiveDataSourceStatus {
    pub fn from_index(index: u8) -> Option<Self> {
        Some(match index {
            0 => Self::Waiting,
            1 => Self::Connecting,
            2 => Self::Active,
            3 => Self::Error,
            4 => Self::Stopped,
            _ => return None,
        })
    }
}

/// The preference values live sources read from `window.preferences`. `None` for a field is
/// an `undefined` property.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LivePreferences {
    pub live_discard: Option<f64>,
}

/// What a source may touch besides itself: the hub's timers and `window.preferences`, which is
/// `undefined` (`None`) until the hub has loaded preferences.
pub struct LiveContext<'a> {
    pub timers: &'a mut TimerQueue,
    pub preferences: Option<&'a LivePreferences>,
}

pub struct LiveDataSourceBase {
    uuid: String,
    status: LiveDataSourceStatus,
    log: Option<LiveLog>,
    address: Option<String>,
    status_callback: Option<StatusCallback>,
    output_callback: Option<OutputCallback>,
    clear_data_callback: Option<TimerId>,
    discard_timers: Vec<TimerId>,
    time_supplier: Option<TimeSupplier>,
}

impl LiveDataSourceBase {
    pub fn new(rng: &mut dyn Rng) -> Self {
        Self {
            uuid: create_uuid(rng),
            status: LiveDataSourceStatus::Waiting,
            log: None,
            address: None,
            status_callback: None,
            output_callback: None,
            clear_data_callback: None,
            discard_timers: Vec::new(),
            time_supplier: None,
        }
    }

    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    pub fn status(&self) -> LiveDataSourceStatus {
        self.status
    }

    pub fn address(&self) -> Option<&str> {
        self.address.as_deref()
    }

    pub fn log(&self) -> Option<&LiveLog> {
        self.log.as_ref()
    }

    pub fn set_log(&mut self, log: Option<LiveLog>) {
        self.log = log;
    }

    pub fn has_output_callback(&self) -> bool {
        self.output_callback.is_some()
    }

    pub fn connect(
        &mut self,
        address: &str,
        status_callback: StatusCallback,
        output_callback: OutputCallback,
        enable_clear_data: bool,
        timers: &mut TimerQueue,
    ) {
        self.address = Some(address.to_owned());
        self.status_callback = Some(status_callback);
        self.output_callback = Some(output_callback);
        self.set_status(LiveDataSourceStatus::Connecting);

        if enable_clear_data {
            // A second connect leaves the first interval running, as upstream's does.
            let id = timers.set_interval(DISCARD_PERIOD_MS);
            self.clear_data_callback = Some(id);
            self.discard_timers.push(id);
        }
    }

    pub fn stop(&mut self, timers: &mut TimerQueue) {
        self.set_status(LiveDataSourceStatus::Stopped);
        if let Some(id) = self.clear_data_callback {
            timers.clear(id);
        }
    }

    pub fn set_status(&mut self, status: LiveDataSourceStatus) {
        if status != self.status && self.status != LiveDataSourceStatus::Stopped {
            self.status = status;
            if let Some(callback) = self.status_callback.as_mut() {
                callback(status);
            }
        }
        if status != LiveDataSourceStatus::Active {
            self.time_supplier = None;
        }
    }

    /// `this.outputCallback(this.log, timeSupplier)` behind the `outputCallback !== null` check
    /// every subclass makes. Returns whether a callback ran.
    pub fn output(&mut self, time_supplier: TimeSupplier) -> bool {
        let Some(callback) = self.output_callback.as_mut() else {
            return false;
        };
        self.time_supplier = Some(time_supplier.clone());
        callback(self.log.clone(), time_supplier);
        true
    }

    /// Runs the discard interval if `id` is one this base created. Returns whether it was.
    pub fn handle_timer(&mut self, id: TimerId, preferences: Option<&LivePreferences>) -> bool {
        if !self.discard_timers.contains(&id) {
            return false;
        }
        if let (Some(log), Some(time_supplier)) = (&self.log, &self.time_supplier) {
            let live_discard_secs = preferences.and_then(|p| p.live_discard);
            if let Some(secs) = live_discard_secs
                && secs != -1.0
            {
                let min_time = time_supplier() - secs;
                log.borrow_mut()
                    .clear_before_time(math::max(&[0.0, min_time]), false);
            }
        }
        true
    }
}

/// The abstract class's overridable surface.
pub trait LiveDataSource {
    fn base(&self) -> &LiveDataSourceBase;

    fn base_mut(&mut self) -> &mut LiveDataSourceBase;

    fn connect(
        &mut self,
        address: &str,
        status_callback: StatusCallback,
        output_callback: OutputCallback,
        cx: &mut LiveContext<'_>,
    );

    fn stop(&mut self, cx: &mut LiveContext<'_>) {
        self.base_mut().stop(cx.timers);
    }

    fn handle_main_message(&mut self, _data: &JsValue, _cx: &mut LiveContext<'_>) {}

    fn tuner(&mut self) -> Option<&mut dyn LiveDataTuner> {
        None
    }

    /// Dispatches a fired timer. Sources that schedule their own timers check their ids and
    /// fall back to the base.
    fn handle_timer(&mut self, id: TimerId, cx: &mut LiveContext<'_>) -> bool {
        self.base_mut().handle_timer(id, cx.preferences)
    }
}

#[cfg(test)]
mod tests {
    use asrs_js::time::SetRng;

    use super::*;
    use crate::timers::ManualClock;

    fn base() -> LiveDataSourceBase {
        LiveDataSourceBase::new(&mut SetRng::new(vec![0.0; 32]))
    }

    #[test]
    fn output_without_a_callback_does_nothing() {
        let mut source = base();
        assert!(!source.output(Rc::new(|| 0.0)));
        assert!(!source.has_output_callback());
    }

    #[test]
    fn foreign_timers_are_left_to_the_subclass() {
        let mut timers = TimerQueue::new(Rc::new(ManualClock::new(0.0)));
        let mut source = base();
        let foreign = timers.set_timeout(0.0);
        source.connect(
            "127.0.0.1",
            Box::new(|_| {}),
            Box::new(|_, _| {}),
            true,
            &mut timers,
        );
        assert!(!source.handle_timer(foreign, None));
        assert_eq!(source.uuid(), "a".repeat(32));
        assert_eq!(source.status(), LiveDataSourceStatus::Connecting);
    }
}
