//! The RLOG transport over real sockets: held writes, `Connect`/`Data`, `destroy` dropping
//! already-queued data, `localhost` across address families, a refused connection, a full backlog
//! and a refused port.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use asrs_live::rlog::{RlogNetEvent, RlogTransport, TcpId, TokioRlogTransport};
use asrs_live::{LiveRuntime, NetOptions};

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
    let id = net.tcp_connect("127.0.0.1", f64::from(port)).unwrap();
    net.tcp_write(id, &[6, 3, 2, 8]);
    let mut events = Vec::new();
    collect(&mut net, &mut events, |e| received(e, id) == b"abc");
    assert_eq!(events[0], RlogNetEvent::Connect(id));

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
    let id = net.tcp_connect("", f64::from(port)).unwrap();
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
    let id = net.tcp_connect("127.0.0.1", f64::from(port)).unwrap();
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

#[test]
fn a_backlog_stops_reading_the_socket() {
    const TOTAL: usize = 96 * 1024 * 1024;
    let runtime = LiveRuntime::new().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let written = Arc::new(AtomicUsize::new(0));
    let count = written.clone();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let chunk = vec![7u8; 1024 * 1024];
        for _ in 0..TOTAL / chunk.len() {
            stream.write_all(&chunk).unwrap();
            count.fetch_add(chunk.len(), Ordering::SeqCst);
        }
    });
    let mut net = TokioRlogTransport::new(NetOptions::new(runtime.handle().clone()));
    let id = net.tcp_connect("127.0.0.1", f64::from(port)).unwrap();
    let start = Instant::now();
    let mut last = 0;
    let mut still = 0;
    while still < 10 {
        assert!(start.elapsed() < Duration::from_secs(60));
        std::thread::sleep(Duration::from_millis(50));
        let now = written.load(Ordering::SeqCst);
        still = if now == last && now > 0 { still + 1 } else { 0 };
        last = now;
    }
    assert!(
        last < TOTAL / 2,
        "{last} bytes were read with no one to take them"
    );

    let mut received = 0;
    while received < TOTAL {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "{received} received"
        );
        match net.next_event() {
            Some(RlogNetEvent::Data(_, bytes)) => received += bytes.len(),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    server.join().unwrap();
    net.tcp_destroy(id);
    drop(net);
    let start = Instant::now();
    while runtime.alive_tasks() > 0 {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn a_port_node_refuses_makes_no_socket() {
    let runtime = LiveRuntime::new().unwrap();
    let dialed = Arc::new(AtomicUsize::new(0));
    let count = dialed.clone();
    let mut options = NetOptions::new(runtime.handle().clone());
    options.dial = Some(Arc::new(move |host, port| {
        count.fetch_add(1, Ordering::SeqCst);
        (host.to_owned(), port)
    }));
    let mut net = TokioRlogTransport::new(options);
    for port in [70000.0, 1.5, -1.0, 65536.0, 1e-7, f64::INFINITY, f64::NAN] {
        net.tcp_connect("127.0.0.1", port).unwrap_err();
    }
    assert_eq!(dialed.load(Ordering::SeqCst), 0);
    assert_eq!(runtime.alive_tasks(), 0);
    assert_eq!(net.next_event(), None);
}
