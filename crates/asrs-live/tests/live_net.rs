//! The transports against `live/net/*`, recorded in Electron's renderer: what the URLs built from
//! an address connect to and how the calls settle, what a response body decodes to and which
//! ports are blocked. One loopback server stands behind every host and port.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use asrs_js::json;
use asrs_live::http::{FetchEvent, FetchTransport, Response};
use asrs_live::nt4::{NetEvent, Nt4Transport, SocketId, TokioTransport};
use asrs_live::{LiveRuntime, NetOptions};
use asrs_parity::json::Json;
use asrs_parity::vectors;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;

struct Server {
    port: u16,
    requests: Receiver<Head>,
    body: Arc<Mutex<Vec<u8>>>,
}

#[derive(Debug, PartialEq)]
struct Head {
    target: String,
    host: String,
    authorization: Option<String>,
    protocols: Option<String>,
}

fn read_head(stream: &mut TcpStream) -> Option<Vec<String>> {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        if stream.read(&mut byte).ok()? == 0 {
            return None;
        }
        head.push(byte[0]);
    }
    let text = String::from_utf8(head).unwrap();
    Some(text.split("\r\n").map(str::to_owned).collect())
}

fn answer(mut stream: TcpStream, body: &[u8]) -> Option<Head> {
    let lines = read_head(&mut stream)?;
    let header = |name: &str| {
        lines.iter().find_map(|line| {
            let (n, value) = line.split_once(": ")?;
            n.eq_ignore_ascii_case(name).then(|| value.to_owned())
        })
    };
    let head = Head {
        target: lines[0].split(' ').nth(1).unwrap().to_owned(),
        host: header("host").unwrap(),
        authorization: header("authorization"),
        protocols: header("sec-websocket-protocol"),
    };
    let mut reply = match header("sec-websocket-key") {
        Some(key) => {
            let protocol = match &head.protocols {
                Some(all) => format!(
                    "Sec-WebSocket-Protocol: {}\r\n",
                    all.split(',').next().unwrap()
                ),
                None => String::new(),
            };
            format!(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n{protocol}\r\n",
                derive_accept_key(key.as_bytes())
            )
            .into_bytes()
        }
        None => format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes(),
    };
    if header("sec-websocket-key").is_none() {
        reply.extend_from_slice(body);
    }
    stream.write_all(&reply).ok()?;
    if header("sec-websocket-key").is_some() {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        let _ = stream.read(&mut [0; 64]);
        let _ = stream.write_all(&[0x88, 0x00]);
    }
    Some(head)
}

fn serve() -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (seen, requests) = channel();
    let body = Arc::new(Mutex::new(Vec::new()));
    let reply = body.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let body = reply.lock().unwrap().clone();
            let Some(head) = answer(stream.unwrap(), &body) else {
                continue;
            };
            if seen.send(head).is_err() {
                return;
            }
        }
    });
    Server {
        port,
        requests,
        body,
    }
}

type Dialed = Arc<Mutex<Vec<(String, u16)>>>;

fn transport(runtime: &LiveRuntime, port: u16) -> (TokioTransport, Dialed) {
    let dialed = Dialed::default();
    let log = dialed.clone();
    let mut options = NetOptions::new(runtime.handle().clone());
    options.dial = Some(Arc::new(move |host, to| {
        log.lock().unwrap().push((host.to_owned(), to));
        ("127.0.0.1".to_owned(), port)
    }));
    (TokioTransport::with_bodies(options), dialed)
}

fn next(net: &mut TokioTransport) -> NetEvent {
    let start = Instant::now();
    loop {
        if let Some(event) = net.next_event() {
            return event;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "no event");
        std::thread::sleep(Duration::from_micros(200));
    }
}

fn fetch(net: &mut TokioTransport, url: &str) -> Option<Response> {
    let id = net.fetch(url);
    match next(net) {
        NetEvent::Fetch(FetchEvent::Response(got, response)) if got == id => Some(response),
        NetEvent::Fetch(FetchEvent::Failed(got)) if got == id => None,
        other => panic!("{url}: {other:?}"),
    }
}

fn text(json: &Json, key: &str) -> String {
    json.get(key)
        .and_then(Json::as_str)
        .unwrap_or_else(|| panic!("{key} of {json:?}"))
        .to_string_lossy()
}

fn records(id: &str) -> Vec<Json> {
    let records = vectors::load(id).unwrap();
    records.into_iter().map(|r| r.json).collect()
}

fn recorded_heads(record: &Json) -> Vec<Head> {
    let requests = record.get("requests").and_then(Json::as_array).unwrap();
    let optional = |json: &Json, key| json.get(key).and_then(Json::as_str).map(|s| s.to_string_lossy());
    requests
        .iter()
        .map(|request| Head {
            target: text(request, "target"),
            host: text(request, "host"),
            authorization: optional(request, "authorization"),
            protocols: optional(request, "protocols"),
        })
        .collect()
}

/// Where a request with this `Host` header connects.
fn address(host: &str) -> (String, u16) {
    let (name, port) = match host.rsplit_once(':') {
        Some((name, port)) if !host.ends_with(']') => (name, port.parse().unwrap()),
        _ => (host, 80),
    };
    let bare = name.strip_prefix('[').and_then(|n| n.strip_suffix(']'));
    (bare.unwrap_or(name).to_owned(), port)
}

