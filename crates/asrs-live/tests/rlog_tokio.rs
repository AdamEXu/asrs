//! The RLOG transport over real sockets: held writes, `Connect`/`Data`, the idle timeout's one
//! report, `destroy` dropping already-queued data, `localhost` across address families, and a
//! refused connection.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use asrs_live::{LiveRuntime, NetOptions};
use asrs_live::rlog::{RlogNetEvent, RlogTransport, TcpId, TokioRlogTransport};

fn collect(
    net: &mut TokioRlogTransport,
    events: &mut Vec<RlogNetEvent>,
    done: impl Fn(&[RlogNetEvent]) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(events) && Instant::now() < deadline {
        while let Some(event) = net.next_event() {
            events.push(event);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(done(events), "timed out with {events:?}");
}

fn received(events: &[RlogNetEvent], id: TcpId) -> Vec<u8> {
    events
        .iter()
        .filter_map(|e| match e {
            RlogNetEvent::Data(i, bytes) if *i == id => Some(bytes.as_slice()),
            _ => None,
        })
        .flatten()
        .copied()
        .collect()
}

#[test]
fn follows_node_socket_rules_over_real_sockets() {
    let runtime = LiveRuntime::new().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (late_tx, late_rx) = std::sync::mpsc::channel::<()>();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut held = [0u8; 4];
        stream.read_exact(&mut held).unwrap();
        stream.write_all(b"abc").unwrap();
        late_rx.recv().unwrap();
        stream.write_all(b"late").unwrap();
        let mut rest = Vec::new();
        let _ = stream.read_to_end(&mut rest);
        (held, rest)
    });

    let mut net = TokioRlogTransport::new(NetOptions::new(runtime.handle().clone()));
    let id = net.tcp_connect("127.0.0.1", f64::from(port));
    net.tcp_write(id, &[6, 3, 2, 8]);
    net.tcp_set_timeout(id, 200.0);
    let mut events = Vec::new();
    collect(&mut net, &mut events, |e| {
        received(e, id) == b"abc" && e.contains(&RlogNetEvent::Timeout(id))
    });
    assert_eq!(events[0], RlogNetEvent::Connect(id));
    assert_eq!(
        events
            .iter()
            .filter(|e| **e == RlogNetEvent::Timeout(id))
            .count(),
        1
    );

    late_tx.send(()).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    net.tcp_destroy(id);
    net.tcp_write(id, b"gone");
    let mut after = Vec::new();
    collect(&mut net, &mut after, |e| !e.is_empty());
    std::thread::sleep(Duration::from_millis(100));
    while let Some(event) = net.next_event() {
        after.push(event);
    }
    assert_eq!(after, vec![RlogNetEvent::Close(id)]);
    let (held, rest) = server.join().unwrap();
    assert_eq!(held, [6, 3, 2, 8]);
    assert!(rest.is_empty(), "{rest:?}");
}

#[test]
fn an_empty_host_falls_back_across_families_like_node() {
    let runtime = LiveRuntime::new().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || listener.accept().map(|_| ()));
    let mut net = TokioRlogTransport::new(NetOptions::new(runtime.handle().clone()));
    let id = net.tcp_connect("", f64::from(port));
    net.tcp_set_timeout(id, 1000.0);
    let mut events = Vec::new();
    collect(&mut net, &mut events, |e| !e.is_empty());
    assert_eq!(events[0], RlogNetEvent::Connect(id));
    server.join().unwrap().unwrap();
}

#[test]
fn a_refused_connection_reports_error_then_close() {
    let runtime = LiveRuntime::new().unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut net = TokioRlogTransport::new(NetOptions::new(runtime.handle().clone()));
    let id = net.tcp_connect("127.0.0.1", f64::from(port));
    net.tcp_write(id, b"held");
    let mut events = Vec::new();
    collect(&mut net, &mut events, |e| {
        e.contains(&RlogNetEvent::Close(id))
    });
    assert_eq!(
        events,
        vec![RlogNetEvent::Error(id), RlogNetEvent::Close(id)]
    );
}
