//! Port of `src/hub/dataSources/nt4/NT4Source.ts` and `NT4Tuner.ts`.
//!
//! The source's client callbacks live on an [`Inner`] handler so the client can invoke them
//! while the outer [`Nt4Source`] keeps the subscription and timer bookkeeping `periodic`
//! owns. `window` is [`Nt4Window`]: the hub log, `window.preferences`, the active fields the
//! tabs and sidebar report, and `window.sendMainMessage`.

use std::cell::{RefCell, RefMut};
use std::rc::Rc;

use asrs_formats::custom_schemas::{self, SchemaError};
use asrs_js::json;
use asrs_js::math;
use asrs_js::ops;
use asrs_js::string::JsString;
use asrs_js::time::{Clock, Rng};
use asrs_js::value::{JsObject, JsValue};
use asrs_log::log::log_key_utils::{
    PHOTON_PREFIX, PROTO_PREFIX, STRUCT_PREFIX, get_structured_type_from_raw,
};
use asrs_log::log::log_util::get_urcl_keys;
use asrs_log::log::log_value_sets::RawValue;
use asrs_log::log::robot_state::{AUTONOMOUS_KEYS, ENABLED_KEYS, UTILITY_KEYS};
use asrs_log::log::{Log, LogError, LoggableType};

use super::client::{
    NT4_PORTS_DEFAULT, NT4_PORTS_DS, NT4_PORTS_SYSTEMCORE, Nt4Client, Nt4Context, Nt4Handler,
    Nt4Topic,
};
use super::transport::NetEvent;
use crate::live_data_source::{
    LiveDataSourceBase, LiveDataSourceStatus, LiveLog, LivePreferences, OutputCallback,
    StatusCallback,
};
use crate::live_data_tuner::TunedValue;
use crate::thrown::Thrown;
use crate::timers::{TimerId, TimerQueue};

pub const WPILOG_PREFIX: &str = "NT:";
pub const DS_PREFIX: &str = "DS:";
pub const AKIT_PREFIX: &str = "/AdvantageKit";
pub const AKIT_TUNING_PREFIX: &str = "/Tuning";

/// `NT4Mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nt4Mode {
    Default,
    AdvantageKit,
    Systemcore,
    DriverStation,
}

fn js(text: &str) -> JsString {
    JsString::from(text)
}

/// `window.sendMainMessage(name, data)`.
pub type SendMainMessage = Box<dyn FnMut(&str, JsValue)>;

/// `window` as the NT4 classes see it.
pub struct Nt4Window {
    /// `window.log`: the hub's log. `=== this.log` once the source's output callback runs.
    pub log: LiveLog,
    /// `window.preferences`; `None` is `null`.
    pub preferences: Option<LivePreferences>,
    /// `window.tabs.getActiveFields()`.
    pub tabs_fields: Vec<JsString>,
    /// `window.sidebar.getActiveFields()`.
    pub sidebar_fields: Vec<JsString>,
    /// `window.sendMainMessage`.
    pub send_main_message: SendMainMessage,
}

/// `NT4Source`.
pub struct Nt4Source {
    inner: Inner,
    client: Option<Rc<RefCell<Nt4Client>>>,
    idle_timer: Option<TimerId>,
    logging_subscription: Option<f64>,
    low_bandwidth_topic_subscription: Option<f64>,
    low_bandwidth_data_subscriptions: JsObject<f64>,
    clock: Rc<dyn Clock>,
}

impl Nt4Source {
    /// `new NT4Source(mode)`: starts the idle loop. Upstream's `periodicCallback` is never
    /// assigned, so `stop()` never kills the loop; it keeps drawing subscription ids from
    /// `Math.random` on preference changes even then.
    pub fn new(
        mode: Nt4Mode,
        rng: &mut dyn Rng,
        clock: Rc<dyn Clock>,
        timers: &mut TimerQueue,
    ) -> Self {
        Self {
            inner: Inner {
                base: LiveDataSourceBase::new(rng),
                mode,
                should_run_output_callback: false,
                connect_time: None,
                no_fields_timeout: None,
            },
            client: None,
            idle_timer: Some(timers.post_delayed_task(100.0)),
            logging_subscription: None,
            low_bandwidth_topic_subscription: None,
            low_bandwidth_data_subscriptions: JsObject::new(),
            clock,
        }
    }

    pub fn base(&self) -> &LiveDataSourceBase {
        &self.inner.base
    }

