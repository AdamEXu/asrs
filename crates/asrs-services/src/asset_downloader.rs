//! `src/main/electron/assetDownloader.ts` and `src/main/github.ts`: the loop that keeps
//! `autoAssets` in sync with the GitHub asset releases, and the status text the About dialog
//! shows. Upstream downloads through the `download` package (got 8 plus `decompress`); the port
//! reproduces what that stack does on the wire and on disk, one HTTP request at a time through
//! [`DownloadHost`].

use asrs_js::json::parse as json_parse;
use asrs_js::string::{JsString, decode_uri_component};
use asrs_js::time::{LocalZone, to_locale_time_string};
use asrs_js::value::JsValue;

use crate::asset_loader::AssetFolders;
use crate::fs::Fs;
use crate::legacy_url::{self, LegacyUrl};
use crate::path::PathStyle;
use crate::zip;

pub const GITHUB_REPOSITORY: &str = "Mechanical-Advantage/AdvantageScope";
pub const GITHUB_ASSETS_REPOSITORY: &str = "Mechanical-Advantage/AdvantageScopeAssets";
pub const GITHUB_ASSET_TAG_DEFAULT: &str = "default-assets-v2";
pub const GITHUB_ASSET_TAG_FRC6328: &str = "frc-6328-assets-v2";

pub const REQUIRED_SPACE_GB: f64 = 8.0;
pub const FAILURE_TIMEOUT_MS: f64 = 30.0 * 1000.0;
pub const SUCCESS_TIMEOUT_MS: f64 = 6.0 * 60.0 * 60.0 * 1000.0;

const FETCH_TIMEOUT_MS: f64 = 5000.0;
const DISK_FAIL_STATUS: &str = "Not enough disk space. Trying again soon.";
const GITHUB_FAIL_STATUS: &str = "Cannot connect to GitHub. Trying again soon.";
const SUCCESS_STATUS: &str =
    "All assets downloaded. AdvantageScope will check for updates periodically.";
const INITIAL_STATUS: &str = "No status available.";

/// `buildConstants.DISTRIBUTION`: WPILib builds treat the FRC 6328 release as optional.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Distribution {
    Frc6328,
    WpiLib,
}

/// The GitHub API call: `fetch(url, { method, headers, signal: AbortSignal.timeout(ms) })`.
#[derive(Clone, Debug, PartialEq)]
pub struct FetchRequest {
    pub url: String,
    pub method: String,
    /// In the order upstream's object literal lists them.
    pub headers: Vec<(String, String)>,
    pub timeout_ms: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FetchOutcome {
    /// The final response; `None` is a null body (204, 304).
    Response { status: u16, body: Option<Vec<u8>> },
    /// `fetch` rejects at once.
    NetworkError,
    /// No response ever arrives; only the abort signal ends the call.
    Hang,
}

/// One request on the wire, as got 8 sends it: redirects and retries are separate requests.
#[derive(Clone, Debug, PartialEq)]
pub struct HttpRequest {
    pub url: String,
    pub method: String,
    /// Lowercase names, sorted by name.
    pub headers: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HttpOutcome {
    /// Header names are lowercase.
    Response {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    /// The connection drops before a response (`ECONNRESET`).
    Reset,
    /// The request never completes.
    Hang,
}

/// A rejected promise from the host; upstream catches it without looking at the reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rejected;

/// Everything outside the process the loop talks to.
pub trait DownloadHost {
    /// `checkDiskSpace(path).free` in bytes (`NaN` when the result has none), or the call's
    /// rejection.
    fn free_space(&mut self, path: &str) -> Result<f64, Rejected>;

    fn fetch(&mut self, request: &FetchRequest) -> FetchOutcome;

    fn request(&mut self, request: &HttpRequest) -> HttpOutcome;

    /// `Math.random()`, which got's retry backoff adds as noise.
    fn random(&mut self) -> f64;

    /// main's `updateCallback` (reload and resend the assets).
    fn assets_updated(&mut self) -> Result<(), Rejected>;
}

/// A pending `setTimeout`, including got's retry backoff and the fetch abort signal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timer {
    pub due_ms: f64,
    pub delay_ms: f64,
}

struct AssetInfo {
    source: Option<JsString>,
    target: String,
    optional: bool,
}

struct Download {
    url: LegacyUrl,
    protocol: String,
    first_href: String,
    retries: u32,
    redirects: u32,
    download_path: String,
    target: String,
    generation: u64,
    settled: bool,
}

enum TimerKind {
    Check,
    Retry(usize),
    AbortFetch,
}

struct PendingTimer {
    due: f64,
    delay: f64,
    seq: u64,
    kind: TimerKind,
}

/// The module state of `assetDownloader.ts` plus the work it has in flight: the check loop,
/// downloads waiting on a retry or a stalled request, and the pending timers.
pub struct AssetDownloader {
    style: PathStyle,
    auto: String,
    distribution: Distribution,
    status: String,
    clock: f64,
    seq: u64,
    generation: u64,
    timers: Vec<PendingTimer>,
    downloads: Vec<Download>,
    pending: usize,
    check_failed: bool,
    spawning: bool,
    fetch_pending: bool,
    abort_seq: u64,
    targets: Vec<String>,
}

impl AssetDownloader {
    pub fn new(folders: &AssetFolders, distribution: Distribution) -> AssetDownloader {
        AssetDownloader {
            style: folders.style,
            auto: folders.auto.to_string_lossy(),
            distribution,
            status: INITIAL_STATUS.to_owned(),
            clock: 0.0,
            seq: 0,
            generation: 0,
            timers: Vec::new(),
            downloads: Vec::new(),
            pending: 0,
            check_failed: false,
            spawning: false,
            fetch_pending: false,
            abort_seq: 0,
            targets: Vec::new(),
        }
    }

