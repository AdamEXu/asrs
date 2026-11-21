//! Replays every `nt4-publisher` live session against the `Nt4Publisher` port and compares
//! the trace with the oracle's, line for line. The harness and driver live in asrs-live's
//! tests and are included by path: this crate can reach `SelectionImpl` while the layout
//! rules forbid asrs-live itself from depending on asrs-model.

#![allow(clippy::disallowed_methods)]

#[path = "../../asrs-live/tests/nt4_drivers/mod.rs"]
mod nt4_drivers;
#[path = "../../asrs-live/tests/nt4_drivers/publisher.rs"]
mod publisher_driver;
#[path = "../../asrs-live/tests/support/mod.rs"]
mod support;

use asrs_parity::json::Json;
use asrs_parity::live::{self, Session};

use support::Driver;

pub fn replay(id: &str, session: &Session) -> Result<usize, String> {
    let produced = support::run(session, |env| {
        Box::new(publisher_driver::PublisherDriver::new(session, env)) as Box<dyn Driver>
    })?;
    support::check_trace(id, produced)
}

fn publisher_sessions() -> Vec<(String, Session)> {
    live::session_files()
        .into_iter()
        .filter_map(|path| {
            let session = Session::load(&path).unwrap_or_else(|e| panic!("{e}"));
            let is_publisher = session
                .client
                .get("source")
                .and_then(Json::as_str)
                .is_some_and(|s| s.to_string_lossy() == "nt4-publisher");
            (session.protocol == "nt4" && is_publisher).then(|| (live::vector_id(&path), session))
        })
        .collect()
}

#[test]
fn every_publisher_session_matches_the_oracle_trace() {
    let sessions = publisher_sessions();
    assert!(!sessions.is_empty());
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
fn a_wrong_published_sample_is_caught() {
    let (id, mut session) = publisher_sessions()
        .into_iter()
        .find(|(id, _)| id == "live/nt4/publisher-sim")
        .expect("session");
    let step = session
        .client_steps
        .iter_mut()
        .find(|s| {
            s.op[0]
                .as_str()
                .is_some_and(|op| op.to_string_lossy() == "select")
        })
        .expect("select step");
    step.op[1] = Json::Number(1.9);
    let error = replay(&id, &session).unwrap_err();
    assert!(error.contains("differs"), "{error}");
}