    /// `connect(address, statusCallback, outputCallback)`.
    pub fn connect(
        &mut self,
        address: &str,
        status_callback: StatusCallback,
        output_callback: OutputCallback,
        preferences: Option<&LivePreferences>,
        cx: &mut Nt4Context<'_>,
    ) {
        self.inner
            .base
            .connect(address, status_callback, output_callback, true, cx.timers);
        self.inner.should_run_output_callback = false;

        if preferences.is_none() {
            self.inner.base.set_status(LiveDataSourceStatus::Error);
        } else {
            self.inner
                .base
                .set_log(Some(Rc::new(RefCell::new(Log::new(true)))));
            let ports = match self.inner.mode {
                Nt4Mode::Systemcore => NT4_PORTS_SYSTEMCORE,
                Nt4Mode::DriverStation => NT4_PORTS_DS,
                _ => NT4_PORTS_DEFAULT,
            };
            let mut client = Nt4Client::new(address, ports, "AdvantageScope", cx.timers);
            client.connect(cx);
            self.client = Some(Rc::new(RefCell::new(client)));
        }
    }

    /// `stop()`. `client.disconnect()` reaches the disconnect callback; whatever it throws
    /// escapes as upstream's would.
    pub fn stop(&mut self, cx: &mut Nt4Context<'_>) -> Result<(), Thrown> {
        self.inner.base.stop(cx.timers);
        if let Some(client) = &self.client {
            client.borrow_mut().disconnect(&mut self.inner, cx)?;
        }
        Ok(())
    }

    /// `getTuner()`; upstream throws a string before the connection starts.
    pub fn tuner(&self) -> Result<Nt4Tuner, Thrown> {
        match (&self.client, self.inner.base.log()) {
            (Some(client), Some(_)) => Ok(Nt4Tuner {
                client: Rc::clone(client),
                mode: self.inner.mode,
            }),
            _ => Err(Thrown::new(
                "Cannot create NT4 tuner before starting connection",
            )),
        }
    }

    /// The socket events this source's client listens to.
    pub fn handle_net_event(
        &mut self,
        event: &NetEvent,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        match &self.client {
            Some(client) => client
                .borrow_mut()
                .handle_net_event(event, &mut self.inner, cx),
            None => Ok(()),
        }
    }

    /// A fired timer: the idle loop, the AdvantageKit no-fields timeout, the client's
    /// timers, then the base's discard intervals.
    pub fn handle_timer(
        &mut self,
        id: TimerId,
        window: &mut Nt4Window,
        cx: &mut Nt4Context<'_>,
    ) -> bool {
        if self.idle_timer == Some(id) {
            self.idle_timer = None;
            if self.periodic(window, cx).is_ok() {
                self.idle_timer = Some(cx.timers.post_delayed_task(100.0));
            }
            return true;
        }
        if self.inner.no_fields_timeout == Some(id) {
            (window.send_main_message)(
                "error",
                ops::object([
                    ("title", JsValue::string("Problem with NT4 connection")),
                    (
                        "content",
                        JsValue::string(
                            "No fields were received from the server. AdvantageKit mode is selected. Are you connecting to a server without AdvantageKit?",
                        ),
                    ),
                ]),
            );
            return true;
        }
        if let Some(client) = &self.client
            && client
                .borrow_mut()
                .handle_timer(id, &mut self.inner, cx)
                .is_some()
        {
            return true;
        }
        self.inner
            .base
            .handle_timer(id, window.preferences.as_ref())
    }

