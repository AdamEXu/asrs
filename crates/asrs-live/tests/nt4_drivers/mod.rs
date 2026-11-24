//! Thin adapters driving the real `NT4Source`/`NT4Tuner` and `NT4Publisher` ports the way
//! `oracle/src/liveSession.ts` does: `window` is `Nt4Window` plus the session's tabs, sidebar
//! and preferences, the output callback assigns `window.log` and records `output` events with
//! the oracle's `logId` numbering, and `dump` ops serialize `window.log`. Stopped sources stay
//! alive so their idle loops keep running, as upstream's do.

mod publisher;

use std::cell::RefCell;
use std::rc::Rc;

use asrs_js::string::JsString;
use asrs_js::time::Clock;
use asrs_live::nt4::{Nt4Mode, Nt4Source, Nt4Tuner, Nt4Window};
use asrs_live::{LiveLog, OutputCallback, Thrown, TimerId, TunedValue};
use asrs_log::log::Log;
use asrs_parity::Value;
use asrs_parity::json::Json;
use asrs_parity::live::Session;

use crate::support::{self, Driver, Env, Prefs, initial_prefs, log_id, prefs_op, status_callback};

pub use publisher::PublisherDriver;

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
        source.connect(&self.address, status, output, prefs.as_ref(), &mut env.cx());
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
                support::log_dump::dump(&log.borrow()),
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

    fn notify(&mut self, event: support::SessionEvent, env: &mut Env) {
        let support::SessionEvent::Net(event) = event else {
            panic!("unexpected {event:?}");
        };
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
