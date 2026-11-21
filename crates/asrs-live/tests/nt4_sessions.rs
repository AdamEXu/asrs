//! Replays every NT4 live session against the port and compares the trace with the oracle's,
//! line for line. `nt4-client` sessions drive `NT4_Client` directly; `nt4` and
//! `nt4-publisher` sessions drive it the way `NT4Source`/`NT4Tuner` and `NT4Publisher` do,
//! through the drivers in `nt4_drivers` over the real ports.

#![allow(clippy::disallowed_methods)]

mod nt4_drivers;
mod support;

use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_live::Thrown;
use asrs_live::TimerId;
use asrs_live::nt4::{NT4_PORTS_DEFAULT, Nt4Client, Nt4Context, Nt4Handler, Nt4Topic};
use asrs_log::log::Log;
use asrs_parity::Value;
use asrs_parity::json::Json;
use asrs_parity::live::{self, Session};


use support::{Driver, Env, js_arg, js_value};

struct Recorder<'a> {
    trace: &'a support::Trace,
    now: f64,
}

impl Recorder<'_> {
    fn event(&self, v: Vec<Value>) {
        self.trace.app(self.now, "nt4", Some(&Value::Array(v)));
    }

    fn topic(&self, kind: &str, topic: &Nt4Topic) {
        self.event(vec![
            Value::string(kind),
            js_value(&topic.uid),
            js_value(&topic.name),
            js_value(&topic.ty),
            js_value(&topic.properties),
        ]);
    }
}

impl Nt4Handler for Recorder<'_> {
    fn on_topic_announce(
        &mut self,
        topic: &Nt4Topic,
        _cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        self.topic("announce", topic);
        Ok(())
    }

    fn on_topic_unannounce(
        &mut self,
        topic: &Nt4Topic,
        _cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        self.topic("unannounce", topic);
        Ok(())
    }

    fn on_topic_properties_update(
        &mut self,
        topic: &Nt4Topic,
        _cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        self.topic("properties", topic);
        Ok(())
    }

    fn on_new_topic_data(
        &mut self,
        topic: &Nt4Topic,
        timestamp_us: f64,
        value: JsValue,
        _cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        self.event(vec![
            Value::string("data"),
            js_value(&topic.uid),
            js_value(&topic.name),
            Value::number(timestamp_us),
            js_value(&value),
        ]);
        Ok(())
    }

    fn on_connect(&mut self, _cx: &mut Nt4Context<'_>) -> Result<(), Thrown> {
        self.event(vec![Value::string("connect")]);
        Ok(())
    }

    fn on_disconnect(&mut self, _cx: &mut Nt4Context<'_>) -> Result<(), Thrown> {
        self.event(vec![Value::string("disconnect")]);
        Ok(())
    }
}

