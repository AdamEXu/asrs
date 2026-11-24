//! `asrs_parity::live`'s mock servers behind one real loopback socket, for the app's live
//! connections. The port is ephemeral: the hub's dial override sends every connection to it, and
//! the mock still sees the address and port the page asked for.

use std::collections::BTreeMap;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use asrs_live::Dial;
use asrs_parity::json;
use asrs_parity::live::{ClientEvent, MockServer, Payload, ServerEvent, Session};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};

struct State {
    server: MockServer,
    start: Instant,
    next_id: u32,
    open: BTreeMap<u32, UnboundedSender<ServerEvent>>,
    accepted: usize,
    opened: usize,
    seen: Vec<ClientEvent>,
}

impl State {
    fn now(&self) -> f64 {
        self.start.elapsed().as_millis() as f64
    }

    fn route(&self, events: Vec<ServerEvent>) {
        for event in events {
            let id = match &event {
                ServerEvent::WsAccept { id, .. }
                | ServerEvent::WsReject { id }
                | ServerEvent::WsPush { id, .. }
                | ServerEvent::WsDrop { id }
                | ServerEvent::HttpReply { id, .. }
                | ServerEvent::HttpRefuse { id }
                | ServerEvent::TcpAccept { id }
                | ServerEvent::TcpRefuse { id }
                | ServerEvent::TcpPush { id, .. }
                | ServerEvent::TcpDrop { id } => *id,
            };
            if let Some(connection) = self.open.get(&id) {
                let _ = connection.send(event);
            }
        }
    }

    fn client(&mut self, event: ClientEvent) -> Vec<ServerEvent> {
        let now = self.now();
        let events = self.server.client(now, &event).expect("mock server");
        let opened = |e: &ServerEvent| {
            matches!(
                e,
                ServerEvent::WsAccept { .. } | ServerEvent::TcpAccept { .. }
            )
        };
        self.opened += usize::from(events.iter().any(opened));
        self.seen.push(event);
        events
    }
}

type Shared = Arc<Mutex<State>>;

fn with<R>(shared: &Shared, f: impl FnOnce(&mut State) -> R) -> R {
    f(&mut shared.lock().unwrap_or_else(PoisonError::into_inner))
}

/// A bound socket that is not listening refuses connections and keeps its port from anyone else.
struct Closed(#[allow(dead_code)] TcpSocket);

pub struct LiveServer {
    runtime: Runtime,
    shared: Shared,
    port: Arc<AtomicU16>,
    protocol: String,
    up: watch::Sender<bool>,
    closed: Option<Closed>,
}

fn bound() -> io::Result<(TcpSocket, u16)> {
    let socket = TcpSocket::new_v4()?;
    socket.bind(SocketAddr::from(([127, 0, 0, 1], 0)))?;
    let port = socket.local_addr()?.port();
    Ok((socket, port))
}

impl LiveServer {
    /// `server` is a live session's `server` object.
    pub fn new(protocol: &str, server: &str) -> Self {
        let session = format!(
            r#"{{"protocol":"{protocol}","seed":1,"endMs":0,"client":{{"source":"{protocol}","mode":"default"}},"server":{server},"steps":[]}}"#
        );
        let server = Session::parse(&session)
            .and_then(|s| s.server())
            .expect("a live session");
        let mut live = LiveServer {
            runtime: Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .expect("a runtime"),
            shared: Arc::new(Mutex::new(State {
                server,
                start: Instant::now(),
                next_id: 1,
                open: BTreeMap::new(),
                accepted: 0,
                opened: 0,
                seen: Vec::new(),
            })),
            port: Arc::new(AtomicU16::new(0)),
            protocol: protocol.to_owned(),
            up: watch::channel(false).0,
            closed: None,
        };
        live.up();
        live
    }

    pub fn dial(&self) -> Dial {
        let port = self.port.clone();
        Arc::new(move |_, _| ("127.0.0.1".to_owned(), port.load(Ordering::SeqCst)))
    }

    /// Starts listening, on a new port.
    pub fn up(&mut self) {
        let (socket, port) = bound().expect("a loopback port");
        let listener = {
            let _context = self.runtime.enter();
            socket.listen(64).expect("listen")
        };
        self.port.store(port, Ordering::SeqCst);
        self.closed = None;
        self.up = watch::channel(true).0;
        let rlog = self.protocol == "rlog";
        self.runtime.spawn(accept(
            listener,
            self.shared.clone(),
            self.up.subscribe(),
            rlog,
        ));
    }

    /// The server dies: its connections break and new ones are refused.
    pub fn down(&mut self) {
        let (socket, port) = bound().expect("a loopback port");
        self.port.store(port, Ordering::SeqCst);
        self.closed = Some(Closed(socket));
        self.up.send_replace(false);
    }

    /// A session file's server step.
    pub fn step(&self, op: &str) {
        let op = json::parse(op).expect("a step");
        with(&self.shared, |state| {
            let now = state.now();
            let op = op.as_array().expect("a list");
            let events = state.server.step(now, op).expect("a server step");
            state.route(events);
        });
    }

    pub fn connections(&self) -> usize {
        with(&self.shared, |state| state.open.len())
    }

    /// Connections and requests since the start.
    pub fn accepted(&self) -> usize {
        with(&self.shared, |state| state.accepted)
    }

    /// Sockets that server steps have reached since the start.
    pub fn opened(&self) -> usize {
        with(&self.shared, |state| state.opened)
    }

    /// What clients sent since the last call.
    pub fn take_seen(&self) -> Vec<ClientEvent> {
        with(&self.shared, |state| std::mem::take(&mut state.seen))
    }
}

async fn accept(listener: TcpListener, shared: Shared, up: watch::Receiver<bool>, rlog: bool) {
    let mut down = up.clone();
    loop {
        let stream = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(_) => return,
            },
            _ = down.wait_for(|up| !up) => return,
        };
        let (send, events) = unbounded_channel();
        let id = with(&shared, |state| {
            let id = state.next_id;
            state.next_id += 1;
            state.accepted += 1;
            state.open.insert(id, send);
            id
        });
        let shared = shared.clone();
        let mut down = up.clone();
        tokio::spawn(async move {
            let serve = async {
                if rlog {
                    serve_tcp(id, stream, events, &shared).await;
                } else {
                    serve_http(id, stream, events, &shared).await;
                }
            };
            tokio::select! {
                () = serve => {}
                _ = down.wait_for(|up| !up) => {}
            }
            with(&shared, |state| {
                state.open.remove(&id);
                state.client(if rlog {
                    ClientEvent::TcpClose { id }
                } else {
                    ClientEvent::WsClose { id }
                });
            });
        });
    }
}

