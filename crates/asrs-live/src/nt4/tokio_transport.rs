//! [`Nt4Transport`] over real sockets on the live tokio runtime. Socket tasks post raw events;
//! [`TokioTransport::next_event`] applies the browser's `readyState` rules to one event at a
//! time on the foreground thread, so each sees the state the previous one left behind.
//!
//! `fetch` is [`crate::http`]'s GET, settling once the response headers are in, or the whole body
//! in a [`TokioTransport::with_bodies`]. A server that selects none of the requested
//! subprotocols fails the handshake (tungstenite's rule), where Chromium would open with an empty
//! `protocol`; NT4 servers always select one.
//!
//! Dropping the transport closes its sockets as a page that navigates away does: a closing
//! handshake the server has five seconds to answer.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::client::{Request, generate_key};

use super::transport::{NetEvent, Nt4Transport, ReadyState, SocketId, WsMessage};
use crate::channel::{EventReceiver, EventSender, event_channel_notifying};
use crate::http::{FetchTasks, FetchTransport, RequestId};
use crate::live_url::LiveUrl;
use crate::net_options::{Backlog, NetOptions, Waiting};
use crate::thrown::Thrown;

const CLOSE_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

enum Command {
    Send(WsMessage),
    Close,
}

struct Socket {
    state: ReadyState,
    /// `None` when there is no connection to make.
    commands: Option<UnboundedSender<Command>>,
    /// Gone once the socket is closing: what it still has to send gets five seconds.
    open: Option<oneshot::Sender<()>>,
}

impl Socket {
    fn command(&self, command: Command) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(command);
        }
    }
}

struct Event {
    event: NetEvent,
    _waiting: Option<Waiting>,
}

impl From<NetEvent> for Event {
    fn from(event: NetEvent) -> Self {
        Event {
            event,
            _waiting: None,
        }
    }
}

pub struct TokioTransport {
    options: NetOptions,
    events_tx: EventSender<Event>,
    events_rx: EventReceiver<Event>,
    pending: VecDeque<Event>,
    next_id: u32,
    sockets: BTreeMap<u32, Socket>,
    requests: FetchTasks,
}

impl TokioTransport {
    pub fn new(options: NetOptions) -> Self {
        Self::build(options, false)
    }

    /// A transport whose `fetch` settles once the whole body is in, for sources that read it.
    pub fn with_bodies(options: NetOptions) -> Self {
        Self::build(options, true)
    }

    fn build(options: NetOptions, read_body: bool) -> Self {
        let (events_tx, events_rx) = event_channel_notifying(options.notify.clone());
        Self {
            requests: FetchTasks::new(options.clone(), read_body),
            options,
            events_tx,
            events_rx,
            pending: VecDeque::new(),
            next_id: 1,
            sockets: BTreeMap::new(),
        }
    }

    fn next_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// The next event a browser would dispatch now, if any. Hand it to the client before
    /// asking for the next one.
    pub fn next_event(&mut self) -> Option<NetEvent> {
        loop {
            let event = match self.pending.pop_front() {
                Some(event) => event,
                None => self.events_rx.try_recv()?,
            };
            if self.admit(&event.event) {
                return Some(event.event);
            }
        }
    }

    fn admit(&mut self, event: &NetEvent) -> bool {
        match event {
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
            NetEvent::Fetch(event) => self.requests.admit(event.id()),
        }
    }

    /// Waits until a raw event has arrived, without consuming it; `next_event` then decides
    /// whether it is dispatched.
    pub async fn wait(&mut self) {
        if self.pending.is_empty()
            && let Some(event) = self.events_rx.recv().await
        {
            self.pending.push_back(event);
        }
    }
}

fn invalid_url(problem: impl fmt::Display) -> Thrown {
    Thrown::error(
        "SyntaxError",
        format!("Failed to construct 'WebSocket': The URL {problem}"),
    )
}

fn socket_url(url: &str) -> Result<LiveUrl, Thrown> {
    let parsed = LiveUrl::parse(url)
        .filter(|url| url.scheme == "ws")
        .ok_or_else(|| invalid_url(format!("'{url}' is invalid.")))?;
    match &parsed.fragment {
        Some(fragment) => Err(invalid_url(format!(
            "contains a fragment identifier ('{fragment}'). \
             Fragment identifiers are not allowed in WebSocket URLs."
        ))),
        None => Ok(parsed),
    }
}