    /// `getAssetDownloadStatus()`.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// `startAssetDownloadLoop(updateCallback)` with the clock at `now_ms`. Returns once nothing
    /// can make progress without a timer or a stalled request.
    pub fn start(
        &mut self,
        now_ms: f64,
        fs: &dyn Fs,
        host: &mut dyn DownloadHost,
        zone: &impl LocalZone,
    ) {
        self.clock = now_ms;
        self.check(fs, host, zone);
    }

    /// Runs every timer due at or before `to_ms`, earliest first (ties in the order they were
    /// set), each with the clock at its own due time and run until nothing else can progress;
    /// then the clock reads `to_ms`.
    pub fn advance(
        &mut self,
        to_ms: f64,
        fs: &dyn Fs,
        host: &mut dyn DownloadHost,
        zone: &impl LocalZone,
    ) {
        while let Some(pos) = self
            .timers
            .iter()
            .enumerate()
            .filter(|(_, t)| t.due <= to_ms)
            .min_by(|a, b| a.1.due.total_cmp(&b.1.due).then(a.1.seq.cmp(&b.1.seq)))
            .map(|(i, _)| i)
        {
            let timer = self.timers.remove(pos);
            self.clock = timer.due;
            match timer.kind {
                TimerKind::Check => self.check(fs, host, zone),
                TimerKind::Retry(di) => self.issue_request(di, fs, host),
                TimerKind::AbortFetch => {
                    if self.fetch_pending {
                        self.fetch_pending = false;
                        self.github_fail();
                    }
                }
            }
        }
        self.clock = to_ms;
    }

    /// Every pending timer, in any order.
    pub fn timers(&self) -> Vec<Timer> {
        self.timers
            .iter()
            .map(|t| Timer {
                due_ms: t.due,
                delay_ms: t.delay,
            })
            .collect()
    }

    fn join(&self, a: &str, b: &str) -> String {
        self.style
            .join(&[&JsString::from(a), &JsString::from(b)])
            .to_string_lossy()
    }

    fn push_timer(&mut self, delay: f64, kind: TimerKind) -> u64 {
        let seq = self.seq;
        self.seq += 1;
        self.timers.push(PendingTimer {
            due: self.clock + delay,
            delay,
            seq,
            kind,
        });
        seq
    }

    fn disk_fail(&mut self) {
        self.status = DISK_FAIL_STATUS.to_owned();
        self.push_timer(FAILURE_TIMEOUT_MS, TimerKind::Check);
    }

    fn github_fail(&mut self) {
        self.status = GITHUB_FAIL_STATUS.to_owned();
        self.push_timer(FAILURE_TIMEOUT_MS, TimerKind::Check);
    }

