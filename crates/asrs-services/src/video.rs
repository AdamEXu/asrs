//! `src/main/electron/VideoProcessor.ts` (everything but the OCR inside `readTimerText`, which is
//! P06) and `src/main/electron/getElectronPlatform.ts`.
//!
//! The processor is sans-IO: every call upstream makes into Node, Electron or an npm package is a
//! [`VideoHost`] call, and everything upstream awaits comes back later as a [`VideoEvent`] naming
//! the [`Handle`] the host returned when the request was made.

use asrs_js::string::{JsError, JsString};
use asrs_js::value::JsValue;

/// `getElectronPlatform()` for `process.platform` and `process.arch`.
pub fn get_electron_platform(platform: &str, arch: &str) -> String {
    let _ = (platform, arch);
    todo!("P05")
}

/// `VideoProcessor.getVideoId(url)`.
pub fn get_video_id(url: &JsString) -> Option<JsString> {
    let _ = url;
    todo!("P05")
}

/// What upstream reads from the process and from Electron's `app` while loading the module.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoConfig {
    /// `process.platform`: `"darwin"`, `"linux"` or `"win32"`; it also picks the path style.
    pub platform: String,
    /// `process.arch`.
    pub arch: String,
    /// `app.getPath("userData")`.
    pub user_data: String,
    /// `app.getPath("temp")`.
    pub temp: String,
    /// Full path of the yt-dlp executable (upstream's `fullBinaryPath` in a packaged build).
    pub yt_dlp_binary: String,
    /// `WINDOW_ICON`.
    pub window_icon: Option<String>,
}

/// Names one outstanding host request. The host picks it; events carry it back.
pub type Handle = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Log,
    Warn,
    Error,
}

/// One `console.*` argument: a string, or a thrown error reduced to its `name`.
#[derive(Clone, Debug, PartialEq)]
pub enum LogArg {
    Text(JsString),
    Error(JsString),
}

/// `dialog.showMessageBox` options, in upstream's key order: `type`, `title`, `message`,
/// `detail`, then `buttons` and `defaultId` when present, then `icon` (always present,
/// `undefined` when `None`).
#[derive(Clone, Debug, PartialEq)]
pub struct MessageBox {
    pub kind: String,
    pub title: String,
    pub message: String,
    pub detail: String,
    pub buttons: Option<Vec<String>>,
    pub default_id: Option<f64>,
    pub icon: Option<String>,
}

/// `dialog.showOpenDialog` options: `{title, properties, filters: [{name, extensions}]}`.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenDialog {
    pub title: String,
    pub properties: Vec<String>,
    pub filters: Vec<(String, Vec<String>)>,
}

/// Everything VideoProcessor reaches outside itself. Synchronous upstream calls (and
/// `fs.promises`, which this port treats as settling at once) return their result directly;
/// asynchronous ones return a [`Handle`] whose result arrives as a [`VideoEvent`].
pub trait VideoHost {
    /// `fs.existsSync(path)`.
    fn exists(&mut self, path: &str) -> bool;
    /// `fs.rmSync(path, { recursive: true })`.
    fn rm_recursive(&mut self, path: &str) -> Result<(), JsError>;
    /// `fs.mkdirSync(path, { recursive: true })`.
    fn mkdir_recursive(&mut self, path: &str) -> Result<(), JsError>;
    /// `fs.promises.readFile(path)`.
    fn read_file(&mut self, path: &str) -> Result<Vec<u8>, JsError>;
    /// `fs.readFileSync(path, {})`, as `jsonfile.readFileSync` calls it.
    fn read_file_sync(&mut self, path: &str) -> Result<Vec<u8>, JsError>;
    /// `fs.promises.unlink(path)`.
    fn unlink(&mut self, path: &str) -> Result<(), JsError>;
    /// `fs.promises.rename(from, to)`.
    fn rename(&mut self, from: &str, to: &str) -> Result<(), JsError>;
    /// `fs.promises.chmod(path, mode)`.
    fn chmod(&mut self, path: &str, mode: u32) -> Result<(), JsError>;
    /// `crypto.createHash("sha256").update(data).digest("hex")`.
    fn sha256_hex(&mut self, data: &[u8]) -> String;
    /// `Math.random()`.
    fn random(&mut self) -> f64;
    /// `clipboard.readText()`.
    fn read_clipboard(&mut self) -> JsString;
    /// `window.isDestroyed()`.
    fn window_destroyed(&mut self) -> bool;
    /// `child_process.spawn(command, args, cwd === None ? undefined : { cwd })`. `Err` is the
    /// synchronous throw (an argument with a NUL byte).
    fn spawn(
        &mut self,
        command: &str,
        args: &[JsString],
        cwd: Option<&str>,
    ) -> Result<Handle, JsError>;
    /// `child.kill()`: false once the process has exited or never started.
    fn kill(&mut self, process: Handle) -> bool;
    /// `dialog.showMessageBox(window, options)`.
    fn message_box(&mut self, options: &MessageBox) -> Handle;
    /// `dialog.showOpenDialog(window, options)`.
    fn open_dialog(&mut self, options: &OpenDialog) -> Handle;
    /// `download(url, folder, { filename })` (the `download` npm package).
    fn download(&mut self, url: &str, folder: &str, filename: &str) -> Handle;
    /// `fetch(url, { method: "GET", signal: AbortSignal.timeout(timeout_ms), headers })`, with
    /// `headers` as the `[[name, value], ...]` array upstream passes.
    fn fetch(&mut self, url: &JsString, headers: &[(JsString, JsValue)], timeout_ms: f64)
    -> Handle;
    /// A `Menu` of `labels` and `menu.popup({ window, x, y })`.
    fn popup_menu(&mut self, labels: &[JsString], x: f64, y: f64) -> Handle;
    /// `setTimeout(callback, ms)`.
    fn set_timeout(&mut self, ms: f64) -> Handle;
    /// `VideoProcessor.readTimerText(path, width, height)` (P06 ports its body).
    fn read_timer_text(&mut self, path: &JsString, width: f64, height: f64) -> Handle;
    /// `console.log` / `console.warn` / `console.error`.
    fn log(&mut self, level: LogLevel, args: &[LogArg]);
    /// `dataCallback(data)`: main sends it to the hub window as `video-data`.
    fn send(&mut self, data: JsValue);
}