/// The request line has only the path and query, so the URI's host stands in for names that
/// `http::Uri` does not take.
fn handshake_request(url: &LiveUrl, protocols: &[&str]) -> Option<Request> {
    let mut request = Request::builder()
        .uri(format!("ws://localhost{}", url.target))
        .header("Host", &url.authority)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", generate_key());
    if !protocols.is_empty() {
        request = request.header("Sec-WebSocket-Protocol", protocols.join(", "));
    }
    request.body(()).ok()
}

impl Nt4Transport for TokioTransport {
    fn ws_open(&mut self, url: &str, protocols: &[&str]) -> Result<SocketId, Thrown> {
        let url = socket_url(url)?;
        let id = self.next_id();
        let mut socket = Socket {
            state: ReadyState::Connecting,
            commands: None,
            open: None,
        };
        let request = handshake_request(&url, protocols).filter(|_| !url.port_is_blocked());
        if let Some(request) = request {
            let (commands, rx) = unbounded_channel();
            let (open, closing) = oneshot::channel();
            socket.commands = Some(commands);
            socket.open = Some(open);
            self.options.handle.spawn(run_socket(Connection {
                id: SocketId(id),
                request,
                address: self.options.dial(&url.host, url.port),
                commands: rx,
                closing,
                events: self.events_tx.clone(),
            }));
        } else {
            let _ = self.events_tx.send(NetEvent::WsError(SocketId(id)).into());
            let _ = self.events_tx.send(NetEvent::WsClose(SocketId(id)).into());
        }
        self.sockets.insert(id, socket);
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
                s.command(Command::Send(data));
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
            s.open = None;
            s.command(Command::Close);
        }
    }
}

impl FetchTransport for TokioTransport {
    fn fetch(&mut self, url: &str) -> RequestId {
        let id = RequestId(self.next_id());
        self.requests.spawn(id, url, &self.events_tx, |event| {
            NetEvent::Fetch(event).into()
        });
        id
    }

    fn fetch_abort(&mut self, id: RequestId) {
        self.requests.abort(id);
    }
}

fn to_message(data: WsMessage) -> Message {
    match data {
        WsMessage::Text(text) => Message::text(text),
        WsMessage::Binary(bytes) => Message::binary(bytes),
    }
}

struct Connection {
    id: SocketId,
    request: Request,
    address: (String, u16),
    commands: UnboundedReceiver<Command>,
    closing: oneshot::Receiver<()>,
    events: EventSender<Event>,
}

async fn run_socket(connection: Connection) {
    let Connection {
        id,
        request,
        address: (host, port),
        mut commands,
        closing,
        events,
    } = connection;
    let fail = |events: &EventSender<Event>| {
        let _ = events.send(NetEvent::WsError(id).into());
        let _ = events.send(NetEvent::WsClose(id).into());
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
    let _ = events.send(NetEvent::WsOpen(id, protocol).into());
    let (mut sink, mut stream) = ws.split();
    let backlog = Backlog::new();
    let receive = async {
        while let Some(Ok(message)) = stream.next().await {
            let message = match message {
                Message::Text(text) => WsMessage::Text(text.as_str().to_owned()),
                Message::Binary(bytes) => WsMessage::Binary(bytes.to_vec()),
                _ => continue,
            };
            let bytes = match &message {
                WsMessage::Text(text) => text.len(),
                WsMessage::Binary(bytes) => bytes.len(),
            };
            let waiting = backlog.reserve(bytes).await;
            let _ = events.send(Event {
                event: NetEvent::WsMessage(id, message),
                _waiting: waiting,
            });
        }
    };
    let send = async {
        while let Some(Command::Send(data)) = commands.recv().await {
            if sink.send(to_message(data)).await.is_err() {
                return false;
            }
        }
        true
    };
    let stalled = async {
        let _ = closing.await;
        tokio::time::sleep(CLOSE_HANDSHAKE_TIMEOUT).await;
    };
    let closed_here = tokio::select! {
        () = receive => false,
        closed_here = send => closed_here,
        () = stalled => false,
    };
    if closed_here {
        let close = async {
            let _ = sink.close().await;
            while let Some(Ok(_)) = stream.next().await {}
        };
        let _ = tokio::time::timeout(CLOSE_HANDSHAKE_TIMEOUT, close).await;
    }
    let _ = events.send(NetEvent::WsClose(id).into());
}
