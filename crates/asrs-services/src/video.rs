//! `src/main/electron/VideoProcessor.ts` (everything but the OCR inside `readTimerText`, which is
//! P06) and `src/main/electron/getElectronPlatform.ts`.
//!
//! The processor is sans-IO: every call upstream makes into Node, Electron or an npm package is a
//! [`VideoHost`] call, and everything upstream awaits comes back later as a [`VideoEvent`] naming
//! the [`Handle`] the host returned when the request was made.

use std::collections::HashMap;

use asrs_js::json;
use asrs_js::math::round;
use asrs_js::number;
use asrs_js::ops;
use asrs_js::regex::RegExp;
use asrs_js::sort::array_sort;
use asrs_js::string::{JsError, JsString};
use asrs_js::time::Rng;
use asrs_js::value::{JsObject, JsValue};
use asrs_log::match_info::get_elim_match_string;
use asrs_log::util::{create_uuid, zfill};
use asrs_model::json_file;

use crate::path::PathStyle;

fn s(text: &str) -> JsString {
    JsString::from(text)
}

const FFMPEG_DOWNLOAD_INFO: [(&str, &str, &str); 4] = [
    (
        "mac-x64",
        "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.0/ffmpeg-darwin-x64",
        "cfe20936c83ecf5d68e424b87e8cc45b24dd6be81787810123bb964a0df686f9",
    ),
    (
        "mac-arm64",
        "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.0/ffmpeg-darwin-arm64",
        "a90e3db6a3fd35f6074b013f948b1aa45b31c6375489d39e572bea3f18336584",
    ),
    (
        "win-x64",
        "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.0/ffmpeg-win32-x64",
        "e9fd5e711debab9d680955fc1e38a2c1160fd280b144476cc3f62bc43ef49db1",
    ),
    (
        "win-arm64",
        "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.0/ffmpeg-win32-ia32",
        "fb3766af5cc193ca863e15cd4554a33732973209dad5e3c1433b5e291bceb16c",
    ),
];

const VIDEO_EXTENSIONS: [&str; 34] = [
    "3g2", "3gp", "aaf", "asf", "avchd", "avi", "drc", "flv", "m2v", "m3u8", "m4p", "m4v", "mkv",
    "mng", "mov", "mp2", "mp4", "mpe", "mpeg", "mpg", "mpv", "mxf", "nsv", "ogg", "ogv", "qt",
    "rm", "rmvb", "roq", "svi", "vob", "webm", "wmv", "yuv",
];

const DUMP_FLAGS: [&str; 8] = [
    "--dump-single-json",
    "--no-warnings",
    "--no-check-certificates",
    "--force-ipv4",
    "--format",
    "best",
    "--extractor-args",
    "youtube:player_client=android",
];

const TITLE_FLAGS: [&str; 3] = ["--get-title", "--no-warnings", "--no-check-certificates"];

/// `getElectronPlatform()` for `process.platform` and `process.arch`.
pub fn get_electron_platform(platform: &str, arch: &str) -> String {
    let platform = match platform {
        "darwin" => "mac",
        "linux" => "linux",
        "win32" => "win",
        _ => "",
    };
    let arch = if arch == "arm" { "armv7l" } else { arch };
    format!("{platform}-{arch}")
}

/// `VideoProcessor.getVideoId(url)`.
pub fn get_video_id(url: &JsString) -> Option<JsString> {
    let regex = RegExp::new(
        r#"(?:youtube\.com\/(?:[^\/]+\/.+\/|(?:v|e(?:mbed)?)\/|.*[?&]v=)|youtu\.be\/)([^"&?\/\s]{11})"#,
        "",
    )
    .expect("upstream pattern");
    regex
        .find_from(url.units(), 0)
        .and_then(|matched| matched.group(url.units(), 1))
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

fn send_uuid(host: &mut dyn VideoHost, uuid: &JsString) {
    host.send(ops::object([("uuid", JsValue::String(uuid.clone()))]));
}

fn send_error(host: &mut dyn VideoHost, uuid: &JsString) {
    host.send(ops::object([
        ("uuid", JsValue::String(uuid.clone())),
        ("error", JsValue::Bool(true)),
    ]));
}

fn send_status(
    host: &mut dyn VideoHost,
    uuid: &JsString,
    cache_path: &JsString,
    fps: f64,
    total_frames: f64,
    completed_frames: f64,
    auto_end_frame: f64,
) {
    host.send(ops::object([
        ("uuid", JsValue::String(uuid.clone())),
        ("imgFolder", JsValue::String(cache_path.clone())),
        ("fps", JsValue::Number(fps)),
        ("totalFrames", JsValue::Number(total_frames)),
        ("completedFrames", JsValue::Number(completed_frames)),
        ("autoEndFrame", JsValue::Number(auto_end_frame)),
    ]));
}

/// A thrown value as upstream's catch sites see it: `name`, `message` (only ever compared for
/// `"Error: Python missing"`) and an optional `code` (`ENOENT` on failed spawns).
struct ErrVal {
    name: String,
    message: String,
    code: Option<String>,
}

impl ErrVal {
    fn type_error() -> ErrVal {
        ErrVal {
            name: "TypeError".to_owned(),
            message: String::new(),
            code: None,
        }
    }

    fn python_missing() -> ErrVal {
        ErrVal {
            name: "Error".to_owned(),
            message: "Python missing".to_owned(),
            code: None,
        }
    }

    fn is_python_missing(&self) -> bool {
        self.name == "Error" && self.message == "Python missing"
    }

    fn display(&self) -> String {
        if self.message.is_empty() {
            self.name.clone()
        } else {
            format!("{}: {}", self.name, self.message)
        }
    }
}

fn type_err(error: JsError) -> ErrVal {
    ErrVal {
        name: error.name.to_owned(),
        message: error.message,
        code: None,
    }
}

/// `value.toString()`: `undefined`/`null` throw a `TypeError`; everything else is `ToString`.
fn dot_to_string(value: &JsValue) -> Option<JsString> {
    match value {
        JsValue::Undefined | JsValue::Null => None,
        other => Some(ops::to_string(other)),
    }
}

struct HostRng<'a>(&'a mut dyn VideoHost);

