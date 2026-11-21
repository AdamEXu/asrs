//! Replays every FTC Dashboard live session against the port and compares the trace with the
//! oracle's, line for line. The driver stands in for `oracle/src/liveSession.ts`'s `ftc` arm:
//! `window.preferences`, `window.log` with the oracle's `logId` numbering, and a stopped source
//! list that keeps idle intervals running, as upstream's leaked `setInterval`s do.

#![allow(clippy::disallowed_methods)]

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use asrs_js::string::JsString;
use asrs_js::time::Clock;
use asrs_live::ftc::{FtcContext, FtcSource, config};
use asrs_live::nt4::NetEvent;
use asrs_live::{LiveLog, OutputCallback, Thrown, TimerId, TunedValue};
use asrs_log::log::Log;
use asrs_parity::json::Json;
use asrs_parity::live::{self, Session};
use asrs_parity::vectors;
use asrs_parity::{FnCase, Response, Utf16String, Value, fn_vectors};

use support::{Driver, Env, Prefs, initial_prefs, log_id, prefs_op, status_callback};

fn fcx(env: &mut Env) -> FtcContext<'_> {
    FtcContext {
        timers: &mut env.timers,
        net: &mut env.net,
        clock: &env.wall,
    }
}

struct FtcDriver {
    address: String,
    prefs: Option<Prefs>,
    window_log: Rc<RefCell<LiveLog>>,
    // Every log object stays alive so its `Rc::as_ptr` keeps a stable `logId` slot, matching the
    // oracle's identity-keyed map.
    live_logs: Rc<RefCell<Vec<LiveLog>>>,
    log_ids: Rc<RefCell<Vec<usize>>>,
    sources: Vec<FtcSource>,
}

impl FtcDriver {
    fn new(session: &Session) -> Self {
        let initial: LiveLog = Rc::new(RefCell::new(Log::new(true)));
        FtcDriver {
            address: session
                .client
                .get("address")
                .and_then(Json::as_str)
                .map_or_else(|| "127.0.0.1".to_owned(), |s| s.to_string_lossy()),
            prefs: initial_prefs(session),
            window_log: Rc::new(RefCell::new(initial.clone())),
            live_logs: Rc::new(RefCell::new(vec![initial.clone()])),
            log_ids: Rc::new(RefCell::new(vec![Rc::as_ptr(&initial) as usize])),
            sources: Vec::new(),
        }
    }

    fn stop_current(&mut self, env: &mut Env) {
        if let Some(source) = self.sources.last_mut() {
            source.stop(&mut fcx(env));
        }
    }

    /// `liveSource?.stop(); publisher?.stop(); if (!preferences) return;` then a fresh source.
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
        let mut source = FtcSource::new(&mut *env.realm.random, Rc::new(env.wall.clone()));
        let prefs = self.prefs.as_ref().map(Prefs::as_live);
        source.connect(&self.address, status, output, prefs.as_ref(), &mut fcx(env));
        self.sources.push(source);
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

fn op_key(op: &[Json], i: usize) -> JsString {
    JsString::from_units(op[i].as_str().expect("key").units().to_vec())
}

impl Driver for FtcDriver {
    fn op(&mut self, op: &[Json], env: &mut Env) -> Result<(), Thrown> {
        match op[0].as_str().expect("op name").to_string_lossy().as_str() {
            "connect" => self.connect(env),
            "stop" => self.stop_current(env),
            "prefs" => prefs_op(&mut self.prefs, &op[1]),
            "fields" | "select" => {}
            "log" => {
                let log = self.window_log.borrow().clone();
                support::log_op(&mut log.borrow_mut(), &mut env.realm, op)?;
            }
            "tune" => {
                let value = match &op[2] {
                    Json::Number(n) => TunedValue::Number(*n),
                    Json::Bool(b) => TunedValue::Boolean(*b),
                    other => panic!("tune value {other:?}"),
                };
                let key = op_key(op, 1);
                match self.sources.last_mut() {
                    Some(source) => source.publish(&key, value, &mut fcx(env))?,
                    None => return Err(Thrown::new("no tuner")),
                }
            }
            "untune" => {
                let key = op_key(op, 1);
                match self.sources.last_mut() {
                    Some(source) => source.unpublish(&key),
                    None => return Err(Thrown::new("no tuner")),
                }
            }
            "dump" => self.dump(env),
            other => panic!("the FTC driver does not model {other}"),
        }
        Ok(())
    }

