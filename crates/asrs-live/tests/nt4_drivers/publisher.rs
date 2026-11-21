//! The `nt4-publisher` driver: `window.selection` is the oracle's stub, two plain values
//! the session's `select` op assigns.

use std::cell::RefCell;

use asrs_js::time::Clock;
use asrs_live::nt4::{
    Nt4Publisher, PublisherSelection, PublisherStatusCallback, PublisherWindow,
};
use asrs_live::{Thrown, TimerId};
use asrs_log::log::Log;
use asrs_parity::Value;
use asrs_parity::json::Json;
use asrs_parity::live::Session;

use super::{Prefs, initial_prefs, op_name, prefs_op};
use crate::support::{self, Driver, Env};

#[derive(Default)]
struct SessionSelection {
    selected: Option<f64>,
    hovered: Option<f64>,
}

impl PublisherSelection for SessionSelection {
    fn get_hovered_time(&self) -> Option<f64> {
        self.hovered
    }

    fn get_selected_time(&mut self) -> Option<f64> {
        self.selected
    }
}

pub struct PublisherDriver {
    is_sim: bool,
    platform: String,
    log: RefCell<Log>,
    prefs: Option<Prefs>,
    selection: SessionSelection,
    publishers: Vec<Nt4Publisher>,
}

impl PublisherDriver {
    pub fn new(session: &Session) -> Self {
        let c = &session.client;
        let is_sim = match c
            .get("mode")
            .and_then(Json::as_str)
            .map(|m| m.to_string_lossy())
            .as_deref()
        {
            Some("sim") => true,
            Some("robot") => false,
            other => panic!("publisher mode must be \"sim\" or \"robot\", got {other:?}"),
        };
        PublisherDriver {
            is_sim,
            platform: c
                .get("platform")
                .and_then(Json::as_str)
                .map_or_else(|| "darwin".to_owned(), |s| s.to_string_lossy()),
            selection: SessionSelection::default(),
            log: RefCell::new(Log::new(true)),
            prefs: initial_prefs(session),
            publishers: Vec::new(),
        }
    }

    fn window(&mut self) -> PublisherWindow<'_> {
        PublisherWindow {
            log: &self.log,
            preferences: self.prefs.as_ref().map(|p| &p.publisher),
            platform: &self.platform,
            selection: &mut self.selection,
        }
    }

    fn stop_current(&mut self, env: &mut Env) -> Result<(), Thrown> {
        if let Some(p) = self.publishers.last_mut() {
            p.stop(&mut env.cx())?;
        }
        Ok(())
    }
}

impl Driver for PublisherDriver {
    fn op(&mut self, op: &[Json], env: &mut Env) -> Result<(), Thrown> {
        match op_name(op).as_str() {
            "connect" => {
                if !self.publishers.is_empty() {
                    self.stop_current(env)?;
                }
                let trace = env.trace.clone();
                let clock = env.wall.loop_clock.clone();
                let status: PublisherStatusCallback = Box::new(move |status| {
                    trace.app(
                        clock.now(),
                        "status",
                        Some(&Value::number(f64::from(status as u8))),
                    );
                });
                let is_sim = self.is_sim;
                let publisher = {
                    let window = self.window();
                    Nt4Publisher::new(is_sim, status, &window, &mut env.cx())
                };
                self.publishers.push(publisher);
            }
            "stop" => self.stop_current(env)?,
            "prefs" => prefs_op(&mut self.prefs, &op[1]),
            "select" => {
                self.selection.selected = op[1].as_f64();
                self.selection.hovered = op[2].as_f64();
            }
            "log" => support::log_op(&mut self.log.borrow_mut(), &mut env.realm, op)?,
            "dump" => {
                // `g.log` is the publisher's log, registered under id 0 at session start.
                env.app(
                    "dump",
                    Some(&Value::Array(vec![
                        Value::number(0.0),
                        support::dump(&self.log.borrow()),
                    ])),
                );
            }
            other => panic!("the publisher driver does not model {other}"),
        }
        Ok(())
    }

    fn notify(&mut self, event: support::SessionEvent, env: &mut Env) {
        let support::SessionEvent::Net(event) = event else {
            return;
        };
        for p in &mut self.publishers {
            let _ = p.handle_net_event(&event, &mut env.cx());
        }
    }

    fn timer(&mut self, id: TimerId, env: &mut Env) {
        let mut cx = env.cx();
        let mut window = PublisherWindow {
            log: &self.log,
            preferences: self.prefs.as_ref().map(|p| &p.publisher),
            platform: &self.platform,
            selection: &mut self.selection,
        };
        for publisher in &mut self.publishers {
            if publisher.handle_timer(id, &mut window, &mut cx) {
                return;
            }
        }
    }

    fn finish(&mut self, env: &mut Env) {
        env.app(
            "dump",
            Some(&Value::Array(vec![
                Value::number(0.0),
                support::dump(&self.log.borrow()),
            ])),
        );
    }
}
