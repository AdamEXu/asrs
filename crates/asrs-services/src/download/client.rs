//! ssh2's `Client` as main.ts uses it: which events a client emits, in which order, and when a call
//! on it throws. The socket is modelled after node's `net.Socket`, whose state upstream's error
//! handling leans on: main ends the failed client from inside its `error` event, and writing to a
//! socket that has no handle yet is itself an error, emitted on that client later.
//!
//! [`Clients`] owns every client. Calls and connection events queue work; [`Clients::next_event`] runs it up
//! to the next event for the caller, so that a listener runs before anything ssh2 does after it.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use super::connection::{ChannelEvent, Connection, ConnectionEvent};
use super::downloader::{ClientId, DirEntry, SftpId, Token};
use super::handshake::HandshakeEvent;
use super::sftp::{Fd, Output, Sftp};
use crate::fs::uv_error;

const NOT_CONNECTED: &str = "Not connected";
const CONNECTION_LOST: &str = "Connection lost before handshake";
const NO_RESPONSE: &str = "No response from server";
const SUBSYSTEM_FAILED: &str = "Unable to start subsystem: sftp";
const BY_APPLICATION: u32 = 11;

/// What a client authenticates with, in ssh2's order: `none`, the password, the key.
#[derive(Clone, Debug)]
pub struct Login {
    pub username: String,
    pub password: Option<String>,
    /// An OpenSSH private key. Upstream never uses one.
    pub key: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub ready_timeout_ms: f64,
    pub login: Login,
}

#[derive(Debug, PartialEq)]
pub enum ClientEvent {
    Error(ClientId, String),
    Ready(ClientId),
    Close(ClientId),
    Sftp(ClientId, Token, Result<SftpId, String>),
    Readdir(ClientId, Token, Result<Vec<DirEntry>, String>),
    Step {
        token: Token,
        transferred: f64,
        total: f64,
    },
    Got(ClientId, Token, Result<(), String>),
    /// `setTimeout` and `clearTimeout` for `readyTimeout`.
    StartTimeout(ClientId, f64),
    ClearTimeout(ClientId),
    /// `dns.lookup`
    Resolve(ClientId, String),
}

/// What a connection or name lookup reports. Opaque to the caller, which passes it back to
/// [`Clients::received`].
#[derive(Debug)]
pub struct NetworkEvent(pub(super) Network);

#[derive(Debug)]
pub(super) enum Network {
    Resolved(Result<IpAddr, &'static str>),
    Connection(ConnectionEvent),
}

enum Work {
    Emit(ClientEvent),
    Resolved(ClientId, Result<IpAddr, &'static str>),
    /// `if (isWritable(sock)) sock.end()`
    EndSocket(ClientId),
    Destroy(ClientId),
    /// The failed write to a socket without a handle.
    SocketClosed(ClientId),
    Closed(ClientId),
    CloseChannels(ClientId),
    /// Nothing can happen to the client any more, unless its name lookup is still running.
    Forget(ClientId),
    LocalFile(SftpId, Result<Fd, String>),
}

enum Stage {
    Opening,
    Subsystem,
    Started,
}

struct Channel {
    id: SftpId,
    stage: Stage,
    token: Token,
    sftp: Sftp,
    /// Whether `Client.sftp`'s callback is still to run.
    starting: bool,
    /// No EOF yet: ssh2 fails a session's requests once, at its EOF or its close.
    readable: bool,
    exited: bool,
    closed: bool,
}

struct Client {
    target: Target,
    connection: Option<Connection>,
    /// The name lookup has not finished.
    resolving: bool,
    writable: bool,
    destroyed: bool,
    was_connected: bool,
    saw_header: bool,
    done: bool,
    openssh_limits: bool,
    /// The protocol was cleaned up by a close, and the socket connected again afterwards.
    reused: bool,
    channels: BTreeMap<u32, Channel>,
    next_channel: u32,
}

pub struct Clients {
    clients: HashMap<ClientId, Client>,
    next_client: ClientId,
    next_sftp: SftpId,
    next_token: Token,
    sftps: HashMap<SftpId, (ClientId, u32)>,
    files: HashMap<Fd, (ClientId, File)>,
    next_fd: Fd,
    work: VecDeque<Work>,
    notify: Arc<dyn Fn(ClientId, NetworkEvent) + Send + Sync>,
}

impl Clients {
    /// `notify` is called on a runtime thread; the event goes to [`Clients::received`]. The other
    /// methods must be called inside a tokio runtime.
    pub fn new(notify: impl Fn(ClientId, NetworkEvent) + Send + Sync + 'static) -> Clients {
        Clients {
            clients: HashMap::new(),
            next_client: 0,
            next_sftp: 0,
            next_token: 0,
            sftps: HashMap::new(),
            files: HashMap::new(),
            next_fd: 1,
            work: VecDeque::new(),
            notify: Arc::new(notify),
        }
    }

