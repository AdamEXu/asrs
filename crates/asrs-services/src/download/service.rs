//! The log downloader on its own thread. The download window sends commands through
//! [`DownloadService`] and shows what [`DownloadEvents`] yields; every request that needs the user
//! carries a token its answer names.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::thread;
use std::time::Duration;

use tokio::runtime::Builder;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::time::{Instant, sleep_until};

use super::client::{ClientEvent, Clients, Login, NetworkEvent, Target, lookup};
use super::downloader::{
    ClientId, ConnectOptions, DownloadHost, Downloader, OpenLogBox, SaveLogDialog, SftpId, Thrown,
    TimerId, Token, WindowMessage,
};
use crate::dialog::OpenDialog;
use crate::fs::{Fs, RealFs};
use crate::path::PathStyle;

#[derive(Clone, Debug, PartialEq)]
pub enum DownloadEvent {
    /// A message to the download window.
    Window(WindowMessage),
    /// Answered by [`DownloadService::save_location`].
    ChooseFolder {
        token: Token,
        dialog: OpenDialog,
    },
    /// Answered by [`DownloadService::save_location`].
    ChooseFile {
        token: Token,
        dialog: SaveLogDialog,
    },
    /// Answered by [`DownloadService::open_log`].
    AskOpenLog {
        token: Token,
        options: OpenLogBox,
    },
    DestroyWindow,
    /// Focus the hub window used last.
    FocusHub,
    /// The `open-files` message to the hub window used last, with `merge: false`.
    OpenFiles(Vec<String>),
    /// An exception no listener catches.
    Uncaught(String),
}

/// Where and how fast the service connects. Only tests differ from [`DownloadConfig::new`].
pub struct DownloadConfig {
    pub style: PathStyle,
    pub target: Box<dyn Fn(&ConnectOptions) -> Target + Send>,
    /// Milliseconds a `setTimeout` or `setInterval` of main really takes.
    pub delay: Box<dyn Fn(f64) -> f64 + Send>,
}

impl DownloadConfig {
    pub fn new(style: PathStyle) -> DownloadConfig {
        DownloadConfig {
            style,
            target: Box::new(|options| Target {
                host: options.host.clone(),
                port: options.port,
                ready_timeout_ms: options.ready_timeout_ms,
                login: Login {
                    username: options.username.to_owned(),
                    password: Some(options.password.to_owned()),
                    key: None,
                },
            }),
            delay: Box::new(|ms| ms),
        }
    }
}

enum Command {
    WindowOpened,
    WindowClosed,
    Start { address: String, path: String },
    Close,
    Save(Vec<String>),
    SaveLocation(Token, Option<String>),
    OpenLog(Token, usize),
    Stop,
}

pub struct DownloadService {
    commands: UnboundedSender<Command>,
}

/// Awaitable on any executor.
pub struct DownloadEvents(UnboundedReceiver<DownloadEvent>);

impl DownloadEvents {
    /// `None` once the service is dropped.
    pub async fn next(&mut self) -> Option<DownloadEvent> {
        self.0.recv().await
    }

    pub fn try_next(&mut self) -> Option<DownloadEvent> {
        self.0.try_recv().ok()
    }
}

impl DownloadService {
    /// The thread ends when the service is dropped.
    pub fn spawn(config: DownloadConfig) -> io::Result<(DownloadService, DownloadEvents)> {
        let runtime = Builder::new_current_thread().enable_all().build()?;
        let (commands, receiver) = mpsc::unbounded_channel();
        let (events, event_receiver) = mpsc::unbounded_channel();
        thread::Builder::new()
            .name("download".to_owned())
            .spawn(move || runtime.block_on(run(config, receiver, events)))?;
        Ok((DownloadService { commands }, DownloadEvents(event_receiver)))
    }

