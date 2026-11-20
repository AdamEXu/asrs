//! [`Nt4Transport`] over real sockets on the live tokio runtime. Socket tasks post raw events;
//! [`TokioTransport::poll`] applies the browser's `readyState` rules to them on the foreground
//! thread and returns the events to hand to the client, in arrival order.
//!
//! `fetch` is a plain HTTP/1.1 GET that resolves on the status line, without following
//! redirects. A server that selects no subprotocol fails the handshake (tungstenite's rule),
//! where Chromium would open with an empty `protocol`; NT4 servers always select one.

use std::collections::BTreeMap;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, Uri};

use super::transport::{NetEvent, Nt4Transport, ReadyState, RequestId, SocketId, WsMessage};
use crate::channel::{EventReceiver, EventSender, event_channel};
use crate::thrown::Thrown;

const CLOSE_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_STATUS_LINE: usize = 8192;

enum Command {
    Send(WsMessage),
    Close,
}

struct Socket {
    state: ReadyState,
    commands: UnboundedSender<Command>,
    task: JoinHandle<()>,
}

pub struct TokioTransport {
    handle: Handle,
    events_tx: EventSender<NetEvent>,
    events_rx: EventReceiver<NetEvent>,
    next_id: u32,
    sockets: BTreeMap<u32, Socket>,
    requests: BTreeMap<u32, JoinHandle<()>>,
}

impl TokioTransport {
    pub fn new(handle: Handle) -> Self {
        let (events_tx, events_rx) = event_channel();
        Self {
            handle,
            events_tx,
            events_rx,
            next_id: 1,
            sockets: BTreeMap::new(),
            requests: BTreeMap::new(),
        }
    }

    fn next_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// The events that arrived since the last call and that a browser would dispatch.
    pub fn poll(&mut self) -> Vec<NetEvent> {
        let mut out = Vec::new();
        for event in self.events_rx.drain() {
            let deliver = match &event {
                NetEvent::WsOpen(id, _) => match self.sockets.get_mut(&id.0) {
                    Some(s) if s.state == ReadyState::Connecting => {
                        s.state = ReadyState::Open;
                        true
                    }
                    _ => false,
                },
                NetEvent::WsMessage(id, _) => self
                    .sockets
                    .get(&id.0)
                    .is_some_and(|s| s.state == ReadyState::Open),
                NetEvent::WsError(id) => match self.sockets.get_mut(&id.0) {
                    Some(s) => {
                        s.state = ReadyState::Closed;
                        true
                    }
                    None => false,
                },
                NetEvent::WsClose(id) => self.sockets.remove(&id.0).is_some(),
                NetEvent::FetchResponse(id, ..) | NetEvent::FetchFailed(id) => {
                    self.requests.remove(&id.0).is_some()
                }
            };
            if deliver {
                out.push(event);
            }
        }
        out
    }

    /// Waits until an event is available, for an owner that has nothing else to do.
    pub async fn wait(&mut self) -> Option<NetEvent> {
        self.events_rx.recv().await
    }
}

impl Drop for TokioTransport {
    fn drop(&mut self) {
        for socket in self.sockets.values() {
            socket.task.abort();
        }
        for task in self.requests.values() {
            task.abort();
        }
    }
}

fn invalid_url(url: &str) -> Thrown {
    Thrown::error(
        "SyntaxError",
        format!("Failed to construct 'WebSocket': The URL '{url}' is invalid."),
    )
}

fn host_port(uri: &Uri, default_port: u16) -> Option<(String, u16)> {
    let host = uri.host()?;
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    Some((host.to_owned(), uri.port_u16().unwrap_or(default_port)))
}

impl Nt4Transport for TokioTransport {
    fn ws_open(&mut self, url: &str, protocols: &[&str]) -> Result<SocketId, Thrown> {
        let mut request = url.into_client_request().map_err(|_| invalid_url(url))?;
        let (host, port) = host_port(request.uri(), 80).ok_or_else(|| invalid_url(url))?;
        if !protocols.is_empty() {
            let value =
                HeaderValue::from_str(&protocols.join(", ")).map_err(|_| invalid_url(url))?;
            request
                .headers_mut()
                .insert("Sec-WebSocket-Protocol", value);
        }
        let id = self.next_id();
        let (commands, rx) = unbounded_channel();
        let events = self.events_tx.clone();
        let task = self
            .handle
            .spawn(run_socket(SocketId(id), request, host, port, rx, events));
        self.sockets.insert(
            id,
            Socket {
                state: ReadyState::Connecting,
                commands,
                task,
            },
        );
        Ok(SocketId(id))
    }

    fn ws_ready_state(&self, id: SocketId) -> ReadyState {
        self.sockets
            .get(&id.0)
            .map_or(ReadyState::Closed, |s| s.state)
    }

