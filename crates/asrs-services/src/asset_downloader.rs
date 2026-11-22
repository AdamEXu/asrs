//! `src/main/electron/assetDownloader.ts` and `src/main/github.ts`: the loop that keeps
//! `autoAssets` in sync with the GitHub asset releases, and the status text the About dialog
//! shows. Upstream downloads through the `download` package (got 8 plus `decompress`); the port
//! reproduces what that stack does on the wire and on disk, one HTTP request at a time through
//! [`DownloadHost`].

use crate::Invalid;
use asrs_js::json::parse as json_parse;
use asrs_js::string::{JsString, decode_uri_component};
use asrs_js::time::{LocalZone, to_locale_time_string};
use asrs_js::value::JsValue;

use crate::asset_loader::AssetFolders;
use crate::download_deps::{
    disposition_filename, ext_name_mime, filenamify, url_parse_lax, url_path_basename,
};
use crate::file_type::{Guess, archive_type, file_type_11};
use crate::fs::Fs;
use crate::legacy_url::{self, LegacyUrl};
use crate::path::PathStyle;
use crate::zip::{self, EntryKind, ZipEntry};

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

    fn dirname(&self, path: &str) -> String {
        self.style.dirname(&JsString::from(path)).to_string_lossy()
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
        let wanted: Vec<&AssetInfo> = infos
            .iter()
            .filter(|info| !info.optional && !fs.exists(&info.target))
            .collect();
        for info in wanted {
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
        let source = if source.to_string_lossy().starts_with("unix:") {
            let mut units = JsString::from("http://").units().to_vec();
            units.extend_from_slice(source.units());
            JsString::from_units(units)
        } else {
            source.clone()
        };
        if decode_uri_component(&source).is_err() {
            return SpawnResult::SyncFail;
        }
        let Ok(parsed) = url_parse_lax(&source) else {
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
                        Err(Invalid) => self.fail_download(di),
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
        let filename = match self.filename(headers, body, &first_href) {
            Err(Invalid) => {
                self.fail_download(di);
                return;
            }
            Ok(None) => return,
            Ok(Some(filename)) => filename,
        };
        let output_file = self.join(&download_path, &filenamify(&filename));
        let output = self.dirname(&output_file);
        let saved = match archive_type(body) {
            None => fs.mkdir_all(&output).is_ok() && fs.write(&output_file, body).is_ok(),
            Some("zip") => match zip::decompress_unzip(body) {
                Err(Invalid) => false,
                Ok(entries) => entries
                    .iter()
                    .map(|entry| self.extract_entry(fs, &output, entry).is_ok())
                    // Not `all`: decompress writes every entry even after one fails.
                    .fold(true, |all, ok| all & ok),
            },
            // decompress has no plugin for these, so it writes nothing and succeeds.
            Some("7z" | "rar" | "xz") => true,
            // tar, gz and bz2 extraction is not ported (#220).
            Some(_) => false,
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

    /// download's `getFilename`: `Ok(None)` when file-type's ASF walk never returns.
    fn filename(
        &self,
        headers: &[(String, String)],
        body: &[u8],
        request_url: &str,
    ) -> Result<Option<String>, Invalid> {
        let header = |name: &str| {
            headers
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
        };
        if let Some(disposition) = header("content-disposition").filter(|h| !h.is_empty())
            && let Some(name) = disposition_filename(disposition)?
            && !name.is_empty()
        {
            return Ok(Some(name));
        }
        let mut filename = url_path_basename(request_url, self.style)?;
        if self
            .style
            .extname(&JsString::from(filename.as_str()))
            .units()
            .is_empty()
        {
            let ext = match file_type_11(body) {
                Guess::Stalls => return Ok(None),
                Guess::Ext(ext) => Some(ext),
                Guess::Unknown => header("content-type")
                    .filter(|h| !h.is_empty())
                    .map(ext_name_mime)
                    .and_then(|exts| (exts.len() == 1).then(|| exts[0])),
            };
            if let Some(ext) = ext {
                filename = format!("{filename}.{ext}");
            }
        }
        Ok(Some(filename))
    }

    /// One entry of decompress 4.2's `extractFile`, with its guards against leaving the output
    /// folder through symlinks. Symlink entries themselves are not ported and fail.
    fn extract_entry(&self, fs: &dyn Fs, output: &str, entry: &ZipEntry) -> Result<(), Invalid> {
        let dest = self.join(output, &entry.name);
        fs.mkdir_all(output).map_err(|_| Invalid)?;
        let real_output = fs.realpath(output).map_err(|_| Invalid)?;
        if entry.kind == EntryKind::Directory {
            self.safe_make_dir(fs, &dest, &real_output)?;
            return Ok(());
        }
        let parent = self.dirname(&dest);
        self.safe_make_dir(fs, &parent, &real_output)?;
        if entry.kind == EntryKind::File && fs.readlink(&dest).is_ok() {
            return Err(Invalid);
        }
        let real_parent = fs.realpath(&parent).map_err(|_| Invalid)?;
        if !real_parent.starts_with(&real_output) || entry.kind == EntryKind::Symlink {
            return Err(Invalid);
        }
        fs.write(&dest, &entry.data).map_err(|_| Invalid)
    }

    fn safe_make_dir(&self, fs: &dyn Fs, dir: &str, real_output: &str) -> Result<String, Invalid> {
        let real_parent = match fs.realpath(dir) {
            Ok(real) => real,
            Err(_) => {
                let parent = self.dirname(dir);
                if parent == dir {
                    return Err(Invalid);
                }
                self.safe_make_dir(fs, &parent, real_output)?
            }
        };
        if !real_parent.starts_with(real_output) {
            return Err(Invalid);
        }
        fs.mkdir_all(dir).map_err(|_| Invalid)?;
        fs.realpath(dir).map_err(|_| Invalid)
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