/// Calls each client entry point with a recording handler, as the oracle's `nt4-client`
/// source does.
pub fn with_handler<R>(
    env: &mut Env,
    f: impl FnOnce(&mut dyn Nt4Handler, &mut Nt4Context<'_>) -> R,
) -> R {
    let trace = env.trace.clone();
    let mut recorder = Recorder {
        trace: &trace,
        now: env.now(),
    };
    f(&mut recorder, &mut env.cx())
}

struct ClientDriver {
    address: String,
    ports: Vec<u16>,
    app_name: String,
    client: Option<Nt4Client>,
    uids: Vec<f64>,
    log: Log,
}

fn str_arg(v: &JsValue) -> Result<JsString, Thrown> {
    v.as_str()
        .cloned()
        .ok_or_else(|| Thrown::new("expected a string"))
}

fn bool_arg(v: Option<&JsValue>, default: bool) -> bool {
    match v {
        None | Some(JsValue::Undefined) => default,
        Some(JsValue::Bool(b)) => *b,
        Some(other) => panic!("expected a boolean, got {other:?}"),
    }
}

fn num_arg(v: Option<&JsValue>, default: f64) -> f64 {
    match v {
        None | Some(JsValue::Undefined) => default,
        Some(JsValue::Number(n)) => *n,
        Some(other) => panic!("expected a number, got {other:?}"),
    }
}

fn strings(v: &JsValue) -> Vec<JsString> {
    v.as_array()
        .expect("a list of patterns")
        .iter()
        .map(|p| p.as_str().cloned().expect("string patterns"))
        .collect()
}

impl ClientDriver {
    fn nt4_op(&mut self, args: &[Json], env: &mut Env) -> Result<(), Thrown> {
        let Some(client) = self.client.as_mut() else {
            return Err(Thrown::new("no nt4 client"));
        };
        let name = args[0].as_str().expect("op name").to_string_lossy();
        let v: Vec<JsValue> = args[1..].iter().map(js_arg).collect::<Result<_, _>>()?;
        let uids = &mut self.uids;
        let uid = |k: f64| {
            if k.fract() == 0.0 && k >= 0.0 && (k as usize) < uids.len() {
                uids[k as usize]
            } else {
                k
            }
        };
        match name.as_str() {
            "subscribe" => {
                let id = client.subscribe(
                    &strings(&v[0]),
                    bool_arg(v.get(1), false),
                    bool_arg(v.get(2), false),
                    num_arg(v.get(3), 0.1),
                    &mut env.cx(),
                )?;
                self.uids.push(id);
            }
            "subscribeTopicsOnly" => {
                let id = client.subscribe_topics_only(
                    &strings(&v[0]),
                    bool_arg(v.get(1), false),
                    &mut env.cx(),
                )?;
                self.uids.push(id);
            }
            "unsubscribe" => {
                let id = uid(num_arg(v.first(), f64::NAN));
                client.unsubscribe(id, &mut env.cx())?;
            }
            "clearAllSubscriptions" => client.clear_all_subscriptions(&mut env.cx())?,
            "publish" => client.publish_topic(&str_arg(&v[0])?, &str_arg(&v[1])?, &mut env.cx())?,
            "unpublish" => client.unpublish_topic(&str_arg(&v[0])?, &mut env.cx())?,
            "setProperties" => {
                let properties = v[1].as_object().cloned().expect("an object");
                client.set_properties(&str_arg(&v[0])?, &properties, &mut env.cx())?;
            }
            "setPersistent" => {
                client.set_persistent(&str_arg(&v[0])?, bool_arg(v.get(1), false), &mut env.cx())?
            }
            "setRetained" => {
                client.set_retained(&str_arg(&v[0])?, bool_arg(v.get(1), false), &mut env.cx())?
            }
            "sample" => client.add_sample(&str_arg(&v[0])?, v[1].clone(), &mut env.cx())?,
            "sampleAt" => client.add_timestamped_sample(
                &str_arg(&v[0])?,
                num_arg(v.get(1), f64::NAN),
                v[2].clone(),
                &mut env.cx(),
            )?,
            "time" => {
                let client_time = Nt4Client::client_time_us(&env.wall);
                let server_time = client.server_time_us(&env.wall, None);
                env.app(
                    "nt4",
                    Some(&Value::Array(vec![
                        Value::string("time"),
                        Value::number(client_time),
                        server_time.map_or(Value::Null, Value::number),
                        Value::number(client.network_latency_us()),
                    ])),
                );
            }
            "connect" => client.connect(&mut env.cx()),
            "disconnect" => with_handler(env, |h, cx| client.disconnect(h, cx))?,
            other => panic!("unknown nt4 op {other}"),
        }
        Ok(())
    }
}

impl Driver for ClientDriver {
    fn op(&mut self, op: &[Json], env: &mut Env) -> Result<(), Thrown> {
        let name = op[0].as_str().expect("op name").to_string_lossy();
        match name.as_str() {
            "connect" => {
                let client = self.client.get_or_insert_with(|| {
                    Nt4Client::new(&self.address, &self.ports, &self.app_name, &mut env.timers)
                });
                client.connect(&mut env.cx());
                Ok(())
            }
            "stop" => match self.client.as_mut() {
                Some(client) => with_handler(env, |h, cx| client.disconnect(h, cx)),
                None => Ok(()),
            },
            "nt4" => self.nt4_op(&op[1..], env),
            "log" => support::log_op(&mut self.log, &mut env.realm, op),
            "dump" => {
                env.app(
                    "dump",
                    Some(&Value::Array(vec![
                        Value::number(0.0),
                        support::dump(&self.log),
                    ])),
                );
                Ok(())
            }
            other => panic!("nt4-client sessions do not use {other}"),
        }
    }

    fn notify(&mut self, event: support::SessionEvent, env: &mut Env) {
        let support::SessionEvent::Net(event) = event else {
            return;
        };
        let Some(client) = self.client.as_mut() else {
            return;
        };
        with_handler(env, |h, cx| support::dispatch(client, &event, h, cx));
    }

    fn timer(&mut self, id: TimerId, env: &mut Env) {
        if let Some(client) = self.client.as_mut() {
            let _ = with_handler(env, |h, cx| client.handle_timer(id, h, cx));
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

fn client_driver(session: &Session) -> ClientDriver {
    let c = &session.client;
    let text = |key: &str, default: &str| {
        c.get(key)
            .and_then(Json::as_str)
            .map_or_else(|| default.to_owned(), |s| s.to_string_lossy())
    };
    let ports = match c.get("ports").and_then(Json::as_array) {
        Some(ports) => ports
            .iter()
            .map(|p| p.as_f64().expect("numeric port") as u16)
            .collect(),
        None => NT4_PORTS_DEFAULT.to_vec(),
    };
    ClientDriver {
        address: text("address", "127.0.0.1"),
        ports,
        app_name: text("appName", "AdvantageScope"),
        client: None,
        uids: Vec::new(),
        log: Log::new(true),
    }
}

pub fn replay(id: &str, session: &Session) -> Result<usize, String> {
    let source = session
        .client
        .get("source")
        .and_then(Json::as_str)
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    let produced = support::run(session, |env| -> Box<dyn Driver> {
        match source.as_str() {
            "nt4-client" => Box::new(client_driver(session)),
            "nt4" => Box::new(nt4_drivers::SourceDriver::new(session, env)),
            "nt4-publisher" => Box::new(nt4_drivers::PublisherDriver::new(session)),
            other => panic!("{other} is not an NT4 source"),
        }
    })?;
    support::check_trace(id, produced)
}

fn nt4_sessions() -> Vec<(String, Session)> {
    live::session_files()
        .into_iter()
        .filter_map(|path| {
            let session = Session::load(&path).unwrap_or_else(|e| panic!("{e}"));
            (session.protocol == "nt4").then(|| (live::vector_id(&path), session))
        })
        .collect()
}

#[test]
fn every_nt4_session_matches_the_oracle_trace() {
    let sessions = nt4_sessions();
    assert!(sessions.len() >= 3);
    let mut failures = Vec::new();
    for (id, session) in &sessions {
        match replay(id, session) {
            Ok(lines) => {
                assert!(lines > 10, "{id}: only {lines} lines");
                eprintln!("{id}: {lines} lines equal");
            }
            Err(e) => failures.push(e),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn a_one_millisecond_clock_skew_is_caught() {
    let (id, mut session) = nt4_sessions()
        .into_iter()
        .find(|(id, _)| id == "live/nt4/client-v41-api")
        .expect("session");
    session.epoch_ms += 1.0;
    let error = replay(&id, &session).unwrap_err();
    assert!(error.contains("differs"), "{error}");
}

#[test]
fn a_wrong_published_sample_is_caught() {
    let (id, mut session) = nt4_sessions()
        .into_iter()
        .find(|(id, _)| id == "live/nt4/publisher-sim")
        .expect("session");
    let step = session
        .client_steps
        .iter_mut()
        .find(|s| s.op[0].as_str().is_some_and(|op| op.eq_str("select")))
        .expect("select step");
    step.op[1] = Json::Number(1.9);
    let error = replay(&id, &session).unwrap_err();
    assert!(error.contains("differs"), "{error}");
}
