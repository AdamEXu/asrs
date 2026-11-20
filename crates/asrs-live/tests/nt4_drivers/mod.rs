//! Thin stand-ins for `NT4Source` (with `NT4Tuner`) and `NT4Publisher`: they call the client
//! exactly where and when upstream's classes do, which is all that reaches the wire, but keep no
//! real `Log`. The source models each `Log` by the fields announcements create, which is all its
//! subscription logic and the tuner read; values are only type-checked, as the data callback
//! does to decide whether to run the output callback. The publisher needs values, so it uses the
//! real `Log`.

use std::cell::RefCell;
use std::rc::Rc;

use asrs_js::json;
use asrs_js::math;
use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue};
use asrs_live::nt4::{
    NT4_PORTS_DEFAULT, NT4_PORTS_DS, NT4_PORTS_SYSTEMCORE, Nt4Client, Nt4Context, Nt4Handler,
    Nt4Topic,
};
use asrs_live::{LiveDataSourceBase, LiveDataSourceStatus, Thrown, TimerId};
use asrs_log::log::Log;
use asrs_log::log::LoggableType;
use asrs_log::log::log_key_utils::filter_field_by_prefixes;
use asrs_log::log::log_util::{get_or_default, log_values_equal};
use asrs_log::log::log_value_sets::RawValue;
use asrs_log::log::robot_state::{AUTONOMOUS_KEYS, ENABLED_KEYS, UTILITY_KEYS};
use asrs_parity::Value;
use asrs_parity::json::Json;
use asrs_parity::live::Session;

use crate::support::{self, Driver, Env, Notify, Trace, js_arg};

const WPILOG_PREFIX: &str = "NT:";
const DS_PREFIX: &str = "DS:";
const AKIT_PREFIX: &str = "/AdvantageKit";
const AKIT_TUNING_PREFIX: &str = "/Tuning";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Default,
    AdvantageKit,
    Systemcore,
    DriverStation,
}

fn js(s: &str) -> JsString {
    JsString::from(s)
}

fn starts(a: &JsString, b: &JsString) -> bool {
    a.starts_with(b, None)
}

/// `window.preferences`, reduced to what the NT4 classes read; `None` is `null`.
#[derive(Clone)]
struct Prefs {
    live_subscribe_mode: String,
    publish_filter: JsString,
}

impl Prefs {
    fn defaults() -> Self {
        Prefs {
            live_subscribe_mode: "low-bandwidth".to_owned(),
            publish_filter: JsString::new(),
        }
    }

    fn merge(&mut self, json: &Json) {
        if let Some(mode) = json.get("liveSubscribeMode").and_then(Json::as_str) {
            self.live_subscribe_mode = mode.to_string_lossy();
        }
        if let Some(filter) = json.get("publishFilter").and_then(Json::as_str) {
            self.publish_filter = JsString::from_units(filter.units().to_vec());
        }
    }
}

fn prefs_op(prefs: &mut Option<Prefs>, arg: &Json) {
    match arg {
        Json::Null => *prefs = None,
        update => prefs.get_or_insert_with(Prefs::defaults).merge(update),
    }
}

fn initial_prefs(session: &Session) -> Option<Prefs> {
    match session.client.get("prefs") {
        Some(Json::Null) => None,
        Some(update) => {
            let mut prefs = Prefs::defaults();
            prefs.merge(update);
            Some(prefs)
        }
        None => Some(Prefs::defaults()),
    }
}

fn string_list(json: Option<&Json>) -> Vec<JsString> {
    json.and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .map(|s| JsString::from_units(s.as_str().expect("field key").units().to_vec()))
                .collect()
        })
        .unwrap_or_default()
}

fn status_callback(
    trace: &Trace,
    clock: &asrs_live::ManualClock,
) -> Box<dyn FnMut(LiveDataSourceStatus)> {
    let trace = trace.clone();
    let clock = clock.clone();
    Box::new(move |status| {
        use asrs_js::time::Clock;
        trace.app(
            clock.now(),
            "status",
            Some(&Value::number(f64::from(status as u8))),
        );
    })
}

