//! Thin adapters driving the real `NT4Source`/`NT4Tuner` and `NT4Publisher` ports the way
//! `oracle/src/liveSession.ts` does: `window` is `Nt4Window` plus the session's tabs, sidebar
//! and preferences, the output callback assigns `window.log` and records `output` events with
//! the oracle's `logId` numbering, and `dump` ops serialize `window.log`. Stopped sources stay
//! alive so their idle loops keep running, as upstream's do.
//!
//! N03 must add robot mode (`getRobotAddress`); this driver runs sim only.

use std::cell::RefCell;
use std::rc::Rc;

use asrs_js::string::JsString;
use asrs_js::time::Clock;
use asrs_js::value::{JsObject, JsValue};
use asrs_live::nt4::{
    NT4_PORTS_DEFAULT, NetEvent, Nt4Client, Nt4Context, Nt4Handler, Nt4Mode, Nt4Source, Nt4Tuner, Nt4Window,
};
use asrs_live::{
    LiveDataSourceStatus, LiveLog, LivePreferences, OutputCallback, Thrown, TimerId, TunedValue,
};
use asrs_log::log::Log;
use asrs_log::log::LoggableType;
use asrs_log::log::log_key_utils::filter_field_by_prefixes;
use asrs_log::log::log_util::{get_or_default, log_values_equal};
use asrs_log::log::log_value_sets::RawValue;
use asrs_parity::Value;
use asrs_parity::json::Json;
use asrs_parity::live::Session;

use crate::support::{self, Driver, Env, Trace};

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
    live_discard: f64,
    publish_filter: JsString,
}

impl Prefs {
    fn defaults() -> Self {
        Prefs {
            live_subscribe_mode: "low-bandwidth".to_owned(),
            live_discard: 1200.0,
            publish_filter: JsString::new(),
        }
    }

    fn merge(&mut self, json: &Json) {
        if let Some(mode) = json.get("liveSubscribeMode").and_then(Json::as_str) {
            self.live_subscribe_mode = mode.to_string_lossy();
        }
        if let Some(discard) = json.get("liveDiscard").and_then(Json::as_f64) {
            self.live_discard = discard;
        }
        if let Some(filter) = json.get("publishFilter").and_then(Json::as_str) {
            self.publish_filter = JsString::from_units(filter.units().to_vec());
        }
    }

