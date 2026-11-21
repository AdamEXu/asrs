//! Port of `src/hub/dataSources/nt4/NT4Publisher.ts`.
//!
//! Publishes the current value of every filtered field to an NT server ("Publish NT Data").
//! `window` is [`PublisherWindow`]: the hub log, `window.preferences`, `window.platform` and
//! `window.selection`. The interval callback is [`Nt4Publisher::handle_timer`]; exceptions out
//! of `periodic` are reported and swallowed, as the browser does for interval callbacks.

use std::cell::RefCell;

use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue};
use asrs_log::log::log_key_utils::filter_field_by_prefixes;
use asrs_log::log::log_util::{get_or_default, log_values_equal};
use asrs_log::log::log_value_sets::RawValue;
use asrs_log::log::{Log, LoggableType};

use super::client::{NT4_PORTS_DEFAULT, Nt4Client, Nt4Context, Nt4Handler};
use super::transport::NetEvent;
use crate::thrown::Thrown;
use crate::timers::TimerId;

fn js(text: &str) -> JsString {
    JsString::from(text)
}

/// `NT4PublisherStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Nt4PublisherStatus {
    Connecting = 0,
    Active = 1,
    Stopped = 2,
}

/// `(status: NT4PublisherStatus) => void`.
pub type PublisherStatusCallback = Box<dyn FnMut(Nt4PublisherStatus)>;

/// The `window.preferences` fields `NT4Publisher` reads.
pub struct PublisherPreferences {
    pub publish_filter: JsString,
    /// `getRobotAddress` inputs, read in the constructor of a robot-mode publisher only.
    pub robot_address: JsString,
    /// `""`, `"usb"` or `"wifi"`.
    pub systemcore_static_address: JsString,
}

/// `getRobotAddress(preferences, platform)`, `platform` being a `process.platform` value.
fn robot_address(preferences: &PublisherPreferences, platform: &str) -> JsString {
    if preferences.systemcore_static_address == js("usb") {
        return js(if platform == "win32" {
            "172.26.0.1"
        } else {
            "172.27.0.1"
        });
    }
    if preferences.systemcore_static_address == js("wifi") {
        return js("172.30.0.1");
    }
    preferences.robot_address.clone()
}

/// `window.selection` as the publisher uses it: `getHoveredTime` and `getSelectedTime`, the
/// latter mutating the way upstream's getter does.
pub trait PublisherSelection {
    fn get_hovered_time(&self) -> Option<f64>;
    fn get_selected_time(&mut self) -> Option<f64>;
}

/// `window` as `NT4Publisher` sees it.
pub struct PublisherWindow<'a> {
    /// `window.log`, shared with the hub the way upstream hands the same object around.
    pub log: &'a RefCell<Log>,
    /// `window.preferences`; `None` is `null`.
    pub preferences: Option<&'a PublisherPreferences>,
    /// `window.platform`.
    pub platform: &'a str,
    /// `window.selection`.
    pub selection: &'a mut dyn PublisherSelection,
}

struct Inner {
    stopped: bool,
    status_callback: PublisherStatusCallback,
}

impl Inner {
    fn status(&mut self, status: Nt4PublisherStatus) {
        (self.status_callback)(status);
    }
}

impl Nt4Handler for Inner {
    fn on_connect(&mut self, _cx: &mut Nt4Context<'_>) -> Result<(), Thrown> {
        if !self.stopped {
            self.status(Nt4PublisherStatus::Active);
        }
        Ok(())
    }

    fn on_disconnect(&mut self, _cx: &mut Nt4Context<'_>) -> Result<(), Thrown> {
        if !self.stopped {
            self.status(Nt4PublisherStatus::Connecting);
        }
        Ok(())
    }
}

/// `NT4Publisher`.
pub struct Nt4Publisher {
    inner: Inner,
    client: Nt4Client,
    interval: TimerId,
    published_topics: JsObject<Option<JsValue>>,
}

