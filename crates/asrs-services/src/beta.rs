//! `src/main/electron/betaUtil.ts` and `src/main/betaConfig.ts`: the `beta-2027.json` state
//! file and the survey-scheduling rules. `BETA_FILENAME` stays in `constants`.

use asrs_js::math;
use asrs_js::ops;
use asrs_js::string::{JsError, JsString};
use asrs_js::time::{local_date, Clock, LocalZone};
use asrs_js::value::JsValue;
use asrs_model::json_file;

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

/// `getBetaConfig()` — upstream's config is a compile-time constant per distribution.
pub fn beta_config(is_beta: bool, zone: &impl LocalZone) -> Option<BetaConfig> {
    is_beta.then(|| BetaConfig {
        year: "2027",
        is_alpha: true,
        expiration_ms: local_date(&[2027.0, 0.0, 9.0], zone),
        survey_url: None,
    })
}

/// `DEFAULT_STATE` / `DEFAULT_ALPHA_STATE` both reduce to this.
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
    ops::set_key(&mut state, &JsString::from("welcomeComplete"), JsValue::Bool(true))?;
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
        let Some(survey_url) = config.and_then(|c| c.survey_url) else {
            return Ok(false);
        };
        let _ = survey_url;
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
        return Ok(clock.now() > scheduled);
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
    ops::set_key(&mut state, &JsString::from("surveyStatus"), JsValue::Bool(true))?;
    fs.write(path, &json_file::stringify(&state)?)
}
