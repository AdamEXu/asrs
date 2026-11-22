//! `src/main/electron/betaUtil.ts` and `src/main/betaConfig.ts`: the `beta-2027.json` state
//! file and the survey-scheduling rules. `BETA_FILENAME` stays in `constants`.

use asrs_js::math;
use asrs_js::ops;
use asrs_js::string::{JsError, JsString};
use asrs_js::time::{Clock, LocalZone, local_date, time_clip};
use asrs_js::value::JsValue;
use asrs_model::build_constants::Distribution;
use asrs_model::json_file;

use crate::dialog::MessageBox;
use crate::fs::Fs;

/// `BETA_CONFIG` for this release, or `None` on non-beta builds.
#[derive(Clone)]
pub struct BetaConfig {
    pub year: &'static str,
    pub is_alpha: bool,
    /// `expiration.getTime()`; `new Date(2027, 0, 9)` is local time.
    pub expiration_ms: f64,
    pub survey_url: Option<&'static str>,
}

/// `BETA_CONFIG` from `betaConfig.ts`; `None` stands for a release build's `null`.
pub fn beta_config(is_beta: bool, zone: &impl LocalZone) -> Option<BetaConfig> {
    is_beta.then(|| BetaConfig {
        year: "2027",
        is_alpha: true,
        expiration_ms: local_date(&[2027.0, 0.0, 9.0], zone),
        survey_url: None,
    })
}

/// `structuredClone(DefaultBetaState)`.
fn default_state() -> JsValue {
    ops::object([
        ("welcomeComplete", JsValue::Bool(false)),
        ("surveyStatus", JsValue::Bool(false)),
    ])
}

/// `getState()`: `null` outside beta builds, else defaults merged with the file via
/// `Object.assign` (parse failures propagate).
pub fn get_state(fs: &dyn Fs, beta_file: Option<&str>) -> Result<JsValue, JsError> {
    let Some(path) = beta_file else {
        return Ok(JsValue::Null);
    };
    let mut state = default_state();
    if fs.exists(path) {
        let file_state = json_file::parse(&fs.read(path)?)?;
        for (k, v) in ops::spread(&file_state) {
            ops::set_key(&mut state, &k, v.clone())?;
        }
    }
    Ok(state)
}

/// `isBetaWelcomeComplete()`: `true` outside beta builds.
pub fn is_beta_welcome_complete(fs: &dyn Fs, beta_file: Option<&str>) -> Result<JsValue, JsError> {
    let state = get_state(fs, beta_file)?;
    if state.is_null() {
        return Ok(JsValue::Bool(true));
    }
    ops::get(&state, "welcomeComplete")
}

/// `saveBetaWelcomeComplete()`.
pub fn save_beta_welcome_complete(fs: &dyn Fs, beta_file: Option<&str>) -> Result<(), JsError> {
    let Some(path) = beta_file else { return Ok(()) };
    let mut state = get_state(fs, beta_file)?;
    if state.is_null() {
        return Ok(());
    }
    ops::set_key(
        &mut state,
        &JsString::from("welcomeComplete"),
        JsValue::Bool(true),
    )?;
    fs.write(path, &json_file::stringify(&state)?)
}

/// `isBetaExpired()`.
pub fn is_beta_expired(config: Option<&BetaConfig>, clock: &dyn Clock) -> bool {
    config.is_some_and(|c| clock.now() > c.expiration_ms)
}

const TWO_WEEKS_MS: f64 = 14.0 * 24.0 * 60.0 * 60.0 * 1000.0;
const TWO_DAYS_MS: f64 = 2.0 * 24.0 * 60.0 * 60.0 * 1000.0;

/// `shouldPromptBetaSurvey()`.
pub fn should_prompt_beta_survey(
    fs: &dyn Fs,
    beta_file: Option<&str>,
    config: Option<&BetaConfig>,
    clock: &dyn Clock,
) -> Result<bool, JsError> {
    let Some(path) = beta_file else {
        return Ok(false);
    };
    let mut state = get_state(fs, beta_file)?;
    if state.is_null() {
        return Ok(false);
    }
    let survey_status = ops::get(&state, "surveyStatus")?;
    if survey_status == JsValue::Bool(true) {
        return Ok(false);
    }
    if survey_status == JsValue::Bool(false) {
        if config.and_then(|c| c.survey_url).is_none() {
            return Ok(false);
        }
        let survey_time = math::round(clock.now() + TWO_WEEKS_MS);
        let expiration = config.map_or(f64::NAN, |c| c.expiration_ms);
        if survey_time < expiration {
            ops::set_key(
                &mut state,
                &JsString::from("surveyStatus"),
                JsValue::Number(survey_time),
            )?;
            fs.write(path, &json_file::stringify(&state)?)?;
        }
        return Ok(false);
    }
    if let JsValue::Number(scheduled) = survey_status {
        return Ok(clock.now() > time_clip(scheduled));
    }
    Ok(false)
}

/// `delayBetaSurvey()`.
pub fn delay_beta_survey(
    fs: &dyn Fs,
    beta_file: Option<&str>,
    clock: &dyn Clock,
) -> Result<(), JsError> {
    let Some(path) = beta_file else { return Ok(()) };
    let mut state = get_state(fs, beta_file)?;
    if state.is_null() {
        return Ok(());
    }
    if matches!(ops::get(&state, "surveyStatus")?, JsValue::Number(_)) {
        ops::set_key(
            &mut state,
            &JsString::from("surveyStatus"),
            JsValue::Number(math::round(clock.now() + TWO_DAYS_MS)),
        )?;
        fs.write(path, &json_file::stringify(&state)?)?;
    }
    Ok(())
}