impl Nt4Publisher {
    /// `new NT4Publisher(isSim, statusCallback)`: resolves the address, reports `Connecting`,
    /// creates and connects the client and starts the 50 Hz publish interval.
    pub fn new(
        is_sim: bool,
        status_callback: PublisherStatusCallback,
        window: &PublisherWindow<'_>,
        cx: &mut Nt4Context<'_>,
    ) -> Self {
        let address = if is_sim {
            js("127.0.0.1")
        } else {
            match window.preferences {
                Some(preferences) => robot_address(preferences, window.platform),
                None => JsString::new(),
            }
        };
        let mut inner = Inner {
            stopped: false,
            status_callback,
        };
        inner.status(Nt4PublisherStatus::Connecting);
        let mut client = Nt4Client::new(
            &address.to_string_lossy(),
            NT4_PORTS_DEFAULT,
            "AdvantageScope",
            cx.timers,
        );
        client.connect(cx);
        Nt4Publisher {
            inner,
            client,
            interval: cx.timers.set_interval(0.02 * 1000.0),
            published_topics: JsObject::new(),
        }
    }

    /// `stop()`: reports `Stopped` even when called twice, as upstream does.
    pub fn stop(&mut self, cx: &mut Nt4Context<'_>) -> Result<(), Thrown> {
        self.inner.stopped = true;
        self.inner.status(Nt4PublisherStatus::Stopped);
        self.client.disconnect(&mut self.inner, cx)?;
        cx.timers.clear(self.interval);
        Ok(())
    }

    /// The socket events the publisher's client listens to.
    pub fn handle_net_event(
        &mut self,
        event: &NetEvent,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        self.client.handle_net_event(event, &mut self.inner, cx)
    }

    /// A fired timer: the publish interval first, then the client's own timers.
    pub fn handle_timer(
        &mut self,
        id: TimerId,
        window: &mut PublisherWindow<'_>,
        cx: &mut Nt4Context<'_>,
    ) -> bool {
        if id == self.interval {
            let _ = self.periodic(window, cx);
            return true;
        }
        self.client.handle_timer(id, &mut self.inner, cx).is_some()
    }