    fn check(&mut self, fs: &dyn Fs, host: &mut dyn DownloadHost, zone: &impl LocalZone) {
        match host.free_space(&self.auto) {
            Ok(free)
                if !matches!(
                    (free / 1e9).partial_cmp(&REQUIRED_SPACE_GB),
                    Some(std::cmp::Ordering::Less)
                ) => {}
            _ => {
                self.disk_fail();
                return;
            }
        }
        let request = FetchRequest {
            url: format!("https://api.github.com/repos/{GITHUB_ASSETS_REPOSITORY}/releases"),
            method: "GET".to_owned(),
            headers: vec![
                ("pragma".to_owned(), "no-cache".to_owned()),
                ("cache-control".to_owned(), "no-cache".to_owned()),
            ],
            timeout_ms: FETCH_TIMEOUT_MS,
        };
        self.abort_seq = self.push_timer(FETCH_TIMEOUT_MS, TimerKind::AbortFetch);
        self.fetch_pending = true;
        let outcome = host.fetch(&request);
        self.settle_fetch(outcome, fs, host, zone);
    }

    fn settle_fetch(
        &mut self,
        outcome: FetchOutcome,
        fs: &dyn Fs,
        host: &mut dyn DownloadHost,
        zone: &impl LocalZone,
    ) {
        if !self.fetch_pending {
            return;
        }
        if matches!(outcome, FetchOutcome::Hang) {
            return;
        }
        self.fetch_pending = false;
        self.timers.retain(|t| t.seq != self.abort_seq);
        let FetchOutcome::Response { status, body } = outcome else {
            self.github_fail();
            return;
        };
        if !(200..=299).contains(&status) {
            self.github_fail();
            return;
        }
        let text = String::from_utf8_lossy(&body.unwrap_or_default()).into_owned();
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned();
        let Ok(json) = json_parse(&JsString::from(text)) else {
            self.github_fail();
            return;
        };
        let Some(infos) = self.release_infos(&json) else {
            self.github_fail();
            return;
        };
        self.status = format!(
            "Download started at {}. Please wait a few minutes for the download to complete.",
            to_locale_time_string(self.clock, zone)
        );
        self.targets = infos.iter().map(|i| i.target.clone()).collect();
        self.generation += 1;
        self.pending = 0;
        self.check_failed = false;
        self.spawning = true;
        let mut sync_fail = false;
        for info in &infos {
            if info.optional || fs.exists(&info.target) {
                continue;
            }
            match self.spawn(info, fs, host) {
                SpawnResult::SyncFail => {
                    sync_fail = true;
                    break;
                }
                SpawnResult::AsyncFail => {
                    if !self.check_failed {
                        self.check_failed = true;
                        self.github_fail();
                    }
                }
                SpawnResult::Issued => {}
            }
        }
        self.spawning = false;
        if sync_fail {
            self.check_failed = true;
            self.github_fail();
            return;
        }
        if !self.check_failed && self.pending == 0 {
            self.finish_check(fs, host);
        }
    }

    fn release_infos(&self, json: &JsValue) -> Option<Vec<AssetInfo>> {
        let releases = json.as_array()?;
        let mut infos = Vec::new();
        for release in releases {
            if release.is_null() {
                return None;
            }
            let tag = release.get("tag_name").and_then(JsValue::as_str);
            let Some(tag) = tag else { continue };
            let optional = self.distribution == Distribution::WpiLib
                && tag.to_string_lossy() == GITHUB_ASSET_TAG_FRC6328;
            if tag.to_string_lossy() != GITHUB_ASSET_TAG_DEFAULT
                && tag.to_string_lossy() != GITHUB_ASSET_TAG_FRC6328
            {
                continue;
            }
            let assets = release.get("assets").and_then(JsValue::as_array)?;
            for asset in assets {
                if asset.is_null() {
                    return None;
                }
                let name = asset.get("name").and_then(JsValue::as_str)?;
                let units = name.units();
                let len = units.len() as i64;
                let end = if len >= 4 {
                    len - 4
                } else {
                    (2 * len - 4).max(0)
                };
                let stripped =
                    JsString::from_units(units[..usize::try_from(end).unwrap_or(0)].to_vec());
                let target = self
                    .style
                    .join(&[&JsString::from(self.auto.as_str()), &stripped])
                    .to_string_lossy();
                infos.push(AssetInfo {
                    source: asset
                        .get("browser_download_url")
                        .and_then(JsValue::as_str)
                        .cloned(),
                    target,
                    optional,
                });
            }
        }
        Some(infos)
    }

