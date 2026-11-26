//! Runs `assetDownloader.ts` (`asrs_services::asset_downloader`) for the app: its loop on a
//! thread, its requests on threads of their own, and its `updateCallback` in the main process.

mod confined;
mod net;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use asrs_js::time::{Clock, NamedZone, Rng, SystemClock, V8MathRandom};
use asrs_model::build_constants;
#[cfg(feature = "parity")]
use asrs_services::asset_downloader::Timer;
use asrs_services::asset_downloader::{
    AssetDownloader, Distribution, DownloadHost, FetchId, FetchOutcome, FetchRequest, HttpOutcome,
    HttpRequest, INITIAL_STATUS, Rejected, RequestId,
};
use asrs_services::asset_loader::AssetFolders;
use asrs_ui::gpui::{App, Global};

use crate::main_process::MainProcess;

use confined::Confined;
use net::Net;

/// Not in a `parity` build, whose scenarios and tests start the loop themselves.
pub const ENABLED: bool = !cfg!(feature = "parity");

/// Keeps the app off the network, for the smoke test of the release packages.
pub const OFF_VARIABLE: &str = "ASRS_NO_ASSET_DOWNLOADS";

/// Whether the app downloads assets when it starts.
pub fn enabled() -> bool {
    ENABLED && std::env::var_os(OFF_VARIABLE).is_none()
}

const GITHUB_API: &str = "https://api.github.com";

/// What the loop runs against.
pub struct Config {
    pub folders: AssetFolders,
    pub distribution: Distribution,
    /// Stands in for `https://api.github.com`.
    pub api: String,
    /// `checkDiskSpace(path).free`.
    pub free_space: fn(&str) -> Option<f64>,
}

impl Config {
    pub fn new(folders: AssetFolders) -> Config {
        Config {
            folders,
            distribution: match build_constants::DISTRIBUTION {
                build_constants::Distribution::WpiLib => Distribution::WpiLib,
                _ => Distribution::Frc6328,
            },
            api: GITHUB_API.to_owned(),
            free_space,
        }
    }
}

/// `check-disk-space` 3: `df`'s available space of the first folder above `path` that exists, or
/// the free space of its drive on Windows.
pub fn free_space(path: &str) -> Option<f64> {
    let path = Path::new(path);
    if cfg!(windows) {
        let drive = path.as_os_str().as_encoded_bytes().get(1) == Some(&b':');
        let root = path.ancestors().last().filter(|_| drive)?;
        return fs4::free_space(root).ok().map(|bytes| bytes as f64);
    }
    let found = path.ancestors().find(|path| path.exists())?;
    fs4::available_space(found).ok().map(|bytes| bytes as f64)
}

enum Event {
    Fetched(FetchId, FetchOutcome),
    Answered(RequestId, HttpOutcome),
    /// Moves the timers' clock forward, in milliseconds.
    #[cfg(feature = "parity")]
    Skip(f64),
    Stop,
}

/// The running loop.
struct AssetDownloads {
    status: Arc<Mutex<String>>,
    #[cfg(feature = "parity")]
    timers: Arc<Mutex<Vec<Timer>>>,
    stopped: Arc<AtomicBool>,
    extracting: Arc<Mutex<()>>,
    events: Sender<Event>,
    /// `updateCallback` calls, each with where its result goes.
    updates: async_channel::Receiver<Sender<bool>>,
    net: Net,
    worker: Option<JoinHandle<()>>,
}

impl Global for AssetDownloads {}

/// `getAssetDownloadStatus()`.
pub fn status(cx: &App) -> String {
    cx.try_global::<AssetDownloads>().map_or_else(
        || INITIAL_STATUS.to_owned(),
        |downloads| locked(&downloads.status).clone(),
    )
}