    fn command(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// `openDownload` made a new window.
    pub fn window_opened(&self) {
        self.command(Command::WindowOpened);
    }

    /// The window was closed from its frame.
    pub fn window_closed(&self) {
        self.command(Command::WindowClosed);
    }

    /// The window's `start` message, sent every time it receives preferences. `address` went through
    /// [`super::window::normalize_address`].
    pub fn start(&self, address: &str, path: &str) {
        self.command(Command::Start {
            address: address.to_owned(),
            path: path.to_owned(),
        });
    }

    /// The window's `close` message (its exit button).
    pub fn close(&self) {
        self.command(Command::Close);
    }

    /// The window's `save` message: the selected names, in selection order.
    pub fn save(&self, files: Vec<String>) {
        self.command(Command::Save(files));
    }

    /// `None` when the dialog was cancelled.
    pub fn save_location(&self, token: Token, path: Option<String>) {
        self.command(Command::SaveLocation(token, path));
    }

    /// The index of the pressed button.
    pub fn open_log(&self, token: Token, response: usize) {
        self.command(Command::OpenLog(token, response));
    }

    /// `downloadStop`, which upstream also runs when a licenses or source list help window closes.
    pub fn stop(&self) {
        self.command(Command::Stop);
    }
}

#[derive(Clone, Copy)]
enum Timer {
    Main(TimerId, Option<Duration>),
    ReadyTimeout(ClientId),
}

struct Host {
    config: DownloadConfig,
    clients: Clients,
    events: UnboundedSender<DownloadEvent>,
    timers: BTreeMap<(Instant, u64), Timer>,
    ready_timeouts: HashMap<ClientId, (Instant, u64)>,
    main_timers: HashMap<TimerId, (Instant, u64)>,
    next_timer: TimerId,
    sequence: u64,
}

impl Host {
    fn event(&self, event: DownloadEvent) {
        let _ = self.events.send(event);
    }

    fn at(&mut self, after: Duration, timer: Timer) -> (Instant, u64) {
        self.sequence += 1;
        let key = (Instant::now() + after, self.sequence);
        self.timers.insert(key, timer);
        key
    }

    fn set_main(&mut self, ms: f64, repeats: bool) -> TimerId {
        self.next_timer += 1;
        let id = self.next_timer;
        let period = milliseconds((self.config.delay)(ms));
        let key = self.at(period, Timer::Main(id, repeats.then_some(period)));
        self.main_timers.insert(id, key);
        id
    }

    fn clear_main(&mut self, id: TimerId) {
        if let Some(key) = self.main_timers.remove(&id) {
            self.timers.remove(&key);
        }
    }

    fn clear_ready_timeout(&mut self, client: ClientId) {
        if let Some(key) = self.ready_timeouts.remove(&client) {
            self.timers.remove(&key);
        }
    }

    fn dialog(&mut self, event: impl FnOnce(Token) -> DownloadEvent) -> Token {
        let token = self.clients.token();
        self.event(event(token));
        token
    }
}

fn milliseconds(ms: f64) -> Duration {
    Duration::from_secs_f64(ms / 1000.0)
}

impl DownloadHost for Host {
    fn connect(&mut self, options: &ConnectOptions) -> ClientId {
        self.clients.connect((self.config.target)(options))
    }

    fn end(&mut self, client: ClientId) {
        self.clients.end(client);
    }

    fn sftp(&mut self, client: ClientId) -> Result<Token, String> {
        self.clients.sftp(client)
    }

    fn readdir(&mut self, sftp: SftpId, path: &str) -> Result<Token, String> {
        Ok(self.clients.readdir(sftp, path))
    }

    fn fast_get(&mut self, sftp: SftpId, remote: &str, local: &str) -> Result<Token, String> {
        Ok(self.clients.fast_get(sftp, remote, local))
    }

    fn set_timeout(&mut self, ms: f64) -> TimerId {
        self.set_main(ms, false)
    }

    fn clear_timeout(&mut self, id: TimerId) {
        self.clear_main(id);
    }

    fn set_interval(&mut self, ms: f64) -> TimerId {
        self.set_main(ms, true)
    }

    fn clear_interval(&mut self, id: TimerId) {
        self.clear_main(id);
    }

    fn send(&mut self, message: WindowMessage) {
        self.event(DownloadEvent::Window(message));
    }

    fn show_open_dialog(&mut self, dialog: &OpenDialog) -> Token {
        let dialog = dialog.clone();
        self.dialog(|token| DownloadEvent::ChooseFolder { token, dialog })
    }

    fn show_save_dialog(&mut self, dialog: &SaveLogDialog) -> Token {
        let dialog = dialog.clone();
        self.dialog(|token| DownloadEvent::ChooseFile { token, dialog })
    }

    fn show_message_box(&mut self, options: &OpenLogBox) -> Token {
        let options = options.clone();
        self.dialog(|token| DownloadEvent::AskOpenLog { token, options })
    }

    fn destroy_window(&mut self) {
        self.event(DownloadEvent::DestroyWindow);
    }

    fn focus_hub(&mut self) {
        self.event(DownloadEvent::FocusHub);
    }

    fn open_files(&mut self, files: &[String]) {
        self.event(DownloadEvent::OpenFiles(files.to_vec()));
    }

    fn exists(&mut self, path: &str) -> bool {
        RealFs.exists(path)
    }

