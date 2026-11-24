//! Runs `VideoProcessor.ts` (`asrs_services::video`) on the app: `select-video` from a hub window
//! starts a flow whose child processes, HTTP, dialogs and menu run here, and everything it sends
//! goes back to that window as `video-data`.

mod children;
mod net;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use asrs_js::ops;
use asrs_js::string::{JsError, JsString};
use asrs_js::time::{Rng, V8MathRandom};
use asrs_js::value::JsValue;
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage};
use asrs_ocr::{OcrError, TimerError, TimerReader};
use asrs_services::lifecycle::AppPaths;
use asrs_services::video::{
    Handle, LogArg, LogLevel, MessageBox, OpenDialog, VideoConfig, VideoEvent, VideoHost,
    VideoProcessor,
};
use asrs_services::window_manager::Platform;
use asrs_ui::gpui::{App, Global};
use sha2::{Digest, Sha256};

use crate::main_process::{MainProcess, process_arch, process_platform};
use crate::menus::bridge;
use crate::menus::dispatch;
use crate::windows::{
    self, FileFilter, MessageBoxOptions, MessageBoxType, OpenDialogOptions, WindowId, Windows,
};

use children::Children;

/// The video processor, its children, and the window each outstanding request belongs to.
struct Videos {
    processor: Option<VideoProcessor>,
    io: Io,
}

impl Global for Videos {}

struct Io {
    next: Handle,
    owners: HashMap<Handle, WindowId>,
    children: Children,
    random: V8MathRandom,
    events: async_channel::Sender<VideoEvent>,
    timers: Option<TimerReader>,
    uncaught: async_channel::Sender<TimerError>,
}

/// `VideoProcessor`'s module: it lives as long as the app.
pub fn init(paths: &AppPaths, cx: &mut App) {
    let (events, receive) = async_channel::unbounded();
    let (uncaught, thrown) = async_channel::unbounded();
    cx.set_global(Videos {
        processor: Some(VideoProcessor::new(config(paths))),
        io: Io {
            next: 1,
            owners: HashMap::new(),
            children: Children::default(),
            random: V8MathRandom::from_entropy(),
            events,
            timers: None,
            uncaught,
        },
    });
    cx.spawn(async move |cx| {
        while let Ok(event) = receive.recv().await {
            cx.update(|cx| deliver(event, cx));
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        let mut workers_failed = false;
        while let Ok(error) = thrown.recv().await {
            let error = match error {
                // main.ts ignores unhandled rejections.
                TimerError::Read(_) => continue,
                TimerError::Job(error) => error,
                // Upstream's failed workers throw as they start, not for each job.
                TimerError::NoWorker(_) if workers_failed => continue,
                TimerError::NoWorker(error) => {
                    workers_failed = true;
                    error
                }
            };
            // `showErrorBox` blocks, so the next error waits for this one's box to close.
            let (closed, wait) = async_channel::bounded::<()>(1);
            let error = format!("Error: {error}");
            cx.update(|cx| dispatch::show_uncaught(&error, cx, move |_| drop(closed)));
            let _ = wait.recv().await;
        }
    })
    .detach();
}

/// `select-video` from a hub window.
pub fn hub_message(window: WindowId, message: &NamedMessage<HubToMain>, cx: &mut App) {
    if message.name != HubToMain::SelectVideo || !cx.has_global::<Videos>() {
        return;
    }
    let member = |key| ops::get(&message.data, key);
    let (Ok(uuid), Ok(source), Ok(match_info), Ok(menu)) = (
        member("uuid"),
        member("source"),
        member("matchInfo"),
        member("menuCoordinates"),
    ) else {
        eprintln!("asrs: select-video: {:?}", message.data);
        return;
    };
    let uuid = ops::to_string(&uuid);
    // Upstream's workers start with the app. They are ready well before a conversion's first
    // sample, 2 s of frames in.
    let io = &mut cx.global_mut::<Videos>().io;
    io.timers.get_or_insert_with(TimerReader::new);
    with_processor(window, cx, |processor, host| {
        processor.prepare(host, &uuid, &source, &match_info, &menu);
    });
}

/// `VideoProcessor.cleanup()`, on quit.
pub fn cleanup(cx: &mut App) {
    if cx.has_global::<Videos>() {
        with_processor(0, cx, |processor, host| processor.cleanup(host));
        cx.global_mut::<Videos>().io.timers = None;
    }
}

fn config(paths: &AppPaths) -> VideoConfig {
    VideoConfig {
        platform: process_platform().to_owned(),
        arch: process_arch().to_owned(),
        user_data: paths.user_data.clone(),
        temp: cache_parent(&paths.temp),
        yt_dlp_binary: yt_dlp_binary().to_string_lossy().into_owned(),
        window_icon: None,
    }
}

/// Where `VIDEO_CACHE` goes. Quitting deletes the whole cache, so outside `parity` builds it must
/// not be the one a running AdvantageScope writes to.
fn cache_parent(temp: &str) -> String {
    if cfg!(feature = "parity") {
        return temp.to_owned();
    }
    let parent = Path::new(temp).join(crate::IDENTITY.name.to_lowercase());
    parent.to_string_lossy().into_owned()
}

/// The yt-dlp executable beside the app, or else the first on the `PATH`.
fn yt_dlp_binary() -> PathBuf {
    let name = match process_platform() {
        "win32" => "yt-dlp.exe",
        "darwin" => "yt-dlp_macos",
        _ => "yt-dlp",
    };
    let exe = std::env::current_exe().unwrap_or_default();
    let bundled = exe.parent().unwrap_or(Path::new(".")).join(name);
    if bundled.is_file() {
        return bundled;
    }
    let installed = if cfg!(windows) { name } else { "yt-dlp" };
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join(installed))
        .find(|file| file.is_file())
        .unwrap_or(bundled)
}