fn locked<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `startAssetDownloadLoop(updateCallback)`.
pub fn start(config: Config, cx: &mut App) {
    if cx.has_global::<AssetDownloads>() {
        return;
    }
    let (events, receive) = channel();
    let (updates, updated) = async_channel::unbounded::<Sender<bool>>();
    let status = Arc::new(Mutex::new(INITIAL_STATUS.to_owned()));
    #[cfg(feature = "parity")]
    let timers = Arc::<Mutex<Vec<Timer>>>::default();
    let stopped = Arc::new(AtomicBool::new(false));
    let extracting = Arc::new(Mutex::new(()));
    let net = Net::new(config.api.starts_with("https:"));
    let worker = {
        let status = status.clone();
        let host = Host {
            api: config.api,
            free_space: config.free_space,
            net: net.clone(),
            events: events.clone(),
            updates,
            random: V8MathRandom::from_entropy(),
        };
        let root = config.folders.auto.to_string_lossy();
        let worker = Worker {
            downloader: AssetDownloader::new(&config.folders, config.distribution),
            fs: Confined::new(&root, stopped.clone()),
            host,
            started: Instant::now(),
            skipped_ms: 0.0,
            stopped: stopped.clone(),
            extracting: extracting.clone(),
            status,
            #[cfg(feature = "parity")]
            timers: timers.clone(),
        };
        thread::Builder::new()
            .name("asset-download".to_owned())
            .spawn(move || worker.run(&receive))
    };
    let worker = worker
        .inspect_err(|e| eprintln!("asrs: starting the asset download loop: {e}"))
        .ok();
    cx.set_global(AssetDownloads {
        status,
        #[cfg(feature = "parity")]
        timers,
        stopped,
        extracting,
        events,
        updates: updated.clone(),
        net,
        worker,
    });
    cx.spawn(async move |cx| {
        while let Ok(done) = updated.recv().await {
            let loaded = cx.update(MainProcess::assets_updated);
            let _ = done.send(loaded);
        }
    })
    .detach();
}

/// Runs `load` unless an asset is being extracted, whose half-written folder would be reported as
/// one that failed to load.
pub fn between_extractions(cx: &mut App, load: impl FnOnce(&mut App)) {
    let extracting = cx
        .try_global::<AssetDownloads>()
        .map(|downloads| downloads.extracting.clone());
    let Some(extracting) = extracting else {
        return load(cx);
    };
    let _guard = match extracting.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(TryLockError::WouldBlock) => return,
    };
    load(cx);
}

/// Ends the loop and its requests without waiting for them: no file changes from here on, apart
/// from one already being written. The thread is returned for tests.
pub fn stop(cx: &mut App) -> Option<JoinHandle<()>> {
    if !cx.has_global::<AssetDownloads>() {
        return None;
    }
    let downloads = cx.global_mut::<AssetDownloads>();
    downloads.stopped.store(true, Ordering::SeqCst);
    let _ = downloads.events.send(Event::Stop);
    downloads.net.shutdown();
    downloads.updates.close();
    while downloads.updates.try_recv().is_ok() {}
    downloads.worker.take()
}

/// The loop's pending timers, for tests.
#[cfg(feature = "parity")]
pub fn timers(cx: &App) -> Vec<Timer> {
    cx.try_global::<AssetDownloads>()
        .map(|downloads| locked(&downloads.timers).clone())
        .unwrap_or_default()
}

/// Moves the loop's timers forward, for tests.
#[cfg(feature = "parity")]
pub fn skip(ms: f64, cx: &App) {
    if let Some(downloads) = cx.try_global::<AssetDownloads>() {
        let _ = downloads.events.send(Event::Skip(ms));
    }
}

struct Worker {
    downloader: AssetDownloader,
    fs: Confined,
    host: Host,
    started: Instant,
    skipped_ms: f64,
    stopped: Arc<AtomicBool>,
    extracting: Arc<Mutex<()>>,
    status: Arc<Mutex<String>>,
    #[cfg(feature = "parity")]
    timers: Arc<Mutex<Vec<Timer>>>,
}

impl Worker {
    fn now(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0 + self.skipped_ms
    }