    fn client_mut(&self) -> RefMut<'_, Nt4Client> {
        self.client
            .as_ref()
            .expect("subscriptions only run while connected")
            .borrow_mut()
    }

    /// `periodic()`: clears old window data, maintains subscriptions, runs the output
    /// callback once if any callback marked it.
    fn periodic(&mut self, window: &Nt4Window, cx: &mut Nt4Context<'_>) -> Result<(), Thrown> {
        if let (Some(client), Some(connect_time)) = (&self.client, self.inner.connect_time)
            && let Some(server_time) = client.borrow().server_time_us(cx.clock, Some(connect_time))
        {
            window
                .log
                .borrow_mut()
                .clear_before_time(server_time / 1e6, true);
        }

        if self.client.is_some() {
            let logging = window
                .preferences
                .as_ref()
                .and_then(|p| p.live_subscribe_mode.as_ref())
                .is_some_and(|mode| *mode == js("logging"));
            let patterns = match self.inner.mode {
                Nt4Mode::AdvantageKit => vec![js("/AdvantageKit/"), js("/Tuning/")],
                _ => vec![js("")],
            };
            if logging {
                let ids: Vec<f64> = self
                    .low_bandwidth_data_subscriptions
                    .values()
                    .copied()
                    .collect();
                for id in ids {
                    self.client_mut().unsubscribe(id, cx)?;
                }
                self.low_bandwidth_data_subscriptions = JsObject::new();
                if let Some(id) = self.low_bandwidth_topic_subscription.take() {
                    self.client_mut().unsubscribe(id, cx)?;
                }
                if self.logging_subscription.is_none() {
                    let id = self
                        .client_mut()
                        .subscribe(&patterns, true, true, 0.02, cx)?;
                    self.logging_subscription = Some(id);
                }
            } else {
                if let Some(id) = self.logging_subscription.take() {
                    self.client_mut().unsubscribe(id, cx)?;
                }
                if self.low_bandwidth_topic_subscription.is_none() {
                    let id = self
                        .client_mut()
                        .subscribe_topics_only(&patterns, true, cx)?;
                    self.low_bandwidth_topic_subscription = Some(id);
                }

                let active_fields = self.active_fields(window);
                for field in &active_fields {
                    if !subscription_present(&self.low_bandwidth_data_subscriptions, field) {
                        let id = self.client_mut().subscribe(
                            std::slice::from_ref(field),
                            true,
                            true,
                            0.02,
                            cx,
                        )?;
                        self.low_bandwidth_data_subscriptions
                            .insert(field.clone(), id);
                    }
                }
                let entries: Vec<(JsString, f64)> = self
                    .low_bandwidth_data_subscriptions
                    .iter()
                    .map(|(k, v)| (k.clone(), *v))
                    .collect();
                for (field, id) in entries {
                    if !active_fields.contains(&field) {
                        self.client_mut().unsubscribe(id, cx)?;
                        self.low_bandwidth_data_subscriptions.remove_key(&field);
                    }
                }
            }
        }

        self.inner.maybe_output(&self.client, &self.clock);
        Ok(())
    }

    /// The `activeFields` set of the low-bandwidth path.
    fn active_fields(&self, window: &Nt4Window) -> Vec<JsString> {
        let mut active: Vec<JsString> = Vec::new();
        let mut add = |field: JsString| {
            if !active.contains(&field) {
                active.push(field);
            }
        };
        let this_log = self.inner.base.log().cloned();
        if this_log.is_some_and(|l| Rc::ptr_eq(&l, &window.log)) {
            let log = window.log.borrow();
            let announced: Vec<JsString> = log
                .get_field_keys()
                .into_iter()
                .filter(|key| log.get_type(key) != Some(LoggableType::Empty))
                .collect();
            let initial: Vec<JsString> = match self.inner.mode {
                Nt4Mode::AdvantageKit => vec![js("/.schema"), js("/Timestamp")],
                Nt4Mode::DriverStation => vec![js("DS:/.schema")],
                _ => vec![
                    js("NT:/.schema"),
                    js("NT:/AdvantageKit/.schema"),
                    js("NT:/AdvantageKit/Timestamp"),
                ],
            };
            let keys: Vec<JsString> = initial
                .into_iter()
                .chain(ENABLED_KEYS.iter().map(|k| js(k)))
                .chain(AUTONOMOUS_KEYS.iter().map(|k| js(k)))
                .chain(UTILITY_KEYS.iter().map(|k| js(k)))
                .chain(window.tabs_fields.iter().cloned())
                .chain(window.sidebar_fields.iter().cloned())
                .chain(get_urcl_keys(&log))
                .collect();
            for key in &keys {
                for announced_key in &announced {
                    let subscribe_key = if announced_key.starts_with(key, None) {
                        key
                    } else if key.starts_with(announced_key, None) {
                        announced_key
                    } else {
                        continue;
                    };
                    if self.inner.mode == Nt4Mode::AdvantageKit {
                        if subscribe_key.starts_with(&js(AKIT_TUNING_PREFIX), None) {
                            add(subscribe_key.clone());
                        } else {
                            let mut field = js(AKIT_PREFIX);
                            field.push_str(subscribe_key);
                            add(field);
                        }
                    } else {
                        add(subscribe_key.slice(self.inner.log_prefix().len() as f64, None));
                    }
                }
            }
        }

        let copy = active.clone();
        for field0 in &copy {
            for field1 in &copy {
                if field0 != field1 && field0.starts_with(field1, None) {
                    active.retain(|field| field != field0);
                }
            }
        }
        active
    }
}