fn deliver(event: VideoEvent, cx: &mut App) {
    if !cx.has_global::<Videos>() {
        return;
    }
    let handle = handle_of(&event);
    let Some(window) = cx.global::<Videos>().io.owners.get(&handle).copied() else {
        return;
    };
    let closed = matches!(event, VideoEvent::Close { .. });
    if matches!(event, VideoEvent::TimerText { .. }) {
        cx.global_mut::<Videos>().io.owners.remove(&handle);
    }
    with_processor(window, cx, |processor, host| processor.handle(host, event));
    if closed && cx.has_global::<Videos>() {
        let io = &mut cx.global_mut::<Videos>().io;
        io.children.closed(handle);
        io.owners.remove(&handle);
    }
}

fn handle_of(event: &VideoEvent) -> Handle {
    match event {
        VideoEvent::Stdout { process, .. }
        | VideoEvent::Stderr { process, .. }
        | VideoEvent::Exit { process, .. }
        | VideoEvent::Close { process, .. }
        | VideoEvent::SpawnError { process, .. } => *process,
        VideoEvent::MessageBoxClosed { dialog, .. }
        | VideoEvent::OpenDialogClosed { dialog, .. } => *dialog,
        VideoEvent::DownloadFinished { download, .. } => *download,
        VideoEvent::FetchFinished { fetch, .. } => *fetch,
        VideoEvent::MenuClick { menu, .. } | VideoEvent::MenuWillClose { menu } => *menu,
        VideoEvent::TimerFired { timer } => *timer,
        VideoEvent::TimerText { job, .. } => *job,
    }
}

fn with_processor(
    window: WindowId,
    cx: &mut App,
    f: impl FnOnce(&mut VideoProcessor, &mut Host<'_>),
) {
    let Some(mut processor) = cx.global_mut::<Videos>().processor.take() else {
        return;
    };
    f(&mut processor, &mut Host { window, cx });
    cx.global_mut::<Videos>().processor = Some(processor);
}

/// One call into the processor, for the window whose flow it belongs to.
struct Host<'a> {
    window: WindowId,
    cx: &'a mut App,
}

impl Host<'_> {
    fn io(&mut self) -> &mut Io {
        &mut self.cx.global_mut::<Videos>().io
    }

    fn handle(&mut self) -> Handle {
        let window = self.window;
        let io = self.io();
        let handle = io.next;
        io.next += 1;
        io.owners.insert(handle, window);
        handle
    }

    fn sender(&mut self) -> async_channel::Sender<VideoEvent> {
        self.io().events.clone()
    }
}

