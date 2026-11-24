//! [`RlogTransport`] over real TCP sockets on the live tokio runtime. Each socket is a task
//! that applies Node's rules itself (held writes while connecting, `Error` then `Close` on a
//! refused connection or a socket error, one `Close`, the idle `setTimeout`) and posts events to
//! the foreground side, which drains them in send order. `destroy` takes effect at once, as
//! `socket.destroyed` does: the task is gone and nothing but the `Close` is dispatched for that
//! socket afterwards.

use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::channel::{EventReceiver, EventSender, event_channel_notifying};
use crate::net_options::{Backlog, NetOptions, Waiting};

use super::transport::{RlogNetEvent, RlogTransport, TcpId};

enum Command {
    Write(Vec<u8>),
    SetTimeout(Duration),
}

enum State {
    Connecting,
    Open,
    Closed,
}

struct Socket {
    state: State,
    commands: UnboundedSender<Command>,
    task: JoinHandle<()>,
}

struct Event {
    event: RlogNetEvent,
    _waiting: Option<Waiting>,
}

impl From<RlogNetEvent> for Event {
    fn from(event: RlogNetEvent) -> Self {
        Event {
            event,
            _waiting: None,
        }
    }
}

pub struct TokioRlogTransport {
    options: NetOptions,
    events_tx: EventSender<Event>,
    events_rx: EventReceiver<Event>,
    pending: VecDeque<Event>,
    next_id: u32,
    sockets: BTreeMap<u32, Socket>,
}

impl TokioRlogTransport {
    pub fn new(options: NetOptions) -> Self {
        let (events_tx, events_rx) = event_channel_notifying(options.notify.clone());
        Self {
            options,
            events_tx,
            events_rx,
            pending: VecDeque::new(),
            next_id: 1,
            sockets: BTreeMap::new(),
        }
    }

