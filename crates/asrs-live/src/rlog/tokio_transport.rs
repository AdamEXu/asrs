//! [`RlogTransport`] over real TCP sockets on the live tokio runtime. Each socket is a task
//! that applies Node's rules itself (held writes while connecting, `Error` then `Close` on a
//! refused connection, one `Close`, the idle `setTimeout`) and posts events to the foreground
//! side, which drains them in send order.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::channel::{EventReceiver, EventSender, event_channel};

use super::transport::{RlogNetEvent, RlogTransport, TcpId};

enum Command {
    Write(Vec<u8>),
    Destroy,
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

pub struct TokioRlogTransport {
    handle: Handle,
    events_tx: EventSender<RlogNetEvent>,
    events_rx: EventReceiver<RlogNetEvent>,
    pending: VecDeque<RlogNetEvent>,
    next_id: u32,
    sockets: BTreeMap<u32, Socket>,
}

impl TokioRlogTransport {
    pub fn new(handle: Handle) -> Self {
        let (events_tx, events_rx) = event_channel();
        Self {
            handle,
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
            if self.admit(&event) {
                return Some(event);
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
                Some(s) => {
                    s.state = State::Closed;
                    true
                }
                None => false,
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

impl RlogTransport for TokioRlogTransport {
    fn tcp_connect(&mut self, host: &str, port: f64) -> TcpId {
        let id = TcpId(self.next_id);
        self.next_id += 1;
        let (commands, rx) = unbounded_channel();
        let events = self.events_tx.clone();
        let task = self.handle.spawn(run_socket(
            id,
            host.to_owned(),
            port as u16,
            rx,
            events,
        ));
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
        if let Some(s) = self.sockets.get(&id.0)
            && !matches!(s.state, State::Closed)
        {
            let _ = s.commands.send(Command::Destroy);
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
    events: EventSender<RlogNetEvent>,
) {
    let close = |events: &EventSender<RlogNetEvent>| {
        let _ = events.send(RlogNetEvent::Close(id));
    };
    let mut idle = Idle {
        ms: None,
        deadline: None,
        pending_report: false,
    };
    let mut held: Vec<Vec<u8>> = Vec::new();
    let connect = TcpStream::connect((host.as_str(), port));
    tokio::pin!(connect);
    let mut stream = loop {
        tokio::select! {
            connected = &mut connect => match connected {
                Ok(stream) => break stream,
                Err(_) => {
                    let _ = events.send(RlogNetEvent::Error(id));
                    close(&events);
                    return;
                }
            },
            command = commands.recv() => match command {
                Some(Command::Write(data)) => held.push(data),
                Some(Command::SetTimeout(ms)) => {
                    idle.ms = Some(ms);
                    idle.pending_report = true;
                    idle.deadline = Some(Instant::now() + ms);
                }
                Some(Command::Destroy) | None => return close(&events),
            },
            _ = idle.fire() => {
                if idle.pending_report {
                    idle.pending_report = false;
                    let _ = events.send(RlogNetEvent::Timeout(id));
                }
            }
        }
    };
    let _ = stream.set_nodelay(true);
    let _ = events.send(RlogNetEvent::Connect(id));
    idle.arm();
    for data in held {
        if stream.write_all(&data).await.is_err() {
            return close(&events);
        }
        idle.arm();
    }
    let mut buf = vec![0u8; 65536];
    loop {
        tokio::select! {
            read = stream.read(&mut buf) => match read {
                Ok(0) | Err(_) => return close(&events),
                Ok(n) => {
                    idle.arm();
                    let _ = events.send(RlogNetEvent::Data(id, buf[..n].to_vec()));
                }
            },
            command = commands.recv() => match command {
                Some(Command::Write(data)) => {
                    if stream.write_all(&data).await.is_err() {
                        return close(&events);
                    }
                    idle.arm();
                }
                Some(Command::SetTimeout(ms)) => {
                    idle.ms = Some(ms);
                    idle.pending_report = true;
                    idle.deadline = Some(Instant::now() + ms);
                }
                Some(Command::Destroy) | None => return close(&events),
            },
            _ = idle.fire() => {
                if idle.pending_report {
                    idle.pending_report = false;
                    let _ = events.send(RlogNetEvent::Timeout(id));
                }
            }
        }
    }
}
