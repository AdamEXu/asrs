//! Replays every RLOG live session against the port and compares the trace with the
//! oracle's, line for line. The driver stands in for `oracle/src/liveSession.ts`'s `rlog`
//! arm: `window.preferences`, `window.log` with the oracle's `logId` numbering, the emulated
//! main process (`RlogMain` over the same fake network) and `sendMainMessage`/`sendMessage`
//! as zero-delay IPC deliveries.

#![allow(clippy::disallowed_methods)]

mod support;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use asrs_js::time::Clock;
use asrs_js::value::JsValue;
use asrs_live::rlog::{
    Ipc, MainCall, MainContext, RlogContext, RlogMain, RlogPreferences, RlogSource,
};
use asrs_live::{LiveLog, OutputCallback, Thrown, TimerId};
use asrs_log::log::Log;
use asrs_parity::Value;
use asrs_parity::json::Json;
use asrs_parity::live::{self, Session};

use support::{
    Delivery, Driver, Env, Prefs, SessionEvent, initial_prefs, log_id, prefs_op, status_callback,
};

fn rcx<'a>(
    send_main: &'a mut VecDeque<MainCall>,
    prefs: &'a Option<RlogPreferences>,
    window_log: &'a Rc<RefCell<LiveLog>>,
    env: &'a mut Env,
) -> RlogContext<'a> {
    RlogContext {
        timers: &mut env.timers,
        send_main,
        preferences: prefs.as_ref(),
        window_log,
        realm: &mut env.realm,
        clock: &env.wall,
    }
}

fn mcx(env: &mut Env) -> MainContext<'_> {
    MainContext {
        timers: &mut env.timers,
        net: &mut env.net,
    }
}

struct RlogDriver {
    address: String,
    prefs: Option<Prefs>,
    /// `window.preferences.rlogPort`, kept in step with `prefs`.
    rlog_prefs: Option<RlogPreferences>,
    window_log: Rc<RefCell<LiveLog>>,
    live_logs: Rc<RefCell<Vec<LiveLog>>>,
    log_ids: Rc<RefCell<Vec<usize>>>,
    sources: Vec<RlogSource>,
    /// `rlogMain`, whose construction registers the heartbeat interval.
    main: RlogMain,
    /// `window.sendMainMessage` calls not yet delivered to the main process.
    send_main: VecDeque<MainCall>,
}

impl RlogDriver {
    fn new(session: &Session, env: &mut Env) -> Self {
        let initial: LiveLog = Rc::new(RefCell::new(Log::new(true)));
        let prefs = initial_prefs(session);
        RlogDriver {
            address: session
                .client
                .get("address")
                .and_then(Json::as_str)
                .map_or_else(|| "127.0.0.1".to_owned(), |s| s.to_string_lossy()),
            rlog_prefs: prefs.as_ref().map(Self::rlog_prefs),
            prefs,
            window_log: Rc::new(RefCell::new(initial.clone())),
            live_logs: Rc::new(RefCell::new(vec![initial.clone()])),
            log_ids: Rc::new(RefCell::new(vec![Rc::as_ptr(&initial) as usize])),
            sources: Vec::new(),
            main: RlogMain::new(&mut env.timers),
            send_main: VecDeque::new(),
        }
    }

    fn rlog_prefs(prefs: &Prefs) -> RlogPreferences {
        RlogPreferences {
            rlog_port: prefs.rlog_port,
        }
    }

    /// IPC is a task with no delay on either hop, in send order on each side. A task runs
    /// either renderer or main-process code, so at most one side has anything queued and the
    /// two loops keep upstream's call order.
    fn flush(&mut self, env: &mut Env) {
        assert!(
            self.send_main.is_empty() || self.main.outbox.is_empty(),
            "IPC queued on both sides in one task"
        );
        while let Some(call) = self.send_main.pop_front() {
            env.net.deliver(0.0, Delivery::Ipc(Ipc::ToMain(call)));
        }
        while let Some(message) = self.main.outbox.pop_front() {
            env.net
                .deliver(0.0, Delivery::Ipc(Ipc::ToSource(message.into_value())));
        }
    }