    pub fn token(&mut self) -> Token {
        self.next_token += 1;
        self.next_token
    }

    /// `new Client().connect(...)` with `forceIPv4`.
    pub fn connect(&mut self, mut target: Target) -> ClientId {
        self.next_client += 1;
        let id = self.next_client;
        if target.host.is_empty() {
            target.host = "localhost".to_owned();
        }
        self.work.push_back(match literal_ip(&target.host) {
            Some(ip) => Work::Resolved(id, Ok(ip)),
            None => Work::Emit(ClientEvent::Resolve(id, target.host.clone())),
        });
        self.clients.insert(
            id,
            Client {
                target,
                connection: None,
                resolving: true,
                writable: true,
                destroyed: false,
                was_connected: false,
                saw_header: false,
                done: false,
                openssh_limits: false,
                reused: false,
                channels: BTreeMap::new(),
                next_channel: 0,
            },
        );
        id
    }

    /// `Client.end()`
    pub fn end(&mut self, id: ClientId) {
        let Some(client) = self.clients.get_mut(&id) else {
            return;
        };
        if !client.writable {
            return;
        }
        client.writable = false;
        match &client.connection {
            Some(connection) => connection.end(),
            None => self.work.push_back(Work::SocketClosed(id)),
        }
    }

    /// `Client.sftp`
    pub fn sftp(&mut self, id: ClientId) -> Result<Token, String> {
        let Some(client) = self.clients.get_mut(&id).filter(|client| client.writable) else {
            return Err(NOT_CONNECTED.to_owned());
        };
        self.next_token += 1;
        self.next_sftp += 1;
        let channel = client.next_channel;
        client.next_channel += 1;
        client.channels.insert(
            channel,
            Channel {
                id: self.next_sftp,
                stage: Stage::Opening,
                token: self.next_token,
                sftp: Sftp::new(client.openssh_limits),
                starting: true,
                readable: true,
                exited: false,
                closed: false,
            },
        );
        self.sftps.insert(self.next_sftp, (id, channel));
        if let Some(connection) = &client.connection {
            connection.open_sftp(channel);
        }
        Ok(self.next_token)
    }

    fn channel(&mut self, sftp: SftpId) -> Option<(ClientId, u32, &mut Channel)> {
        let (client, channel) = *self.sftps.get(&sftp)?;
        let state = self.clients.get_mut(&client)?.channels.get_mut(&channel)?;
        Some((client, channel, state))
    }

    /// `SFTP.readdir`
    pub fn readdir(&mut self, sftp: SftpId, path: &str) -> Token {
        let token = self.token();
        if let Some((client, channel, state)) = self.channel(sftp) {
            state.sftp.readdir(token, path);
            self.flush(client, channel);
        }
        token
    }

    /// `SFTP.fastGet`
    pub fn fast_get(&mut self, sftp: SftpId, remote: &str, local: &str) -> Token {
        let token = self.token();
        if let Some((client, channel, state)) = self.channel(sftp) {
            state.sftp.fast_get(token, remote, local);
            self.flush(client, channel);
        }
        token
    }

    /// `readyTimeout` ran out.
    pub fn timed_out(&mut self, id: ClientId) {
        self.error(id, "Timed out while waiting for handshake");
        self.work.push_back(Work::Destroy(id));
    }