    /// The next socket event a `net.Socket` would emit, if any has arrived.
    pub fn next_event(&mut self) -> Option<RlogNetEvent> {
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

    fn admit(&mut self, event: &RlogNetEvent) -> bool {
        match event {
            RlogNetEvent::Connect(id) => match self.sockets.get_mut(&id.0) {
                Some(s) if matches!(s.state, State::Connecting) => {
                    s.state = State::Open;
                    true
                }
                _ => false,
            },
            RlogNetEvent::Data(id, _) => self
                .sockets
                .get(&id.0)
                .is_some_and(|s| matches!(s.state, State::Open)),
            RlogNetEvent::Timeout(id) => self
                .sockets
                .get(&id.0)
                .is_some_and(|s| !matches!(s.state, State::Closed)),
            RlogNetEvent::Error(id) => match self.sockets.get_mut(&id.0) {
                Some(s) if !matches!(s.state, State::Closed) => {
                    s.state = State::Closed;
                    true
                }
                _ => false,
            },
            RlogNetEvent::Close(id) => self.sockets.remove(&id.0).is_some(),
        }
    }

    /// Waits until a raw event has arrived, without consuming it.
    pub async fn wait(&mut self) {
        if self.pending.is_empty()
            && let Some(event) = self.events_rx.recv().await
        {
            self.pending.push_back(event);
        }
    }
}

impl Drop for TokioRlogTransport {
    fn drop(&mut self) {
        for socket in self.sockets.values() {
            socket.task.abort();
        }
    }
}

const AUTO_SELECT_FAMILY_ATTEMPT_TIMEOUT: Duration = Duration::from_millis(250);

/// Node's `autoSelectFamily` (on by default since Node 20): addresses alternate between
/// families starting with the first one resolved, each attempt but the last is abandoned after
/// 250 ms, and a failed attempt moves straight on. Without it, `localhost` on Windows waits
/// about two seconds for `::1` to be refused before trying `127.0.0.1`.
async fn connect_auto_select_family(host: &str, port: u16) -> io::Result<TcpStream> {
    let mut families: [Vec<SocketAddr>; 2] = [Vec::new(), Vec::new()];
    let mut first_is_v6 = None;
    for addr in tokio::net::lookup_host((host, port)).await? {
        let first_is_v6 = *first_is_v6.get_or_insert(addr.is_ipv6());
        let group = &mut families[usize::from(addr.is_ipv6() != first_is_v6)];
        if !group.contains(&addr) {
            group.push(addr);
        }
    }
    let [preferred, other] = families;
    let mut attempts: Vec<SocketAddr> = Vec::with_capacity(preferred.len() + other.len());
    for i in 0..preferred.len().max(other.len()) {
        attempts.extend(preferred.get(i).copied());
        attempts.extend(other.get(i).copied());
    }
    let mut last_error = io::Error::new(io::ErrorKind::NotFound, "no addresses resolved");
    let count = attempts.len();
    for (i, addr) in attempts.into_iter().enumerate() {
        let attempt = TcpStream::connect(addr);
        let result = if i + 1 < count {
            match tokio::time::timeout(AUTO_SELECT_FAMILY_ATTEMPT_TIMEOUT, attempt).await {
                Ok(result) => result,
                Err(_) => Err(io::ErrorKind::TimedOut.into()),
            }
        } else {
            attempt.await
        };
        match result {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

impl RlogTransport for TokioRlogTransport {
    fn tcp_connect(&mut self, host: &str, port: f64) -> TcpId {
        let id = TcpId(self.next_id);
        self.next_id += 1;
        let (commands, rx) = unbounded_channel();
        let events = self.events_tx.clone();
        // Node connects to `localhost` when `host` is empty.
        let host = if host.is_empty() { "localhost" } else { host };
        let (host, port) = self.options.dial(host, port as u16);
        let task = self
            .options
            .handle
            .spawn(run_socket(id, host, port, rx, events));
        self.sockets.insert(
            id.0,
            Socket {
                state: State::Connecting,
                commands,
                task,
            },
        );
        id
    }

    fn tcp_write(&mut self, id: TcpId, data: &[u8]) {
        if let Some(s) = self.sockets.get(&id.0)
            && !matches!(s.state, State::Closed)
        {
            let _ = s.commands.send(Command::Write(data.to_vec()));
        }
    }

    fn tcp_destroy(&mut self, id: TcpId) {
        if let Some(s) = self.sockets.get_mut(&id.0)
            && !matches!(s.state, State::Closed)
        {
            s.state = State::Closed;
            s.task.abort();
            let _ = self.events_tx.send(RlogNetEvent::Close(id).into());
        }
    }

    fn tcp_set_timeout(&mut self, id: TcpId, ms: f64) {
        if let Some(s) = self.sockets.get(&id.0)
            && !matches!(s.state, State::Closed)
        {
            let _ = s
                .commands
                .send(Command::SetTimeout(Duration::from_secs_f64(ms / 1000.0)));
        }
    }
}

struct Idle {
    ms: Option<Duration>,
    deadline: Option<Instant>,
    /// The `once` listener registered by `setTimeout`: consumed by the first fire after
    /// each `SetTimeout` command.
    pending_report: bool,
}

impl Idle {
    fn arm(&mut self) {
        if self.ms.is_some() {
            self.deadline = self.ms.map(|ms| Instant::now() + ms);
        }
    }

    async fn fire(&mut self) {
        match self.deadline {
            Some(at) => tokio::time::sleep_until(at).await,
            None => std::future::pending().await,
        }
        self.deadline = None;
    }
}

async fn run_socket(
    id: TcpId,
    host: String,
    port: u16,
    mut commands: UnboundedReceiver<Command>,
    events: EventSender<Event>,
) {
    let close = |events: &EventSender<Event>| {
        let _ = events.send(RlogNetEvent::Close(id).into());
    };
    let fail = |events: &EventSender<Event>| {
        let _ = events.send(RlogNetEvent::Error(id).into());
        close(events);
    };
    let mut idle = Idle {
        ms: None,
        deadline: None,
        pending_report: false,
    };
    let mut held: Vec<Vec<u8>> = Vec::new();
    let connect = connect_auto_select_family(&host, port);
    tokio::pin!(connect);
    let mut stream = loop {
        tokio::select! {
            connected = &mut connect => match connected {
                Ok(stream) => break stream,
                Err(_) => return fail(&events),
            },
            command = commands.recv() => match command {
                Some(Command::Write(data)) => held.push(data),
                Some(Command::SetTimeout(ms)) => {
                    idle.ms = Some(ms);
                    idle.pending_report = true;
                    idle.deadline = Some(Instant::now() + ms);
                }
                None => return close(&events),
            },
            _ = idle.fire() => {
                if idle.pending_report {
                    idle.pending_report = false;
                    let _ = events.send(RlogNetEvent::Timeout(id).into());
                }
            }
        }
    };
    let _ = stream.set_nodelay(true);
    let _ = events.send(RlogNetEvent::Connect(id).into());
    idle.arm();
    for data in held {
        if stream.write_all(&data).await.is_err() {
            return fail(&events);
        }
        idle.arm();
    }
    let backlog = Backlog::new();
    let mut buf = vec![0u8; 65536];
    loop {
        tokio::select! {
            read = stream.read(&mut buf) => match read {
                Ok(0) => return close(&events),
                Err(_) => return fail(&events),
                Ok(n) => {
                    idle.arm();
                    let waiting = backlog.reserve(n).await;
                    let _ = events.send(Event {
                        event: RlogNetEvent::Data(id, buf[..n].to_vec()),
                        _waiting: waiting,
                    });
                }
            },
            command = commands.recv() => match command {
                Some(Command::Write(data)) => {
                    if stream.write_all(&data).await.is_err() {
                        return fail(&events);
                    }
                    idle.arm();
                }
                Some(Command::SetTimeout(ms)) => {
                    idle.ms = Some(ms);
                    idle.pending_report = true;
                    idle.deadline = Some(Instant::now() + ms);
                }
                None => return close(&events),
            },
            _ = idle.fire() => {
                if idle.pending_report {
                    idle.pending_report = false;
                    let _ = events.send(RlogNetEvent::Timeout(id).into());
                }
            }
        }
    }
}
