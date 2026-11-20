use std::collections::BTreeMap;

use asrs_parity::live::{self, Session};
use asrs_parity::vectors;

#[test]
fn rust_mocks_replay_every_recorded_session() {
    let files = live::session_files();
    assert!(
        !files.is_empty(),
        "no session files under parity/specs/live"
    );
    let mut per_protocol: BTreeMap<String, usize> = BTreeMap::new();
    for path in files {
        let id = live::vector_id(&path);
        let session = Session::load(&path).unwrap_or_else(|e| panic!("{e}"));
        let records = vectors::load(&id).unwrap_or_else(|e| panic!("{e}"));
        let events = live::replay(&session, &records).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert!(events > 0, "{id}: the server did nothing");
        *per_protocol.entry(session.protocol).or_default() += 1;
    }
    for protocol in ["nt4", "rlog", "phoenix", "ftc"] {
        assert!(
            per_protocol.get(protocol).copied().unwrap_or(0) >= 3,
            "{protocol} needs at least three sessions: {per_protocol:?}"
        );
    }
}

#[test]
fn replay_reports_a_server_byte_the_oracle_did_not_send() {
    let path = live::session_files()
        .into_iter()
        .find(|p| live::vector_id(p) == "live/nt4/v41-logging")
        .expect("session");
    let session = Session::load(&path).unwrap();
    let mut records = vectors::load("live/nt4/v41-logging").unwrap();
    let push = records
        .iter_mut()
        .find(|r| r.text.contains("\"ev\":\"ws-push\"") && r.text.contains("{\"b\":\"94ff"))
        .expect("a timestamp reply");
    push.text = push.text.replacen("{\"b\":\"94ff", "{\"b\":\"94fe", 1);
    push.json = asrs_parity::json::parse(&push.text).unwrap();
    let error = live::replay(&session, &records).unwrap_err();
    assert!(
        error.contains("differs") && error.contains("94fe"),
        "{error}"
    );
}