/// `field in this.lowBandwidthDataSubscriptions`: `in` also finds `Object.prototype`'s own
/// properties, so a field named after one never gets a data subscription.
fn subscription_present(subscriptions: &JsObject<f64>, field: &JsString) -> bool {
    const PROTO: [&str; 12] = [
        "__proto__",
        "constructor",
        "__defineGetter__",
        "__defineSetter__",
        "hasOwnProperty",
        "__lookupGetter__",
        "__lookupSetter__",
        "isPrototypeOf",
        "propertyIsEnumerable",
        "toString",
        "valueOf",
        "toLocaleString",
    ];
    subscriptions.get_key(field).is_some() || PROTO.iter().any(|p| *field == js(p))
}

/// The callbacks `NT4Source` registers on its client. Everything a callback may touch is
/// here: the base (status, log, callbacks), the output flag, the connection time and the
/// AdvantageKit no-fields timeout.
struct Inner {
    base: LiveDataSourceBase,
    mode: Nt4Mode,
    should_run_output_callback: bool,
    connect_time: Option<f64>,
    no_fields_timeout: Option<TimerId>,
}

impl Inner {
    fn log_prefix(&self) -> &'static str {
        match self.mode {
            Nt4Mode::DriverStation => DS_PREFIX,
            _ => WPILOG_PREFIX,
        }
    }

    /// `getKeyFromTopic`: `topic.name.startsWith` throws for a non-string name in
    /// AdvantageKit mode; the other modes concatenate, which stringifies it.
    fn key_from_topic(&self, topic: &Nt4Topic) -> Result<JsString, Thrown> {
        if self.mode == Nt4Mode::AdvantageKit {
            let Some(name) = topic.name_str() else {
                return Err(Thrown::type_error(
                    "topic.name.startsWith is not a function",
                ));
            };
            if name.starts_with(&js(AKIT_PREFIX), None) {
                Ok(name.slice(AKIT_PREFIX.len() as f64, None))
            } else {
                Ok(name.clone())
            }
        } else {
            let mut key = js(self.log_prefix());
            key.push_str(&ops::to_string(&topic.name));
            Ok(key)
        }
    }

    /// `Object.keys(topic.properties).length > 0 ? JSON.stringify(topic.properties) : ""`.
    fn metadata(topic: &Nt4Topic) -> Result<JsString, Thrown> {
        if ops::object_keys(&topic.properties)?.is_empty() {
            Ok(JsString::new())
        } else {
            Ok(JsString::from(
                json::stringify(&topic.properties)?.unwrap_or_default(),
            ))
        }
    }

    /// The output callback at the end of `periodic`.
    fn maybe_output(&mut self, client: &Option<Rc<RefCell<Nt4Client>>>, clock: &Rc<dyn Clock>) {
        if !self.should_run_output_callback
            || self.base.status() == LiveDataSourceStatus::Stopped
            || !self.base.has_output_callback()
            || self.base.log().is_none()
        {
            return;
        }
        self.should_run_output_callback = false;
        let client = client.clone();
        let clock = Rc::clone(clock);
        self.base.output(Rc::new(move || {
            let Some(client) = &client else {
                return 10.0;
            };
            let client = client.borrow();
            match client.server_time_us(&*clock, None) {
                Some(server_time) => (server_time - client.network_latency_us()) / 1e6,
                None => 10.0,
            }
        }));
    }
}

fn log_type(nt_type: &JsValue) -> LoggableType {
    match nt_type.as_str().map(JsString::to_string_lossy).as_deref() {
        Some("boolean") => LoggableType::Boolean,
        Some("int" | "float" | "double") => LoggableType::Number,
        Some("string" | "json") => LoggableType::String,
        Some("boolean[]") => LoggableType::BooleanArray,
        Some("int[]" | "float[]" | "double[]") => LoggableType::NumberArray,
        Some("string[]") => LoggableType::StringArray,
        _ => LoggableType::Raw,
    }
}

/// `checkArrayType(value, type)`.
fn checked_array<'a>(value: &'a JsValue, ty: &str) -> Option<&'a [JsValue]> {
    let items = value.as_array()?;
    items
        .iter()
        .all(|item| item.type_of() == ty)
        .then_some(items)
}

