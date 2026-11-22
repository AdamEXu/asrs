//! `src/main/electron/UpdateChecker.ts` and the `checkForUpdate`, `prompt-update`,
//! `open-feedback` and hub dom-ready button glue around it in `src/main/electron/main.ts`.

use asrs_js::string::JsString;
use asrs_js::time::LocalZone;
use asrs_js::value::JsValue;

use crate::beta::BetaConfig;
use crate::dialog::MessageBox;

/// What differs between a `parity` build and an asrs release (PLAN note 38). Parity builds keep
/// upstream's repository and flows so the vectors compare; release builds query the public asrs
/// repository, keep the update check off until that repository has a release, and never run the
/// beta survey flow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReleaseProfile {
    /// `GITHUB_REPOSITORY` for the releases API, download, changelog and feedback URLs.
    pub repository: &'static str,
    /// Whether the startup check, "Check for Updates..." and the update button run at all.
    pub update_check: bool,
    /// Whether a hub's dom-ready runs the beta block (expired alert, welcome, survey prompt).
    pub beta_survey: bool,
}

impl ReleaseProfile {
    pub const PARITY: ReleaseProfile = ReleaseProfile {
        repository: "Mechanical-Advantage/AdvantageScope",
        update_check: true,
        beta_survey: true,
    };

    pub const RELEASE: ReleaseProfile = ReleaseProfile {
        repository: "AdamEXu/asrs",
        update_check: false,
        beta_survey: false,
    };
}

/// The process and app facts `check()` and `showPrompt()` read.
#[derive(Clone, Copy, Debug)]
pub struct UpdateEnv<'a> {
    /// `process.platform`: `"darwin"`, `"linux"` or `"win32"`.
    pub platform: &'a str,
    /// `process.arch`.
    pub arch: &'a str,
    /// `app.runningUnderARM64Translation`.
    pub running_under_arm64_translation: bool,
    /// `app.isPackaged`.
    pub packaged: bool,
    /// `app.getVersion()`.
    pub version: &'a str,
    /// `isBeta()`.
    pub is_beta: bool,
    /// `GITHUB_REPOSITORY`.
    pub repository: &'a str,
    /// The app name presentation strings carry (`AppIdentity::name`): upstream hardcodes
    /// "AdvantageScope".
    pub app_name: &'a str,
}

/// The `net.fetch` call `check()` makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchRequest {
    pub url: String,
    pub method: &'static str,
    /// In upstream's key order.
    pub headers: Vec<(&'static str, &'static str)>,
}

/// How the fetch settled: rejected (network failure), or a response whose body `check()` reads
/// with `response.json()`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchReply {
    Rejected,
    Response { status: u16, body: Vec<u8> },
}

/// Whether `check()` resolved or rejected. A rejection is a `TypeError` thrown outside the
/// fetch `try` (a body that is not a release list, a release without a string `tag_name`, a
/// release without an `assets` array, ...); state written before the throw stays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckOutcome {
    Resolved,
    Rejected,
}

/// main.ts `checkForUpdate(alwaysPrompt)` after the check settles: `button` is the
/// "show-update-button" value sent to every hub (`None` when the check rejected and the
/// `.then` never ran), `prompt` the box `showPrompt()` shows when `alwaysPrompt`.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckForUpdate {
    pub button: Option<bool>,
    pub prompt: Option<MessageBox>,
}

/// One `UpdateChecker` instance (main.ts keeps a single one for the app's lifetime).
#[derive(Clone, Debug)]
pub struct UpdateChecker {}

impl Default for UpdateChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdateChecker {
    /// The field initialisers ("Update check not complete").
    pub fn new() -> UpdateChecker {
        todo!("P07: UpdateChecker field initialisers")
    }

    /// `check()`. `fetch` is called at most once, only for packaged builds.
    pub fn check(
        &mut self,
        env: &UpdateEnv<'_>,
        zone: &impl LocalZone,
        fetch: impl FnOnce(&FetchRequest) -> FetchReply,
    ) -> CheckOutcome {
        let _ = (env, zone, fetch);
        todo!("P07: UpdateChecker.check")
    }

    /// `getShouldPrompt()`.
    pub fn should_prompt(&self) -> bool {
        todo!("P07: UpdateChecker.getShouldPrompt")
    }

    /// The box `showPrompt()` shows.
    pub fn prompt_box(&self) -> MessageBox {
        todo!("P07: UpdateChecker.showPrompt box")
    }

    /// The rest of `showPrompt()` once the box resolves with `response`: the value handed to
    /// `shell.openExternal`, if any (any JSON value a release carried as
    /// `browser_download_url`, `undefined` included).
    pub fn prompt_response(&mut self, env: &UpdateEnv<'_>, response: usize) -> Option<JsValue> {
        let _ = (env, response);
        todo!("P07: UpdateChecker.showPrompt response")
    }

    /// main.ts `checkForUpdate(alwaysPrompt)`.
    pub fn check_for_update(
        &mut self,
        env: &UpdateEnv<'_>,
        zone: &impl LocalZone,
        always_prompt: bool,
        fetch: impl FnOnce(&FetchRequest) -> FetchReply,
    ) -> CheckForUpdate {
        let _ = (env, zone, always_prompt, fetch);
        todo!("P07: main.ts checkForUpdate")
    }

    /// The two button messages a hub's dom-ready sends, in order:
    /// `("show-update-button", getShouldPrompt())`, `("show-feedback-button", isBeta())`.
    pub fn hub_ready_messages(&self, beta: Option<&BetaConfig>) -> [(&'static str, bool); 2] {
        let _ = beta;
        todo!("P07: dom-ready button messages")
    }
}

/// The URL the hub's "open-feedback" message opens.
pub fn feedback_url(repository: &str) -> String {
    let _ = repository;
    todo!("P07: open-feedback URL")
}
