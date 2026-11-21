//! The NT4 client over real sockets: a local server answers the alive check, accepts the
//! 4.0 websocket, replies to time sync and announces a topic with a value.

#![allow(clippy::result_large_err)]

use std::rc::Rc;
use std::time::{Duration, Instant};

use asrs_js::time::{SystemClock, V8MathRandom};
use asrs_js::value::JsValue;
use asrs_live::nt4::{
    NT4_PORTS_DEFAULT, Nt4Client, Nt4Context, Nt4Handler, Nt4Topic, TokioTransport,
};
use asrs_live::{LiveRuntime, MonotonicClock, Thrown, TimerQueue};
use asrs_log::msgpack::{self, KeyCache};
use asrs_log::realm::JsRealm;
use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::http::HeaderValue;

#[derive(Default)]
struct Seen {
    connected: bool,
    announced: Vec<String>,
    data: Vec<f64>,
}

impl Nt4Handler for Seen {
    fn on_topic_announce(
        &mut self,
        topic: &Nt4Topic,
        _cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        self.announced.push(
            topic
                .name_str()
                .map(|s| s.to_string_lossy())
                .unwrap_or_default(),
        );
        Ok(())
    }

    fn on_new_topic_data(
        &mut self,
        _topic: &Nt4Topic,
        _ts: f64,
        value: JsValue,
        _cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        self.data.push(value.as_f64().unwrap_or(f64::NAN));
        Ok(())
    }

    fn on_connect(&mut self, _cx: &mut Nt4Context<'_>) -> Result<(), Thrown> {
        self.connected = true;
        Ok(())
    }
}

async fn serve(listener: TcpListener) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(async move {
            let mut head = [0u8; 2048];
            let n = stream.peek(&mut head).await.unwrap_or(0);
            let text = String::from_utf8_lossy(&head[..n]).to_ascii_lowercase();
            if !text.contains("upgrade: websocket") {
                let mut stream = stream;
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
                return;
            }
            let pick = |request: &Request, mut response: Response| {
                let offered = request
                    .headers()
                    .get("Sec-WebSocket-Protocol")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_owned();
                assert!(offered.contains("networktables.first.wpi.edu"), "{offered}");
                response.headers_mut().insert(
                    "Sec-WebSocket-Protocol",
                    HeaderValue::from_static("networktables.first.wpi.edu"),
                );
                Ok(response)
            };
            let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(stream, pick).await else {
                return;
            };
            while let Some(Ok(message)) = ws.next().await {
                match message {
                    Message::Binary(bytes) => {
                        let mut keys = KeyCache::new();
                        let mut rng = V8MathRandom::from_seed(1);
                        let Ok(JsValue::Array(items)) =
                            msgpack::decode(&bytes, &mut keys, &mut rng)
                        else {
                            continue;
                        };
                        let reply = JsValue::Array(vec![
                            JsValue::Number(-1.0),
                            JsValue::Number(1_000_000.0),
                            JsValue::Number(2.0),
                            items[3].clone(),
                        ]);
                        let _ = ws
                            .send(Message::binary(msgpack::encode(&reply).unwrap()))
                            .await;
                    }
                    Message::Text(text) if text.contains("\"subscribe\"") => {
                        let announce = r#"[{"method":"announce","params":{"name":"/x","id":7,"type":"double","properties":{}}}]"#;
                        let _ = ws.send(Message::text(announce)).await;
                        let value = JsValue::Array(vec![
                            JsValue::Number(7.0),
                            JsValue::Number(5.0),
                            JsValue::Number(1.0),
                            JsValue::Number(2.5),
                        ]);
                        let _ = ws
                            .send(Message::binary(msgpack::encode(&value).unwrap()))
                            .await;
                    }
                    _ => {}
                }
            }
        });
    }
}

#[test]
fn connects_syncs_and_receives_over_real_sockets() {
    let runtime = LiveRuntime::new().unwrap();
    let listener = runtime
        .handle()
        .block_on(TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let _server = runtime.spawn(serve(listener));
    assert!(!NT4_PORTS_DEFAULT.contains(&port));

    let mut timers = TimerQueue::new(Rc::new(MonotonicClock::new()));
    let mut net = TokioTransport::new(runtime.handle().clone());
    let mut realm = JsRealm::new(Box::new(V8MathRandom::from_seed(7)));
    let clock = SystemClock;
    let mut seen = Seen::default();
    let mut client = Nt4Client::new("127.0.0.1", &[port], "AdvantageScope", &mut timers);
    {
        let mut cx = Nt4Context {
            timers: &mut timers,
            net: &mut net,
            clock: &clock,
            realm: &mut realm,
        };
        client.connect(&mut cx);
        client
            .subscribe(&["/".into()], true, true, 0.1, &mut cx)
            .unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline
        && !(seen.connected
            && !seen.data.is_empty()
            && client.server_time_us(&clock, None).is_some())
    {
        while let Some(event) = net.next_event() {
            timers.begin_task();
            let mut cx = Nt4Context {
                timers: &mut timers,
                net: &mut net,
                clock: &clock,
                realm: &mut realm,
            };
            let _ = client.handle_net_event(&event, &mut seen, &mut cx);
        }
        while let Some(id) = timers.fire_next() {
            let mut cx = Nt4Context {
                timers: &mut timers,
                net: &mut net,
                clock: &clock,
                realm: &mut realm,
            };
            let _ = client.handle_timer(id, &mut seen, &mut cx);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(seen.connected, "never connected");
    assert_eq!(seen.announced, vec!["/x".to_owned()]);
    assert_eq!(seen.data, vec![2.5]);
    assert!(
        client.server_time_us(&clock, None).is_some(),
        "no time sync"
    );
    assert!(client.is_connected());
}