    fn spawn(&mut self, info: &AssetInfo, fs: &dyn Fs, host: &mut dyn DownloadHost) -> SpawnResult {
        let Some(source) = &info.source else {
            return SpawnResult::SyncFail;
        };
        let mut source = source.to_string_lossy();
        if source.starts_with("unix:") {
            source = format!("http://{source}");
        }
        if decode_uri_component(&JsString::from(source.as_str())).is_err() {
            return SpawnResult::SyncFail;
        }
        let trimmed = JsString::from(source.as_str()).trim().to_string_lossy();
        let trimmed = trimmed.as_str();
        let with_protocol = if prepend_http_preserved(trimmed) || already_slashed(trimmed) {
            trimmed.to_owned()
        } else {
            format!("https://{trimmed}")
        };
        let Ok(parsed) = legacy_url::parse(&with_protocol, false) else {
            return SpawnResult::SyncFail;
        };
        if parsed.auth.as_deref().is_some_and(|a| !a.is_empty()) {
            return SpawnResult::SyncFail;
        }
        let protocol = parsed
            .protocol
            .clone()
            .unwrap_or_else(|| "http:".to_owned());
        if protocol != "http:" && protocol != "https:" {
            return SpawnResult::AsyncFail;
        }
        let index = self.downloads.len();
        self.downloads.push(Download {
            first_href: parsed.href.clone(),
            url: parsed,
            protocol,
            retries: 0,
            redirects: 0,
            download_path: format!("{}_download", info.target),
            target: info.target.clone(),
            generation: self.generation,
            settled: false,
        });
        self.pending += 1;
        self.issue_request(index, fs, host);
        SpawnResult::Issued
    }

    fn issue_request(&mut self, di: usize, fs: &dyn Fs, host: &mut dyn DownloadHost) {
        if self.downloads[di].settled {
            return;
        }
        if self.downloads[di].protocol != "http:" && self.downloads[di].protocol != "https:" {
            self.fail_download(di);
            return;
        }
        let wire = self.downloads[di].url.href.clone();
        let request = HttpRequest {
            url: wire.clone(),
            method: "GET".to_owned(),
            headers: vec![
                ("accept-encoding".to_owned(), "gzip, deflate".to_owned()),
                (
                    "user-agent".to_owned(),
                    "got/8.3.2 (https://github.com/sindresorhus/got)".to_owned(),
                ),
            ],
        };
        match host.request(&request) {
            HttpOutcome::Hang => {}
            HttpOutcome::Reset => {
                self.downloads[di].retries += 1;
                if self.downloads[di].retries > 2 {
                    self.fail_download(di);
                } else {
                    let delay = f64::from(1u32 << self.downloads[di].retries) * 1000.0
                        + host.random() * 100.0;
                    self.push_timer(delay, TimerKind::Retry(di));
                }
            }
            HttpOutcome::Response {
                status,
                headers,
                body,
            } => {
                let location = headers
                    .iter()
                    .find(|(name, _)| name == "location")
                    .map(|(_, v)| v.clone());
                if let Some(location) = location
                    .filter(|_| matches!(status, 300 | 301 | 302 | 303 | 304 | 305 | 307 | 308))
                {
                    if self.downloads[di].redirects >= 10 {
                        self.fail_download(di);
                        return;
                    }
                    self.downloads[di].redirects += 1;
                    let bytes: Vec<u8> =
                        location.chars().map(|c| (c as u32 & 0xff) as u8).collect();
                    let location = String::from_utf8_lossy(&bytes).into_owned();
                    let mut base = self.downloads[di].url.clone();
                    base.protocol = Some(self.downloads[di].protocol.clone());
                    match legacy_url::resolve(&legacy_url::format(&base), &location)
                        .and_then(|r| legacy_url::parse(&r, false))
                    {
                        Ok(url) => {
                            self.downloads[di].protocol = url.protocol.clone().unwrap_or_default();
                            self.downloads[di].url = url;
                            self.issue_request(di, fs, host);
                        }
                        Err(()) => self.fail_download(di),
                    }
                } else if (200..=299).contains(&status) || status == 304 {
                    self.complete_download(di, &headers, &body, fs, host);
                } else {
                    self.fail_download(di);
                }
            }
        }
    }