/// What the call settled with, in the recording's terms.
#[derive(Debug, PartialEq)]
enum Settled {
    Response(f64, String),
    Rejected,
    Threw(String),
    Socket(String, Vec<&'static str>),
}

fn recorded(result: &Json) -> Settled {
    if let Some(threw) = result.get("threw") {
        Settled::Threw(format!("{}: {}", text(threw, "name"), text(threw, "message")))
    } else if result.get("rejected").is_some() {
        Settled::Rejected
    } else if let Some(events) = result.get("events").and_then(Json::as_array) {
        let known = |event: &Json| {
            let event = event.as_str().unwrap().to_string_lossy();
            *["open", "error", "close"].iter().find(|e| **e == event).unwrap()
        };
        Settled::Socket(text(result, "protocol"), events.iter().map(known).collect())
    } else {
        let status = result.get("status").and_then(Json::as_f64).unwrap();
        Settled::Response(status, text(result, "url"))
    }
}

fn socket(net: &mut TokioTransport, url: &str, protocols: &[&str]) -> Settled {
    let id = match net.ws_open(url, protocols) {
        Ok(id) => id,
        Err(thrown) => return Settled::Threw(thrown.to_string()),
    };
    let mut protocol = String::new();
    let mut events = Vec::new();
    loop {
        let is = |got: SocketId| assert_eq!(got, id, "{url}");
        match next(net) {
            NetEvent::WsOpen(got, selected) => {
                is(got);
                protocol = selected;
                events.push("open");
                net.ws_close(id);
            }
            NetEvent::WsError(got) => {
                is(got);
                events.push("error");
            }
            NetEvent::WsClose(got) => {
                is(got);
                events.push("close");
                return Settled::Socket(protocol, events);
            }
            other => panic!("{url}: {other:?}"),
        }
    }
}

#[test]
fn urls_connect_where_chromium_does() {
    let runtime = LiveRuntime::new().unwrap();
    let server = serve();
    let (mut net, dialed) = transport(&runtime, server.port);
    let records = records("live/net/urls");
    let mut failures = Vec::new();
    for record in &records {
        let url = text(record, "url");
        let list = record.get("protocols").and_then(Json::as_array).unwrap();
        let protocols: Vec<String> = list
            .iter()
            .map(|p| p.as_str().unwrap().to_string_lossy())
            .collect();
        let protocols: Vec<&str> = protocols.iter().map(String::as_str).collect();
        let settled = match text(record, "api").as_str() {
            "fetch" => match fetch(&mut net, &url) {
                Some(response) => Settled::Response(f64::from(response.status), response.url),
                None => Settled::Rejected,
            },
            _ => socket(&mut net, &url, &protocols),
        };
        let heads = recorded_heads(record);
        let connections: Vec<_> = heads.iter().map(|head| address(&head.host)).collect();
        let made = std::mem::take(&mut *dialed.lock().unwrap());
        let mut reached = Vec::new();
        for _ in &made {
            match server.requests.recv_timeout(Duration::from_secs(10)) {
                Ok(head) => reached.push(head),
                Err(e) => failures.push(format!("{url:?}: no request arrived: {e}")),
            }
        }
        let expected = recorded(record.get("result").unwrap());
        if settled != expected {
            failures.push(format!("{url:?}: {settled:?}, recorded {expected:?}"));
        }
        if made != connections {
            failures.push(format!("{url:?}: dialed {made:?}, recorded {connections:?}"));
        } else if reached != heads {
            failures.push(format!("{url:?}: sent {reached:?}, recorded {heads:?}"));
        }
    }
    assert!(server.requests.try_recv().is_err(), "an unrecorded request");
    assert!(
        failures.is_empty(),
        "{} of {}:\n{}",
        failures.len(),
        records.len(),
        failures.join("\n")
    );
    assert!(records.len() > 1500);
}

#[test]
fn blocked_ports_are_chromiums() {
    let runtime = LiveRuntime::new().unwrap();
    let (mut net, dialed) = transport(&runtime, 0);
    let records = records("live/net/ports");
    let [record] = &records[..] else {
        panic!("{} records", records.len());
    };
    let range = record.get("ports").and_then(Json::as_array).unwrap();
    let number = |json: &Json| json.as_f64().unwrap() as u16;
    let blocked = record.get("blocked").and_then(Json::as_array).unwrap();
    let blocked: Vec<u16> = blocked.iter().map(number).collect();
    assert!(!blocked.is_empty());
    let ports: Vec<u16> = (number(&range[0])..=number(&range[1])).collect();
    for ports in ports.chunks(512) {
        for port in ports {
            net.fetch(&format!("http://robot.local:{port}/"));
        }
        for _ in ports {
            let event = next(&mut net);
            assert!(matches!(event, NetEvent::Fetch(FetchEvent::Failed(_))), "{event:?}");
        }
    }
    let mut dialed: Vec<u16> = dialed.lock().unwrap().iter().map(|(_, port)| *port).collect();
    dialed.sort_unstable();
    let open: Vec<u16> = ports.into_iter().filter(|port| !blocked.contains(port)).collect();
    assert_eq!(dialed, open);
}

#[test]
fn bodies_decode_like_the_renderers_response() {
    let runtime = LiveRuntime::new().unwrap();
    let server = serve();
    let (mut net, _) = transport(&runtime, server.port);
    let records = records("live/net/bodies");
    assert!(records.len() > 30);
    for record in &records {
        let hex = text(record, "body");
        let bytes: Vec<u8> = (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        *server.body.lock().unwrap() = bytes;
        let response = fetch(&mut net, "http://robot.local/").expect("a response");
        let recorded = record.get("text").unwrap();
        assert_eq!(response.body, text(recorded, "value"), "{hex}");
        let recorded = record.get("json").unwrap();
        match json::parse_str(&response.body) {
            Ok(value) => {
                let value = json::stringify(&value).unwrap().unwrap();
                let expected = recorded.get("value").expect("recorded a SyntaxError").to_compact();
                assert_eq!(value, expected, "{hex}");
            }
            Err(e) => assert_eq!(e.name, text(recorded, "rejected"), "{hex}"),
        }
    }
}