    fn run(mut self, events: &Receiver<Event>) {
        let now = self.now();
        let zone = NamedZone::system();
        self.downloader.start(now, &self.fs, &mut self.host, &zone);
        loop {
            self.publish();
            let event = match self.wait(events) {
                Some(Event::Stop) => return,
                event => event,
            };
            if self.stopped.load(Ordering::SeqCst) {
                return;
            }
            #[cfg(feature = "parity")]
            if let Some(Event::Skip(ms)) = event {
                self.skipped_ms += ms;
            }
            let now = self.now();
            let zone = NamedZone::system();
            let extracting = self.extracting.clone();
            let _extracting = locked(&extracting);
            let Worker {
                downloader,
                fs,
                host,
                ..
            } = &mut self;
            downloader.run_due(now, fs, host, &zone);
            match event {
                Some(Event::Fetched(id, outcome)) => {
                    downloader.fetched(id, outcome, fs, host, &zone);
                }
                Some(Event::Answered(id, outcome)) => downloader.answered(id, outcome, fs, host),
                _ => {}
            }
        }
    }

    /// The next event, or `None` once a timer is due.
    fn wait(&self, events: &Receiver<Event>) -> Option<Event> {
        let timers = self.downloader.timers();
        let due = timers.iter().map(|timer| timer.due_ms).reduce(f64::min);
        let Some(due) = due else {
            return Some(events.recv().unwrap_or(Event::Stop));
        };
        let wait = (due - self.now()).max(0.0);
        match events.recv_timeout(Duration::from_secs_f64(wait / 1000.0)) {
            Ok(event) => Some(event),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Event::Stop),
        }
    }

    fn publish(&self) {
        let mut status = locked(&self.status);
        if *status != self.downloader.status() {
            self.downloader.status().clone_into(&mut status);
        }
        #[cfg(feature = "parity")]
        {
            *locked(&self.timers) = self.downloader.timers();
        }
    }
}

struct Host {
    api: String,
    free_space: fn(&str) -> Option<f64>,
    net: Net,
    events: Sender<Event>,
    updates: async_channel::Sender<Sender<bool>>,
    random: V8MathRandom,
}

impl Host {
    /// Runs a request on its own thread; false when it cannot have one.
    fn spawn(&self, request: impl FnOnce(&Net) -> Event + Send + 'static) -> bool {
        let net = self.net.clone();
        let events = self.events.clone();
        thread::Builder::new()
            .name("asset-request".to_owned())
            .spawn(move || {
                let _ = events.send(request(&net));
            })
            .is_ok()
    }
}

impl DownloadHost for Host {
    fn free_space(&mut self, path: &str) -> Result<f64, Rejected> {
        (self.free_space)(path).ok_or(Rejected)
    }

    fn fetch(&mut self, id: FetchId, request: &FetchRequest) -> FetchOutcome {
        let mut request = request.clone();
        request.url = request.url.replacen(GITHUB_API, &self.api, 1);
        if self.spawn(move |net| Event::Fetched(id, net.fetch(&request))) {
            FetchOutcome::Hang
        } else {
            FetchOutcome::NetworkError
        }
    }

    fn request(&mut self, id: RequestId, request: &HttpRequest) -> HttpOutcome {
        let request = request.clone();
        if self.spawn(move |net| Event::Answered(id, net.request(&request))) {
            HttpOutcome::Hang
        } else {
            HttpOutcome::Reset
        }
    }

    fn now(&mut self, _: f64) -> f64 {
        SystemClock.now()
    }

    fn random(&mut self) -> f64 {
        self.random.next_f64()
    }

    fn assets_updated(&mut self) -> Result<(), Rejected> {
        let (done, loaded) = channel();
        self.updates.try_send(done).map_err(|_| Rejected)?;
        loaded
            .recv()
            .ok()
            .filter(|&ok| ok)
            .map(drop)
            .ok_or(Rejected)
    }
}
