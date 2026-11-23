//! `files/download-handshake`: ssh2's parser on a server's unencrypted bytes, cut into chunks.
//! `files/download-offer`: what the client offers the server.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener};

use asrs_parity::Json;
use asrs_parity::vectors::load;
use asrs_services::download::client::{Clients, Login, Target};
use asrs_services::download::handshake::{Handshake, HandshakeEvent};

fn latin1(text: &Json) -> Vec<u8> {
    text.as_str()
        .expect("a chunk")
        .to_string_lossy()
        .chars()
        .map(|c| u8::try_from(c).expect("latin1"))
        .collect()
}

fn event(event: &HandshakeEvent) -> Json {
    match event {
        HandshakeEvent::Header { openssh_limits } => Json::object([
            ("k", Json::string("header")),
            ("opensshLimits", Json::Bool(*openssh_limits)),
        ]),
        HandshakeEvent::Disconnect {
            reason,
            description,
        } => Json::object([
            ("k", Json::string("disconnect")),
            ("reason", Json::Number(f64::from(*reason))),
            ("description", Json::string(description)),
        ]),
        HandshakeEvent::Thrown(message) => Json::object([
            ("k", Json::string("thrown")),
            ("message", Json::string(message)),
        ]),
        HandshakeEvent::Fatal { message, handshake } => Json::object([
            ("k", Json::string("fatal")),
            ("message", Json::string(message)),
            (
                "level",
                Json::string(if *handshake { "handshake" } else { "protocol" }),
            ),
        ]),
    }
}

/// SFTP.js tests the whole ident line with `RE_OPENSSH`.
fn expected_event(recorded: &Json) -> Json {
    match recorded.get("ident") {
        None => recorded.clone(),
        Some(ident) => {
            let ident = ident.as_str().expect("ident").to_string_lossy();
            let limits =
                ident.starts_with("SSH-2.0-OpenSSH") || ident.starts_with("SSH-2.0-dropbear");
            Json::object([
                ("k", Json::string("header")),
                ("opensshLimits", Json::Bool(limits)),
            ])
        }
    }
}

#[test]
fn parser() {
    let records = load("files/download-handshake").expect("vector");
    let mut kinds = std::collections::HashMap::<String, usize>::new();
    for record in &records {
        let mut handshake = Handshake::new();
        let mut actual = Vec::new();
        for chunk in record
            .field("chunks")
            .and_then(Json::as_array)
            .expect("chunks")
        {
            let mut forward = Vec::new();
            actual.extend(handshake.feed(&latin1(chunk), &mut forward));
            let stopped = actual.last().is_some_and(|event| {
                matches!(
                    event,
                    HandshakeEvent::Thrown(_) | HandshakeEvent::Fatal { .. }
                )
            });
            if stopped {
                break;
            }
        }
        let actual: Vec<Json> = actual.iter().map(event).collect();
        let expected: Vec<Json> = record
            .field("events")
            .and_then(Json::as_array)
            .expect("events")
            .iter()
            .map(expected_event)
            .collect();
        assert_eq!(
            Json::Array(expected).to_compact(),
            Json::Array(actual.clone()).to_compact(),
            "line {}",
            record.line
        );
        for event in &actual {
            let kind = event.get("k").and_then(Json::as_str).expect("k");
            *kinds.entry(kind.to_string_lossy()).or_default() += 1;
        }
    }
    for kind in ["header", "disconnect", "thrown", "fatal"] {
        assert!(kinds.get(kind).is_some_and(|n| *n > 50), "{kinds:?}");
    }
}

#[test]
fn forwards_the_ident_line_and_what_follows() {
    let mut handshake = Handshake::new();
    let mut forward = Vec::new();
    handshake.feed(
        b"hello\r\nSSH-2.0-x\r\n\0\0\0\x0c\x0a\x02abcdefghij\0\0",
        &mut forward,
    );
    assert_eq!(forward, b"SSH-2.0-x\r\n\0\0\0\x0c\x0a\x02abcdefghij\0\0");
}

/// What the port sends: the ident and KEXINIT up to the packet's end.
fn offer() -> Vec<u8> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
    let port = listener.local_addr().expect("address").port();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept");
        socket.write_all(b"SSH-2.0-fixture\r\n").expect("ident");
        let mut received = Vec::new();
        loop {
            let mut buffer = [0; 4096];
            let n = socket.read(&mut buffer).expect("read");
            assert!(n > 0, "the client hung up");
            received.extend_from_slice(&buffer[..n]);
            let Some(start) = received.windows(2).position(|w| w == b"\r\n") else {
                continue;
            };
            let body = &received[start + 2..];
            if let Some(len) = body.first_chunk::<4>()
                && body.len() >= 4 + u32::from_be_bytes(*len) as usize
            {
                return body.to_vec();
            }
        }
    });
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("runtime");
    let _context = runtime.enter();
    let mut clients = Clients::new(|_, _| {});
    clients.connect(Target {
        host: "127.0.0.1".to_owned(),
        port,
        ready_timeout_ms: 3000.0,
        login: Login {
            username: "systemcore".to_owned(),
            password: Some("systemcore".to_owned()),
            key: None,
        },
    });
    while clients.next().is_some() {}
    server.join().expect("server")
}

#[test]
fn offered_algorithms() {
    const STRICT: &str = ",kex-strict-c-v00@openssh.com";
    let recorded = load("files/download-offer").expect("vector");
    let recorded = &recorded[0];
    let packet = offer();
    assert_eq!(packet[5], 20);
    let mut rest = &packet[22..];
    for name in [
        "kex",
        "hostKey",
        "cipherCs",
        "cipherSc",
        "macCs",
        "macSc",
        "compressionCs",
        "compressionSc",
        "languageCs",
        "languageSc",
    ] {
        let len = u32::from_be_bytes(*rest.first_chunk().expect("a length")) as usize;
        let list = String::from_utf8(rest[4..4 + len].to_vec()).expect("names");
        rest = &rest[4 + len..];
        let expected = recorded.string(name).expect("a recorded list");
        if name == "kex" {
            assert_eq!(list, format!("{expected}{STRICT}"));
        } else {
            assert_eq!(list, expected, "{name}");
        }
    }
    assert_eq!(
        recorded.field("firstFollows").and_then(Json::as_f64),
        Some(f64::from(rest[0]))
    );
}
