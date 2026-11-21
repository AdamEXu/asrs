//! `src/hub/dataSources/rlog/RLOGServerSource.ts`: the renderer half of the RLOG live source.
//! The TCP socket lives in the emulated main process ([`super::main`]); this side feeds each
//! framed payload to one `RLOGDecoder`, keeps `liveZeroTime` tracking the stream's end, and
//! reconnects 500 ms after any failure, including IPC that carries no `success` key at all.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use asrs_formats::rlog::RlogDecoder;
use asrs_formats::worker::Abort;
use asrs_js::ops;
use asrs_js::string::JsString;
use asrs_js::time::{Clock, Rng};
use asrs_js::value::JsValue;
use asrs_log::log::Log;
use asrs_log::realm::JsRealm;

use crate::live_data_source::{
    DISCARD_PERIOD_MS, LiveDataSourceBase, LiveDataSourceStatus, LiveLog, OutputCallback,
    StatusCallback,
};
use crate::timers::{TimerId, TimerQueue};

use super::main::MainCall;

const RECONNECT_DELAY_MS: f64 = 500.0;
const TIME_SYNC_INTERVAL: f64 = 0.5;

/// `window.preferences`, reduced to what the RLOG messages read.
#[derive(Clone, Debug)]
pub struct RlogPreferences {
    pub rlog_port: f64,
}

/// Everything a method may touch: `window` (preferences and `window.log`), the hub's timers,
/// the renderer realm for the decoder, `Date.now()` and the `sendMainMessage` sink.
pub struct RlogContext<'a> {
    pub timers: &'a mut TimerQueue,
    pub send_main: &'a mut VecDeque<MainCall>,
    pub preferences: Option<&'a RlogPreferences>,
    /// `window.log`: the cell, so `getTimestampRange` sees the log the output callback last
    /// installed, as upstream reads the global at message time.
    pub window_log: &'a Rc<RefCell<LiveLog>>,
    pub realm: &'a mut JsRealm,
    pub clock: &'a dyn Clock,
}

