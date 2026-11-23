//! `src/main/electron/UpdateChecker.ts` and the `checkForUpdate`, `prompt-update`,
//! `open-feedback` and hub dom-ready button glue around it in `src/main/electron/main.ts`.

use asrs_js::json;
use asrs_js::ops;
use asrs_js::string::{JsString, response_text};
use asrs_js::time::{LocalZone, date_from_value, format_date};
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
    /// Where "Contact Us" sends mail; without an address it opens the repository's issues page.
    pub contact_email: Option<&'static str>,
}

impl ReleaseProfile {
    pub const PARITY: ReleaseProfile = ReleaseProfile {
        repository: "Mechanical-Advantage/AdvantageScope",
        update_check: true,
        beta_survey: true,
        contact_email: Some("software@team6328.org"),
    };

    pub const RELEASE: ReleaseProfile = ReleaseProfile {
        repository: "AdamEXu/asrs",
        update_check: false,
        beta_survey: false,
        contact_email: None,
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
pub struct UpdateChecker {
    should_prompt: bool,
    alert_message: JsString,
    alert_detail: JsString,
    alert_options: Option<Vec<&'static str>>,
    alert_cancel_id: Option<f64>,
    latest_version: JsString,
    /// `browser_download_url` verbatim (any JSON value); `Null` is upstream's "not set".
    alert_download_url: JsValue,
}

impl Default for UpdateChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdateChecker {
    /// The field initialisers ("Update check not complete").
    pub fn new() -> UpdateChecker {
        UpdateChecker {
            should_prompt: false,
            alert_message: JsString::from("Update check not complete"),
            alert_detail: JsString::from("Checking for update information. Please try again."),
            alert_options: None,
            alert_cancel_id: None,
            latest_version: JsString::new(),
            alert_download_url: JsValue::Null,
        }
    }

    /// The info state every early return writes.
    fn info(&mut self, message: &str, detail: JsString) {
        self.should_prompt = false;
        self.alert_options = None;
        self.alert_cancel_id = None;
        self.alert_download_url = JsValue::Null;
        self.alert_message = JsString::from(message);
        self.alert_detail = detail;
    }

    /// `check()`. `fetch` is called at most once, only for packaged builds.
    pub fn check(
        &mut self,
        env: &UpdateEnv<'_>,
        zone: &impl LocalZone,
        fetch: impl FnOnce(&FetchRequest) -> FetchReply,
    ) -> CheckOutcome {
        if !env.packaged {
            self.info(
                "Cannot check for updates",
                JsString::from("This app is running in a development environment."),
            );
            return CheckOutcome::Resolved;
        }

        let request = FetchRequest {
            url: format!("https://api.github.com/repos/{}/releases", env.repository),
            method: "GET",
            headers: vec![("pragma", "no-cache"), ("cache-control", "no-cache")],
        };
        let data = match fetch(&request) {
            FetchReply::Response { body, .. } => json::parse(&response_text(&body)).ok(),
            FetchReply::Rejected => None,
        };
        let Some(data) = data else {
            self.info(
                "Cannot check for updates",
                JsString::from(
                    "Failed to retrieve update information from GitHub. Please check your internet connection and try again.",
                ),
            );
            return CheckOutcome::Resolved;
        };

        match self.read_releases(env, zone, data) {
            Some(()) => CheckOutcome::Resolved,
            None => CheckOutcome::Rejected,
        }
    }

    /// Everything after upstream's fetch `try`; `None` is the `TypeError` path, which keeps the
    /// state already written.
    fn read_releases(
        &mut self,
        env: &UpdateEnv<'_>,
        zone: &impl LocalZone,
        mut data: JsValue,
    ) -> Option<()> {
        if !env.is_beta {
            let mut kept = Vec::new();
            for release in data.as_array()? {
                let prerelease = ops::get(release, "prerelease").ok()?;
                if !ops::to_boolean(&prerelease) {
                    kept.push(release.clone());
                }
            }
            data = JsValue::Array(kept);
        }

        let current_major = env.version.split('.').next().unwrap_or("");
        let current_major = JsString::from(current_major);
        let dot = JsString::from(".");
        let mut releases = Vec::new();
        for release in data.as_array()? {
            let tag = ops::get(release, "tag_name").ok()?;
            let JsValue::String(tag) = &tag else {
                return None;
            };
            if tag.substring(1.0, None).split(Some(&dot), None).first() == Some(&current_major) {
                releases.push(release.clone());
            }
        }
        if releases.is_empty() {
            let app = env.app_name;
            self.info(
                "No update data available",
                JsString::from(format!(
                    "No update information was found for the installed version of {app}. Please check the {app} GitHub repository to download the latest release."
                )),
            );
            return Some(());
        }

        let latest = &releases[0];
        let tag = ops::get(latest, "tag_name").ok()?;
        let JsValue::String(tag) = &tag else {
            return None;
        };
        self.latest_version = tag.slice(1.0, None);
        let published = ops::get(latest, "published_at").ok()?;
        let date_text = format_date(date_from_value(&published, zone).ok()?, zone);
        let translated = env.arch != "arm64" && env.running_under_arm64_translation;

        self.should_prompt = true;
        self.alert_options = Some(if env.platform == "darwin" {
            vec!["Download", "Later", "View Changelog"]
        } else {
            vec!["Download", "View Changelog", "Later"]
        });
        self.alert_cancel_id = Some(if env.platform == "darwin" { 1.0 } else { 2.0 });
        self.alert_download_url = JsValue::Null;

        let differs = self.latest_version != JsString::from(env.version);
        if differs && translated {
            self.alert_message = JsString::from("Download latest native version?");
            self.alert_detail = join_js(&[
                &JsString::from("Version "),
                &self.latest_version,
                &JsString::from(" is available (released "),
                &JsString::from(date_text),
                &JsString::from("). You're currently running the x86 build of version "),
                &JsString::from(env.version),
                &JsString::from(
                    " on an arm64 platform. Would you like to download the latest native version?",
                ),
            ]);
        } else if differs {
            self.alert_message = JsString::from("Download latest version?");
            self.alert_detail = join_js(&[
                &JsString::from("Version "),
                &self.latest_version,
                &JsString::from(" is available (released "),
                &JsString::from(date_text),
                &JsString::from("). You're currently running version "),
                &JsString::from(env.version),
                &JsString::from(". Would you like to download the latest version?"),
            ]);
        } else if translated {
            self.alert_message = JsString::from("Download native version?");
            self.alert_detail = JsString::from(
                "It looks like you're running the x86 version of this app on an arm64 platform. Would you like to download the native version?",
            );
        } else {
            self.info(
                "No updates available",
                join_js(&[
                    &JsString::from("You're currently running version "),
                    &JsString::from(env.version),
                    &JsString::from(" (released "),
                    &JsString::from(date_text),
                    &JsString::from(")."),
                ]),
            );
            return Some(());
        }

        let platform_key = match env.platform {
            "win32" => "win",
            "linux" => "linux",
            "darwin" => "mac",
            _ => "",
        };
        let arch = if translated { "arm64" } else { env.arch };
        let assets = ops::get(latest, "assets").ok()?;
        let JsValue::Array(assets) = &assets else {
            return None;
        };
        let mut matches = Vec::new();
        for asset in assets.iter() {
            let name = ops::get(asset, "name").ok()?;
            let hit = match &name {
                JsValue::String(name) => {
                    name.includes(&JsString::from(platform_key), None)
                        && name.includes(&JsString::from(arch), None)
                }
                JsValue::Array(parts) => {
                    parts
                        .iter()
                        .any(|p| ops::same_value_zero(p, &JsValue::string(platform_key)))
                        && parts
                            .iter()
                            .any(|p| ops::same_value_zero(p, &JsValue::string(arch)))
                }
                _ => return None,
            };
            if hit {
                matches.push(asset);
            }
        }
        if matches.len() == 1 {
            self.alert_download_url = ops::get(matches[0], "browser_download_url").ok()?;
        }
        Some(())
    }

    /// `getShouldPrompt()`.
    pub fn should_prompt(&self) -> bool {
        self.should_prompt
    }

    /// The box `showPrompt()` shows.
    pub fn prompt_box(&self) -> MessageBox {
        let base = MessageBox {
            kind: "info",
            title: "Update Checker",
            message: self.alert_message.clone(),
            detail: self.alert_detail.clone(),
            buttons: None,
            default_id: None,
            cancel_id: None,
            icon: true,
            modal: false,
        };
        match (&self.alert_options, self.alert_cancel_id) {
            (Some(options), Some(cancel)) => MessageBox {
                kind: "question",
                buttons: Some(options.clone()),
                default_id: Some(0.0),
                cancel_id: Some(cancel),
                ..base
            },
            _ => base,
        }
    }

    /// The rest of `showPrompt()` once the box resolves with `response`: the value handed to
    /// `shell.openExternal`, if any (any JSON value a release carried as
    /// `browser_download_url`, `undefined` included).
    pub fn prompt_response(&mut self, env: &UpdateEnv<'_>, response: usize) -> Option<JsValue> {
        let (Some(options), Some(_)) = (&self.alert_options, self.alert_cancel_id) else {
            return None;
        };
        match options.get(response).copied() {
            Some("Download") => {
                if self.alert_download_url == JsValue::Null {
                    let mut url =
                        JsString::from(format!("https://github.com/{}/releases/v", env.repository));
                    url.push_str(&self.latest_version);
                    self.alert_download_url = JsValue::String(url);
                }
                Some(self.alert_download_url.clone())
            }
            Some("View Changelog") => Some(JsValue::string(&format!(
                "https://github.com/{}/releases",
                env.repository
            ))),
            _ => None,
        }
    }

    /// main.ts `checkForUpdate(alwaysPrompt)`.
    pub fn check_for_update(
        &mut self,
        env: &UpdateEnv<'_>,
        zone: &impl LocalZone,
        always_prompt: bool,
        fetch: impl FnOnce(&FetchRequest) -> FetchReply,
    ) -> CheckForUpdate {
        match self.check(env, zone, fetch) {
            CheckOutcome::Rejected => CheckForUpdate {
                button: None,
                prompt: None,
            },
            CheckOutcome::Resolved => CheckForUpdate {
                button: Some(self.should_prompt()),
                prompt: always_prompt.then(|| self.prompt_box()),
            },
        }
    }

    /// The two button messages a hub's dom-ready sends, in order:
    /// `("show-update-button", getShouldPrompt())`, `("show-feedback-button", isBeta())`.
    pub fn hub_ready_messages(&self, beta: Option<&BetaConfig>) -> [(&'static str, bool); 2] {
        [
            ("show-update-button", self.should_prompt()),
            ("show-feedback-button", beta.is_some()),
        ]
    }
}

fn join_js(parts: &[&JsString]) -> JsString {
    let mut out = JsString::new();
    for part in parts {
        out.push_str(part);
    }
    out
}

/// The URL the hub's "open-feedback" message opens.
pub fn feedback_url(repository: &str) -> String {
    format!("https://github.com/{repository}/issues/new/choose")
}