async fn serve_tcp(
    id: u32,
    mut stream: TcpStream,
    mut events: UnboundedReceiver<ServerEvent>,
    shared: &Shared,
) {
    with(shared, |state| {
        let opened = ClientEvent::TcpOpen {
            id,
            host: "127.0.0.1".to_owned(),
            port: 5800.0,
        };
        state.client(opened);
    });
    let mut buffer = [0u8; 4096];
    loop {
        tokio::select! {
            read = stream.read(&mut buffer) => match read {
                Ok(0) | Err(_) => return,
                Ok(n) => with(shared, |state| {
                    let data = buffer[..n].to_vec();
                    let events = state.client(ClientEvent::TcpSend { id, data });
                    state.route(events);
                }),
            },
            event = events.recv() => match event {
                Some(ServerEvent::TcpPush { data, .. }) => {
                    if stream.write_all(&data).await.is_err() {
                        return;
                    }
                }
                _ => return,
            },
        }
    }
}

async fn serve_http(
    id: u32,
    mut stream: TcpStream,
    events: UnboundedReceiver<ServerEvent>,
    shared: &Shared,
) {
    let mut head = vec![0u8; 8192];
    let head = loop {
        match stream.peek(&mut head).await {
            Ok(n) if head[..n].windows(4).any(|w| w == b"\r\n\r\n") => {
                break String::from_utf8_lossy(&head[..n]).into_owned();
            }
            Ok(0) | Err(_) => return,
            Ok(_) => tokio::task::yield_now().await,
        }
    };
    let header = |name: &str| {
        head.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
    };
    let host = header("host").expect("a Host header").to_owned();
    let path = head.split(' ').nth(1).unwrap_or("/").to_owned();
    if header("upgrade").is_some() {
        return serve_ws(id, stream, format!("ws://{host}{path}"), events, shared).await;
    }
    let url = format!("http://{host}{path}");
    let reply = with(shared, |state| {
        state.client(ClientEvent::HttpGet { id, url })
    });
    let mut request = vec![0u8; head.len()];
    let _ = stream.read_exact(&mut request).await;
    if let Some(ServerEvent::HttpReply { status, body, .. }) = reply.first() {
        let reply = format!(
            "HTTP/1.1 {status} Status\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(reply.as_bytes()).await;
        let mut rest = [0u8; 64];
        let _ = stream.read(&mut rest).await;
    }
}

#[allow(clippy::result_large_err)]
async fn serve_ws(
    id: u32,
    stream: TcpStream,
    url: String,
    mut events: UnboundedReceiver<ServerEvent>,
    shared: &Shared,
) {
    let choose = |request: &Request, mut response: Response| {
        let requested = request.headers().get("Sec-WebSocket-Protocol");
        let protocols = match requested {
            Some(list) => {
                let list = list.to_str().expect("subprotocol names");
                list.split(", ").map(str::to_owned).collect()
            }
            None => Vec::new(),
        };
        let opened = ClientEvent::WsOpen { id, url, protocols };
        let reply = with(shared, |state| state.client(opened));
        let Some(ServerEvent::WsAccept { protocol, .. }) = reply.first() else {
            let mut refused = ErrorResponse::new(None);
            *refused.status_mut() = StatusCode::NOT_FOUND;
            return Err(refused);
        };
        if let Some(protocol) = Some(protocol).filter(|p| !p.is_empty()) {
            let value = HeaderValue::from_str(protocol).expect("a subprotocol");
            response
                .headers_mut()
                .insert("Sec-WebSocket-Protocol", value);
        }
        Ok(response)
    };
    let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, choose).await else {
        return;
    };
    let (mut sink, mut stream) = ws.split();
    loop {
        tokio::select! {
            message = stream.next() => {
                let data = match message {
                    Some(Ok(Message::Text(text))) => Payload::Text(text.as_str().to_owned()),
                    Some(Ok(Message::Binary(bytes))) => Payload::Binary(bytes.to_vec()),
                    Some(Ok(_)) => continue,
                    Some(Err(_)) | None => return,
                };
                with(shared, |state| {
                    let events = state.client(ClientEvent::WsSend { id, data });
                    state.route(events);
                });
            }
            event = events.recv() => match event {
                Some(ServerEvent::WsPush { data, .. }) => {
                    let message = match data {
                        Payload::Text(text) => Message::text(text),
                        Payload::Binary(bytes) => Message::binary(bytes),
                    };
                    if sink.send(message).await.is_err() {
                        return;
                    }
                }
                _ => return,
            },
        }
    }
}