    fn complete_download(
        &mut self,
        di: usize,
        headers: &[(String, String)],
        body: &[u8],
        fs: &dyn Fs,
        host: &mut dyn DownloadHost,
    ) {
        let (download_path, target, first_href) = {
            let dl = &self.downloads[di];
            (
                dl.download_path.clone(),
                dl.target.clone(),
                dl.first_href.clone(),
            )
        };
        let filename = match single_filename(headers, body, &first_href) {
            Err(()) => {
                self.fail_download(di);
                return;
            }
            Ok(filename) => filename,
        };
        let output_file = self.join(&download_path, &filenamify(&filename));
        let saved = if zip::is_zip_archive(body) {
            match zip::read_entries(body) {
                Err(()) => false,
                Ok(entries) => {
                    let output = posix_dirname(&output_file);
                    let mut failed = false;
                    for entry in entries {
                        if fs.mkdir_all(&output).is_err() {
                            failed = true;
                            continue;
                        }
                        let dest = self.join(&output, &entry.name);
                        if entry.is_dir {
                            if fs.mkdir_all(&dest).is_err() {
                                failed = true;
                            }
                        } else {
                            let parent = posix_dirname(&dest);
                            if fs.mkdir_all(&parent).is_err()
                                || fs.write(&dest, &entry.data).is_err()
                            {
                                failed = true;
                            }
                        }
                    }
                    !failed
                }
            }
        } else {
            fs.mkdir_all(&posix_dirname(&output_file)).is_ok()
                && fs.write(&output_file, body).is_ok()
        };
        if !saved || fs.rename(&download_path, &target).is_err() {
            self.fail_download(di);
            return;
        }
        self.downloads[di].settled = true;
        if self.downloads[di].generation == self.generation {
            self.pending = self.pending.saturating_sub(1);
            if self.pending == 0 && !self.check_failed && !self.spawning {
                self.finish_check(fs, host);
            }
        }
    }

    fn fail_download(&mut self, di: usize) {
        self.downloads[di].settled = true;
        if self.downloads[di].generation != self.generation || self.check_failed {
            return;
        }
        self.check_failed = true;
        self.github_fail();
    }

    fn finish_check(&mut self, fs: &dyn Fs, host: &mut dyn DownloadHost) {
        let entries = match fs.read_dir(&self.auto) {
            Ok(entries) => entries,
            Err(_) => {
                self.github_fail();
                return;
            }
        };
        for entry in entries {
            let folder_path = self.join(&self.auto, &entry.name);
            if !self.targets.iter().any(|t| t == &folder_path)
                && fs.remove_all(&folder_path).is_err()
            {
                self.github_fail();
                return;
            }
        }
        self.status = SUCCESS_STATUS.to_owned();
        if host.assets_updated().is_err() {
            self.github_fail();
            return;
        }
        self.push_timer(SUCCESS_TIMEOUT_MS, TimerKind::Check);
    }
}

enum SpawnResult {
    Issued,
    SyncFail,
    AsyncFail,
}

/// `prepend-http`: `^\.*\/` or `^(?!localhost)\w+:` keeps the trimmed source.
fn prepend_http_preserved(url: &str) -> bool {
    let dots = url.bytes().take_while(|&b| b == b'.').count();
    if url[dots..].starts_with('/') {
        return true;
    }
    if url.starts_with("localhost") {
        return false;
    }
    let word = url
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    word > 0 && url.as_bytes().get(word) == Some(&b':')
}

/// The negative lookahead in `prepend-http`'s replacement: `^(?:\w+:)?\/\/`.
fn already_slashed(url: &str) -> bool {
    if url.starts_with("//") {
        return true;
    }
    let word = url
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    word > 0 && url.as_bytes().get(word) == Some(&b':') && url[word + 1..].starts_with("//")
}

fn posix_dirname(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.is_empty() { "." } else { "/" }.to_owned();
    }
    match trimmed.rfind('/') {
        Some(0) => "/".to_owned(),
        Some(i) => trimmed[..i].to_owned(),
        None => ".".to_owned(),
    }
}

fn posix_basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or_default().to_owned()
}

fn posix_extname(path: &str) -> String {
    let base = posix_basename(path);
    let base = base.trim_start_matches('.');
    match base.rfind('.') {
        Some(i) => base[i..].to_owned(),
        None => String::new(),
    }
}

const WINDOWS_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
    "com8", "com9", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// `filenamify` 3.0.0 with the default `!` replacement, on UTF-16 code units.
