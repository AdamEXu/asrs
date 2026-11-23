//! One TCP connection and the `russh` session on it. The socket is pumped here rather than handed
//! to `russh`, so that [`Handshake`] sees every byte first and socket errors are reported the way
//! node names them.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use russh::client::{self, AuthResult, DisconnectReason, Handle};
use russh::keys::{Algorithm, PrivateKey, PrivateKeyWithHashAlg};
use russh::{
    Channel, ChannelMsg, ChannelOpenFailure, Disconnect, Limits, Preferred, Sig, SshId, cipher,
    compression, kex, mac,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc::{self, Receiver, Sender, UnboundedReceiver, UnboundedSender};
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinSet;

use super::client::Login;
use super::handshake::{
    Handshake, HandshakeEvent, OFFERED_CIPHERS, OFFERED_COMPRESSION, OFFERED_HOST_KEYS,
    OFFERED_KEX, OFFERED_MACS,
};

const PIPE_SIZE: usize = 256 * 1024;
const READ_SIZE: usize = 64 * 1024;
const WINDOW_SIZE: u32 = 2 * 1024 * 1024;
const PACKET_SIZE: u32 = 32 * 1024;
const REKEY_BYTES: usize = 1 << 30;
const REKEY_TIME: Duration = Duration::from_secs(3600);
const IDENT: &str = "SSH-2.0-asrs";

#[derive(Debug)]
pub(super) enum ChannelEvent {
    Opened,
    OpenFailed(String),
    SubsystemFailed,
    SubsystemStarted,
    Data(Vec<u8>),
    Eof,
    Exit(u32),
    Signal(String),
    Closed,
}

#[derive(Debug)]
pub(super) enum ConnectionEvent {
    Connected,
    ConnectFailed(String),
    Handshake(HandshakeEvent),
    Ready,
    AuthFailed,
    SocketError(String),
    SocketEnd,
    Channel(u32, ChannelEvent),
}

#[derive(Debug)]
enum Command {
    Sftp(u32),
    Send(u32, Vec<u8>),
    Close(u32),
    End,
}

/// Dropping it destroys the socket.
pub(super) struct Connection {
    commands: UnboundedSender<Command>,
    _task: JoinSet<()>,
}

impl Connection {
    pub(super) fn open(
        address: SocketAddr,
        login: Login,
        events: impl Fn(ConnectionEvent) + Send + Sync + 'static,
    ) -> Connection {
        let (commands, receiver) = mpsc::unbounded_channel();
        let mut task = JoinSet::new();
        task.spawn(run(address, login, receiver, Events(Arc::new(events))));
        Connection {
            commands,
            _task: task,
        }
    }

    fn command(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    pub(super) fn open_sftp(&self, channel: u32) {
        self.command(Command::Sftp(channel));
    }

    pub(super) fn send(&self, channel: u32, data: Vec<u8>) {
        self.command(Command::Send(channel, data));
    }

    pub(super) fn close_channel(&self, channel: u32) {
        self.command(Command::Close(channel));
    }

    /// Says goodbye where a session exists, and closes the sending side.
    pub(super) fn end(&self) {
        self.command(Command::End);
    }
}

#[derive(Clone)]
struct Events(Arc<dyn Fn(ConnectionEvent) + Send + Sync>);

impl Events {
    fn send(&self, event: ConnectionEvent) {
        (self.0)(event);
    }
}

fn error_code(error: &io::Error) -> &'static str {
    match error.kind() {
        io::ErrorKind::ConnectionRefused => "ECONNREFUSED",
        io::ErrorKind::ConnectionReset => "ECONNRESET",
        io::ErrorKind::ConnectionAborted => "ECONNABORTED",
        io::ErrorKind::BrokenPipe => "EPIPE",
        io::ErrorKind::TimedOut => "ETIMEDOUT",
        io::ErrorKind::HostUnreachable => "EHOSTUNREACH",
        io::ErrorKind::NetworkUnreachable => "ENETUNREACH",
        io::ErrorKind::NetworkDown => "ENETDOWN",
        io::ErrorKind::AddrNotAvailable => "EADDRNOTAVAIL",
        io::ErrorKind::PermissionDenied => "EACCES",
        _ => "UNKNOWN",
    }
}

async fn run(
    address: SocketAddr,
    login: Login,
    mut commands: UnboundedReceiver<Command>,
    events: Events,
) {
    let mut ended = false;
    let connecting = TcpStream::connect(address);
    tokio::pin!(connecting);
    let socket = loop {
        tokio::select! {
            result = &mut connecting => break result,
            command = commands.recv(), if !ended => {
                ended = !matches!(command, Some(Command::Sftp(_) | Command::Send(..) | Command::Close(_)));
            }
        }
    };
    let socket = match socket {
        Ok(socket) => socket,
        Err(e) => {
            events.send(ConnectionEvent::ConnectFailed(format!(
                "connect {} {address}",
                error_code(&e)
            )));
            return;
        }
    };
    events.send(ConnectionEvent::Connected);
    let _ = socket.set_nodelay(true);
    let (socket_reader, mut socket_writer) = socket.into_split();
    let (near, far) = tokio::io::duplex(PIPE_SIZE);
    let (pipe_reader, pipe_writer) = tokio::io::split(near);
    let (guard, drained) = mpsc::channel(1);
    let reported = Arc::new(AtomicBool::new(false));
    let mut tasks = JoinSet::new();
    if ended {
        let _ = socket_writer.shutdown().await;
        drop((far, guard, commands, pipe_reader));
        receive(
            socket_reader,
            pipe_writer,
            drained,
            events,
            reported,
            Arc::default(),
        )
        .await;
    } else {
        let disconnected = Arc::new(AtomicBool::new(false));
        let (abandon, abandoned) = oneshot::channel();
        tasks.spawn(transmit(
            pipe_reader,
            socket_writer,
            abandoned,
            events.clone(),
            reported.clone(),
        ));
        tasks.spawn(session(
            far,
            login,
            commands,
            abandon,
            events.clone(),
            guard,
            disconnected.clone(),
        ));
        receive(
            socket_reader,
            pipe_writer,
            drained,
            events,
            reported,
            disconnected,
        )
        .await;
    }
}

async fn transmit(
    mut pipe: ReadHalf<DuplexStream>,
    mut socket: OwnedWriteHalf,
    mut abandoned: oneshot::Receiver<()>,
    events: Events,
    reported: Arc<AtomicBool>,
) {
    let mut buffer = vec![0; READ_SIZE];
    let mut may_abandon = true;
    loop {
        let n = tokio::select! {
            result = pipe.read(&mut buffer) => result.unwrap_or(0),
            result = &mut abandoned, if may_abandon => {
                may_abandon = false;
                if result.is_err() {
                    continue;
                }
                0
            }
        };
        if n == 0 {
            let _ = socket.shutdown().await;
            return;
        }
        if let Err(e) = socket.write_all(&buffer[..n]).await {
            if !reported.swap(true, Ordering::SeqCst) {
                events.send(ConnectionEvent::SocketError(format!(
                    "write {}",
                    error_code(&e)
                )));
            }
            return;
        }
    }
}

/// Reports the end of the socket only once everything received before it was delivered, which is
/// when every `drained` sender is gone.
async fn receive(
    mut socket: OwnedReadHalf,
    mut pipe: WriteHalf<DuplexStream>,
    mut drained: Receiver<()>,
    events: Events,
    reported: Arc<AtomicBool>,
    disconnected: Arc<AtomicBool>,
) {
    let mut handshake = Handshake::new();
    let mut forward = Vec::new();
    let mut buffer = vec![0; READ_SIZE];
    let mut stopped = false;
    let end = loop {
        let n = match socket.read(&mut buffer).await {
            Ok(0) => break ConnectionEvent::SocketEnd,
            Ok(n) => n,
            Err(e) => {
                break ConnectionEvent::SocketError(format!("read {}", error_code(&e)));
            }
        };
        if stopped {
            continue;
        }
        forward.clear();
        for event in handshake.feed(&buffer[..n], &mut forward) {
            match event {
                HandshakeEvent::Disconnect { .. } => disconnected.store(true, Ordering::SeqCst),
                HandshakeEvent::Thrown(_) | HandshakeEvent::Fatal { .. } => stopped = true,
                HandshakeEvent::Header { .. } => {}
            }
            events.send(ConnectionEvent::Handshake(event));
        }
        if stopped {
            let _ = pipe.shutdown().await;
        } else {
            let _ = pipe.write_all(&forward).await;
        }
    };
    let _ = pipe.shutdown().await;
    drop(pipe);
    let _ = drained.recv().await;
    if !reported.swap(true, Ordering::SeqCst) {
        events.send(end);
    }
}

struct SessionHandler {
    events: Events,
    open_failures: UnboundedSender<String>,
    disconnected: Arc<AtomicBool>,
    _guard: Sender<()>,
}

impl client::Handler for SessionHandler {
    type Error = russh::Error;

    /// Upstream sets no host verifier.
    async fn check_server_key(
        &mut self,
        _: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, russh::Error> {
        Ok(true)
    }

    async fn channel_open_failure(
        &mut self,
        _: russh::ChannelId,
        _: ChannelOpenFailure,
        description: &str,
        _: &str,
        _: &mut client::Session,
    ) -> Result<(), russh::Error> {
        let _ = self.open_failures.send(description.to_owned());
        Ok(())
    }

    async fn disconnected(
        &mut self,
        reason: DisconnectReason<russh::Error>,
    ) -> Result<(), russh::Error> {
        match reason {
            DisconnectReason::ReceivedDisconnect(info) => {
                if !self.disconnected.load(Ordering::SeqCst) {
                    self.events
                        .send(ConnectionEvent::Handshake(HandshakeEvent::Disconnect {
                            reason: info.reason_code as u32,
                            description: info.message,
                        }));
                }
            }
            DisconnectReason::Error(e) => report(&e, &self.disconnected, &self.events),
        }
        Ok(())
    }
}

/// Protocol errors after encryption started. The socket's own end is reported by [`receive`].
fn report(error: &russh::Error, disconnected: &AtomicBool, events: &Events) {
    use russh::Error;
    if disconnected.load(Ordering::SeqCst) {
        return;
    }
    let event = match error {
        Error::IO(_) | Error::Disconnect | Error::HUP | Error::SendError | Error::RecvError => {
            return;
        }
        Error::NoAuthMethod => {
            events.send(ConnectionEvent::AuthFailed);
            return;
        }
        Error::WrongServerSig => HandshakeEvent::Fatal {
            message: "Handshake failed: signature verification failed".to_owned(),
            handshake: true,
        },
        Error::PacketAuth => HandshakeEvent::Thrown("Invalid MAC".to_owned()),
        Error::DecryptionError => {
            HandshakeEvent::Thrown("Unsupported state or unable to authenticate data".to_owned())
        }
        Error::PacketSize(_) => HandshakeEvent::Thrown("Bad packet length".to_owned()),
        other => HandshakeEvent::Thrown(format!("{other}")),
    };
    events.send(ConnectionEvent::Handshake(event));
}

/// The algorithms of [`super::handshake`]'s offers, plus the strict key exchange marker, which ssh2
/// 1.14.0 predates: it closes the prefix truncation attack (CVE-2023-48795) and changes nothing
/// else.
fn preferred() -> Preferred {
    let mut kex: Vec<kex::Name> = OFFERED_KEX
        .iter()
        .filter_map(|name| kex::Name::try_from(*name).ok())
        .collect();
    kex.push(kex::EXTENSION_SUPPORT_AS_CLIENT);
    kex.push(kex::EXTENSION_OPENSSH_STRICT_KEX_AS_CLIENT);
    Preferred {
        kex: kex.into(),
        key: OFFERED_HOST_KEYS
            .iter()
            .filter_map(|name| Algorithm::new(name).ok())
            .collect(),
        host_key_certificates: Cow::Borrowed(&[]),
        cipher: OFFERED_CIPHERS
            .iter()
            .filter_map(|name| cipher::Name::try_from(*name).ok())
            .collect(),
        mac: OFFERED_MACS
            .iter()
            .filter_map(|name| mac::Name::try_from(*name).ok())
            .collect(),
        compression: OFFERED_COMPRESSION
            .iter()
            .filter_map(|name| compression::Name::try_from(*name).ok())
            .collect(),
    }
}

fn config() -> client::Config {
    client::Config {
        client_id: SshId::Standard(Cow::Borrowed(IDENT)),
        limits: Limits::new(REKEY_BYTES, REKEY_BYTES, REKEY_TIME),
        window_size: WINDOW_SIZE,
        maximum_packet_size: PACKET_SIZE,
        preferred: preferred(),
        ..client::Config::default()
    }
}

async fn login(
    pipe: DuplexStream,
    login: Login,
    handler: SessionHandler,
) -> Result<Option<Handle<SessionHandler>>, russh::Error> {
    let mut handle = client::connect_stream(Arc::new(config()), pipe, handler).await?;
    let mut result = handle.authenticate_none(login.username.as_str()).await?;
    if let Some(password) = login.password
        && may_continue(&result)?
    {
        result = handle
            .authenticate_password(login.username.as_str(), password)
            .await?;
    }
    if let Some(key) = login.key
        && may_continue(&result)?
    {
        let key = PrivateKey::from_openssh(&key).map_err(russh::keys::Error::from)?;
        result = handle
            .authenticate_publickey(
                login.username.as_str(),
                PrivateKeyWithHashAlg::new(Arc::new(key), None),
            )
            .await?;
    }
    may_continue(&result)?;
    Ok(result.success().then_some(handle))
}

/// Whether the attempt failed. A failure with no method to continue with is also what `russh` reports
/// when the session ends during the attempt; the handler learns which it was.
fn may_continue(result: &AuthResult) -> Result<bool, russh::Error> {
    match result {
        AuthResult::Success => Ok(false),
        AuthResult::Failure {
            remaining_methods, ..
        } if remaining_methods.is_empty() => Err(russh::Error::Disconnect),
        AuthResult::Failure { .. } => Ok(true),
    }
}

async fn session(
    pipe: DuplexStream,
    credentials: Login,
    mut commands: UnboundedReceiver<Command>,
    abandon: oneshot::Sender<()>,
    events: Events,
    guard: Sender<()>,
    disconnected: Arc<AtomicBool>,
) {
    let (open_failures, descriptions) = mpsc::unbounded_channel();
    let handler = SessionHandler {
        events: events.clone(),
        open_failures,
        disconnected: disconnected.clone(),
        _guard: guard.clone(),
    };
    let logging_in = login(pipe, credentials, handler);
    tokio::pin!(logging_in);
    let handle = loop {
        tokio::select! {
            result = &mut logging_in => match result {
                Ok(Some(handle)) => break handle,
                Ok(None) => {
                    events.send(ConnectionEvent::AuthFailed);
                    return;
                }
                Err(e) => {
                    report(&e, &disconnected, &events);
                    return;
                }
            },
            command = commands.recv() => {
                if matches!(command, None | Some(Command::End)) {
                    let _ = abandon.send(());
                    return;
                }
            }
        }
    };
    events.send(ConnectionEvent::Ready);
    let handle = Arc::new(handle);
    let descriptions = Arc::new(Mutex::new(descriptions));
    let mut channels: HashMap<u32, UnboundedSender<Option<Vec<u8>>>> = HashMap::new();
    let mut tasks = JoinSet::new();
    let weak_guard = guard.downgrade();
    drop(guard);
    while let Some(command) = commands.recv().await {
        match command {
            Command::Sftp(id) => {
                let (sender, receiver) = mpsc::unbounded_channel();
                channels.insert(id, sender);
                let Some(guard) = weak_guard.upgrade() else {
                    continue;
                };
                let channel_events = events.clone();
                tasks.spawn(channel(
                    handle.clone(),
                    descriptions.clone(),
                    receiver,
                    move |event| channel_events.send(ConnectionEvent::Channel(id, event)),
                    guard,
                ));
            }
            Command::Send(id, data) => {
                if let Some(channel) = channels.get(&id) {
                    let _ = channel.send(Some(data));
                }
            }
            Command::Close(id) => {
                if let Some(channel) = channels.get(&id) {
                    let _ = channel.send(None);
                }
            }
            Command::End => break,
        }
    }
    let _ = handle.disconnect(Disconnect::ByApplication, "", "").await;
    while tasks.join_next().await.is_some() {}
}

/// One `sftp` channel: `data` is what to write to it, `None` closes it.
async fn channel(
    handle: Arc<Handle<SessionHandler>>,
    descriptions: Arc<Mutex<UnboundedReceiver<String>>>,
    mut data: UnboundedReceiver<Option<Vec<u8>>>,
    event: impl Fn(ChannelEvent),
    _guard: Sender<()>,
) {
    let channel: Channel<client::Msg> = match handle.channel_open_session().await {
        Ok(channel) => channel,
        Err(russh::Error::ChannelOpenFailure(_)) => {
            let description = descriptions.lock().await.recv().await.unwrap_or_default();
            event(ChannelEvent::OpenFailed(description));
            return;
        }
        Err(_) => return,
    };
    event(ChannelEvent::Opened);
    if channel.request_subsystem(true, "sftp").await.is_err() {
        return;
    }
    let (mut reader, writer) = channel.split();
    let write = async {
        while let Some(chunk) = data.recv().await {
            let result = match chunk {
                Some(chunk) => writer.data_bytes(chunk).await,
                None => writer.close().await,
            };
            if result.is_err() {
                break;
            }
        }
    };
    let read = async {
        while let Some(message) = reader.wait().await {
            match message {
                ChannelMsg::Success => event(ChannelEvent::SubsystemStarted),
                ChannelMsg::Failure => event(ChannelEvent::SubsystemFailed),
                ChannelMsg::Data { data } => event(ChannelEvent::Data(data.to_vec())),
                ChannelMsg::Eof => event(ChannelEvent::Eof),
                ChannelMsg::ExitStatus { exit_status } => event(ChannelEvent::Exit(exit_status)),
                ChannelMsg::ExitSignal { signal_name, .. } => {
                    event(ChannelEvent::Signal(signal(&signal_name)));
                }
                ChannelMsg::Close => {
                    event(ChannelEvent::Closed);
                    break;
                }
                _ => {}
            }
        }
    };
    tokio::pin!(write, read);
    let mut writing = true;
    loop {
        tokio::select! {
            () = &mut read => break,
            () = &mut write, if writing => writing = false,
        }
    }
}

fn signal(signal: &Sig) -> String {
    let name = match signal {
        Sig::ABRT => "ABRT",
        Sig::ALRM => "ALRM",
        Sig::FPE => "FPE",
        Sig::HUP => "HUP",
        Sig::ILL => "ILL",
        Sig::INT => "INT",
        Sig::KILL => "KILL",
        Sig::PIPE => "PIPE",
        Sig::QUIT => "QUIT",
        Sig::SEGV => "SEGV",
        Sig::TERM => "TERM",
        Sig::USR1 => "USR1",
        Sig::Custom(name) => name,
    };
    format!("SIG{name}")
}