    /// A listener threw out of an event of `id`: the `data` listener reports it and ends the socket.
    pub fn threw(&mut self, id: ClientId, message: &str) {
        self.error(id, message);
        self.work.push_back(Work::EndSocket(id));
    }

    pub fn received(&mut self, id: ClientId, event: NetworkEvent) {
        match event.0 {
            Network::Resolved(result) => self.work.push_back(Work::Resolved(id, result)),
            Network::Connection(event) => self.connection_event(id, event),
        }
    }

    fn emit(&mut self, event: ClientEvent) {
        self.work.push_back(Work::Emit(event));
    }

    fn error(&mut self, id: ClientId, message: &str) {
        self.emit(ClientEvent::Error(id, message.to_owned()));
    }

    /// Runs queued work up to the next event.
    pub fn next_event(&mut self) -> Option<ClientEvent> {
        while let Some(work) = self.work.pop_front() {
            match work {
                Work::Emit(event) => return Some(event),
                Work::Resolved(id, result) => self.resolved(id, result),
                Work::EndSocket(id) => {
                    if let Some(client) = self.clients.get_mut(&id)
                        && client.writable
                    {
                        client.writable = false;
                        if let Some(connection) = &client.connection {
                            connection.end();
                        }
                    }
                }
                Work::Destroy(id) => {
                    if let Some(client) = self.clients.get_mut(&id)
                        && !client.destroyed
                    {
                        client.destroyed = true;
                        client.writable = false;
                        client.connection = None;
                        self.work.push_back(Work::Closed(id));
                    }
                }
                Work::SocketClosed(id) => {
                    if let Some(client) = self.clients.get_mut(&id) {
                        client.destroyed = true;
                        client.connection = None;
                        self.emit(ClientEvent::ClearTimeout(id));
                        self.error(id, "Socket is closed");
                        self.work.push_back(Work::Closed(id));
                    }
                }
                Work::Closed(id) => self.closed(id),
                Work::CloseChannels(id) => self.close_channels(id),
                Work::Forget(id) => {
                    if self
                        .clients
                        .get(&id)
                        .is_some_and(|c| !c.resolving && c.destroyed)
                    {
                        self.clients.remove(&id);
                        self.sftps.retain(|_, (client, _)| *client != id);
                        self.files.retain(|_, (client, _)| *client != id);
                    }
                }
                Work::LocalFile(sftp, result) => {
                    if let Some((client, channel, state)) = self.channel(sftp) {
                        state.sftp.fs_done(result);
                        self.flush(client, channel);
                    }
                }
            }
        }
        None
    }

    fn resolved(&mut self, id: ClientId, result: Result<IpAddr, &'static str>) {
        let Some(client) = self.clients.get_mut(&id) else {
            return;
        };
        client.resolving = false;
        let ip = match result {
            Ok(ip) => ip,
            Err(code) => {
                let host = client.target.host.clone();
                self.error(
                    id,
                    &format!(
                        "Error while looking up IPv4 address for '{host}': Error: getaddrinfo \
                         {code} {host}"
                    ),
                );
                self.emit(ClientEvent::Close(id));
                return;
            }
        };
        client.reused = client.destroyed;
        client.destroyed = false;
        let notify = self.notify.clone();
        client.connection = Some(Connection::open(
            SocketAddr::new(ip, client.target.port),
            client.target.login.clone(),
            move |event| notify(id, NetworkEvent(Network::Connection(event))),
        ));
        let ready_timeout_ms = client.target.ready_timeout_ms;
        self.emit(ClientEvent::StartTimeout(id, ready_timeout_ms));
    }