/// The `switch (topic.type)` of the data callback: writes `value` into the log as the
/// topic's type requires. `Ok(false)` is the `console.warn` path (a type mismatch writes
/// nothing); an error is a throw escaping the callback, which aborts the rest of the frame.
fn put_topic_value(
    log: &mut Log,
    realm: &mut asrs_log::realm::JsRealm,
    key: &JsString,
    topic: &Nt4Topic,
    timestamp: f64,
    value: &JsValue,
) -> Result<bool, Thrown> {
    if let Some(ty) = topic.type_str() {
        match ty.to_string_lossy().as_str() {
            "boolean" => {
                return Ok(match value {
                    JsValue::Bool(b) => {
                        log.put_boolean(key, timestamp, *b);
                        true
                    }
                    _ => false,
                });
            }
            "int" | "float" | "double" => {
                return Ok(match value {
                    JsValue::Number(n) => {
                        log.put_number(key, timestamp, *n);
                        true
                    }
                    _ => false,
                });
            }
            "string" => {
                return Ok(match value {
                    JsValue::String(s) => {
                        log.put_string(key, timestamp, s.clone());
                        true
                    }
                    _ => false,
                });
            }
            "boolean[]" => {
                return Ok(match checked_array(value, "boolean") {
                    Some(items) => {
                        log.put_boolean_array(
                            key,
                            timestamp,
                            items
                                .iter()
                                .map(|v| v.as_bool().unwrap_or_default())
                                .collect(),
                        );
                        true
                    }
                    None => false,
                });
            }
            "int[]" | "float[]" | "double[]" => {
                return Ok(match checked_array(value, "number") {
                    Some(items) => {
                        log.put_number_array(
                            key,
                            timestamp,
                            items
                                .iter()
                                .map(|v| v.as_f64().unwrap_or_default())
                                .collect(),
                        );
                        true
                    }
                    None => false,
                });
            }
            "string[]" => {
                return Ok(match checked_array(value, "string") {
                    Some(items) => {
                        log.put_string_array(
                            key,
                            timestamp,
                            items
                                .iter()
                                .map(|v| v.as_str().cloned().unwrap_or_default())
                                .collect(),
                        );
                        true
                    }
                    None => false,
                });
            }
            "json" => {
                return Ok(match value {
                    JsValue::String(s) => {
                        log.put_json(key, timestamp, s.clone())?;
                        true
                    }
                    _ => false,
                });
            }
            "msgpack" => {
                return Ok(match value {
                    JsValue::Bytes(b) => {
                        log.put_msgpack(realm, key, timestamp, RawValue::from(b.clone()))?;
                        true
                    }
                    _ => false,
                });
            }
            _ => {}
        }
    }
    let JsValue::Bytes(bytes) = value else {
        return Ok(false);
    };
    let Some(ty) = topic.type_str() else {
        return Err(Thrown::type_error(
            "topic.type.startsWith is not a function",
        ));
    };
    if ty.starts_with(&js(STRUCT_PREFIX), None) {
        let schema_type = ty.split(Some(&js(STRUCT_PREFIX)), None)[1].clone();
        if schema_type.ends_with(&js("[]"), None) {
            log.put_struct(
                key,
                timestamp,
                RawValue::from(bytes.clone()),
                &schema_type.slice(0.0, Some(-2.0)),
                true,
            )?;
        } else {
            log.put_struct(
                key,
                timestamp,
                RawValue::from(bytes.clone()),
                &schema_type,
                false,
            )?;
        }
    } else if ty.starts_with(&js(PHOTON_PREFIX), None) {
        let schema_type = ty.split(Some(&js(PHOTON_PREFIX)), None)[1].clone();
        log.put_photon_struct(key, timestamp, RawValue::from(bytes.clone()), &schema_type)?;
    } else if ty.starts_with(&js(PROTO_PREFIX), None) {
        let schema_type = ty.split(Some(&js(PROTO_PREFIX)), None)[1].clone();
        log.put_proto(
            realm,
            key,
            timestamp,
            RawValue::from(bytes.clone()),
            &schema_type,
        )?;
    } else {
        log.put_raw(key, timestamp, RawValue::from(bytes.clone()))?;
        if let Some(parser) = custom_schemas::get(ty) {
            match parser(log, key, timestamp, bytes) {
                Ok(()) | Err(SchemaError::Threw) | Err(SchemaError::Log(LogError::Proto(_))) => {}
                Err(SchemaError::Log(error)) => return Err(error.into()),
            }
            log.set_generated_parent(key);
        }
    }
    Ok(true)
}