/// The settled result of an earlier host request.
#[derive(Clone, Debug, PartialEq)]
pub enum VideoEvent {
    /// A child's `stdout` `data` event.
    Stdout { process: Handle, data: Vec<u8> },
    /// A child's `stderr` `data` event.
    Stderr { process: Handle, data: Vec<u8> },
    /// A child's `exit` event.
    Exit {
        process: Handle,
        code: Option<i32>,
        signal: Option<String>,
    },
    /// A child's `close` event.
    Close {
        process: Handle,
        code: Option<i32>,
        signal: Option<String>,
    },
    /// A child's `error` event for a spawn that failed (`code` is e.g. `"ENOENT"`).
    SpawnError {
        process: Handle,
        code: String,
        errno: i32,
        syscall: String,
        path: String,
    },
    /// `showMessageBox` resolved with `{ response }`.
    MessageBoxClosed { dialog: Handle, response: u32 },
    /// `showOpenDialog` resolved with `{ canceled, filePaths }`.
    OpenDialogClosed {
        dialog: Handle,
        canceled: bool,
        file_paths: Vec<String>,
    },
    /// `download` resolved (the file is written) or rejected.
    DownloadFinished { download: Handle, ok: bool },
    /// `fetch` resolved with a response, or rejected (`None`).
    FetchFinished {
        fetch: Handle,
        response: Option<FetchResponse>,
    },
    /// The user clicked item `index` of a popup menu.
    MenuClick { menu: Handle, index: usize },
    /// The popup menu's `menu-will-close` event.
    MenuWillClose { menu: Handle },
    /// A `setTimeout` callback is due.
    TimerFired { timer: Handle },
    /// `readTimerText` resolved.
    TimerText { job: Handle, text: JsString },
}

#[derive(Clone, Debug, PartialEq)]
pub struct FetchResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// `VideoProcessor`'s static state: the process map plus every flow waiting on the host.
pub struct VideoProcessor {
    config: VideoConfig,
}

impl VideoProcessor {
    /// Loading the module (its static block makes no host calls the port reproduces).
    pub fn new(config: VideoConfig) -> VideoProcessor {
        VideoProcessor { config }
    }

    pub fn config(&self) -> &VideoConfig {
        &self.config
    }

    /// `VideoProcessor.prepare(window, uuid, source, matchInfo, menuCoordinates, dataCallback)`.
    /// `source`, `match_info` and `menu_coordinates` are the `select-video` message's members as
    /// sent.
    pub fn prepare(
        &mut self,
        host: &mut dyn VideoHost,
        uuid: &JsString,
        source: &JsValue,
        match_info: &JsValue,
        menu_coordinates: &JsValue,
    ) {
        let _ = (host, uuid, source, match_info, menu_coordinates);
        todo!("P05")
    }

    /// Delivers one settled host request. Events for handles no flow waits on are ignored.
    pub fn handle(&mut self, host: &mut dyn VideoHost, event: VideoEvent) {
        let _ = (host, event);
        todo!("P05")
    }

    /// `VideoProcessor.cleanup()`.
    pub fn cleanup(&mut self, host: &mut dyn VideoHost) {
        let _ = host;
        todo!("P05")
    }
}