    fn connection_event(&mut self, id: ClientId, event: ConnectionEvent) {
        let Some(client) = self.clients.get_mut(&id) else {
            return;
        };
        if client.destroyed {
            return;
        }
        match event {
            ConnectionEvent::Connected => {
                client.was_connected = true;
                if client.reused {
                    self.error(id, "Instance unusable after fatal error");
                }
            }
            ConnectionEvent::ConnectFailed(message) => {
                client.writable = false;
                self.emit(ClientEvent::ClearTimeout(id));
                self.error(id, &message);
                self.work.push_back(Work::Destroy(id));
            }
            // A reused client threw before it listened to its socket.
            _ if client.reused => {}
            ConnectionEvent::Handshake(event) => self.handshake_event(id, event),
            ConnectionEvent::Ready => {
                self.emit(ClientEvent::ClearTimeout(id));
                self.emit(ClientEvent::Ready(id));
            }
            ConnectionEvent::AuthFailed => {
                self.error(id, "All configured authentication methods failed");
                self.work.push_back(Work::EndSocket(id));
            }
            ConnectionEvent::SocketError(message) => {
                client.writable = false;
                self.emit(ClientEvent::ClearTimeout(id));
                self.error(id, &message);
                self.work.push_back(Work::Destroy(id));
            }
            ConnectionEvent::SocketEnd => {
                client.writable = false;
                self.lost_before_handshake(id);
                self.emit(ClientEvent::ClearTimeout(id));
                self.work.push_back(Work::Destroy(id));
            }
            ConnectionEvent::Channel(channel, event) => self.channel_event(id, channel, event),
        }
    }

    fn handshake_event(&mut self, id: ClientId, event: HandshakeEvent) {
        let Some(client) = self.clients.get_mut(&id) else {
            return;
        };
        match event {
            HandshakeEvent::Header { openssh_limits } => {
                client.saw_header = true;
                client.openssh_limits = openssh_limits;
            }
            HandshakeEvent::Thrown(message) => self.threw(id, &message),
            HandshakeEvent::Fatal { message, handshake } => {
                if handshake {
                    self.emit(ClientEvent::ClearTimeout(id));
                }
                self.threw(id, &message);
            }
            HandshakeEvent::Disconnect {
                reason,
                description,
            } => {
                if reason != BY_APPLICATION {
                    let message = if description.is_empty() {
                        disconnect_reason(reason)
                    } else {
                        description
                    };
                    self.error(id, &message);
                }
                self.work.push_back(Work::EndSocket(id));
            }
        }
    }

    /// `onDone`
    fn lost_before_handshake(&mut self, id: ClientId) {
        if let Some(client) = self.clients.get_mut(&id)
            && !client.done
        {
            client.done = true;
            if client.was_connected && !client.saw_header {
                self.error(id, CONNECTION_LOST);
            }
        }
    }

    fn closed(&mut self, id: ClientId) {
        let mut rest = std::mem::take(&mut self.work);
        self.lost_before_handshake(id);
        self.emit(ClientEvent::ClearTimeout(id));
        self.emit(ClientEvent::Close(id));
        self.work.push_back(Work::CloseChannels(id));
        self.work.append(&mut rest);
    }

    fn close_channels(&mut self, id: ClientId) {
        let Some(client) = self.clients.get_mut(&id) else {
            return;
        };
        let channels: Vec<u32> = client.channels.keys().copied().collect();
        for channel in channels {
            let Some(state) = self.channel_mut(id, channel) else {
                continue;
            };
            if matches!(state.stage, Stage::Opening) {
                self.started(id, channel, Err(NO_RESPONSE.to_owned()));
            } else {
                self.channel_closed(id, channel);
            }
        }
        self.work.push_back(Work::Forget(id));
    }

    fn channel_mut(&mut self, id: ClientId, channel: u32) -> Option<&mut Channel> {
        self.clients.get_mut(&id)?.channels.get_mut(&channel)
    }

    /// Runs `Client.sftp`'s callback once.
    fn started(&mut self, id: ClientId, channel: u32, result: Result<(), String>) {
        let Some(state) = self.channel_mut(id, channel) else {
            return;
        };
        if std::mem::take(&mut state.starting) {
            let event = ClientEvent::Sftp(id, state.token, result.map(|()| state.id));
            self.emit(event);
        }
    }

