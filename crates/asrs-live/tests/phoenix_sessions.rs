//! Replays every Phoenix live session against the `PhoenixDiagnosticsSource` port and
//! compares the trace with the oracle's, line for line. The driver stands in for
//! `liveSession.ts`: `window` is `PhoenixWindow` plus the session's tabs and preferences,
//! the output callback assigns `window.log` and records `output` events with the oracle's
//! `logId` numbering, and `dump` ops serialize `window.log`. Stopped sources stay alive so
//! their in-flight requests keep settling, as upstream's do.

#![allow(clippy::disallowed_methods)]

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use asrs_js::string::JsString;
use asrs_js::time::Clock;
use asrs_live::nt4::NetEvent;
use asrs_live::phoenix::{PhoenixSource, PhoenixWindow};
use asrs_live::{LiveDataSourceStatus, LiveLog, LivePreferences, OutputCallback, Thrown, TimerId};
use asrs_log::log::Log;
use asrs_parity::Value;
use asrs_parity::json::Json;
use asrs_parity::live::{self, Session};

use support::{Driver, Env, Trace};

/// `window.preferences`, reduced to `liveDiscard`, the only one the Phoenix source reads;
/// `None` is `null`.
struct Prefs {
    live_discard: f64,
}

impl Prefs {
    fn defaults() -> Self {
        Prefs {
            live_discard: 1200.0,
        }
    }

    fn merge(&mut self, json: &Json) {
        if let Some(discard) = json.get("liveDiscard").and_then(Json::as_f64) {
            self.live_discard = discard;
        }
    }

    fn as_live(&self) -> LivePreferences {
        LivePreferences {
            live_discard: Some(self.live_discard),
            live_subscribe_mode: None,
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

struct PhoenixDriver {
    address: String,
    prefs: Option<Prefs>,
    window: PhoenixWindow,
    window_log: Rc<RefCell<LiveLog>>,
    live_logs: Rc<RefCell<Vec<LiveLog>>>,
    log_ids: Rc<RefCell<Vec<usize>>>,
    sources: Vec<PhoenixSource>,
}

impl PhoenixDriver {
    fn new(session: &Session, _env: &mut Env) -> Self {
        let c = &session.client;
        let initial: LiveLog = Rc::new(RefCell::new(Log::new(true)));
        let prefs = initial_prefs(session);
        PhoenixDriver {
            address: c
                .get("address")
                .and_then(Json::as_str)
                .map_or_else(|| "127.0.0.1".to_owned(), |s| s.to_string_lossy()),
            window: PhoenixWindow {
                tabs_fields: string_list(c.get("tabs")),
                preferences: prefs.as_ref().map(Prefs::as_live),
            },
            window_log: Rc::new(RefCell::new(initial.clone())),
            live_logs: Rc::new(RefCell::new(vec![initial.clone()])),
            log_ids: Rc::new(RefCell::new(vec![Rc::as_ptr(&initial) as usize])),
            prefs,
            sources: Vec::new(),
        }
    }

    /// `liveSource?.stop()`: only the latest source; the rest keep settling on their own.
    fn stop_current(&mut self, env: &mut Env) {
        if let Some(source) = self.sources.last_mut() {
            source.stop(&mut env.phoenix_cx());
        }
    }

    fn connect(&mut self, env: &mut Env) {
        self.stop_current(env);
        if self.prefs.is_none() {
            return;
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
        let clock: Rc<dyn Clock> = Rc::new(env.wall.clone());
        let mut source = PhoenixSource::new(&mut *env.realm.random, clock);
        source.connect(&self.address, status, output, &mut env.phoenix_cx());
        self.sources.push(source);
    }

    /// `liveSource?.getTuner() ?? throw "no tuner"`: the Phoenix source has none.
    fn tuner(&self) -> Result<(), Thrown> {
        Err(Thrown::new("no tuner"))
    }

    fn dump(&mut self, env: &mut Env) {
        let log = self.window_log.borrow().clone();
        env.app(
            "dump",
            Some(&Value::Array(vec![
                log_id(&self.log_ids, Some(&log)),
                support::log_dump::dump(&log.borrow()),
            ])),
        );
    }
}

fn op_name(op: &[Json]) -> String {
    op[0].as_str().expect("op name").to_string_lossy()
}

impl PhoenixDriver {
    fn sync_window(&mut self) {
        self.window.preferences = self.prefs.as_ref().map(Prefs::as_live);
    }
}

impl Driver for PhoenixDriver {
    fn op(&mut self, op: &[Json], env: &mut Env) -> Result<(), Thrown> {
        self.sync_window();
        match op_name(op).as_str() {
            "connect" => self.connect(env),
            "stop" => self.stop_current(env),
            "prefs" => prefs_op(&mut self.prefs, &op[1]),
            "fields" => {
                self.window.tabs_fields = string_list(op.get(1));
            }
            "log" => {
                let log = self.window_log.borrow().clone();
                support::log_op(&mut log.borrow_mut(), &mut env.realm, op)?;
            }
            "select" => {}
            "tune" | "untune" => {
                self.tuner()?;
            }
            "dump" => self.dump(env),
            other => panic!("the PhoenixDiagnosticsSource driver does not model {other}"),
        }
        Ok(())
    }

    fn notify(&mut self, event: support::SessionEvent, env: &mut Env) {
        let support::SessionEvent::Net(NetEvent::Fetch(event)) = event else {
            panic!("unexpected {event:?}");
        };
        for source in &mut self.sources {
            source.on_fetch_event(&event, &mut env.phoenix_cx());
        }
    }

    fn timer(&mut self, id: TimerId, env: &mut Env) {
        self.sync_window();
        let window = &self.window;
        for source in &mut self.sources {
            if source.handle_timer(id, window, &mut env.phoenix_cx()) {
                return;
            }
        }
    }

    fn finish(&mut self, env: &mut Env) {
        self.dump(env);
    }
}

fn replay(id: &str, session: &Session) -> Result<usize, String> {
    let trace = support::run(session, |env| -> Box<dyn Driver> {
        Box::new(PhoenixDriver::new(session, env))
    })?;
    support::check_trace(id, trace)
}

fn phoenix_sessions() -> Vec<(String, Session)> {
    live::session_files()
        .into_iter()
        .filter_map(|path| {
            let session = Session::load(&path).unwrap_or_else(|e| panic!("{e}"));
            (session.protocol == "phoenix").then(|| (live::vector_id(&path), session))
        })
        .collect()
}

#[test]
fn every_phoenix_session_matches_the_oracle_trace() {
    let sessions = phoenix_sessions();
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
fn a_planted_bug_is_caught() {
    let (id, mut session) = phoenix_sessions()
        .into_iter()
        .find(|(id, _)| id == "live/phoenix/basic")
        .expect("session");
    // Every point timestamp moves, so the dumps must differ.
    let Json::Object(server) = &mut session.server else {
        panic!("server object")
    };
    let (_, start_us) = server
        .iter_mut()
        .find(|(k, _)| k.eq_str("startUs"))
        .expect("startUs");
    *start_us = Json::Number(12345.0);
    let error = replay(&id, &session).unwrap_err();
    assert!(error.contains("differs"), "{error}");
}