    fn notify(&mut self, event: NetEvent, env: &mut Env) {
        let mut cx = fcx(env);
        for source in &mut self.sources {
            source.handle_net_event(&event, &mut cx);
        }
    }

    fn timer(&mut self, id: TimerId, env: &mut Env) {
        let prefs = self.prefs.as_ref().map(Prefs::as_live);
        let mut cx = fcx(env);
        for source in &mut self.sources {
            if source.handle_timer(id, prefs.as_ref(), &mut cx) {
                return;
            }
        }
    }

    fn finish(&mut self, env: &mut Env) {
        self.dump(env);
    }
}

pub fn replay(id: &str, session: &Session) -> Result<usize, String> {
    assert_eq!(
        session
            .client
            .get("source")
            .and_then(Json::as_str)
            .map(|s| s.to_string_lossy())
            .as_deref(),
        Some("ftc"),
        "{id}: not an ftc session"
    );
    let trace = support::run(session, |_env| -> Box<dyn Driver> {
        Box::new(FtcDriver::new(session))
    })?;
    let expected: Vec<String> = vectors::load(id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|r| r.text)
        .collect();
    let produced: Vec<String> = trace;
    if let Some(i) =
        (0..expected.len().max(produced.len())).find(|&i| expected.get(i) != produced.get(i))
    {
        let context = |lines: &[String]| {
            lines[i.saturating_sub(3)..(i + 1).min(lines.len())]
                .iter()
                .map(|l| format!("    {}", &l[..l.len().min(400)]))
                .collect::<Vec<_>>()
                .join("\n")
        };
        return Err(format!(
            "{id}: line {i} differs\n  oracle:\n{}\n  port:\n{}",
            context(&expected),
            context(&produced)
        ));
    }
    Ok(produced.len())
}

fn ftc_sessions() -> Vec<(String, Session)> {
    live::session_files()
        .into_iter()
        .filter_map(|path| {
            let session = Session::load(&path).unwrap_or_else(|e| panic!("{e}"));
            (session.protocol == "ftc").then(|| (live::vector_id(&path), session))
        })
        .collect()
}

#[test]
fn every_ftc_session_matches_the_oracle_trace() {
    let sessions = ftc_sessions();
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

/// Nudging a telemetry packet's robot timestamp changes the skew the port computes and every
/// timestamp it writes, so the replayed trace cannot still match: the comparison is sensitive
/// to the wire bytes the source consumes.
#[test]
fn a_mutated_wire_value_is_caught() {
    let (id, mut session) = ftc_sessions()
        .into_iter()
        .find(|(id, _)| id == "live/ftc/telemetry")
        .expect("session");
    let mut bumped = false;
    'steps: for step in &mut session.server_steps {
        let Some(Json::Object(fields)) = step.op.get_mut(1) else {
            continue;
        };
        let Some(telemetry) = fields
            .iter_mut()
            .find(|(k, _)| k.eq_str("telemetry"))
            .map(|(_, v)| v)
        else {
            continue;
        };
        let Json::Array(packets) = telemetry else {
            continue;
        };
        for packet in packets {
            let Json::Object(fields) = packet else {
                continue;
            };
            if let Some((_, Json::Number(n))) =
                fields.iter_mut().find(|(k, _)| k.eq_str("timestamp"))
            {
                *n += 1.0;
                bumped = true;
                break 'steps;
            }
        }
    }
    assert!(bumped, "no telemetry timestamp to mutate");
    let error = replay(&id, &session).unwrap_err();
    assert!(error.contains("differs"), "{error}");
}

fn config_reducer_case(case: &FnCase<'_>) -> Response {
    let arg = |i| support::js_json::to_js(case.arg(i)).expect("reducer argument");
    match config::config_reducer(&arg(0), &arg(1)) {
        Ok((state, _)) => Response::Ok(support::js_value(&state)),
        Err(Thrown(text)) => {
            let text = text.to_string_lossy();
            let (name, message) = text.split_once(": ").unwrap_or(("", &text));
            Response::Throw(Value::Error(
                Utf16String::from(name),
                Utf16String::from(message),
            ))
        }
    }
}

fn_vectors!(
    config_reducer_matches_upstream,
    "live/ftc/config-reducer",
    config_reducer_case
);