/// One `Log` as the source's logic sees it: field key, type, wpilib type and metadata.
#[derive(Default)]
struct LogModel {
    fields: Vec<(JsString, LoggableType, Option<JsString>, JsString)>,
}

impl LogModel {
    fn field(
        &self,
        key: &JsString,
    ) -> Option<&(JsString, LoggableType, Option<JsString>, JsString)> {
        self.fields.iter().find(|f| f.0 == *key)
    }

    fn field_mut(
        &mut self,
        key: &JsString,
    ) -> Option<&mut (JsString, LoggableType, Option<JsString>, JsString)> {
        self.fields.iter_mut().find(|f| f.0 == *key)
    }

    fn urcl_keys(&self) -> Vec<JsString> {
        self.fields
            .iter()
            .filter(|f| f.2.as_ref().is_some_and(|t| starts(t, &js("URCL"))))
            .map(|f| f.0.clone())
            .collect()
    }
}

/// The hub's side: `window.log` and the harness's log numbering.
struct Hub {
    logs: Vec<LogModel>,
    ids: Vec<Option<u32>>,
    next_id: u32,
    window_log: usize,
    tabs: Vec<JsString>,
    sidebar: Vec<JsString>,
    prefs: Option<Prefs>,
}

impl Hub {
    fn new_log(&mut self) -> usize {
        self.logs.push(LogModel::default());
        self.ids.push(None);
        self.logs.len() - 1
    }

    fn log_id(&mut self, log: usize) -> u32 {
        *self.ids[log].get_or_insert_with(|| {
            self.next_id += 1;
            self.next_id - 1
        })
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

fn array_of(value: &JsValue, ty: &str) -> bool {
    value
        .as_array()
        .is_some_and(|items| items.iter().all(|v| v.type_of() == ty))
}

/// The parts of `NT4Source` its client callbacks touch.
struct SourceState {
    mode: Mode,
    base: LiveDataSourceBase,
    log: Option<usize>,
    should_run_output_callback: bool,
    connect_time: Option<f64>,
    no_fields_timeout: Option<TimerId>,
    hub: Rc<RefCell<Hub>>,
}

impl SourceState {
    fn key_from_topic(&self, name: &JsString) -> JsString {
        if self.mode == Mode::AdvantageKit {
            if starts(name, &js(AKIT_PREFIX)) {
                name.slice(AKIT_PREFIX.len() as f64, None)
            } else {
                name.clone()
            }
        } else {
            let mut key = js(self.log_prefix());
            key.push_str(name);
            key
        }
    }

    fn log_prefix(&self) -> &'static str {
        if self.mode == Mode::DriverStation {
            DS_PREFIX
        } else {
            WPILOG_PREFIX
        }
    }

    fn metadata(topic: &Nt4Topic) -> JsString {
        match topic.properties.as_object() {
            Some(o) if !o.is_empty() => js(&json::stringify(&topic.properties)
                .ok()
                .flatten()
                .unwrap_or_default()),
            _ => JsString::new(),
        }
    }
}

impl Nt4Handler for SourceState {
    fn on_topic_announce(
        &mut self,
        topic: &Nt4Topic,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        let Some(log) = self.log else {
            return Ok(());
        };
        if let Some(t) = self.no_fields_timeout {
            cx.timers.clear(t);
        }
        let Some(name) = topic.name_str() else {
            return Ok(());
        };
        if name.is_empty() {
            return Ok(());
        }
        let key = self.key_from_topic(name);
        let metadata = Self::metadata(topic);
        let wpilib = topic.type_str().cloned();
        let ty = log_type(&topic.ty);
        let mut hub = self.hub.borrow_mut();
        let model = &mut hub.logs[log];
        if model.field(&key).is_none() {
            model.fields.push((key.clone(), ty, None, JsString::new()));
        }
        let field = model.field_mut(&key).expect("created");
        field.2 = wpilib;
        field.3 = metadata;
        self.should_run_output_callback = true;
        Ok(())
    }

    fn on_topic_properties_update(
        &mut self,
        topic: &Nt4Topic,
        _cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        let Some(log) = self.log else {
            return Ok(());
        };
        let Some(name) = topic.name_str() else {
            return Ok(());
        };
        if name.is_empty() {
            return Ok(());
        }
        let key = self.key_from_topic(name);
        let metadata = Self::metadata(topic);
        if let Some(field) = self.hub.borrow_mut().logs[log].field_mut(&key) {
            field.3 = metadata;
        }
        self.should_run_output_callback = true;
        Ok(())
    }