    /// `liveSource?.stop()`.
    fn stop_current(&mut self, env: &mut Env) {
        if let Some(source) = self.sources.last_mut() {
            let mut cx = rcx(&mut self.send_main, &self.rlog_prefs, &self.window_log, env);
            source.stop(&mut cx);
        }
        self.flush(env);
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
        let mut source = RlogSource::new(&mut *env.realm.random, Rc::new(env.wall.clone()));
        let mut cx = rcx(&mut self.send_main, &self.rlog_prefs, &self.window_log, env);
        source.connect(&self.address, status, output, &mut cx);
        self.sources.push(source);
        self.flush(env);
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

impl Driver for RlogDriver {
    fn op(&mut self, op: &[Json], env: &mut Env) -> Result<(), Thrown> {
        match op[0].as_str().expect("op name").to_string_lossy().as_str() {
            "connect" => self.connect(env),
            "stop" => self.stop_current(env),
            "prefs" => {
                prefs_op(&mut self.prefs, &op[1]);
                self.rlog_prefs = self.prefs.as_ref().map(Self::rlog_prefs);
            }
            "fields" | "select" => {}
            "log" => {
                let log = self.window_log.borrow().clone();
                support::log_op(&mut log.borrow_mut(), &mut env.realm, op)?;
            }
            "dump" => self.dump(env),
            "tune" | "untune" => return Err(Thrown::new("no tuner")),
            other => panic!("the RLOG driver does not model {other}"),
        }
        Ok(())
    }

    fn notify(&mut self, event: SessionEvent, env: &mut Env) {
        match event {
            SessionEvent::Net(event) => panic!("unexpected {event:?}"),
            SessionEvent::Tcp(event) => {
                self.main.handle_net_event(&event, &mut mcx(env));
            }
            SessionEvent::Ipc(Ipc::ToMain(call)) => {
                if let Err(thrown) = self.main.handle(&call, &mut mcx(env)) {
                    env.app(
                        "uncaught",
                        Some(&support::js_value(&JsValue::String(thrown.0))),
                    );
                }
            }
            SessionEvent::Ipc(Ipc::ToSource(data)) => {
                if let Some(source) = self.sources.last_mut() {
                    let mut cx = rcx(&mut self.send_main, &self.rlog_prefs, &self.window_log, env);
                    if let Err(abort) = source.handle_main_message(&data, &mut cx) {
                        panic!("the RLOG decoder aborted: {abort:?}");
                    }
                }
            }
        }
        self.flush(env);
    }

    fn timer(&mut self, id: TimerId, env: &mut Env) {
        let handled = self.main.handle_timer(id, &mut mcx(env))
            || self.sources.last_mut().is_some_and(|source| {
                let mut cx = rcx(&mut self.send_main, &self.rlog_prefs, &self.window_log, env);
                source.handle_timer(id, &mut cx)
            });
        assert!(handled, "timer {id:?} has no owner");
        self.flush(env);
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
        Some("rlog"),
        "{id}: not an rlog session"
    );
    let trace = support::run(session, |env| -> Box<dyn Driver> {
        Box::new(RlogDriver::new(session, env))
    })?;
    support::check_trace(id, trace)
}

fn rlog_sessions() -> Vec<(String, Session)> {
    live::session_files()
        .into_iter()
        .filter_map(|path| {
            let session = Session::load(&path).unwrap_or_else(|e| panic!("{e}"));
            (session.protocol == "rlog").then(|| (live::vector_id(&path), session))
        })
        .collect()
}

#[test]
fn every_rlog_session_matches_the_oracle_trace() {
    let sessions = rlog_sessions();
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

/// Flipping one byte inside a framed payload changes what the decoder writes to the log, so
/// the replayed trace cannot still match: the comparison is sensitive to the wire bytes the
/// port consumes.
#[test]
fn a_mutated_wire_value_is_caught() {
    let (id, mut session) = rlog_sessions()
        .into_iter()
        .find(|(id, _)| id == "live/rlog/r2-stream")
        .expect("session");
    let mut flipped = false;
    for step in &mut session.server_steps {
        if step
            .op
            .first()
            .and_then(Json::as_str)
            .map(|s| s.to_string_lossy())
            .as_deref()
            == Some("send")
            && let Some(Json::String(hex)) = step.op.get_mut(1)
        {
            let mut units = std::mem::take(hex).into_units();
            let byte = &mut units[40];
            *byte = if *byte == u16::from(b'0') {
                u16::from(b'1')
            } else {
                u16::from(b'0')
            };
            *hex = asrs_parity::Utf16String::from_units(units);
            flipped = true;
            break;
        }
    }
    assert!(flipped, "no send payload to mutate");
    let error = replay(&id, &session).unwrap_err();
    assert!(error.contains("differs"), "{error}");
}