/// `openBetaSurvey()`; `open` is `shell.openExternal`, called before the file write.
pub fn open_beta_survey(
    fs: &dyn Fs,
    beta_file: Option<&str>,
    config: Option<&BetaConfig>,
    open: impl FnOnce(&str),
) -> Result<(), JsError> {
    let Some(path) = beta_file else { return Ok(()) };
    let mut state = get_state(fs, beta_file)?;
    if state.is_null() {
        return Ok(());
    }
    let Some(url) = config.and_then(|c| c.survey_url) else {
        return Ok(());
    };
    open(url);
    ops::set_key(
        &mut state,
        &JsString::from("surveyStatus"),
        JsValue::Bool(true),
    )?;
    fs.write(path, &json_file::stringify(&state)?)
}

/// What a hub's dom-ready does in a beta build (main.ts, after the button messages).
#[derive(Clone, Debug, PartialEq)]
pub enum BetaStartup {
    Nothing,
    /// The "is complete" alert; answer with [`beta_expired_response`].
    Expired(MessageBox),
    /// `openBetaWelcome(window)`: see [`beta_welcome_window`].
    Welcome,
    /// The "Share feedback?" prompt; answer with [`beta_survey_response`].
    Survey(MessageBox),
}

/// The beta block of a hub's dom-ready handler. Errors are what `getState()` throws on an
/// unreadable beta file (the handler throws out of dom-ready).
pub fn beta_startup(
    fs: &dyn Fs,
    beta_file: Option<&str>,
    config: Option<&BetaConfig>,
    clock: &dyn Clock,
    distribution: Distribution,
    app_name: &str,
) -> Result<BetaStartup, JsError> {
    let Some(config) = config else {
        return Ok(BetaStartup::Nothing);
    };
    if is_beta_expired(Some(config), clock) {
        let mut detail = JsString::from("The ");
        detail.push_str(&JsString::from(app_name));
        detail.push_str(&JsString::from(if config.is_alpha {
            " alpha is complete. "
        } else {
            " beta is complete. "
        }));
        detail.push_str(&JsString::from(if distribution == Distribution::WpiLib {
            "Please update to the latest stable release of WPILib.".to_owned()
        } else {
            format!("Please download the latest stable release of {app_name} from GitHub.")
        }));
        return Ok(BetaStartup::Expired(MessageBox {
            kind: "info",
            title: "Alert",
            message: JsString::from(if config.is_alpha {
                "Alpha is complete"
            } else {
                "Beta is complete"
            }),
            detail,
            buttons: Some(vec!["Quit", "Ignore"]),
            default_id: Some(0.0),
            cancel_id: None,
            icon: false,
            modal: true,
        }));
    }
    if !ops::to_boolean(&is_beta_welcome_complete(fs, beta_file)?) {
        return Ok(BetaStartup::Welcome);
    }
    if should_prompt_beta_survey(fs, beta_file, Some(config), clock)? {
        let mut detail =
            JsString::from("Please take 30 seconds to share your experience using the ");
        detail.push_str(&JsString::from(app_name));
        detail.push_str(&JsString::from(if config.is_alpha {
            " alpha. We'll keep this quick."
        } else {
            " beta. We'll keep this quick."
        }));
        return Ok(BetaStartup::Survey(MessageBox {
            kind: "info",
            title: "Alert",
            message: JsString::from("Share feedback?"),
            detail,
            buttons: Some(vec!["Open", "Not Now"]),
            default_id: Some(0.0),
            cancel_id: None,
            icon: false,
            modal: true,
        }));
    }
    Ok(BetaStartup::Nothing)
}

/// The expired alert resolved with `response`: whether main calls `app.quit()`.
pub fn beta_expired_response(response: usize) -> bool {
    response == 0
}

/// The survey prompt resolved with `response`: `openBetaSurvey()` or `delayBetaSurvey()`.
pub fn beta_survey_response(
    fs: &dyn Fs,
    beta_file: Option<&str>,
    config: Option<&BetaConfig>,
    clock: &dyn Clock,
    response: usize,
    open: impl FnOnce(&str),
) -> Result<(), JsError> {
    if response == 0 {
        open_beta_survey(fs, beta_file, config, open)
    } else {
        delay_beta_survey(fs, beta_file, clock)
    }
}

/// The `BrowserWindow` options `openBetaWelcome` passes (it is always modal to the hub and
/// created hidden).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WelcomeWindow {
    pub width: f64,
    pub height: f64,
    pub use_content_size: bool,
    pub resizable: bool,
    pub modal: bool,
    pub show: bool,
}

/// `openBetaWelcome`'s window options on `platform` (`process.platform`).
pub fn beta_welcome_window(platform: &str) -> WelcomeWindow {
    WelcomeWindow {
        width: 450.0,
        height: if platform == "win32" { 530.0 } else { 490.0 },
        use_content_size: true,
        resizable: false,
        modal: true,
        show: false,
    }
}

/// The welcome page's dom-ready: the value main posts on the new port (`isAlpha()`).
pub fn beta_welcome_ready(config: Option<&BetaConfig>) -> bool {
    config.is_some_and(|c| c.is_alpha)
}

/// Any message from the welcome page: `saveBetaWelcomeComplete()`, then
/// `shouldPromptBetaSurvey()` for its side effect of scheduling the survey.
pub fn beta_welcome_done(
    fs: &dyn Fs,
    beta_file: Option<&str>,
    config: Option<&BetaConfig>,
    clock: &dyn Clock,
) -> Result<(), JsError> {
    save_beta_welcome_complete(fs, beta_file)?;
    should_prompt_beta_survey(fs, beta_file, config, clock)?;
    Ok(())
}

/// The welcome window's "close" event: whether main calls `app.quit()`.
pub fn beta_welcome_closed() -> bool {
    true
}