    fn on_new_topic_data(
        &mut self,
        topic: &Nt4Topic,
        _timestamp_us: f64,
        value: JsValue,
        _cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        if self.log.is_none() || topic.name_str().is_none_or(JsString::is_empty) {
            return Ok(());
        }
        let ty = topic
            .type_str()
            .map(JsString::to_string_lossy)
            .unwrap_or_default();
        let updated = match ty.as_str() {
            "boolean" => value.type_of() == "boolean",
            "int" | "float" | "double" => value.type_of() == "number",
            "string" | "json" => value.type_of() == "string",
            "boolean[]" => array_of(&value, "boolean"),
            "int[]" | "float[]" | "double[]" => array_of(&value, "number"),
            "string[]" => array_of(&value, "string"),
            _ => matches!(value, JsValue::Bytes(_)),
        };
        if updated {
            self.should_run_output_callback = true;
        }
        Ok(())
    }

    fn on_connect(&mut self, cx: &mut Nt4Context<'_>) -> Result<(), Thrown> {
        self.base.set_status(LiveDataSourceStatus::Active);
        let log = self.hub.borrow_mut().new_log();
        self.log = Some(log);
        self.should_run_output_callback = true;
        if self.connect_time.is_none_or(|t| t == 0.0) {
            self.connect_time = Some(Nt4Client::client_time_us(cx.clock));
        }
        if self.mode == Mode::AdvantageKit {
            self.no_fields_timeout = Some(cx.timers.set_timeout(5000.0));
        }
        Ok(())
    }