    fn as_live(&self) -> LivePreferences {
        LivePreferences {
            live_discard: Some(self.live_discard),
            live_subscribe_mode: Some(js(&self.live_subscribe_mode)),
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

/// The oracle's `logId(log)`: the index a `Log` object first reached the trace under.
fn log_id(ids: &Rc<RefCell<Vec<usize>>>, log: Option<&LiveLog>) -> Value {
    let Some(log) = log else {
        return Value::Null;
    };
    let ptr = Rc::as_ptr(log) as usize;
    let mut ids = ids.borrow_mut();
    if !ids.contains(&ptr) {
        ids.push(ptr);
    }
    Value::number(ids.iter().position(|p| *p == ptr).expect("inserted") as f64)
}

pub struct SourceDriver {
    mode: Nt4Mode,
    address: String,
    prefs: Option<Prefs>,
    window: Nt4Window,
    window_log: Rc<RefCell<LiveLog>>,
    live_logs: Rc<RefCell<Vec<LiveLog>>>,
    log_ids: Rc<RefCell<Vec<usize>>>,
    sources: Vec<Nt4Source>,
}

impl SourceDriver {
    pub fn new(session: &Session, env: &mut Env) -> Self {
        let c = &session.client;
        let mode = match c
            .get("mode")
            .and_then(Json::as_str)
            .map(|m| m.to_string_lossy())
            .as_deref()
        {
            Some("default") => Nt4Mode::Default,
            Some("akit") => Nt4Mode::AdvantageKit,
            Some("systemcore") => Nt4Mode::Systemcore,
            Some("ds") => Nt4Mode::DriverStation,
            other => panic!("unknown nt4 mode {other:?}"),
        };
        let initial: LiveLog = Rc::new(RefCell::new(Log::new(true)));
        let prefs = initial_prefs(session);
        let trace = env.trace.clone();
        let clock = env.wall.loop_clock.clone();
        SourceDriver {
            mode,
            address: c
                .get("address")
                .and_then(Json::as_str)
                .map_or_else(|| "127.0.0.1".to_owned(), |s| s.to_string_lossy()),
            window: Nt4Window {
                log: initial.clone(),
                preferences: prefs.as_ref().map(Prefs::as_live),
                tabs_fields: string_list(c.get("tabs")),
                sidebar_fields: string_list(c.get("sidebar")),
                send_main_message: Box::new(move |name, data| {
                    trace.app(
                        clock.now(),
                        "main",
                        Some(&Value::Array(vec![
                            Value::string(name),
                            support::js_value(&data),
                        ])),
                    );
                }),
            },
            window_log: Rc::new(RefCell::new(initial.clone())),
            live_logs: Rc::new(RefCell::new(vec![initial.clone()])),
            log_ids: Rc::new(RefCell::new(vec![Rc::as_ptr(&initial) as usize])),
            prefs,
            sources: Vec::new(),
        }
    }

    /// The output callback writes `window.log`; re-read it before touching the window.
    fn sync_window(&mut self) {
        self.window.log = self.window_log.borrow().clone();
        self.window.preferences = self.prefs.as_ref().map(Prefs::as_live);
    }

    fn stop_current(&mut self, env: &mut Env) -> Result<(), Thrown> {
        if let Some(source) = self.sources.last_mut() {
            source.stop(&mut env.cx())?;
        }
        Ok(())
    }

    fn connect(&mut self, env: &mut Env) -> Result<(), Thrown> {
        self.stop_current(env)?;
        if self.prefs.is_none() {
            return Ok(());
        }
        let trace = env.trace.clone();
        let clock = env.wall.loop_clock.clone();
        let window_log = Rc::clone(&self.window_log);
        let live_logs = Rc::clone(&self.live_logs);
        let log_ids = Rc::clone(&self.log_ids);
        let output: OutputCallback = Box::new(move |log, supplier| {
            if let Some(l) = &log {
                *window_log.borrow_mut() = Rc::clone(l);
                live_logs.borrow_mut().push(Rc::clone(l));
            }
            trace.app(
                clock.now(),
                "output",
                Some(&Value::Array(vec![
                    log_id(&log_ids, log.as_ref()),
                    Value::number(supplier()),
                ])),
            );
        });
        let status = status_callback(&env.trace, &env.wall.loop_clock);
        let mut source = Nt4Source::new(
            self.mode,
            &mut *env.realm.random,
            Rc::new(env.wall.clone()),
            &mut env.timers,
        );
        let prefs = self.prefs.as_ref().map(Prefs::as_live);
        source.connect(
            &self.address,
            status,
            output,
            prefs.as_ref(),
            &mut env.cx(),
        );
        self.sources.push(source);
        Ok(())
    }

    /// `liveSource?.getTuner() ?? throw "no tuner"`.
    fn tuner(&self) -> Result<Nt4Tuner, Thrown> {
        match self.sources.last() {
            Some(source) => source.tuner(),
            None => Err(Thrown::new("no tuner")),
        }
    }

    fn dump(&mut self, env: &mut Env) {
        let log = self.window_log.borrow().clone();
        env.app(
            "dump",
            Some(&Value::Array(vec![
                log_id(&self.log_ids, Some(&log)),
                support::dump(&log.borrow()),
            ])),
        );
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
        self.sync_window();
        match op_name(op).as_str() {
            "connect" => self.connect(env)?,
            "stop" => self.stop_current(env)?,
            "prefs" => prefs_op(&mut self.prefs, &op[1]),
            "fields" => {
                self.window.tabs_fields = string_list(op.get(1));
                self.window.sidebar_fields = string_list(op.get(2));
            }
            "log" => {
                let log = self.window.log.clone();
                support::log_op(&mut log.borrow_mut(), &mut env.realm, op)?;
            }
            "tune" => {
                let value = match &op[2] {
                    Json::Number(n) => TunedValue::Number(*n),
                    Json::Bool(b) => TunedValue::Boolean(*b),
                    other => panic!("tune value {other:?}"),
                };
                let key = op_key(op, 1);
                let log = self.window.log.clone();
                self.tuner()?.publish(&log, &key, value, &mut env.cx())?;
            }
            "untune" => {
                let key = op_key(op, 1);
                self.tuner()?.unpublish(&key, &mut env.cx())?;
            }
            "dump" => self.dump(env),
            other => panic!("the NT4Source driver does not model {other}"),
        }
        Ok(())
    }

    fn notify(&mut self, event: NetEvent, env: &mut Env) {
        for source in &mut self.sources {
            let _ = source.handle_net_event(&event, &mut env.cx());
        }
    }

    fn timer(&mut self, id: TimerId, env: &mut Env) {
        self.sync_window();
        let window = &mut self.window;
        let mut cx = env.cx();
        for source in &mut self.sources {
            if source.handle_timer(id, window, &mut cx) {
                return;
            }
        }
    }

    fn finish(&mut self, env: &mut Env) {
        self.dump(env);
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
            "log" => support::log_op(&mut self.log, &mut env.realm, op)?,
            "dump" => {
                // `g.log` is the publisher's log, registered under id 0 at session start.
                env.app(
                    "dump",
                    Some(&Value::Array(vec![
                        Value::number(0.0),
                        support::dump(&self.log),
                    ])),
                );
            }
            other => panic!("the publisher driver does not model {other}"),
        }
        Ok(())
    }

    fn notify(&mut self, event: NetEvent, env: &mut Env) {
        for p in &mut self.publishers {
            let mut handler = PublisherHandler {
                stopped: p.stopped,
                trace: &p.trace,
                now: env.now(),
            };
            support::dispatch(&mut p.client, &event, &mut handler, &mut env.cx());
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

    fn finish(&mut self, env: &mut Env) {
        env.app(
            "dump",
            Some(&Value::Array(vec![
                Value::number(0.0),
                support::dump(&self.log),
            ])),
        );
    }
}