fn filenamify(name: &str) -> String {
    let reserved = |u: u16| {
        matches!(u, 0..=0x1f)
            || matches!(
                u as u8 as char,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            ) && u < 0x80
    };
    let control = |u: u16| matches!(u, 0..=0x1f | 0x80..=0x9f);
    let units: Vec<u16> = name.encode_utf16().collect();
    let mut out: Vec<u16> = Vec::new();
    let mut leading = true;
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        if reserved(u) || control(u) {
            out.push(u16::from(b'!'));
        } else if leading && u == u16::from(b'.') {
            while i + 1 < units.len() && units[i + 1] == u16::from(b'.') {
                i += 1;
            }
            out.push(u16::from(b'!'));
        } else {
            out.push(u);
        }
        if u != u16::from(b'.') || !leading {
            leading = false;
        }
        i += 1;
    }
    let mut collapsed: Vec<u16> = Vec::new();
    for &u in &out {
        if u == u16::from(b'!') && collapsed.last() == Some(&u16::from(b'!')) {
            continue;
        }
        collapsed.push(u);
    }
    if collapsed.len() > 1 {
        while collapsed.first() == Some(&u16::from(b'!')) {
            collapsed.remove(0);
        }
        while collapsed.last() == Some(&u16::from(b'!')) {
            collapsed.pop();
        }
    }
    let mut s = String::from_utf16_lossy(&collapsed);
    let lowered = JsString::from(s.as_str()).to_lower_case().to_string_lossy();
    if WINDOWS_NAMES.iter().any(|n| *n == lowered) {
        s.push('!');
    }
    let units: Vec<u16> = s.encode_utf16().take(100).collect();
    String::from_utf16_lossy(&units)
}

fn is_token(b: u8) -> bool {
    matches!(b,
        b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'0'..=b'9'
            | b'A'..=b'Z' | b'^' | b'_' | b'`' | b'a'..=b'z' | b'|' | b'~')
}

fn match_param(s: &[u16], at: usize) -> Option<(usize, String, String)> {
    let mut i = at;
    if s.get(i) != Some(&0x3b) {
        return None;
    }
    i += 1;
    while matches!(s.get(i), Some(&0x09) | Some(&0x20)) {
        i += 1;
    }
    let key_start = i;
    while i < s.len() && s[i] < 0x80 && is_token(s[i] as u8) {
        i += 1;
    }
    if i == key_start {
        return None;
    }
    let key = String::from_utf16_lossy(&s[key_start..i]);
    while matches!(s.get(i), Some(&0x09) | Some(&0x20)) {
        i += 1;
    }
    if s.get(i) != Some(&0x3d) {
        return None;
    }
    i += 1;
    while matches!(s.get(i), Some(&0x09) | Some(&0x20)) {
        i += 1;
    }
    let value = if s.get(i) == Some(&0x22) {
        i += 1;
        let mut value = String::new();
        loop {
            match s.get(i) {
                None => return None,
                Some(&0x22) => {
                    i += 1;
                    break;
                }
                Some(&0x5c) => match s.get(i + 1) {
                    Some(&c) if (0x20..=0x7e).contains(&c) => {
                        value.push(char::from_u32(u32::from(c)).unwrap_or('\u{fffd}'));
                        i += 2;
                    }
                    _ => return None,
                },
                Some(&u) if matches!(u, 0x20 | 0x21 | 0x23..=0x5b | 0x5d..=0x7e | 0x80..=0xff) => {
                    value.push(char::from_u32(u32::from(u)).unwrap_or('\u{fffd}'));
                    i += 1;
                }
                _ => return None,
            }
        }
        value
    } else {
        let value_start = i;
        while i < s.len() && s[i] < 0x80 && is_token(s[i] as u8) {
            i += 1;
        }
        if i == value_start {
            return None;
        }
        String::from_utf16_lossy(&s[value_start..i])
    };
    while matches!(s.get(i), Some(&0x09) | Some(&0x20)) {
        i += 1;
    }
    Some((i, key, value))
}