fn io_error(error: &std::io::Error) -> JsError {
    JsError::new("Error", error.to_string())
}

impl VideoHost for Host<'_> {
    fn exists(&mut self, path: &str) -> bool {
        Path::new(path).exists()
    }

    fn rm_recursive(&mut self, path: &str) -> Result<(), JsError> {
        let path = Path::new(path);
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        removed.map_err(|e| io_error(&e))
    }

    fn mkdir_recursive(&mut self, path: &str) -> Result<(), JsError> {
        std::fs::create_dir_all(path).map_err(|e| io_error(&e))
    }

    fn read_file(&mut self, path: &str) -> Result<Vec<u8>, JsError> {
        std::fs::read(path).map_err(|e| io_error(&e))
    }

    fn read_file_sync(&mut self, path: &str) -> Result<Vec<u8>, JsError> {
        std::fs::read(path).map_err(|e| io_error(&e))
    }

    fn unlink(&mut self, path: &str) -> Result<(), JsError> {
        std::fs::remove_file(path).map_err(|e| io_error(&e))
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), JsError> {
        std::fs::rename(from, to).map_err(|e| io_error(&e))
    }

    fn chmod(&mut self, path: &str, mode: u32) -> Result<(), JsError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
                .map_err(|e| io_error(&e))
        }
        #[cfg(not(unix))]
        {
            let _ = mode;
            std::fs::metadata(path).map(drop).map_err(|e| io_error(&e))
        }
    }

    fn sha256_hex(&mut self, data: &[u8]) -> String {
        let digest = Sha256::digest(data);
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn random(&mut self) -> f64 {
        self.io().random.next_f64()
    }

    fn read_clipboard(&mut self) -> JsString {
        let text = self.cx.read_from_clipboard().and_then(|item| item.text());
        JsString::from(text.unwrap_or_default().as_str())
    }

    fn window_destroyed(&mut self) -> bool {
        Windows::handle(self.cx, self.window).is_none()
    }

    fn spawn(
        &mut self,
        command: &str,
        args: &[JsString],
        cwd: Option<&str>,
    ) -> Result<Handle, JsError> {
        if command.contains('\0') || args.iter().any(|arg| arg.units().contains(&0)) {
            return Err(JsError::type_error(
                "The argument must be a string without null bytes.",
            ));
        }
        let handle = self.handle();
        let events = self.sender();
        let emit = move |event| {
            let _ = events.try_send(event);
        };
        self.io().children.spawn(handle, command, args, cwd, emit);
        Ok(handle)
    }

    fn kill(&mut self, process: Handle) -> bool {
        self.io().children.kill(process)
    }

    fn message_box(&mut self, options: &MessageBox) -> Handle {
        let handle = self.handle();
        let events = self.sender();
        let options = MessageBoxOptions {
            kind: match options.kind.as_str() {
                "error" => MessageBoxType::Error,
                "question" => MessageBoxType::Question,
                _ => MessageBoxType::None,
            },
            title: options.title.clone(),
            message: options.message.clone(),
            detail: options.detail.clone(),
            buttons: options.buttons.clone().unwrap_or_default(),
            default_id: options.default_id.map(|id| id as usize),
            ..MessageBoxOptions::default()
        };
        let window = self.window;
        self.cx.defer(move |cx| {
            dispatch::show_message_box(Some(window), options, cx, move |response, _| {
                let _ = events.try_send(VideoEvent::MessageBoxClosed {
                    dialog: handle,
                    response: response as u32,
                });
            });
        });
        handle
    }

    fn open_dialog(&mut self, options: &OpenDialog) -> Handle {
        let handle = self.handle();
        let events = self.sender();
        let options = OpenDialogOptions {
            title: options.title.clone(),
            filters: options
                .filters
                .iter()
                .map(|(name, extensions)| FileFilter {
                    name: name.clone(),
                    extensions: extensions.clone(),
                })
                .collect(),
            open_file: options.properties.iter().any(|p| p == "openFile"),
            ..OpenDialogOptions::default()
        };
        let window = self.window;
        let done = move |paths: Vec<PathBuf>, _: &mut App| {
            let _ = events.try_send(VideoEvent::OpenDialogClosed {
                dialog: handle,
                canceled: paths.is_empty(),
                file_paths: paths
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect(),
            });
        };
        self.cx.defer(move |cx| {
            dispatch::with_window(Some(window), cx, |window, cx| {
                windows::open_dialog(window, cx, options, done);
            });
        });
        handle
    }

    fn download(&mut self, url: &str, folder: &str, filename: &str) -> Handle {
        let handle = self.handle();
        let events = self.sender();
        let (url, folder, filename) = (url.to_owned(), PathBuf::from(folder), filename.to_owned());
        thread::spawn(move || {
            let done = net::download(&url, &folder, &filename);
            if let Err(error) = &done {
                eprintln!("asrs: downloading FFmpeg: {error}");
            }
            let _ = events.try_send(VideoEvent::DownloadFinished {
                download: handle,
                ok: done.is_ok(),
            });
        });
        handle
    }

    fn fetch(
        &mut self,
        url: &JsString,
        headers: &[(JsString, JsValue)],
        timeout_ms: f64,
    ) -> Handle {
        let handle = self.handle();
        let events = self.sender();
        let url = url.to_string_lossy();
        let headers: Vec<(String, String)> = headers
            .iter()
            .map(|(name, value)| {
                (
                    name.to_string_lossy(),
                    ops::to_string(value).to_string_lossy(),
                )
            })
            .collect();
        let timeout = Duration::from_millis(timeout_ms as u64);
        thread::spawn(move || {
            let response = net::fetch(&url, &headers, timeout);
            let _ = events.try_send(VideoEvent::FetchFinished {
                fetch: handle,
                response,
            });
        });
        handle
    }

    fn popup_menu(&mut self, labels: &[JsString], x: f64, y: f64) -> Handle {
        let handle = self.handle();
        let events = self.sender();
        let labels = labels.iter().map(JsString::to_string_lossy).collect();
        let window = self.window;
        self.cx.defer(move |cx| {
            let Some(window) = Windows::handle(cx, window) else {
                return;
            };
            let _ = window.update(cx, |_, window, cx| {
                let closed = move |chosen: Option<usize>, _: &mut App| {
                    let _ = events.try_send(VideoEvent::MenuWillClose { menu: handle });
                    if let Some(index) = chosen {
                        let _ = events.try_send(VideoEvent::MenuClick {
                            menu: handle,
                            index,
                        });
                    }
                };
                let platform = Platform::current();
                bridge::popup_labels(window, cx, platform, labels, x as i32, y as i32, closed);
            });
        });
        handle
    }

    fn set_timeout(&mut self, ms: f64) -> Handle {
        let handle = self.handle();
        let events = self.sender();
        let executor = self.cx.background_executor();
        let timer = executor.timer(Duration::from_millis(ms as u64));
        executor
            .spawn(async move {
                timer.await;
                let _ = events.try_send(VideoEvent::TimerFired { timer: handle });
            })
            .detach();
        handle
    }

    fn read_timer_text(&mut self, path: &JsString, width: f64, height: f64) -> Handle {
        let job = self.handle();
        let events = self.sender();
        let io = self.io();
        let uncaught = io.uncaught.clone();
        let path = path.to_string_lossy();
        let Some(timers) = &io.timers else {
            return job;
        };
        timers.read_timer_text(path, width, height, move |result| match result {
            Ok(text) => {
                let text = JsString::from(text.as_str());
                let _ = events.try_send(VideoEvent::TimerText { job, text });
            }
            Err(error) => {
                let _ = uncaught.try_send(error);
            }
        });
        job
    }

    fn log(&mut self, _: LogLevel, args: &[LogArg]) {
        let text: Vec<String> = args
            .iter()
            .map(|arg| match arg {
                LogArg::Text(text) | LogArg::Error(text) => text.to_string_lossy(),
            })
            .collect();
        eprintln!("asrs: {}", text.join(" "));
    }

    fn send(&mut self, data: JsValue) {
        MainProcess::send_to_hub(self.cx, self.window, MainToHub::VideoData, data);
    }
}