    /// `periodic()`.
    fn periodic(
        &mut self,
        window: &mut PublisherWindow<'_>,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        let server_time = self.client.server_time_us(cx.clock, None).unwrap_or(0.0);
        let Some(preferences) = window.preferences else {
            return Ok(());
        };
        let log = &*window.log.borrow();
        let topics_to_publish: Vec<JsString> = filter_field_by_prefixes(
            &log.get_field_keys(),
            &preferences.publish_filter,
            true,
            true,
        )
        .into_iter()
        .filter(|topic| {
            !topic.starts_with(&js("$"), None)
                && !log.is_generated(topic)
                && log
                    .get_type(topic)
                    .is_some_and(|ty| ty != LoggableType::Empty)
        })
        .collect();
        for topic in &topics_to_publish {
            if self.published_topics.get_key(topic).is_none() {
                let mut ty = match log.get_type(topic) {
                    Some(LoggableType::Raw) => js("raw"),
                    Some(LoggableType::Boolean) => js("boolean"),
                    Some(LoggableType::Number) => js("double"),
                    Some(LoggableType::String) => js("string"),
                    Some(LoggableType::BooleanArray) => js("boolean[]"),
                    Some(LoggableType::NumberArray) => js("double[]"),
                    Some(LoggableType::StringArray) => js("string[]"),
                    _ => JsString::new(),
                };
                if let Some(wpilib) = log.get_wpilib_type(topic) {
                    ty = wpilib.clone();
                    if ty == js("int64") {
                        ty = js("int");
                    }
                    if ty == js("int64[]") {
                        ty = js("int[]");
                    }
                }
                self.client
                    .publish_topic(&topic.slice(3.0, None), &ty, cx)?;
                self.published_topics.insert(topic.clone(), None);
            }
        }
        let published: Vec<JsString> = self.published_topics.keys().cloned().collect();
        for topic in &published {
            if !topics_to_publish.contains(topic) {
                self.client.unpublish_topic(&topic.slice(3.0, None), cx)?;
                self.published_topics.remove_key(topic);
            }
        }

        let hovered_time = window.selection.get_hovered_time();
        let selected_time = window.selection.get_selected_time();
        let time = selected_time
            .or(hovered_time)
            .unwrap_or_else(|| log.get_timestamp_range()[0]);

        let published: Vec<JsString> = self.published_topics.keys().cloned().collect();
        for topic in &published {
            let last_value = self.published_topics.get_key(topic).cloned().flatten();
            let ty = log.get_type(topic).expect("published fields exist");
            let value: Option<JsValue> = match ty {
                LoggableType::Raw => Some(JsValue::Bytes(
                    get_or_default(log, topic, time, RawValue::from(Vec::<u8>::new()), None)
                        .to_vec(),
                )),
                LoggableType::Boolean => {
                    Some(JsValue::Bool(get_or_default(log, topic, time, false, None)))
                }
                LoggableType::Number => {
                    Some(JsValue::Number(get_or_default(log, topic, time, 0.0, None)))
                }
                LoggableType::String => Some(JsValue::String(get_or_default(
                    log,
                    topic,
                    time,
                    JsString::new(),
                    None,
                ))),
                LoggableType::BooleanArray => Some(JsValue::Array(
                    get_or_default(log, topic, time, Vec::<bool>::new(), None)
                        .into_iter()
                        .map(JsValue::Bool)
                        .collect(),
                )),
                LoggableType::NumberArray => Some(JsValue::Array(
                    get_or_default(log, topic, time, Vec::<f64>::new(), None)
                        .into_iter()
                        .map(JsValue::Number)
                        .collect(),
                )),
                LoggableType::StringArray => Some(JsValue::Array(
                    get_or_default(log, topic, time, Vec::<JsString>::new(), None)
                        .into_iter()
                        .map(JsValue::String)
                        .collect(),
                )),
                LoggableType::Empty => None,
            };
            let has_changed = match (&last_value, &value) {
                (None, _) => true,
                (Some(last), Some(value)) => !published_values_equal(ty, value, last),
                (Some(_), None) => true,
            };
            if has_changed {
                self.published_topics.insert(topic.clone(), value.clone());
                if let Some(value) = value {
                    self.client.add_timestamped_sample(
                        &topic.slice(3.0, None),
                        server_time,
                        value,
                        cx,
                    )?;
                }
            }
        }
        Ok(())
    }
}

/// `logValuesEqual(type, a, b)` on the values `periodic` stores.
fn published_values_equal(ty: LoggableType, a: &JsValue, b: &JsValue) -> bool {
    match (ty, a, b) {
        (LoggableType::Raw, JsValue::Bytes(a), JsValue::Bytes(b)) => {
            log_values_equal(&RawValue::from(a.clone()), &RawValue::from(b.clone()))
        }
        (LoggableType::Boolean, JsValue::Bool(a), JsValue::Bool(b)) => log_values_equal(a, b),
        (LoggableType::Number, JsValue::Number(a), JsValue::Number(b)) => log_values_equal(a, b),
        (LoggableType::String, JsValue::String(a), JsValue::String(b)) => log_values_equal(a, b),
        (_, JsValue::Array(a), JsValue::Array(b)) => match ty {
            LoggableType::BooleanArray => log_values_equal(&bools(a), &bools(b)),
            LoggableType::NumberArray => log_values_equal(&numbers(a), &numbers(b)),
            _ => log_values_equal(&strings(a), &strings(b)),
        },
        _ => false,
    }
}

fn bools(values: &[JsValue]) -> Vec<bool> {
    values
        .iter()
        .map(|v| v.as_bool().unwrap_or_default())
        .collect()
}

fn numbers(values: &[JsValue]) -> Vec<f64> {
    values
        .iter()
        .map(|v| v.as_f64().unwrap_or_default())
        .collect()
}

fn strings(values: &[JsValue]) -> Vec<JsString> {
    values
        .iter()
        .map(|v| v.as_str().cloned().unwrap_or_default())
        .collect()
}