/// `content-disposition` 0.5.4 `parse`: returns the `filename` parameter (the
/// `filename*` form overwrites) or `Err` when the header does not parse.
fn disposition_filename(header: &str) -> Result<Option<String>, ()> {
    let s: Vec<u16> = header.encode_utf16().collect();
    let mut i = 0;
    while i < s.len() && s[i] < 0x80 && is_token(s[i] as u8) {
        i += 1;
    }
    if i == 0 {
        return Err(());
    }
    let mut j = i;
    while matches!(s.get(j), Some(&0x09) | Some(&0x20)) {
        j += 1;
    }
    if j != s.len() && s[j] != 0x3b {
        return Err(());
    }
    let mut index = j;
    let mut names: Vec<String> = Vec::new();
    let mut params: Vec<(String, String)> = Vec::new();
    while let Some((next, key_raw, value)) = match_param(&s, index) {
        index = next;
        let key = JsString::from(key_raw.as_str())
            .to_lower_case()
            .to_string_lossy();
        if names.contains(&key) {
            return Err(());
        }
        names.push(key.clone());
        if key.ends_with('*') {
            let key = key[..key.len() - 1].to_owned();
            let value = decode_ext_value(&value)?;
            if let Some(p) = params.iter_mut().find(|(k, _)| *k == key) {
                p.1 = value;
            } else {
                params.push((key, value));
            }
            continue;
        }
        if params.iter().any(|(k, _)| *k == key) {
            continue;
        }
        params.push((key, value));
    }
    if index != s.len() {
        return Err(());
    }
    Ok(params
        .iter()
        .find(|(k, _)| k == "filename")
        .map(|(_, v)| v.clone()))
}

fn decode_ext_value(value: &str) -> Result<String, ()> {
    let mut parts = value.splitn(3, '\'');
    let charset = JsString::from(parts.next().ok_or(())?)
        .to_lower_case()
        .to_string_lossy();
    if charset.is_empty()
        || !charset.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'+'
                        | b'-'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'{'
                        | b'}'
                        | b'~'
                )
        })
    {
        return Err(());
    }
    let language = parts.next().ok_or(())?;
    let language_ok = language.is_empty()
        || (language
            .bytes()
            .all(|b| b.is_ascii_alphabetic() || b == b'-')
            && language.split('-').all(|seg| (2..=8).contains(&seg.len()))
            && language.len() <= 8);
    if !language_ok {
        return Err(());
    }
    let encoded = parts.next().ok_or(())?;
    if encoded.is_empty()
        || !encoded.bytes().all(|b| {
            b == b'%'
                || b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#'
                        | b'$'
                        | b'&'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
    {
        return Err(());
    }
    let mut bytes = Vec::new();
    let mut chars = encoded.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let h: String = chars.by_ref().take(2).collect();
            if h.len() != 2 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(());
            }
            bytes.push(u8::from_str_radix(&h, 16).map_err(|_| ())?);
        } else {
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    match charset.as_str() {
        "utf-8" => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        "iso-8859-1" => Ok(bytes.iter().map(|&b| char::from(b)).collect()),
        _ => Err(()),
    }
}

fn single_filename(
    headers: &[(String, String)],
    body: &[u8],
    first_href: &str,
) -> Result<String, ()> {
    if let Some((_, header)) = headers.iter().find(|(n, _)| n == "content-disposition")
        && let Some(name) = disposition_filename(header)?
        && !name.is_empty()
    {
        return Ok(name);
    }
    let mut filename = legacy_url::whatwg_basename(first_href).unwrap_or_default();
    if posix_extname(&filename).is_empty()
        && let Some(ext) = infer_ext(body, headers)
    {
        filename = format!("{filename}.{ext}");
    }
    Ok(filename)
}

fn infer_ext(body: &[u8], headers: &[(String, String)]) -> Option<&'static str> {
    if body.starts_with(b"%PDF") {
        return Some("pdf");
    }
    if body.starts_with(&[0x89, 0x50, 0x4e, 0x47]) {
        return Some("png");
    }
    if body.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("jpg");
    }
    if body.starts_with(b"GIF8") {
        return Some("gif");
    }
    match headers
        .iter()
        .find(|(n, _)| n == "content-type")
        .map(|(_, v)| {
            JsString::from(v.split(';').next().unwrap_or_default())
                .trim()
                .to_string_lossy()
        })
        .as_deref()
    {
        Some("application/pdf") => Some("pdf"),
        Some("image/png") => Some("png"),
        Some("image/jpeg") => Some("jpg"),
        Some("image/gif") => Some("gif"),
        Some("image/webp") => Some("webp"),
        Some("image/svg+xml") => Some("svg"),
        _ => None,
    }
}