    fn on_disconnect(&mut self, cx: &mut Nt4Context<'_>) -> Result<(), Thrown> {
        self.base.set_status(LiveDataSourceStatus::Connecting);
        self.should_run_output_callback = false;
        self.connect_time = None;
        if let Some(t) = self.no_fields_timeout {
            cx.timers.clear(t);
        }
        Ok(())
    }
}

struct Source {
    client: Option<Nt4Client>,
    state: SourceState,
    idle_timer: Option<TimerId>,
    logging_subscription: Option<f64>,
    low_bandwidth_topic_subscription: Option<f64>,
    low_bandwidth_data_subscriptions: JsObject<f64>,
}

impl Source {
    fn periodic(&mut self, env: &mut Env) -> Result<(), Thrown> {
        let Some(client) = self.client.as_mut() else {
            return self.output(env);
        };
        let hub = Rc::clone(&self.state.hub);
        let hub = hub.borrow();
        let logging = hub
            .prefs
            .as_ref()
            .is_some_and(|p| p.live_subscribe_mode == "logging");
        let patterns = if self.state.mode == Mode::AdvantageKit {
            vec![js("/AdvantageKit/"), js("/Tuning/")]
        } else {
            vec![js("")]
        };
        if logging {
            let ids: Vec<f64> = self
                .low_bandwidth_data_subscriptions
                .values()
                .copied()
                .collect();
            for id in ids {
                client.unsubscribe(id, &mut env.cx())?;
            }
            self.low_bandwidth_data_subscriptions = JsObject::new();
            if let Some(id) = self.low_bandwidth_topic_subscription {
                client.unsubscribe(id, &mut env.cx())?;
                self.low_bandwidth_topic_subscription = None;
            }
            if self.logging_subscription.is_none() {
                self.logging_subscription =
                    Some(client.subscribe(&patterns, true, true, 0.02, &mut env.cx())?);
            }
        } else {
            if let Some(id) = self.logging_subscription {
                client.unsubscribe(id, &mut env.cx())?;
                self.logging_subscription = None;
            }
            if self.low_bandwidth_topic_subscription.is_none() {
                self.low_bandwidth_topic_subscription =
                    Some(client.subscribe_topics_only(&patterns, true, &mut env.cx())?);
            }

            let mut active: Vec<JsString> = Vec::new();
            let mut add = |field: JsString| {
                if !active.contains(&field) {
                    active.push(field);
                }
            };
            if self.state.log == Some(hub.window_log) {
                let log = &hub.logs[hub.window_log];
                let announced: Vec<JsString> = log
                    .fields
                    .iter()
                    .filter(|f| f.1 != LoggableType::Empty)
                    .map(|f| f.0.clone())
                    .collect();
                let initial: Vec<JsString> = match self.state.mode {
                    Mode::AdvantageKit => vec![js("/.schema"), js("/Timestamp")],
                    Mode::DriverStation => vec![js("DS:/.schema")],
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
                    .chain(hub.tabs.iter().cloned())
                    .chain(hub.sidebar.iter().cloned())
                    .chain(log.urcl_keys())
                    .collect();
                for key in &keys {
                    for announced_key in &announced {
                        let subscribe_key = if starts(announced_key, key) {
                            key
                        } else if starts(key, announced_key) {
                            announced_key
                        } else {
                            continue;
                        };
                        if self.state.mode == Mode::AdvantageKit {
                            if starts(subscribe_key, &js(AKIT_TUNING_PREFIX)) {
                                add(subscribe_key.clone());
                            } else {
                                let mut field = js(AKIT_PREFIX);
                                field.push_str(subscribe_key);
                                add(field);
                            }
                        } else {
                            add(subscribe_key.slice(self.state.log_prefix().len() as f64, None));
                        }
                    }
                }
            }
            let copy = active.clone();
            for field0 in &copy {
                for field1 in &copy {
                    if field0 != field1 && starts(field0, field1) {
                        active.retain(|f| f != field0);
                    }
                }
            }
            for field in &active {
                if self
                    .low_bandwidth_data_subscriptions
                    .get_key(field)
                    .is_none()
                {
                    let id = client.subscribe(
                        std::slice::from_ref(field),
                        true,
                        true,
                        0.02,
                        &mut env.cx(),
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
                if !active.contains(&field) {
                    client.unsubscribe(id, &mut env.cx())?;
                    self.low_bandwidth_data_subscriptions.remove_key(&field);
                }
            }
        }
        drop(hub);
        self.output(env)
    }

    fn output(&mut self, env: &mut Env) -> Result<(), Thrown> {
        let state = &mut self.state;
        if !state.should_run_output_callback
            || state.base.status() == LiveDataSourceStatus::Stopped
            || !state.base.has_output_callback()
        {
            return Ok(());
        }
        let Some(log) = state.log else {
            return Ok(());
        };
        state.should_run_output_callback = false;
        let time = match self.client.as_ref() {
            Some(client) => match client.server_time_us(&env.wall, None) {
                None => 10.0,
                Some(server_time) => (server_time - client.network_latency_us()) / 1000000.0,
            },
            None => 10.0,
        };
        let mut hub = state.hub.borrow_mut();
        hub.window_log = log;
        let id = hub.log_id(log);
        drop(hub);
        env.app(
            "output",
            Some(&Value::Array(vec![
                Value::number(f64::from(id)),
                Value::number(time),
            ])),
        );
        Ok(())
    }
}

pub struct SourceDriver {
    mode: Mode,
    address: String,
    hub: Rc<RefCell<Hub>>,
    sources: Vec<Source>,
}

impl SourceDriver {
    pub fn new(session: &Session, _env: &mut Env) -> Self {
        let c = &session.client;
        let mode = match c
            .get("mode")
            .and_then(Json::as_str)
            .map(|m| m.to_string_lossy())
            .as_deref()
        {
            Some("default") => Mode::Default,
            Some("akit") => Mode::AdvantageKit,
            Some("systemcore") => Mode::Systemcore,
            Some("ds") => Mode::DriverStation,
            other => panic!("unknown nt4 mode {other:?}"),
        };
        let mut hub = Hub {
            logs: Vec::new(),
            ids: Vec::new(),
            next_id: 0,
            window_log: 0,
            tabs: string_list(c.get("tabs")),
            sidebar: string_list(c.get("sidebar")),
            prefs: initial_prefs(session),
        };
        let first = hub.new_log();
        hub.log_id(first);
        let hub = Rc::new(RefCell::new(hub));
        SourceDriver {
            mode,
            address: c
                .get("address")
                .and_then(Json::as_str)
                .map_or_else(|| "127.0.0.1".to_owned(), |s| s.to_string_lossy()),
            hub,
            sources: Vec::new(),
        }
    }

    fn stop_current(&mut self, env: &mut Env) {
        let Some(source) = self.sources.last_mut() else {
            return;
        };
        source.state.base.stop(&mut env.timers);
        if let Some(client) = source.client.as_mut() {
            let state = &mut source.state;
            let _ = client.disconnect(state, &mut env.cx());
        }
    }

    fn connect(&mut self, env: &mut Env) {
        self.stop_current(env);
        if self.hub.borrow().prefs.is_none() {
            return;
        }
        let base = LiveDataSourceBase::new(&mut env.rng);
        let idle_timer = Some(env.timers.set_timeout(100.0));
        let hub = Rc::clone(&self.hub);
        let mut source = Source {
            client: None,
            state: SourceState {
                mode: self.mode,
                base,
                log: None,
                should_run_output_callback: false,
                connect_time: None,
                no_fields_timeout: None,
                hub,
            },
            idle_timer,
            logging_subscription: None,
            low_bandwidth_topic_subscription: None,
            low_bandwidth_data_subscriptions: JsObject::new(),
        };
        let trace = env.trace.clone();
        let clock = env.wall.loop_clock.clone();
        source.state.base.connect(
            &self.address,
            status_callback(&trace, &clock),
            Box::new(|_, _| {}),
            true,
            &mut env.timers,
        );
        source.state.should_run_output_callback = false;
        let log = self.hub.borrow_mut().new_log();
        source.state.log = Some(log);
        let ports = match self.mode {
            Mode::Systemcore => NT4_PORTS_SYSTEMCORE,
            Mode::DriverStation => NT4_PORTS_DS,
            _ => NT4_PORTS_DEFAULT,
        };
        let mut client = Nt4Client::new(&self.address, ports, "AdvantageScope", &mut env.timers);
        client.connect(&mut env.cx());
        source.client = Some(client);
        self.sources.push(source);
    }

    fn tuner_publish(&mut self, key: &JsString, value: &Json, env: &mut Env) -> Result<(), Thrown> {
        let source = self.tuner_source()?;
        let mode = source.state.mode;
        let remote_key = remote_key(mode, key);
        let hub = Rc::clone(&source.state.hub);
        let hub = hub.borrow();
        let window_log = &hub.logs[hub.window_log];
        let field = window_log.field(key);
        let ty = field.map(|f| f.1);
        let dog_log = window_log.field(&js("NT:/Robot/DogLog/Options")).is_some();
        if !matches!(ty, Some(LoggableType::Number | LoggableType::Boolean))
            || starts(&remote_key, &js(AKIT_PREFIX))
            || (dog_log && starts(&remote_key, &js("/Robot")))
        {
            return Ok(());
        }
        let metadata = field.map(|f| f.3.clone()).unwrap_or_default();
        if !metadata.is_empty()
            && let Ok(JsValue::Object(parsed)) = json::parse(&metadata)
            && let Some(JsValue::Bool(mutable)) = parsed.get("mutable")
            && !*mutable
        {
            return Ok(());
        }
        let Some(wpilib_type) = field.and_then(|f| f.2.clone()) else {
            return Ok(());
        };
        let client = source.client.as_mut().expect("tuner client");
        client.publish_topic(&remote_key, &wpilib_type, &mut env.cx())?;
        if let Some(timestamp) = client.server_time_us(&env.wall, None) {
            let value = match value {
                Json::Number(n) if starts(&wpilib_type, &js("int")) => {
                    JsValue::Number(math::floor(*n))
                }
                Json::Number(n) => JsValue::Number(*n),
                Json::Bool(b) => JsValue::Bool(*b),
                other => panic!("tune value {other:?}"),
            };
            client.add_timestamped_sample(&remote_key, timestamp, value, &mut env.cx())?;
        }
        Ok(())
    }

    fn tuner_source(&mut self) -> Result<&mut Source, Thrown> {
        let source = self
            .sources
            .last_mut()
            .ok_or_else(|| Thrown("no tuner".to_owned()))?;
        if source.client.is_none() || source.state.log.is_none() {
            return Err(Thrown(
                "Cannot create NT4 tuner before starting connection".to_owned(),
            ));
        }
        Ok(source)
    }
}

fn remote_key(mode: Mode, key: &JsString) -> JsString {
    if mode == Mode::AdvantageKit {
        if starts(key, &js(AKIT_TUNING_PREFIX)) {
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

fn op_name(op: &[Json]) -> String {
    op[0].as_str().expect("op name").to_string_lossy()
}

fn op_key(op: &[Json], i: usize) -> JsString {
    JsString::from_units(op[i].as_str().expect("key").units().to_vec())
}

impl Driver for SourceDriver {
    fn op(&mut self, op: &[Json], env: &mut Env) -> Result<(), Thrown> {
        match op_name(op).as_str() {
            "connect" => self.connect(env),
            "stop" => self.stop_current(env),
            "prefs" => prefs_op(&mut self.hub.borrow_mut().prefs, &op[1]),
            "fields" => {
                let mut hub = self.hub.borrow_mut();
                hub.tabs = string_list(op.get(1));
                hub.sidebar = string_list(op.get(2));
            }
            "tune" => self.tuner_publish(&op_key(op, 1), &op[2], env)?,
            "untune" => {
                let source = self.tuner_source()?;
                let remote = remote_key(source.state.mode, &op_key(op, 1));
                source
                    .client
                    .as_mut()
                    .expect("tuner client")
                    .unpublish_topic(&remote, &mut env.cx())?;
            }
            "dump" => {}
            other => panic!("the NT4Source driver does not model {other}"),
        }
        Ok(())
    }

    fn notify(&mut self, event: Notify, env: &mut Env) {
        for source in &mut self.sources {
            if let Some(client) = source.client.as_mut() {
                support::dispatch(client, event.clone(), &mut source.state, &mut env.cx());
            }
        }
    }

    fn timer(&mut self, id: TimerId, env: &mut Env) {
        for source in &mut self.sources {
            if source.idle_timer == Some(id) {
                source.idle_timer = None;
                if source.periodic(env).is_ok() {
                    source.idle_timer = Some(env.timers.set_timeout(100.0));
                }
                return;
            }
            if source.state.no_fields_timeout == Some(id) {
                let content = "No fields were received from the server. AdvantageKit mode is selected. Are you connecting to a server without AdvantageKit?";
                env.app(
                    "main",
                    Some(&Value::Array(vec![
                        Value::string("error"),
                        Value::object([
                            ("title", Value::string("Problem with NT4 connection")),
                            ("content", Value::string(content)),
                        ]),
                    ])),
                );
                return;
            }
            if let Some(client) = source.client.as_mut()
                && client
                    .handle_timer(id, &mut source.state, &mut env.cx())
                    .is_some()
            {
                return;
            }
            if source.state.base.handle_timer(id, None) {
                return;
            }
        }
    }
}

struct Publisher {
    client: Nt4Client,
    interval: TimerId,
    stopped: bool,
    published_topics: JsObject<Option<JsValue>>,
    trace: Trace,
}

struct PublisherHandler<'a> {
    stopped: bool,
    trace: &'a Trace,
    now: f64,
}

impl Nt4Handler for PublisherHandler<'_> {
    fn on_connect(&mut self, _cx: &mut Nt4Context<'_>) -> Result<(), Thrown> {
        if !self.stopped {
            self.trace
                .app(self.now, "status", Some(&Value::number(1.0)));
        }
        Ok(())
    }

    fn on_disconnect(&mut self, _cx: &mut Nt4Context<'_>) -> Result<(), Thrown> {
        if !self.stopped {
            self.trace
                .app(self.now, "status", Some(&Value::number(0.0)));
        }
        Ok(())
    }
}

pub struct PublisherDriver {
    log: Log,
    prefs: Option<Prefs>,
    selected: Option<f64>,
    hovered: Option<f64>,
    publishers: Vec<Publisher>,
}

impl PublisherDriver {
    pub fn new(session: &Session, _env: &mut Env) -> Self {
        assert!(
            session
                .client
                .get("mode")
                .and_then(Json::as_str)
                .is_some_and(|m| m.eq_str("sim")),
            "the publisher driver models sim mode only"
        );
        PublisherDriver {
            log: Log::new(true),
            prefs: initial_prefs(session),
            selected: None,
            hovered: None,
            publishers: Vec::new(),
        }
    }

    fn stop_current(&mut self, env: &mut Env) {
        let Some(p) = self.publishers.last_mut() else {
            return;
        };
        p.stopped = true;
        env.app("status", Some(&Value::number(2.0)));
        let mut handler = PublisherHandler {
            stopped: true,
            trace: &p.trace,
            now: env.now(),
        };
        let _ = p.client.disconnect(&mut handler, &mut env.cx());
        env.timers.clear(p.interval);
    }

    fn periodic(&mut self, i: usize, env: &mut Env) -> Result<(), Thrown> {
        let p = &mut self.publishers[i];
        let server_time = p.client.server_time_us(&env.wall, None).unwrap_or(0.0);
        let Some(prefs) = self.prefs.as_ref() else {
            return Ok(());
        };
        let log = &self.log;
        let keys = log.get_field_keys();
        let topics: Vec<JsString> =
            filter_field_by_prefixes(&keys, &prefs.publish_filter, true, true)
                .into_iter()
                .filter(|t| {
                    !starts(t, &js("$"))
                        && !log.is_generated(t)
                        && log.get_type(t).is_some_and(|ty| ty != LoggableType::Empty)
                })
                .collect();
        for topic in &topics {
            if p.published_topics.get_key(topic).is_none() {
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
                p.client
                    .publish_topic(&topic.slice(3.0, None), &ty, &mut env.cx())?;
                p.published_topics.insert(topic.clone(), None);
            }
        }
        let published: Vec<JsString> = p.published_topics.keys().cloned().collect();
        for topic in &published {
            if !topics.contains(topic) {
                p.client
                    .unpublish_topic(&topic.slice(3.0, None), &mut env.cx())?;
                p.published_topics.remove_key(topic);
            }
        }
        let time = self
            .selected
            .or(self.hovered)
            .unwrap_or_else(|| log.get_timestamp_range()[0]);
        let published: Vec<JsString> = p.published_topics.keys().cloned().collect();
        for topic in &published {
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
            let last = p.published_topics.get_key(topic).cloned().flatten();
            let changed = match (&last, &value) {
                (None, _) => true,
                (Some(a), Some(b)) => !js_values_equal(ty, a, b),
                (Some(_), None) => true,
            };
            if changed {
                p.published_topics.insert(topic.clone(), value.clone());
                if let Some(value) = value {
                    p.client.add_timestamped_sample(
                        &topic.slice(3.0, None),
                        server_time,
                        value,
                        &mut env.cx(),
                    )?;
                }
            }
        }
        Ok(())
    }

    fn log_op(&mut self, op: &[Json]) -> Result<(), Thrown> {
        let method = op_name(&op[1..]);
        let key = op_key(op, 2);
        let args: Vec<JsValue> = op[3..].iter().map(js_arg).collect::<Result<_, _>>()?;
        let num = |i: usize| args[i].as_f64().expect("number");
        let log = &mut self.log;
        match method.as_str() {
            "putNumber" => log.put_number(&key, num(0), num(1)),
            "putBoolean" => log.put_boolean(&key, num(0), args[1].as_bool().expect("bool")),
            "putString" => log.put_string(&key, num(0), args[1].as_str().expect("string").clone()),
            "putRaw" => match &args[1] {
                JsValue::Bytes(b) => log.put_raw(&key, num(0), b.clone()),
                other => panic!("putRaw {other:?}"),
            },
            "putNumberArray" => log.put_number_array(
                &key,
                num(0),
                args[1]
                    .as_array()
                    .expect("array")
                    .iter()
                    .map(|v| v.as_f64().expect("number"))
                    .collect(),
            ),
            "putBooleanArray" => log.put_boolean_array(
                &key,
                num(0),
                args[1]
                    .as_array()
                    .expect("array")
                    .iter()
                    .map(|v| v.as_bool().expect("bool"))
                    .collect(),
            ),
            "putStringArray" => log.put_string_array(
                &key,
                num(0),
                args[1]
                    .as_array()
                    .expect("array")
                    .iter()
                    .map(|v| v.as_str().expect("string").clone())
                    .collect(),
            ),
            "setWpilibType" => log.set_wpilib_type(&key, args[0].as_str().expect("type").clone()),
            other => panic!("the publisher driver does not model Log.{other}"),
        }
        Ok(())
    }
}

fn js_values_equal(ty: LoggableType, a: &JsValue, b: &JsValue) -> bool {
    match (ty, a, b) {
        (LoggableType::Raw, JsValue::Bytes(x), JsValue::Bytes(y)) => {
            log_values_equal(&RawValue::from(x.clone()), &RawValue::from(y.clone()))
        }
        (LoggableType::Boolean, JsValue::Bool(x), JsValue::Bool(y)) => log_values_equal(x, y),
        (LoggableType::Number, JsValue::Number(x), JsValue::Number(y)) => log_values_equal(x, y),
        (LoggableType::String, JsValue::String(x), JsValue::String(y)) => log_values_equal(x, y),
        (_, JsValue::Array(x), JsValue::Array(y)) => match ty {
            LoggableType::BooleanArray => log_values_equal(
                &x.iter()
                    .map(|v| v.as_bool().unwrap_or_default())
                    .collect::<Vec<_>>(),
                &y.iter()
                    .map(|v| v.as_bool().unwrap_or_default())
                    .collect::<Vec<_>>(),
            ),
            LoggableType::NumberArray => log_values_equal(
                &x.iter()
                    .map(|v| v.as_f64().unwrap_or_default())
                    .collect::<Vec<_>>(),
                &y.iter()
                    .map(|v| v.as_f64().unwrap_or_default())
                    .collect::<Vec<_>>(),
            ),
            _ => log_values_equal(
                &x.iter()
                    .map(|v| v.as_str().cloned().unwrap_or_default())
                    .collect::<Vec<_>>(),
                &y.iter()
                    .map(|v| v.as_str().cloned().unwrap_or_default())
                    .collect::<Vec<_>>(),
            ),
        },
        _ => false,
    }
}

impl Driver for PublisherDriver {
    fn op(&mut self, op: &[Json], env: &mut Env) -> Result<(), Thrown> {
        match op_name(op).as_str() {
            "connect" => {
                if !self.publishers.is_empty() {
                    self.stop_current(env);
                }
                env.app("status", Some(&Value::number(0.0)));
                let mut client = Nt4Client::new(
                    "127.0.0.1",
                    NT4_PORTS_DEFAULT,
                    "AdvantageScope",
                    &mut env.timers,
                );
                client.connect(&mut env.cx());
                let interval = env.timers.set_interval(0.02 * 1000.0);
                self.publishers.push(Publisher {
                    client,
                    interval,
                    stopped: false,
                    published_topics: JsObject::new(),
                    trace: env.trace.clone(),
                });
            }
            "stop" => self.stop_current(env),
            "prefs" => prefs_op(&mut self.prefs, &op[1]),
            "select" => {
                self.selected = op[1].as_f64();
                self.hovered = op[2].as_f64();
            }
            "log" => self.log_op(op)?,
            "dump" => {}
            other => panic!("the publisher driver does not model {other}"),
        }
        Ok(())
    }

    fn notify(&mut self, event: Notify, env: &mut Env) {
        for p in &mut self.publishers {
            let mut handler = PublisherHandler {
                stopped: p.stopped,
                trace: &p.trace,
                now: env.now(),
            };
            support::dispatch(&mut p.client, event.clone(), &mut handler, &mut env.cx());
        }
    }

    fn timer(&mut self, id: TimerId, env: &mut Env) {
        for i in 0..self.publishers.len() {
            if self.publishers[i].interval == id {
                let _ = self.periodic(i, env);
                return;
            }
            let p = &mut self.publishers[i];
            let mut handler = PublisherHandler {
                stopped: p.stopped,
                trace: &p.trace,
                now: env.now(),
            };
            if p.client
                .handle_timer(id, &mut handler, &mut env.cx())
                .is_some()
            {
                return;
            }
        }
    }
}