fn truthy(value: &JsValue) -> bool {
    match value {
        JsValue::Undefined | JsValue::Null => false,
        JsValue::Bool(b) => *b,
        JsValue::Number(n) => *n != 0.0 && !n.is_nan(),
        JsValue::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// `RLOGServerSource`.
pub struct RlogSource {
    base: LiveDataSourceBase,
    decoder: Option<RlogDecoder>,
    timeout: Option<TimerId>,
    live_shift_interval: Option<TimerId>,
    last_time_sync: f64,
    /// `this.liveZeroTime` and `this.targetLiveZeroTime` as shared cells so the time supplier
    /// reads them lazily, like `this.` fields.
    live_zero_time: Rc<Cell<f64>>,
    target_live_zero_time: Rc<Cell<f64>>,
    /// `this.log` shared with the supplier closure; kept in step with `base`'s log.
    shared_log: Rc<RefCell<Option<LiveLog>>>,
    clock: Rc<dyn Clock>,
}

impl RlogSource {
    pub fn new(rng: &mut dyn Rng, clock: Rc<dyn Clock>) -> Self {
        Self {
            base: LiveDataSourceBase::new(rng),
            decoder: None,
            timeout: None,
            live_shift_interval: None,
            last_time_sync: 0.0,
            live_zero_time: Rc::new(Cell::new(0.0)),
            target_live_zero_time: Rc::new(Cell::new(0.0)),
            shared_log: Rc::new(RefCell::new(None)),
            clock,
        }
    }

    fn install_log(&mut self, log: LiveLog) {
        self.base.set_log(Some(log.clone()));
        *self.shared_log.borrow_mut() = Some(log);
    }

    /// `window.sendMainMessage("live-rlog-start", {uuid, address, port})`.
    fn send_start(&self, cx: &mut RlogContext<'_>, port: f64) {
        cx.send_main.push_back(MainCall::Start {
            uuid: self.base.uuid().to_owned(),
            address: self.base.address().unwrap_or_default().to_owned(),
            port,
        });
    }

    pub fn connect(
        &mut self,
        address: &str,
        status_callback: StatusCallback,
        output_callback: OutputCallback,
        cx: &mut RlogContext<'_>,
    ) {
        self.base
            .connect(address, status_callback, output_callback, false, cx.timers);
        match cx.preferences {
            None => self.base.set_status(LiveDataSourceStatus::Error),
            Some(prefs) => {
                self.install_log(Rc::new(RefCell::new(Log::new(true))));
                self.decoder = Some(RlogDecoder::new(false));
                self.send_start(cx, prefs.rlog_port);
            }
        }
        self.live_shift_interval = Some(cx.timers.set_interval(DISCARD_PERIOD_MS));
    }

    pub fn stop(&mut self, cx: &mut RlogContext<'_>) {
        self.base.stop(cx.timers);
        if let Some(id) = self.timeout.take() {
            cx.timers.clear(id);
        }
        if let Some(id) = self.live_shift_interval.take() {
            cx.timers.clear(id);
        }
        cx.send_main.push_back(MainCall::Stop);
    }

    /// `handleMainMessage(data)`: `"live-data"` IPC from the main-process emulation.
    pub fn handle_main_message(
        &mut self,
        data: &JsValue,
        cx: &mut RlogContext<'_>,
    ) -> Result<(), Abort> {
        if self.base.log().is_none() || self.decoder.is_none() {
            return Ok(());
        }
        let uuid = ops::get(data, "uuid").unwrap_or_default().into_string();
        if uuid.as_ref() != Some(&JsString::from(self.base.uuid())) {
            return Ok(());
        }
        if self.base.status() == LiveDataSourceStatus::Stopped {
            return Ok(());
        }
        if let Some(id) = self.timeout.take() {
            cx.timers.clear(id);
        }
        if !truthy(&ops::get(data, "success").unwrap_or_default()) {
            self.reconnect(cx);
            return Ok(());
        }
        let raw = match ops::get(data, "raw").unwrap_or_default() {
            JsValue::Bytes(ref bytes) => bytes.clone(),
            _ => Vec::new(),
        };
        let decode_ok = {
            let log = self.base.log().expect("checked");
            self.decoder.as_mut().expect("checked").decode(
                cx.realm,
                &mut log.borrow_mut(),
                &raw,
                cx.clock,
                None,
            )?
        };
        if !decode_ok {
            self.reconnect(cx);
            return Ok(());
        }

        let now = cx.clock.now() / 1000.0;
        if self.base.status() == LiveDataSourceStatus::Connecting {
            self.live_zero_time.set(now);
            self.target_live_zero_time.set(now);
            self.last_time_sync = now;
        }
        self.base.set_status(LiveDataSourceStatus::Active);
        let field_count = self
            .base
            .log()
            .map(|log| log.borrow().get_field_keys().len())
            .unwrap_or_default();
        if field_count > 0 && now - self.last_time_sync > TIME_SYNC_INTERVAL {
            let range = cx.window_log.borrow().borrow().get_timestamp_range();
            self.target_live_zero_time.set(now - (range[1] - range[0]));
            self.last_time_sync = now;
        }
        let clock = self.clock.clone();
        let zero = self.live_zero_time.clone();
        let shared = self.shared_log.clone();
        self.base
            .output(Rc::new(move || match shared.borrow().as_ref() {
                Some(log) => {
                    clock.now() / 1000.0 - zero.get() + log.borrow().get_timestamp_range()[0]
                }
                None => 0.0,
            }));
        Ok(())
    }

    fn reconnect(&mut self, cx: &mut RlogContext<'_>) {
        self.base.set_status(LiveDataSourceStatus::Connecting);
        cx.send_main.push_back(MainCall::Stop);
        self.timeout = Some(cx.timers.set_timeout(RECONNECT_DELAY_MS));
    }

    /// A fired `setTimeout`/`setInterval`: the reconnect delay, the zero-time smoother, or
    /// the base's (none, for RLOG).
    pub fn handle_timer(&mut self, id: TimerId, cx: &mut RlogContext<'_>) -> bool {
        if self.live_shift_interval == Some(id) {
            self.live_zero_time
                .set(self.live_zero_time.get() * 0.98 + self.target_live_zero_time.get() * 0.02);
            true
        } else if self.timeout == Some(id) {
            self.timeout = None;
            match cx.preferences {
                None => self.base.set_status(LiveDataSourceStatus::Error),
                Some(prefs) => {
                    self.install_log(Rc::new(RefCell::new(Log::new(true))));
                    self.decoder = Some(RlogDecoder::new(false));
                    self.live_zero_time.set(0.0);
                    self.target_live_zero_time.set(0.0);
                    self.send_start(cx, prefs.rlog_port);
                }
            }
            true
        } else {
            self.base.handle_timer(id, None)
        }
    }

    pub fn base(&self) -> &LiveDataSourceBase {
        &self.base
    }
}