impl Nt4Handler for Inner {
    fn on_topic_announce(
        &mut self,
        topic: &Nt4Topic,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        let Some(log) = self.base.log().cloned() else {
            return Ok(());
        };
        if let Some(timer) = self.no_fields_timeout {
            cx.timers.clear(timer);
        }
        if matches!(&topic.name, JsValue::String(name) if name.is_empty()) {
            return Ok(());
        }
        let key = self.key_from_topic(topic)?;
        let Some(ty) = topic.type_str() else {
            return Err(Thrown::type_error("rawType.startsWith is not a function"));
        };
        let structured_type = get_structured_type_from_raw(ty);
        {
            let mut log = log.borrow_mut();
            log.create_blank_field(&key, log_type(&topic.ty));
            log.set_wpilib_type(&key, ty.clone());
            log.set_structured_type(&key, structured_type);
            let metadata = Self::metadata(topic)?;
            log.set_metadata_string(&key, metadata);
        }
        self.should_run_output_callback = true;
        Ok(())
    }

    fn on_topic_properties_update(
        &mut self,
        topic: &Nt4Topic,
        _cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        let Some(log) = self.base.log().cloned() else {
            return Ok(());
        };
        if matches!(&topic.name, JsValue::String(name) if name.is_empty()) {
            return Ok(());
        }
        let key = self.key_from_topic(topic)?;
        let metadata = Self::metadata(topic)?;
        log.borrow_mut().set_metadata_string(&key, metadata);
        self.should_run_output_callback = true;
        Ok(())
    }

    fn on_new_topic_data(
        &mut self,
        topic: &Nt4Topic,
        timestamp_us: f64,
        value: JsValue,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        let Some(log) = self.base.log().cloned() else {
            return Ok(());
        };
        if matches!(&topic.name, JsValue::String(name) if name.is_empty()) {
            return Ok(());
        }
        let key = self.key_from_topic(topic)?;
        let timestamp = math::max(&[timestamp_us, log.borrow().get_timestamp_range()[0]]) / 1e6;
        let updated = put_topic_value(
            &mut log.borrow_mut(),
            cx.realm,
            &key,
            topic,
            timestamp,
            &value,
        )?;
        if updated {
            self.should_run_output_callback = true;
        }
        Ok(())
    }

    fn on_connect(&mut self, cx: &mut Nt4Context<'_>) -> Result<(), Thrown> {
        self.base.set_status(LiveDataSourceStatus::Active);
        self.base
            .set_log(Some(Rc::new(RefCell::new(Log::new(true)))));
        self.should_run_output_callback = true;
        if self.connect_time.is_none_or(|t| t == 0.0 || t.is_nan()) {
            self.connect_time = Some(Nt4Client::client_time_us(cx.clock));
        }
        if self.mode == Nt4Mode::AdvantageKit {
            self.no_fields_timeout = Some(cx.timers.set_timeout(5000.0));
        }
        Ok(())
    }

    fn on_disconnect(&mut self, cx: &mut Nt4Context<'_>) -> Result<(), Thrown> {
        self.base.set_status(LiveDataSourceStatus::Connecting);
        self.should_run_output_callback = false;
        self.connect_time = None;
        if let Some(timer) = self.no_fields_timeout {
            cx.timers.clear(timer);
        }
        Ok(())
    }
}

/// `NT4Tuner`. It holds the client's `Rc` as upstream holds the object.
pub struct Nt4Tuner {
    client: Rc<RefCell<Nt4Client>>,
    mode: Nt4Mode,
}

impl Nt4Tuner {
    /// `hasTunableFields`; `window.log` is the displayed log.
    pub fn has_tunable_fields(&self, window_log: &Log) -> bool {
        match self.mode {
            Nt4Mode::Default => true,
            Nt4Mode::AdvantageKit => window_log
                .get_field_keys()
                .iter()
                .any(|key| key.starts_with(&js(AKIT_TUNING_PREFIX), None)),
            Nt4Mode::Systemcore | Nt4Mode::DriverStation => false,
        }
    }