    fn channel_event(&mut self, id: ClientId, channel: u32, event: ChannelEvent) {
        let Some(state) = self.channel_mut(id, channel) else {
            return;
        };
        if state.closed {
            return;
        }
        match event {
            ChannelEvent::Opened => state.stage = Stage::Subsystem,
            ChannelEvent::OpenFailed(description) => {
                state.closed = true;
                self.started(
                    id,
                    channel,
                    Err(format!("(SSH) Channel open failure: {description}")),
                );
            }
            ChannelEvent::SubsystemFailed => {
                self.started(id, channel, Err(SUBSYSTEM_FAILED.to_owned()));
            }
            ChannelEvent::SubsystemStarted => {
                state.stage = Stage::Started;
                state.sftp.init();
                self.flush(id, channel);
            }
            ChannelEvent::Data(data) => {
                let result = state.sftp.push(&data);
                self.flush(id, channel);
                if let Err(message) = result {
                    self.threw(id, &message);
                }
            }
            ChannelEvent::Eof => self.channel_ended(id, channel),
            ChannelEvent::Exit(code) => self.exited(id, channel, || {
                format!("Received exit code {code} while establishing SFTP session")
            }),
            ChannelEvent::Signal(signal) => self.exited(id, channel, || {
                format!("Received signal {signal} while establishing SFTP session")
            }),
            ChannelEvent::Closed => {
                if matches!(state.stage, Stage::Opening) {
                    state.closed = true;
                    let message = "(SSH) Channel open failure: server closed channel unexpectedly";
                    self.started(id, channel, Err(message.to_owned()));
                } else {
                    self.channel_closed(id, channel);
                }
            }
        }
    }

    fn channel_ended(&mut self, id: ClientId, channel: u32) {
        let Some(state) = self.channel_mut(id, channel) else {
            return;
        };
        if !std::mem::take(&mut state.readable) {
            return;
        }
        let result = state.sftp.end();
        self.flush(id, channel);
        if let Err(message) = result {
            self.threw(id, &message);
        }
    }

    fn exited(&mut self, id: ClientId, channel: u32, message: impl Fn() -> String) {
        if let Some(state) = self.channel_mut(id, channel)
            && !state.exited
        {
            state.exited = true;
            self.session_ended(id, channel, message);
        }
    }

    /// The `exit` and `close` listeners, attached while the session starts.
    fn session_ended(&mut self, id: ClientId, channel: u32, message: impl Fn() -> String) {
        if let Some(state) = self.channel_mut(id, channel)
            && matches!(state.stage, Stage::Started)
            && state.starting
        {
            state.sftp.stop_listening();
            self.started(id, channel, Err(message()));
        }
    }

    /// `onCHANNEL_CLOSE` for a channel that was opened.
    fn channel_closed(&mut self, id: ClientId, channel: u32) {
        let Some(state) = self.channel_mut(id, channel) else {
            return;
        };
        if std::mem::replace(&mut state.closed, true) {
            return;
        }
        self.channel_ended(id, channel);
        if let Some(state) = self.channel_mut(id, channel)
            && matches!(state.stage, Stage::Subsystem)
        {
            self.started(id, channel, Err(SUBSYSTEM_FAILED.to_owned()));
        }
        self.session_ended(id, channel, || {
            "Received unexpected SFTP session termination".to_owned()
        });
    }

    /// Carries out what the session of `channel` asked for.
    fn flush(&mut self, id: ClientId, channel: u32) {
        let Some(state) = self.channel_mut(id, channel) else {
            return;
        };
        let sftp = state.id;
        for output in state.sftp.take() {
            match output {
                Output::Send(data) => {
                    if let Some(connection) = self.connection(id) {
                        connection.send(channel, data);
                    }
                }
                Output::Close => {
                    if let Some(connection) = self.connection(id) {
                        connection.close_channel(channel);
                    }
                }
                Output::Ready => self.started(id, channel, Ok(())),
                Output::Error(message) => self.started(id, channel, Err(message)),
                Output::Readdir { id: token, result } => {
                    self.emit(ClientEvent::Readdir(id, token, result));
                }
                Output::Step {
                    id: token,
                    total,
                    size,
                    ..
                } => self.emit(ClientEvent::Step {
                    token,
                    transferred: total,
                    total: size,
                }),
                Output::Got { id: token, result } => {
                    self.emit(ClientEvent::Got(id, token, result));
                }
                Output::FsOpen { path } => {
                    let result = create(&path).map(|file| {
                        self.next_fd += 1;
                        self.files.insert(self.next_fd, (id, file));
                        self.next_fd
                    });
                    self.work.push_back(Work::LocalFile(sftp, result));
                }
                Output::FsWrite { fd, data, position } => {
                    let result = match self.files.get_mut(&fd) {
                        Some((_, file)) => write_at(file, &data, position as u64),
                        None => Err("EBADF: bad file descriptor, write".to_owned()),
                    };
                    self.work
                        .push_back(Work::LocalFile(sftp, result.map(|()| fd)));
                }
                Output::FsClose { fd } => {
                    self.files.remove(&fd);
                    self.work.push_back(Work::LocalFile(sftp, Ok(fd)));
                }
            }
        }
    }