    fn ws_send(&mut self, id: SocketId, data: WsMessage) -> Result<(), Thrown> {
        match self.sockets.get(&id.0) {
            Some(s) if s.state == ReadyState::Connecting => Err(Thrown::error(
                "InvalidStateError",
                "Failed to execute 'send' on 'WebSocket': Still in CONNECTING state.",
            )),
            Some(s) if s.state == ReadyState::Open => {
                let _ = s.commands.send(Command::Send(data));
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn ws_close(&mut self, id: SocketId) {
        if let Some(s) = self.sockets.get_mut(&id.0)
            && matches!(s.state, ReadyState::Connecting | ReadyState::Open)
        {
            s.state = ReadyState::Closing;
            let _ = s.commands.send(Command::Close);
        }
    }

    fn fetch(&mut self, url: &str) -> RequestId {
        let id = self.next_id();
        let events = self.events_tx.clone();
        let url = url.to_owned();
        let task = self.handle.spawn(async move {
            let event = match http_get(&url).await {
                Some((status, href)) => NetEvent::FetchResponse(RequestId(id), status, href),
                None => NetEvent::FetchFailed(RequestId(id)),
            };
            let _ = events.send(event);
        });
        self.requests.insert(id, task);
        RequestId(id)
    }

    fn fetch_abort(&mut self, id: RequestId) {
        if let Some(task) = self.requests.remove(&id.0) {
            task.abort();
        }
    }
}

fn to_message(data: WsMessage) -> Message {
    match data {
        WsMessage::Text(text) => Message::text(text),
        WsMessage::Binary(bytes) => Message::binary(bytes),
    }
}

async fn run_socket(
    id: SocketId,
    request: tokio_tungstenite::tungstenite::handshake::client::Request,
    host: String,
    port: u16,
    mut commands: UnboundedReceiver<Command>,
    events: EventSender<NetEvent>,
) {
    let fail = |events: &EventSender<NetEvent>| {
        let _ = events.send(NetEvent::WsError(id));
        let _ = events.send(NetEvent::WsClose(id));
    };
    let connect = async {
        let stream = TcpStream::connect((host.as_str(), port)).await.ok()?;
        let _ = stream.set_nodelay(true);
        tokio_tungstenite::client_async(request, stream).await.ok()
    };
    let (ws, response) = tokio::select! {
        connected = connect => match connected {
            Some(pair) => pair,
            None => return fail(&events),
        },
        _ = commands.recv() => return fail(&events),
    };
    let protocol = response
        .headers()
        .get("Sec-WebSocket-Protocol")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let _ = events.send(NetEvent::WsOpen(id, protocol));
    let (mut sink, mut stream) = ws.split();
    loop {
        tokio::select! {
            message = stream.next() => match message {
                Some(Ok(Message::Text(text))) => {
                    let _ = events.send(NetEvent::WsMessage(id, WsMessage::Text(text.as_str().to_owned())));
                }
                Some(Ok(Message::Binary(bytes))) => {
                    let _ = events.send(NetEvent::WsMessage(id, WsMessage::Binary(bytes.to_vec())));
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => break,
            },
            command = commands.recv() => match command {
                Some(Command::Send(data)) => {
                    if sink.send(to_message(data)).await.is_err() {
                        break;
                    }
                }
                Some(Command::Close) | None => {
                    let _ = sink.close().await;
                    let drain = async { while let Some(Ok(_)) = stream.next().await {} };
                    let _ = tokio::time::timeout(CLOSE_HANDSHAKE_TIMEOUT, drain).await;
                    break;
                }
            },
        }
    }
    let _ = events.send(NetEvent::WsClose(id));
}

/// `fetch(url)` reduced to the status and `response.url`; `None` is a network error.
async fn http_get(url: &str) -> Option<(u16, String)> {
    let uri = Uri::try_from(url).ok()?;
    if uri.scheme_str() != Some("http") {
        return None;
    }
    let (host, port) = host_port(&uri, 80)?;
    let authority = uri.authority()?.as_str().to_owned();
    let path = uri.path_and_query().map_or("/", |p| p.as_str());
    let path = if path.is_empty() { "/" } else { path };
    let mut stream = TcpStream::connect((host.as_str(), port)).await.ok()?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {authority}\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    let line_end = loop {
        if let Some(i) = buffer.windows(2).position(|w| w == b"\r\n") {
            break i;
        }
        if buffer.len() > MAX_STATUS_LINE {
            return None;
        }
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..n]);
    };
    let line = std::str::from_utf8(&buffer[..line_end]).ok()?;
    let mut parts = line.split(' ');
    if !parts.next()?.starts_with("HTTP/1.") {
        return None;
    }
    let code = parts.next()?;
    if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let status = code.bytes().fold(0u16, |n, b| n * 10 + u16::from(b - b'0'));
    let authority = if port == 80 {
        authority.trim_end_matches(":80").to_owned()
    } else {
        authority
    };
    Some((status, format!("http://{authority}{path}")))
}