    fn file_size(&mut self, path: &str) -> Result<f64, String> {
        std::fs::metadata(path)
            .map(|metadata| metadata.len() as f64)
            .map_err(|e| format!("{e}"))
    }

    fn mkdir_all(&mut self, path: &str) -> Thrown {
        RealFs.mkdir_all(path).map_err(|e| e.message)
    }
}

async fn run(
    config: DownloadConfig,
    mut commands: UnboundedReceiver<Command>,
    events: UnboundedSender<DownloadEvent>,
) {
    let (network, mut network_events) = mpsc::unbounded_channel();
    let style = config.style;
    let sender = network.clone();
    let mut host = Host {
        config,
        clients: Clients::new(move |client, event| {
            let _ = sender.send((client, event));
        }),
        events,
        timers: BTreeMap::new(),
        ready_timeouts: HashMap::new(),
        main_timers: HashMap::new(),
        next_timer: 0,
        sequence: 0,
    };
    let mut downloader = Downloader::new(style);
    loop {
        while let Some(event) = host.clients.next() {
            client_event(event, &mut downloader, &mut host, &network);
        }
        let due = host.timers.first_key_value().map(|(key, _)| key.0);
        tokio::select! {
            command = commands.recv() => match command {
                Some(command) => handle(command, &mut downloader, &mut host),
                None => return,
            },
            Some((client, event)) = network_events.recv() => host.clients.received(client, event),
            () = sleep_until(due.unwrap_or_else(Instant::now)), if due.is_some() => {
                fire(&mut downloader, &mut host);
            }
        }
    }
}

fn handle(command: Command, downloader: &mut Downloader, host: &mut Host) {
    match command {
        Command::WindowOpened => downloader.window_opened(),
        Command::WindowClosed => downloader.window_closed(host),
        Command::Start { address, path } => downloader.start_message(&address, &path, host),
        Command::Close => downloader.close_message(host),
        Command::Save(files) => downloader.save_message(files, host),
        Command::SaveLocation(token, path) => downloader.on_save_location(token, path, host),
        Command::OpenLog(token, response) => downloader.on_open_log(token, response, host),
        Command::Stop => downloader.stop(host),
    }
}

fn fire(downloader: &mut Downloader, host: &mut Host) {
    let Some((key, timer)) = host.timers.pop_first() else {
        return;
    };
    match timer {
        Timer::Main(id, period) => {
            match period {
                Some(period) => {
                    host.sequence += 1;
                    let next = ((key.0 + period).max(Instant::now()), host.sequence);
                    host.timers.insert(next, timer);
                    host.main_timers.insert(id, next);
                }
                None => {
                    host.main_timers.remove(&id);
                }
            }
            if let Err(message) = downloader.on_timer(id, host) {
                host.event(DownloadEvent::Uncaught(message));
            }
        }
        Timer::ReadyTimeout(client) => {
            host.ready_timeouts.remove(&client);
            host.clients.timed_out(client);
        }
    }
}

fn client_event(
    event: ClientEvent,
    downloader: &mut Downloader,
    host: &mut Host,
    network: &UnboundedSender<(ClientId, NetworkEvent)>,
) {
    let thrown = match event {
        ClientEvent::Error(_, message) => {
            downloader.on_error(&message, host);
            None
        }
        ClientEvent::Ready(client) => downloader.on_ready(host).err().map(|e| (client, e)),
        ClientEvent::Close(_) => None,
        ClientEvent::Sftp(client, token, result) => downloader
            .on_sftp(token, result, host)
            .err()
            .map(|e| (client, e)),
        ClientEvent::Readdir(_, token, result) => {
            downloader.on_readdir(token, result, host);
            None
        }
        ClientEvent::Step {
            token,
            transferred,
            total,
        } => {
            downloader.on_step(token, transferred, total, host);
            None
        }
        ClientEvent::Got(_, token, result) => {
            downloader.on_got(token, result, host);
            None
        }
        ClientEvent::StartTimeout(client, ms) => {
            let key = host.at(milliseconds(ms), Timer::ReadyTimeout(client));
            host.ready_timeouts.insert(client, key);
            None
        }
        ClientEvent::ClearTimeout(client) => {
            host.clear_ready_timeout(client);
            None
        }
        ClientEvent::Resolve(client, name) => {
            let network = network.clone();
            tokio::spawn(async move {
                let _ = network.send((client, lookup(&name).await));
            });
            None
        }
    };
    if let Some((client, message)) = thrown {
        host.clients.threw(client, &message);
    }
}
