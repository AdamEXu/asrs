//! `src/main/electron/assetDownloader.ts` and `src/main/github.ts`: the loop that keeps
//! `autoAssets` in sync with the GitHub asset releases, and the status text the About dialog
//! shows. Upstream downloads through the `download` package (got 8 plus `decompress`); the port
//! reproduces what that stack does on the wire and on disk, one HTTP request at a time through
//! [`DownloadHost`].

use asrs_js::time::LocalZone;

use crate::asset_loader::AssetFolders;
use crate::fs::Fs;

pub const GITHUB_REPOSITORY: &str = "Mechanical-Advantage/AdvantageScope";
pub const GITHUB_ASSETS_REPOSITORY: &str = "Mechanical-Advantage/AdvantageScopeAssets";
pub const GITHUB_ASSET_TAG_DEFAULT: &str = "default-assets-v2";
pub const GITHUB_ASSET_TAG_FRC6328: &str = "frc-6328-assets-v2";

pub const REQUIRED_SPACE_GB: f64 = 8.0;
pub const FAILURE_TIMEOUT_MS: f64 = 30.0 * 1000.0;
pub const SUCCESS_TIMEOUT_MS: f64 = 6.0 * 60.0 * 60.0 * 1000.0;

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

/// The module state of `assetDownloader.ts` plus the work it has in flight: the check loop,
/// downloads waiting on a retry or a stalled request, and the pending timers.
pub struct AssetDownloader {}

impl AssetDownloader {
    pub fn new(folders: &AssetFolders, distribution: Distribution) -> AssetDownloader {
        let _ = (folders, distribution);
        todo!("A02b: AssetDownloader::new")
    }

    /// `getAssetDownloadStatus()`.
    pub fn status(&self) -> &str {
        todo!("A02b: AssetDownloader::status")
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
        let _ = (now_ms, fs, host, zone);
        todo!("A02b: AssetDownloader::start")
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
        let _ = (to_ms, fs, host, zone);
        todo!("A02b: AssetDownloader::advance")
    }

    /// Every pending timer, in any order.
    pub fn timers(&self) -> Vec<Timer> {
        todo!("A02b: AssetDownloader::timers")
    }
}