    /// `isTunable`.
    pub fn is_tunable(&self, window_log: &Log, key: &JsString) -> bool {
        let remote_key = self.remote_key(key);
        if !matches!(
            window_log.get_type(key),
            Some(LoggableType::Number) | Some(LoggableType::Boolean)
        ) || remote_key.starts_with(&js(AKIT_PREFIX), None)
            || (window_log
                .get_field(&js("NT:/Robot/DogLog/Options"))
                .is_some()
                && remote_key.starts_with(&js("/Robot"), None))
            || window_log.is_generated(key)
        {
            return false;
        }
        let metadata = window_log.get_metadata_string(key);
        if !metadata.is_empty()
            && let Ok(parsed) = json::parse(&metadata)
            && parsed.type_of() == "object"
            && !parsed.is_null()
            && let Ok(mutable) = ops::get(&parsed, "mutable")
            && let Some(mutable) = mutable.as_bool()
        {
            return mutable;
        }
        true
    }

    /// `publish(key, value)`; a non-tunable field or a missing wpilib type is a silent
    /// return.
    pub fn publish(
        &self,
        window_log: &LiveLog,
        key: &JsString,
        value: TunedValue,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        if !self.is_tunable(&window_log.borrow(), key) {
            return Ok(());
        }
        let remote_key = self.remote_key(key);
        let Some(ty) = window_log.borrow().get_wpilib_type(key).cloned() else {
            return Ok(());
        };
        self.client
            .borrow_mut()
            .publish_topic(&remote_key, &ty, cx)?;
        let timestamp = self.client.borrow().server_time_us(cx.clock, None);
        if let Some(timestamp) = timestamp {
            match value {
                TunedValue::Number(value) => {
                    let clean = if ty.starts_with(&js("int"), None) {
                        math::floor(value)
                    } else {
                        value
                    };
                    self.client.borrow_mut().add_timestamped_sample(
                        &remote_key,
                        timestamp,
                        JsValue::Number(clean),
                        cx,
                    )?;
                    window_log.borrow_mut().put_number(key, timestamp, clean);
                }
                TunedValue::Boolean(value) => {
                    self.client.borrow_mut().add_timestamped_sample(
                        &remote_key,
                        timestamp,
                        JsValue::Bool(value),
                        cx,
                    )?;
                    window_log.borrow_mut().put_boolean(key, timestamp, value);
                }
            }
        }
        Ok(())
    }

    /// `unpublish(key)`; throws a string for a topic the client never published.
    pub fn unpublish(&self, key: &JsString, cx: &mut Nt4Context<'_>) -> Result<(), Thrown> {
        let remote_key = self.remote_key(key);
        self.client.borrow_mut().unpublish_topic(&remote_key, cx)
    }

    /// `getRemoteKey`.
    fn remote_key(&self, key: &JsString) -> JsString {
        if self.mode == Nt4Mode::AdvantageKit {
            if key.starts_with(&js(AKIT_TUNING_PREFIX), None) {
                key.clone()
            } else {
                let mut remote = js(AKIT_PREFIX);
                remote.push_str(key);
                remote
            }
        } else {
            key.slice(WPILOG_PREFIX.len() as f64, None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timers::ManualClock;

    fn tuner(mode: Nt4Mode) -> Nt4Tuner {
        let mut timers = TimerQueue::new(Rc::new(ManualClock::new(0.0)));
        let client = Nt4Client::new(
            "127.0.0.1",
            NT4_PORTS_DEFAULT,
            "AdvantageScope",
            &mut timers,
        );
        Nt4Tuner {
            client: Rc::new(RefCell::new(client)),
            mode,
        }
    }

    #[test]
    fn has_tunable_fields_follows_the_mode() {
        let mut log = Log::new(true);
        log.create_blank_field(&js("/Drive/x"), LoggableType::Number);
        assert!(tuner(Nt4Mode::Default).has_tunable_fields(&log));
        assert!(!tuner(Nt4Mode::AdvantageKit).has_tunable_fields(&log));
        assert!(!tuner(Nt4Mode::Systemcore).has_tunable_fields(&log));
        assert!(!tuner(Nt4Mode::DriverStation).has_tunable_fields(&log));
        log.create_blank_field(&js("/TuningX"), LoggableType::Empty);
        assert!(tuner(Nt4Mode::AdvantageKit).has_tunable_fields(&log));
        assert!(!tuner(Nt4Mode::DriverStation).has_tunable_fields(&log));
        assert!(tuner(Nt4Mode::Default).has_tunable_fields(&Log::new(true)));
    }
}