impl Rng for HostRng<'_> {
    fn next_f64(&mut self) -> f64 {
        self.0.random()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CacheKind {
    Main,
    Fallback,
}

#[derive(Clone, Copy)]
enum OnLoadFail {
    Nothing,
    ErrorOnly,
    YoutubeBox,
}

#[derive(Clone)]
enum Caller {
    Youtube { uuid: JsString },
    Tba { tba: usize },
}

struct Direct {
    caller: Caller,
}

struct YtJob {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    kind: YtKind,
}

#[derive(Clone, Copy)]
enum YtKind {
    Dump { direct: usize },
    Title { tba: usize, index: usize },
}

struct Group {
    load: usize,
    secs4_frame: f64,
    results: Vec<Option<JsString>>,
    pending: usize,
}

struct Load {
    uuid: JsString,
    video_path: JsValue,
    headers: Option<JsValue>,
    cache_main: bool,
    fail: OnLoadFail,
    ffmpeg_path: JsString,
    cache_path: JsString,
    process: Option<Handle>,
    running: bool,
    full_output: JsString,
    fps: f64,
    width: f64,
    height: f64,
    duration_secs: f64,
    completed_frames: f64,
    auto_end_frame: f64,
    timer_sample: f64,
    timer_values: Vec<(f64, JsString)>,
    timer_start_found: bool,
}

struct Tba {
    uuid: JsString,
    match_info: JsValue,
    menu_coordinates: JsValue,
    api_key: JsValue,
    phase: TbaPhase,
    event_key: JsValue,
    keys: Vec<JsValue>,
    titles: Vec<JsString>,
    titles_pending: usize,
    menu_clicked: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TbaPhase {
    Events,
    Event,
    Match,
    Titles,
    Menu,
    Done,
    Failed,
}

#[derive(Clone)]
enum Wait {
    OpenDialog { uuid: JsString },
    Probe { load: usize },
    LinuxBox { load: usize },
    DownloadBox { load: usize },
    Download { load: usize, sha: &'static str },
    Conversion { load: usize },
    TimerSample { load: usize, frame: f64 },
    TimerGroup { group: usize, slot: usize },
    Fetch { tba: usize },
    Yt { job: usize },
    Menu { tba: usize },
    MenuTimer { tba: usize },
}

/// `VideoProcessor`'s static state: the process map plus every flow waiting on the host.
pub struct VideoProcessor {
    config: VideoConfig,
    style: PathStyle,
    video_cache: JsString,
    video_cache_fallback: JsString,
    prefs_filename: JsString,
    yt_dirname: String,
    yt_words: Vec<JsString>,
    processes: JsObject<Handle>,
    waits: HashMap<Handle, Wait>,
    loads: Vec<Load>,
    directs: Vec<Direct>,
    yt_jobs: Vec<YtJob>,
    tbas: Vec<Tba>,
    groups: Vec<Group>,
}

fn dirname_basename(path: &JsString, style: PathStyle) -> (String, JsString) {
    let units = path.units();
    let is_sep =
        |unit: u16| unit == b'/' as u16 || (style == PathStyle::Win32 && unit == b'\\' as u16);
    let mut end = units.len();
    while end > 0 && is_sep(units[end - 1]) {
        end -= 1;
    }
    let last = units[..end].iter().rposition(|&unit| is_sep(unit));
    let basename = match last {
        Some(index) => JsString::from_units(units[index + 1..end].to_vec()),
        None => JsString::from_units(units[..end].to_vec()),
    };
    let dirname = match last {
        Some(index) => JsString::from_units(units[..index.max(1)].to_vec()).to_string_lossy(),
        None => ".".to_owned(),
    };
    (dirname, basename)
}

fn yt_clean(bytes: &[u8]) -> JsString {
    let text = s(String::from_utf8_lossy(bytes).as_ref()).trim();
    if text.ends_with(&s("\n"), None) {
        text.slice(0.0, Some(text.len() as f64 - 1.0))
    } else {
        text
    }
}

fn menu_coordinate(coordinates: &JsValue, key: &str) -> Option<f64> {
    let value = ops::get(coordinates, key).ok()?;
    ops::value_to_number(&value).map(round).ok()
}

fn direct_formats(output: &JsValue) -> Result<Option<(JsValue, JsValue)>, ErrVal> {
    let formats = ops::get(output, "formats").map_err(type_err)?;
    let formats = if ops::to_boolean(&formats) {
        formats
    } else {
        JsValue::from(Vec::<JsValue>::new())
    };
    let Some(items) = formats.into_array() else {
        return Err(ErrVal::type_error());
    };
    let mut keyed = Vec::new();
    for format in items {
        let vcodec = ops::get(&format, "vcodec").map_err(type_err)?;
        if ops::to_boolean(&vcodec) && !ops::strict_equals(&vcodec, &JsValue::from("none")) {
            let height = ops::get(&format, "height").map_err(type_err)?;
            if ops::value_to_number(&height).map_err(type_err)? <= 1080.0 {
                keyed.push(format);
            }
        }
    }
    let mut keys = Vec::with_capacity(keyed.len());
    for format in &keyed {
        let height = ops::get(format, "height").map_err(type_err)?;
        keys.push(if ops::to_boolean(&height) {
            ops::value_to_number(&height).map_err(type_err)?
        } else {
            0.0
        });
    }
    let mut keyed: Vec<(f64, JsValue)> = keys.into_iter().zip(keyed).collect();
    array_sort(&mut keyed, |a, b| b.0 - a.0);
    if keyed.is_empty() {
        return Ok(None);
    }
    let url = ops::get(&keyed[0].1, "url").map_err(type_err)?;
    let headers = ops::get(output, "http_headers").map_err(type_err)?;
    Ok(Some((url, headers)))
}

impl VideoProcessor {
    /// Loading the module (its static block makes no host calls the port reproduces).
    pub fn new(config: VideoConfig) -> VideoProcessor {
        let style = if config.platform == "win32" {
            PathStyle::Win32
        } else {
            PathStyle::Posix
        };
        let user_data = s(&config.user_data);
        let temp = s(&config.temp);
        let video_cache = style.join(&[&temp, &s("advantagescope-video-cache")]);
        let video_cache_fallback = style.join(&[&user_data, &s("video-cache")]);
        let prefs_filename = style.join(&[&user_data, &s("prefs.json")]);
        let (yt_dirname, basename) = dirname_basename(&s(&config.yt_dlp_binary), style);
        let mut input = s(".");
        input.push_unit(style.sep());
        input.push_str(&basename);
        let yt_words = input.split(Some(&s(" ")), None);
        VideoProcessor {
            config,
            style,
            video_cache,
            video_cache_fallback,
            prefs_filename,
            yt_dirname,
            yt_words,
            processes: JsObject::new(),
            waits: HashMap::new(),
            loads: Vec::new(),
            directs: Vec::new(),
            yt_jobs: Vec::new(),
            tbas: Vec::new(),
            groups: Vec::new(),
        }
    }

    pub fn config(&self) -> &VideoConfig {
        &self.config
    }

    fn box_opts(
        &self,
        kind: &str,
        message: &str,
        detail: &str,
        buttons: Option<Vec<String>>,
        default_id: Option<f64>,
    ) -> MessageBox {
        MessageBox {
            kind: kind.to_owned(),
            title: if kind == "question" {
                "Alert".to_owned()
            } else {
                "Error".to_owned()
            },
            message: message.to_owned(),
            detail: detail.to_owned(),
            buttons,
            default_id,
            icon: self.config.window_icon.clone(),
        }
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
        let source = match source {
            JsValue::Number(value) => *value,
            _ => return,
        };
        if source == 0.0 {
            let handle = host.open_dialog(&OpenDialog {
                title: "Select a video to open".to_owned(),
                properties: vec!["openFile".to_owned()],
                filters: vec![(
                    "Videos".to_owned(),
                    VIDEO_EXTENSIONS.iter().map(|&ext| ext.to_owned()).collect(),
                )],
            });
            self.waits
                .insert(handle, Wait::OpenDialog { uuid: uuid.clone() });
        } else if source == 1.0 {
            let text = host.read_clipboard();
            if !text.includes(&s("youtube.com"), None) && !text.includes(&s("youtu.be"), None) {
                send_error(host, uuid);
                host.message_box(&self.box_opts(
                    "error",
                    "Copy URL to clipboard",
                    "Please copy a YouTube URL to the clipboard, then try again.",
                    None,
                    None,
                ));
            } else {
                self.start_direct(host, Caller::Youtube { uuid: uuid.clone() }, text);
            }
        } else if source == 2.0 {
            let tba = self.tbas.len();
            self.tbas.push(Tba {
                uuid: uuid.clone(),
                match_info: match_info.clone(),
                menu_coordinates: menu_coordinates.clone(),
                api_key: JsValue::Undefined,
                phase: TbaPhase::Events,
                event_key: JsValue::Undefined,
                keys: Vec::new(),
                titles: Vec::new(),
                titles_pending: 0,
                menu_clicked: false,
            });
            let prefs_path = self.prefs_filename.to_string_lossy();
            let prefs = match host
                .read_file_sync(&prefs_path)
                .and_then(|bytes| json_file::parse(&bytes))
            {
                Ok(prefs) => prefs,
                Err(_) => return self.tba_fail(host, tba, false),
            };
            let api_key = match ops::get(&prefs, "tbaApiKey") {
                Ok(key) => key,
                Err(_) => return self.tba_fail(host, tba, false),
            };
            if !ops::to_boolean(&api_key) {
                return self.tba_fail(host, tba, false);
            }
            self.tbas[tba].api_key = api_key;
            let year = match ops::get(match_info, "year") {
                Ok(year) => year,
                Err(_) => return self.tba_fail(host, tba, false),
            };
            let Some(year) = dot_to_string(&year) else {
                return self.tba_fail(host, tba, false);
            };
            let mut url = s("https://www.thebluealliance.com/api/v3/events/");
            url.push_str(&year);
            self.tba_fetch(host, tba, url);
        }
    }

    /// Delivers one settled host request. Events for handles no flow waits on are ignored.
    pub fn handle(&mut self, host: &mut dyn VideoHost, event: VideoEvent) {
        let handle = match &event {
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
        };
        let Some(wait) = self.waits.get(&handle).cloned() else {
            return;
        };
        match wait {
            Wait::OpenDialog { uuid } => {
                if let VideoEvent::OpenDialogClosed {
                    canceled,
                    file_paths,
                    ..
                } = event
                {
                    self.waits.remove(&handle);
                    if !canceled && !file_paths.is_empty() {
                        self.load_path(
                            host,
                            uuid,
                            JsValue::from(s(&file_paths[0])),
                            CacheKind::Main,
                            None,
                            OnLoadFail::Nothing,
                        );
                    }
                }
            }
            Wait::Probe { load } => match event {
                VideoEvent::Close { code, .. } => {
                    self.waits.remove(&handle);
                    if code == Some(0) {
                        self.loads[load].ffmpeg_path = s("ffmpeg");
                        self.after_ffmpeg_path(host, load);
                    } else {
                        self.ffmpeg_invalid(host, load);
                    }
                }
                VideoEvent::SpawnError { .. } => {
                    self.waits.remove(&handle);
                    self.ffmpeg_invalid(host, load);
                }
                _ => {}
            },
            Wait::LinuxBox { load } => {
                if let VideoEvent::MessageBoxClosed { .. } = event {
                    self.waits.remove(&handle);
                    self.ffmpeg_failed(host, load);
                }
            }
            Wait::DownloadBox { load } => {
                if let VideoEvent::MessageBoxClosed { response, .. } = event {
                    self.waits.remove(&handle);
                    if response == 1 {
                        self.ffmpeg_failed(host, load);
                    } else {
                        self.ffmpeg_download(host, load);
                    }
                }
            }
            Wait::Download { load, sha } => {
                if let VideoEvent::DownloadFinished { ok, .. } = event {
                    self.waits.remove(&handle);
                    if ok {
                        self.ffmpeg_downloaded(host, load, sha);
                    } else {
                        self.ffmpeg_failed(host, load);
                    }
                }
            }
            Wait::Conversion { load } => match event {
                VideoEvent::Stderr { data, .. } => self.conversion_stderr(host, load, &data),
                VideoEvent::Close { code, .. } => self.conversion_close(host, load, code),
                _ => {}
            },
            Wait::TimerSample { load, frame } => {
                if let VideoEvent::TimerText { text, .. } = event {
                    self.waits.remove(&handle);
                    self.timer_sample(host, load, frame, text);
                }
            }
            Wait::TimerGroup { group, slot } => {
                if let VideoEvent::TimerText { text, .. } = event {
                    self.waits.remove(&handle);
                    self.timer_group(host, group, slot, text);
                }
            }
            Wait::Fetch { tba } => {
                if let VideoEvent::FetchFinished { response, .. } = event {
                    self.waits.remove(&handle);
                    self.tba_fetch_done(host, tba, response);
                }
            }
            Wait::Yt { job } => match event {
                VideoEvent::Stdout { data, .. } => {
                    self.yt_jobs[job].stdout.extend_from_slice(&data);
                }
                VideoEvent::Stderr { data, .. } => {
                    self.yt_jobs[job].stderr.extend_from_slice(&data);
                }
                VideoEvent::Exit { code, .. } => {
                    self.waits.remove(&handle);
                    self.yt_exit(host, job, code);
                }
                VideoEvent::SpawnError { code, .. } => {
                    self.waits.remove(&handle);
                    self.yt_job_failed(
                        host,
                        job,
                        ErrVal {
                            name: "Error".to_owned(),
                            message: String::new(),
                            code: Some(code),
                        },
                    );
                }
                _ => {}
            },
            Wait::Menu { tba } => match event {
                VideoEvent::MenuClick { index, .. } => self.menu_click(host, tba, index),
                VideoEvent::MenuWillClose { .. } => {
                    let timer = host.set_timeout(100.0);
                    self.waits.insert(timer, Wait::MenuTimer { tba });
                }
                _ => {}
            },
            Wait::MenuTimer { tba } => {
                if let VideoEvent::TimerFired { .. } = event {
                    self.waits.remove(&handle);
                    self.menu_timer(host, tba);
                }
            }
        }
    }

    /// `VideoProcessor.cleanup()`.
    pub fn cleanup(&mut self, host: &mut dyn VideoHost) {
        let handles: Vec<Handle> = self.processes.iter().map(|(_, handle)| *handle).collect();
        for handle in handles {
            host.kill(handle);
        }
        for cache in [self.video_cache.clone(), self.video_cache_fallback.clone()] {
            let path = cache.to_string_lossy();
            if host.exists(&path) {
                let _ = host.rm_recursive(&path);
            }
        }
    }

    fn load_path(
        &mut self,
        host: &mut dyn VideoHost,
        uuid: JsString,
        video_path: JsValue,
        cache: CacheKind,
        headers: Option<JsValue>,
        fail: OnLoadFail,
    ) {
        let load = self.loads.len();
        self.loads.push(Load {
            uuid,
            video_path,
            headers,
            cache_main: cache == CacheKind::Main,
            fail,
            ffmpeg_path: JsString::new(),
            cache_path: JsString::new(),
            process: None,
            running: false,
            full_output: JsString::new(),
            fps: 0.0,
            width: 0.0,
            height: 0.0,
            duration_secs: 0.0,
            completed_frames: 0.0,
            auto_end_frame: -1.0,
            timer_sample: 0.0,
            timer_values: Vec::new(),
            timer_start_found: false,
        });
        let name = if self.config.platform == "win32" {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        };
        let ffmpeg_path = self.style.join(&[&s(&self.config.user_data), &s(name)]);
        self.loads[load].ffmpeg_path = ffmpeg_path.clone();
        if self.config.platform != "linux" && host.exists(&ffmpeg_path.to_string_lossy()) {
            self.after_ffmpeg_path(host, load);
            return;
        }
        match host.spawn("ffmpeg", &[s("-version")], None) {
            Ok(handle) => {
                self.waits.insert(handle, Wait::Probe { load });
            }
            Err(_) => self.ffmpeg_invalid(host, load),
        }
    }

    fn ffmpeg_invalid(&mut self, host: &mut dyn VideoHost, load: usize) {
        if self.config.platform == "linux" {
            let handle = host.message_box(&self.box_opts(
                "question",
                "FFmpeg Installation Required",
                "FFmpeg is required to process videos files. Please install FFmpeg and add to the PATH.",
                Some(vec!["OK".to_owned()]),
                Some(0.0),
            ));
            self.waits.insert(handle, Wait::LinuxBox { load });
        } else {
            let handle = host.message_box(&self.box_opts(
                "question",
                "Download FFmpeg?",
                "FFmpeg is required to process videos files. Select \"Continue\" to download automatically, or install FFmpeg manually and add to the PATH.",
                Some(vec!["Continue".to_owned(), "Stop".to_owned()]),
                Some(0.0),
            ));
            self.waits.insert(handle, Wait::DownloadBox { load });
        }
    }

    fn ffmpeg_download(&mut self, host: &mut dyn VideoHost, load: usize) {
        let key = get_electron_platform(&self.config.platform, &self.config.arch);
        let Some((_, url, sha)) = FFMPEG_DOWNLOAD_INFO
            .iter()
            .find(|(name, _, _)| *name == key)
        else {
            return self.ffmpeg_failed(host, load);
        };
        let handle = host.download(url, &self.config.user_data, "ffmpeg-download");
        self.waits.insert(handle, Wait::Download { load, sha });
    }

    fn ffmpeg_downloaded(&mut self, host: &mut dyn VideoHost, load: usize, sha: &str) {
        let user_data = s(&self.config.user_data);
        let temp = self.style.join(&[&user_data, &s("ffmpeg-download")]);
        let temp_path = temp.to_string_lossy();
        let data = match host.read_file(&temp_path) {
            Ok(data) => data,
            Err(_) => return self.ffmpeg_failed(host, load),
        };
        let hash = host.sha256_hex(&data);
        if hash != sha {
            host.log(LogLevel::Warn, &[LogArg::Text(s("FFmpeg hash violation!"))]);
            host.log(
                LogLevel::Warn,
                &[LogArg::Text(s(&format!("\tFound: {hash}")))],
            );
            host.log(
                LogLevel::Warn,
                &[LogArg::Text(s(&format!("\tExpected: {sha}")))],
            );
            let _ = host.unlink(&temp_path);
            return self.ffmpeg_failed(host, load);
        }
        let ffmpeg_path = self.loads[load].ffmpeg_path.to_string_lossy();
        if host.rename(&temp_path, &ffmpeg_path).is_err()
            || host.chmod(&ffmpeg_path, 0o755).is_err()
        {
            return self.ffmpeg_failed(host, load);
        }
        self.after_ffmpeg_path(host, load);
    }

    fn ffmpeg_failed(&mut self, host: &mut dyn VideoHost, load: usize) {
        let uuid = self.loads[load].uuid.clone();
        send_error(host, &uuid);
    }

    fn after_ffmpeg_path(&mut self, host: &mut dyn VideoHost, load: usize) {
        let uuid = self.loads[load].uuid.clone();
        send_uuid(host, &uuid);
        let cache_name = {
            let mut rng = HostRng(&mut *host);
            create_uuid(&mut rng)
        };
        let base = if self.loads[load].cache_main {
            self.video_cache.clone()
        } else {
            self.video_cache_fallback.clone()
        };
        let mut cache_path = self.style.join(&[&base, &s(&cache_name)]);
        cache_path.push_unit(self.style.sep());
        self.loads[load].cache_path = cache_path.clone();
        let path = cache_path.to_string_lossy();
        if host.exists(&path) && host.rm_recursive(&path).is_err() {
            return self.load_rejected(host, load);
        }
        if host.mkdir_recursive(&path).is_err() {
            return self.load_rejected(host, load);
        }
        if let Some(&old) = self.processes.get_key(&uuid) {
            host.kill(old);
        }
        let mut args: Vec<JsString> = Vec::new();
        let headers = self.loads[load].headers.clone();
        if let Some(headers) = headers
            && ops::to_boolean(&headers)
        {
            args.push(s("-reconnect"));
            args.push(s("1"));
            args.push(s("-reconnect_streamed"));
            args.push(s("1"));
            args.push(s("-reconnect_delay_max"));
            args.push(s("5"));
            let entries = (|| -> Result<JsString, JsError> {
                let mut text = JsString::new();
                for key in ops::object_keys(&headers)? {
                    let value = ops::get_key(&headers, &key)?;
                    text.push_str(&key);
                    text.push_str(&s(": "));
                    text.push_str(&ops::to_string(&value));
                    text.push_str(&s("\r\n"));
                }
                Ok(text)
            })();
            match entries {
                Ok(text) => {
                    args.push(s("-headers"));
                    args.push(text);
                }
                Err(_) => return self.load_rejected(host, load),
            }
        }
        args.push(s("-i"));
        args.push(ops::to_string(&self.loads[load].video_path));
        args.push(s("-vf"));
        args.push(s("scale=1920:-2,setsar=1:1"));
        args.push(s("-q:v"));
        args.push(s("2"));
        let cache_path = self.loads[load].cache_path.clone();
        args.push(self.style.join(&[&cache_path, &s("%08d.jpg")]));
        let command = self.loads[load].ffmpeg_path.to_string_lossy();
        match host.spawn(&command, &args, None) {
            Ok(handle) => {
                self.processes.insert(self.loads[load].uuid.clone(), handle);
                self.loads[load].process = Some(handle);
                self.loads[load].running = true;
                self.waits.insert(handle, Wait::Conversion { load });
            }
            Err(_) => self.load_rejected(host, load),
        }
    }

    fn load_rejected(&mut self, host: &mut dyn VideoHost, load: usize) {
        match self.loads[load].fail {
            OnLoadFail::Nothing => {}
            OnLoadFail::ErrorOnly => {
                let uuid = self.loads[load].uuid.clone();
                send_error(host, &uuid);
            }
            OnLoadFail::YoutubeBox => {
                let uuid = self.loads[load].uuid.clone();
                send_error(host, &uuid);
                host.message_box(&self.box_opts(
                    "error",
                    "YouTube download failed",
                    "There was an error while trying to open the YouTube video. Note that this feature may fail unexpectedly due to changes on YouTube's servers. Please check for updates or choose a local video file instead.",
                    None,
                    None,
                ));
            }
        }
    }

    fn conversion_stderr(&mut self, host: &mut dyn VideoHost, load: usize, data: &[u8]) {
        if !self.loads[load].running {
            return;
        }
        let text = s(String::from_utf8_lossy(data).as_ref());
        self.loads[load].full_output.push_str(&text);
        if text.includes(&s(" fps, "), None) {
            let fps_index = text.last_index_of(&s(" fps, "), None);
            let start = text.last_index_of(&s(" "), Some(fps_index as f64 - 1.0));
            self.loads[load].fps = number::to_number(
                text.slice(start as f64 + 1.0, Some(fps_index as f64))
                    .units(),
            );
            if self.loads[load].fps.is_nan() {
                return self.conversion_error(host, load);
            }
            let regex = RegExp::new(", [0-9]+x[0-9]+", "").expect("upstream pattern");
            let Some(matched) = regex.find_from(text.units(), 0) else {
                return self.conversion_error(host, load);
            };
            let x_index = text.index_of(&s("x"), Some(matched.index() as f64 + 2.0));
            self.loads[load].width = number::to_number(
                text.substring(matched.index() as f64 + 2.0, Some(x_index as f64))
                    .units(),
            );
            let tail = text.substring(x_index as f64 + 1.0, None);
            let regex = RegExp::new("[^0-9]", "").expect("upstream pattern");
            let Some(matched) = regex.find_from(tail.units(), 0) else {
                return self.conversion_error(host, load);
            };
            let height_end = x_index as f64 + matched.index() as f64;
            self.loads[load].height = number::to_number(
                text.substring(x_index as f64 + 1.0, Some(height_end + 1.0))
                    .units(),
            );
            if self.loads[load].width.is_nan() || self.loads[load].height.is_nan() {
                return self.conversion_error(host, load);
            }
        }
        if text.includes(&s("Duration: "), None) {
            let index = text.last_index_of(&s("Duration: "), None);
            let duration = text.slice(index as f64 + 10.0, Some(index as f64 + 21.0));
            self.loads[load].duration_secs =
                number::to_number(duration.slice(0.0, Some(2.0)).units()) * 3600.0
                    + number::to_number(duration.slice(3.0, Some(5.0)).units()) * 60.0
                    + number::to_number(duration.slice(6.0, Some(8.0)).units())
                    + number::to_number(duration.slice(9.0, Some(11.0)).units()) * 0.01;
            if self.loads[load].duration_secs.is_nan() {
                return self.conversion_error(host, load);
            }
        }
        if text.starts_with(&s("frame="), None) {
            if self.loads[load].fps == 0.0
                || self.loads[load].duration_secs == 0.0
                || self.loads[load].width == 0.0
                || self.loads[load].height == 0.0
            {
                return self.conversion_error(host, load);
            }
            let fps_index = text.index_of(&s(" fps="), None);
            self.loads[load].completed_frames =
                number::to_number(text.slice(6.0, Some(fps_index as f64)).units());
            if self.loads[load].completed_frames.is_nan() {
                return self.conversion_error(host, load);
            }
            if !self.loads[load].timer_start_found {
                let time = self.loads[load].completed_frames / self.loads[load].fps;
                while self.loads[load].timer_sample < time - 2.0 {
                    let frame = round(self.loads[load].timer_sample * self.loads[load].fps) + 1.0;
                    let mut path = self.loads[load].cache_path.clone();
                    path.push_str(&zfill(&s(&number::to_string(frame)), 8.0));
                    path.push_str(&s(".jpg"));
                    let width = self.loads[load].width;
                    let height = self.loads[load].height;
                    let job = host.read_timer_text(&path, width, height);
                    self.waits.insert(job, Wait::TimerSample { load, frame });
                    self.loads[load].timer_sample += 1.0;
                }
            }
            let uuid = self.loads[load].uuid.clone();
            let cache_path = self.loads[load].cache_path.clone();
            let fps = self.loads[load].fps;
            let duration = self.loads[load].duration_secs;
            let completed = self.loads[load].completed_frames;
            let auto_end = self.loads[load].auto_end_frame;
            send_status(
                host,
                &uuid,
                &cache_path,
                fps,
                round(duration * fps),
                completed,
                auto_end,
            );
        }
    }

    fn conversion_close(&mut self, host: &mut dyn VideoHost, load: usize, code: Option<i32>) {
        if !self.loads[load].running {
            return;
        }
        match code {
            Some(0) => {
                let uuid = self.loads[load].uuid.clone();
                let cache_path = self.loads[load].cache_path.clone();
                let fps = self.loads[load].fps;
                let completed = self.loads[load].completed_frames;
                let auto_end = self.loads[load].auto_end_frame;
                send_status(
                    host,
                    &uuid,
                    &cache_path,
                    fps,
                    completed,
                    completed,
                    auto_end,
                );
            }
            Some(1) => {
                if self.loads[load].cache_main
                    && self.loads[load]
                        .full_output
                        .includes(&s("No space left on device"), None)
                {
                    let path = self.loads[load].cache_path.to_string_lossy();
                    let uuid = self.loads[load].uuid.clone();
                    let video_path = self.loads[load].video_path.clone();
                    let headers = self.loads[load].headers.clone();
                    let _ = host.rm_recursive(&path);
                    self.load_path(
                        host,
                        uuid,
                        video_path,
                        CacheKind::Fallback,
                        headers,
                        OnLoadFail::Nothing,
                    );
                } else {
                    self.conversion_error(host, load);
                }
            }
            _ => {}
        }
    }

    fn conversion_error(&mut self, host: &mut dyn VideoHost, load: usize) {
        self.loads[load].running = false;
        if let Some(process) = self.loads[load].process {
            host.kill(process);
        }
        let uuid = self.loads[load].uuid.clone();
        send_error(host, &uuid);
        host.message_box(&self.box_opts(
            "error",
            "Failed to process video",
            "There was a problem while processing the video. Please try again.",
            None,
            None,
        ));
        host.log(
            LogLevel::Log,
            &[LogArg::Text(s("*** START FFMPEG OUTPUT ***"))],
        );
        let output = self.loads[load].full_output.clone();
        host.log(LogLevel::Log, &[LogArg::Text(output)]);
        host.log(
            LogLevel::Log,
            &[LogArg::Text(s("*** END FFMPEG OUTPUT ***"))],
        );
    }

    fn timer_sample(&mut self, host: &mut dyn VideoHost, load: usize, frame: f64, text: JsString) {
        if self.loads[load].timer_start_found {
            return;
        }
        self.loads[load].timer_values.push((frame, text));
        array_sort(&mut self.loads[load].timer_values, |a, b| a.0 - b.0);
        let mut last_is4 = false;
        let mut secs4_frame = None;
        for index in 0..self.loads[load].timer_values.len() {
            let text = &self.loads[load].timer_values[index].1;
            let is4 = text.includes(&s("04"), None);
            let is3 = !is4 && text.includes(&s("03"), None);
            if last_is4 && is3 {
                secs4_frame = Some(self.loads[load].timer_values[index - 1].0);
                break;
            }
            last_is4 = is4;
        }
        let Some(secs4_frame) = secs4_frame else {
            return;
        };
        self.loads[load].timer_start_found = true;
        let group = self.groups.len();
        self.groups.push(Group {
            load,
            secs4_frame,
            results: Vec::new(),
            pending: 0,
        });
        let fps = self.loads[load].fps;
        let width = self.loads[load].width;
        let height = self.loads[load].height;
        let mut frame = secs4_frame;
        while frame < secs4_frame + fps {
            let mut path = self.loads[load].cache_path.clone();
            path.push_str(&zfill(&s(&number::to_string(frame)), 8.0));
            path.push_str(&s(".jpg"));
            let job = host.read_timer_text(&path, width, height);
            self.groups[group].results.push(None);
            self.groups[group].pending += 1;
            let slot = self.groups[group].results.len() - 1;
            self.waits.insert(job, Wait::TimerGroup { group, slot });
            frame += 1.0;
        }
        if self.groups[group].pending == 0 {
            self.finish_group(group);
        }
    }

    fn timer_group(
        &mut self,
        _host: &mut dyn VideoHost,
        group: usize,
        slot: usize,
        text: JsString,
    ) {
        self.groups[group].results[slot] = Some(text);
        self.groups[group].pending -= 1;
        if self.groups[group].pending == 0 {
            self.finish_group(group);
        }
    }

    fn finish_group(&mut self, group: usize) {
        let load = self.groups[group].load;
        let secs4_frame = self.groups[group].secs4_frame;
        for index in 0..self.groups[group].results.len() {
            let text = self.groups[group].results[index]
                .clone()
                .unwrap_or_default();
            if self.loads[load].auto_end_frame > 0.0 {
                continue;
            }
            if text.includes(&s("03"), None) {
                let secs3_frame = secs4_frame + index as f64;
                let fps = self.loads[load].fps;
                self.loads[load].auto_end_frame = round(secs3_frame + fps * 3.0);
            }
        }
    }

    fn start_direct(&mut self, host: &mut dyn VideoHost, caller: Caller, url: JsString) {
        let direct = self.directs.len();
        self.directs.push(Direct { caller });
        match get_video_id(&url) {
            None => self.direct_rejected(host, direct, false),
            Some(id) => {
                let mut clean = s("https://youtube.com/watch?v=");
                clean.push_str(&id);
                let job = self.yt_jobs.len();
                self.yt_jobs.push(YtJob {
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    kind: YtKind::Dump { direct },
                });
                self.yt_spawn(host, job, clean, &DUMP_FLAGS);
            }
        }
    }

    fn yt_spawn(&mut self, host: &mut dyn VideoHost, job: usize, url: JsString, flags: &[&str]) {
        let mut parts = self.yt_words.clone();
        parts.push(url);
        for flag in flags {
            parts.push(s(flag));
        }
        parts.retain(|part| !part.is_empty());
        let command = parts[0].to_string_lossy();
        match host.spawn(&command, &parts[1..], Some(&self.yt_dirname)) {
            Ok(handle) => {
                self.waits.insert(handle, Wait::Yt { job });
            }
            Err(error) => self.yt_job_failed(
                host,
                job,
                ErrVal {
                    name: "Error".to_owned(),
                    message: String::new(),
                    code: Some(error.message),
                },
            ),
        }
    }

    fn yt_exit(&mut self, host: &mut dyn VideoHost, job: usize, code: Option<i32>) {
        if code == Some(0) {
            let stdout = yt_clean(&self.yt_jobs[job].stdout);
            if stdout.starts_with(&s("{"), None) {
                match json::parse(&stdout) {
                    Ok(output) => self.yt_job_output(host, job, output),
                    Err(_) => self.yt_job_failed(
                        host,
                        job,
                        ErrVal {
                            name: "Error".to_owned(),
                            message: String::new(),
                            code: None,
                        },
                    ),
                }
            } else {
                self.yt_job_output(host, job, JsValue::String(stdout));
            }
        } else {
            let stderr = yt_clean(&self.yt_jobs[job].stderr);
            self.yt_job_failed(
                host,
                job,
                ErrVal {
                    name: "ChildProcessError".to_owned(),
                    message: stderr.to_string_lossy(),
                    code: None,
                },
            );
        }
    }

    fn yt_job_output(&mut self, host: &mut dyn VideoHost, job: usize, output: JsValue) {
        match self.yt_jobs[job].kind {
            YtKind::Dump { direct } => self.direct_output(host, direct, output),
            YtKind::Title { tba, index } => {
                let title = match &output {
                    JsValue::String(text) => {
                        let trimmed = text.trim();
                        if trimmed.is_empty() {
                            s("Unknown Video")
                        } else {
                            trimmed
                        }
                    }
                    _ => s("Unknown Video"),
                };
                self.tba_title_done(host, tba, index, title);
            }
        }
    }

    fn yt_job_failed(&mut self, host: &mut dyn VideoHost, job: usize, error: ErrVal) {
        let text = error.display();
        let error = if text.contains("python3")
            || text.contains("Python")
            || text.contains("ENOENT")
            || error.code.as_deref() == Some("ENOENT")
        {
            host.log(
                LogLevel::Error,
                &[LogArg::Error(JsString::from(error.name))],
            );
            if !host.window_destroyed() {
                host.message_box(&self.box_opts(
                    "error",
                    "Python Installation Required",
                    "This feature requires Python 3.10 or above available in your system. Please install Python and try again.",
                    None,
                    None,
                ));
            }
            ErrVal::python_missing()
        } else {
            error
        };
        match self.yt_jobs[job].kind {
            YtKind::Dump { direct } => {
                if !error.is_python_missing() {
                    host.log(
                        LogLevel::Error,
                        &[LogArg::Error(JsString::from(error.name))],
                    );
                }
                self.direct_rejected(host, direct, true);
            }
            YtKind::Title { tba, index } => {
                if error.is_python_missing() {
                    self.tba_titles_rejected(host, tba);
                } else {
                    self.tba_title_done(host, tba, index, s("Unknown Video"));
                }
            }
        }
    }

    fn direct_output(&mut self, host: &mut dyn VideoHost, direct: usize, output: JsValue) {
        match direct_formats(&output) {
            Ok(Some((url, headers))) => self.direct_resolved(host, direct, url, headers),
            Ok(None) => self.direct_rejected(host, direct, false),
            Err(error) => {
                host.log(
                    LogLevel::Error,
                    &[LogArg::Error(JsString::from(error.name))],
                );
                self.direct_rejected(host, direct, true);
            }
        }
    }

    fn direct_resolved(
        &mut self,
        host: &mut dyn VideoHost,
        direct: usize,
        url: JsValue,
        headers: JsValue,
    ) {
        let caller = self.directs[direct].caller.clone();
        match caller {
            Caller::Youtube { uuid } => {
                self.load_path(
                    host,
                    uuid,
                    url,
                    CacheKind::Main,
                    Some(headers),
                    OnLoadFail::YoutubeBox,
                );
            }
            Caller::Tba { tba } => {
                self.tbas[tba].phase = TbaPhase::Done;
                let uuid = self.tbas[tba].uuid.clone();
                self.load_path(
                    host,
                    uuid,
                    url,
                    CacheKind::Main,
                    Some(headers),
                    OnLoadFail::ErrorOnly,
                );
            }
        }
    }

    fn direct_rejected(&mut self, host: &mut dyn VideoHost, direct: usize, silent: bool) {
        let caller = self.directs[direct].caller.clone();
        match caller {
            Caller::Youtube { uuid } => {
                send_error(host, &uuid);
                if !silent {
                    host.message_box(&self.box_opts(
                        "error",
                        "YouTube download failed",
                        "There was an error while trying to open the YouTube video. Note that this feature may fail unexpectedly due to changes on YouTube's servers. Please check for updates or choose a local video file instead.",
                        None,
                        None,
                    ));
                }
            }
            Caller::Tba { tba } => {
                self.tbas[tba].phase = TbaPhase::Done;
                let uuid = self.tbas[tba].uuid.clone();
                send_error(host, &uuid);
            }
        }
    }

    fn tba_fetch(&mut self, host: &mut dyn VideoHost, tba: usize, url: JsString) {
        let headers = [(s("X-TBA-Auth-Key"), self.tbas[tba].api_key.clone())];
        let handle = host.fetch(&url, &headers, 3000.0);
        self.waits.insert(handle, Wait::Fetch { tba });
    }

    fn tba_fetch_done(
        &mut self,
        host: &mut dyn VideoHost,
        tba: usize,
        response: Option<FetchResponse>,
    ) {
        let Some(response) = response else {
            return self.tba_fail(host, tba, false);
        };
        if !(200..=299).contains(&response.status) {
            return self.tba_fail(host, tba, false);
        }
        let data = match json_file::parse(&response.body) {
            Ok(data) => data,
            Err(_) => return self.tba_fail(host, tba, false),
        };
        match self.tbas[tba].phase {
            TbaPhase::Events => self.tba_events(host, tba, data),
            TbaPhase::Event => self.tba_event(host, tba, data),
            TbaPhase::Match => self.tba_match(host, tba, data),
            _ => {}
        }
    }

    fn tba_events(&mut self, host: &mut dyn VideoHost, tba: usize, data: JsValue) {
        let Some(events) = data.into_array() else {
            return self.tba_fail(host, tba, false);
        };
        let mut found = None;
        let mut match_event = None::<JsString>;
        for event in &events {
            let code = match ops::get(event, "first_event_code") {
                Ok(code) => code,
                Err(_) => return self.tba_fail(host, tba, false),
            };
            if !ops::to_boolean(&code) {
                continue;
            }
            let Some(code) = code.into_string() else {
                return self.tba_fail(host, tba, false);
            };
            let want = match &match_event {
                Some(want) => want.clone(),
                None => {
                    let value = match ops::get(&self.tbas[tba].match_info, "event") {
                        Ok(value) => value,
                        Err(_) => return self.tba_fail(host, tba, false),
                    };
                    match value.into_string() {
                        Some(value) => value,
                        None => return self.tba_fail(host, tba, false),
                    }
                }
            };
            match_event = Some(want.clone());
            if code.to_lower_case() == want.to_lower_case() {
                found = Some(event.clone());
                break;
            }
        }
        let Some(event) = found else {
            return self.tba_fail(host, tba, false);
        };
        let key = match ops::get(&event, "key") {
            Ok(key) => key,
            Err(_) => return self.tba_fail(host, tba, false),
        };
        let match_type = match ops::get(&self.tbas[tba].match_info, "matchType") {
            Ok(value) => value,
            Err(_) => return self.tba_fail(host, tba, false),
        };
        if ops::strict_equals(&match_type, &JsValue::from(3.0)) {
            self.tbas[tba].event_key = key.clone();
            self.tbas[tba].phase = TbaPhase::Event;
            let mut url = s("https://www.thebluealliance.com/api/v3/event/");
            url.push_str(&ops::to_string(&key));
            self.tba_fetch(host, tba, url);
        } else {
            let number = match ops::get(&self.tbas[tba].match_info, "matchNumber") {
                Ok(value) => value,
                Err(_) => return self.tba_fail(host, tba, false),
            };
            let Some(number) = dot_to_string(&number) else {
                return self.tba_fail(host, tba, false);
            };
            let mut url = s("https://www.thebluealliance.com/api/v3/match/");
            url.push_str(&ops::to_string(&key));
            url.push_str(&s("_qm"));
            url.push_str(&number);
            self.tbas[tba].phase = TbaPhase::Match;
            self.tba_fetch(host, tba, url);
        }
    }

    fn tba_event(&mut self, host: &mut dyn VideoHost, tba: usize, data: JsValue) {
        let playoff_type = match ops::get(&data, "playoff_type") {
            Ok(value) => value,
            Err(_) => return self.tba_fail(host, tba, false),
        };
        let playoff_type = match playoff_type {
            JsValue::Number(value) => value,
            _ => f64::NAN,
        };
        let number = match ops::get(&self.tbas[tba].match_info, "matchNumber") {
            Ok(value) => value,
            Err(_) => return self.tba_fail(host, tba, false),
        };
        let number = match ops::value_to_number(&number) {
            Ok(value) => value,
            Err(_) => return self.tba_fail(host, tba, false),
        };
        let Some(elim) = get_elim_match_string(playoff_type, number) else {
            return self.tba_fail(host, tba, false);
        };
        let mut url = s("https://www.thebluealliance.com/api/v3/match/");
        url.push_str(&ops::to_string(&self.tbas[tba].event_key));
        url.push_str(&s("_"));
        url.push_str(&elim);
        self.tbas[tba].phase = TbaPhase::Match;
        self.tba_fetch(host, tba, url);
    }

    fn tba_match(&mut self, host: &mut dyn VideoHost, tba: usize, data: JsValue) {
        let videos = match ops::get(&data, "videos") {
            Ok(videos) => videos,
            Err(_) => return self.tba_fail(host, tba, false),
        };
        let Some(videos) = videos.into_array() else {
            return self.tba_fail(host, tba, false);
        };
        let mut keys = Vec::new();
        for video in &videos {
            let kind = match ops::get(video, "type") {
                Ok(kind) => kind,
                Err(_) => return self.tba_fail(host, tba, false),
            };
            if ops::strict_equals(&kind, &JsValue::from("youtube")) {
                match ops::get(video, "key") {
                    Ok(key) => keys.push(key),
                    Err(_) => return self.tba_fail(host, tba, false),
                }
            }
        }
        match keys.len() {
            0 => self.tba_fail(host, tba, false),
            1 => {
                self.tbas[tba].phase = TbaPhase::Done;
                let mut url = s("https://youtube.com/watch?v=");
                url.push_str(&ops::to_string(&keys[0]));
                self.start_direct(host, Caller::Tba { tba }, url);
            }
            _ => {
                self.tbas[tba].phase = TbaPhase::Titles;
                self.tbas[tba].titles = vec![JsString::new(); keys.len()];
                self.tbas[tba].titles_pending = keys.len();
                self.tbas[tba].keys = keys;
                for index in 0..self.tbas[tba].keys.len() {
                    let key = self.tbas[tba].keys[index].clone();
                    let mut url = s("https://youtube.com/watch?v=");
                    url.push_str(&ops::to_string(&key));
                    let job = self.yt_jobs.len();
                    self.yt_jobs.push(YtJob {
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                        kind: YtKind::Title { tba, index },
                    });
                    self.yt_spawn(host, job, url, &TITLE_FLAGS);
                }
            }
        }
    }

    fn tba_title_done(
        &mut self,
        host: &mut dyn VideoHost,
        tba: usize,
        index: usize,
        title: JsString,
    ) {
        if self.tbas[tba].phase != TbaPhase::Titles {
            return;
        }
        self.tbas[tba].titles[index] = title;
        self.tbas[tba].titles_pending -= 1;
        if self.tbas[tba].titles_pending == 0 {
            let coordinates = self.tbas[tba].menu_coordinates.clone();
            let Some(x) = menu_coordinate(&coordinates, "0") else {
                return self.tba_fail(host, tba, false);
            };
            let Some(y) = menu_coordinate(&coordinates, "1") else {
                return self.tba_fail(host, tba, false);
            };
            let labels = self.tbas[tba].titles.clone();
            let handle = host.popup_menu(&labels, x, y);
            self.tbas[tba].phase = TbaPhase::Menu;
            self.waits.insert(handle, Wait::Menu { tba });
        }
    }

    fn tba_titles_rejected(&mut self, host: &mut dyn VideoHost, tba: usize) {
        if self.tbas[tba].phase == TbaPhase::Titles {
            self.tba_fail(host, tba, true);
        }
    }

    fn menu_click(&mut self, host: &mut dyn VideoHost, tba: usize, index: usize) {
        self.tbas[tba].menu_clicked = true;
        if self.tbas[tba].phase != TbaPhase::Menu {
            return;
        }
        self.tbas[tba].phase = TbaPhase::Done;
        let mut url = s("https://youtube.com/watch?v=");
        let key = self.tbas[tba].keys[index].clone();
        url.push_str(&ops::to_string(&key));
        self.start_direct(host, Caller::Tba { tba }, url);
    }

    fn menu_timer(&mut self, host: &mut dyn VideoHost, tba: usize) {
        if self.tbas[tba].menu_clicked || self.tbas[tba].phase != TbaPhase::Menu {
            return;
        }
        self.tba_fail(host, tba, true);
    }

    fn tba_fail(&mut self, host: &mut dyn VideoHost, tba: usize, silent: bool) {
        self.tbas[tba].phase = TbaPhase::Failed;
        let uuid = self.tbas[tba].uuid.clone();
        send_error(host, &uuid);
        if !silent {
            host.message_box(&self.box_opts(
                "error",
                "TBA download failed",
                "There was a problem finding the match video on The Blue Alliance. Please check your API key, or load the video using a YouTube URL or local file instead.",
                None,
                None,
            ));
        }
    }
}