    fn connection(&self, id: ClientId) -> Option<&Connection> {
        self.clients.get(&id)?.connection.as_ref()
    }
}

fn io_error(e: &io::Error, call: &str) -> String {
    match uv_error(e) {
        Some((code, description)) => format!("{code}: {description}, {call}"),
        None => format!("{e}, {call}"),
    }
}

/// `fs.open(path, "w")`
fn create(path: &str) -> Result<File, String> {
    File::create(path).map_err(|e| io_error(&e, &format!("open '{path}'")))
}

fn write_at(file: &mut File, data: &[u8], position: u64) -> Result<(), String> {
    file.seek(SeekFrom::Start(position))
        .and_then(|_| file.write_all(data))
        .map_err(|e| io_error(&e, "write"))
}

/// `net.isIP`: the address `dns.lookup` hands back without looking anything up.
fn literal_ip(host: &str) -> Option<IpAddr> {
    if host.contains(':') {
        return match url::Host::parse(&format!("[{host}]")) {
            Ok(url::Host::Ipv6(ip)) => Some(IpAddr::V6(ip)),
            _ => None,
        };
    }
    let mut octets = [0u8; 4];
    let mut parts = host.split('.');
    for octet in &mut octets {
        let part = parts.next()?;
        let digits = (1..=3).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit());
        if !digits || (part.len() > 1 && part.starts_with('0')) {
            return None;
        }
        let value = part
            .bytes()
            .fold(0, |value, digit| value * 10 + u32::from(digit - b'0'));
        *octet = u8::try_from(value).ok()?;
    }
    match parts.next() {
        None => Some(IpAddr::from(octets)),
        Some(_) => None,
    }
}

fn disconnect_reason(reason: u32) -> String {
    let name = match reason {
        1 => "HOST_NOT_ALLOWED_TO_CONNECT",
        2 => "PROTOCOL_ERROR",
        3 => "KEY_EXCHANGE_FAILED",
        4 => "RESERVED",
        5 => "MAC_ERROR",
        6 => "COMPRESSION_ERROR",
        7 => "SERVICE_NOT_AVAILABLE",
        8 => "PROTOCOL_VERSION_NOT_SUPPORTED",
        9 => "HOST_KEY_NOT_VERIFIABLE",
        10 => "CONNECTION_LOST",
        11 => "BY_APPLICATION",
        12 => "TOO_MANY_CONNECTIONS",
        13 => "AUTH_CANCELED_BY_USER",
        14 => "NO_MORE_AUTH_METHODS_AVAILABLE",
        15 => "ILLEGAL_USER_NAME",
        _ => return format!("Unexpected disconnection reason: {reason}"),
    };
    name.to_owned()
}

/// `dns.lookup(name, 4)`
pub async fn lookup(name: &str) -> NetworkEvent {
    let result = match tokio::net::lookup_host((name, 0)).await {
        Ok(mut addresses) => addresses
            .find(|address| address.is_ipv4())
            .map(|address| address.ip())
            .ok_or("ENOTFOUND"),
        Err(e) if try_again(&e) => Err("EAI_AGAIN"),
        Err(_) => Err("ENOTFOUND"),
    };
    NetworkEvent(Network::Resolved(result))
}

fn try_again(error: &io::Error) -> bool {
    const WSATRY_AGAIN: i32 = 11002;
    (cfg!(windows) && error.raw_os_error() == Some(WSATRY_AGAIN))
        || format!("{error}").contains("Temporary failure in name resolution")
}
