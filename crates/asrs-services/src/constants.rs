//! Persisted file locations and window defaults from `main/electron/ElectronConstants.ts` and
//! `main.ts`. `Fs` paths are `path.join` results in the style of the platform being modelled.

use asrs_js::string::JsString;
use asrs_js::time::{LocalZone, format_date};
use asrs_js::value::{JsObject, JsValue};

use crate::path::PathStyle;

pub const HUB_DEFAULT_WIDTH: f64 = 1100.0;
pub const HUB_DEFAULT_HEIGHT: f64 = 650.0;
pub const SATELLITE_DEFAULT_WIDTH: f64 = 900.0;
pub const SATELLITE_DEFAULT_HEIGHT: f64 = 500.0;
pub const SAVE_PERIOD_MS: f64 = 250.0;
pub const MAX_RECENT_UNITS: usize = 5;
pub const AKIT_PATH_INPUT_PERIOD_MS: f64 = 250.0;

fn join(style: PathStyle, dir: &str, name: &str) -> String {
    style
        .join(&[&JsString::from(dir), &JsString::from(name)])
        .to_string_lossy()
}

/// `path.join(app.getPath("userData"), "prefs.json")`.
pub fn prefs_filename(style: PathStyle, user_data: &str) -> String {
    join(style, user_data, "prefs.json")
}

/// `path.join(app.getPath("userData"), \`state-${APP_VERSION.replaceAll(".", "_")}.json\`)`.
pub fn state_filename(style: PathStyle, user_data: &str, app_version: &str) -> String {
    join(
        style,
        user_data,
        &format!("state-{}.json", app_version.replace('.', "_")),
    )
}

/// `path.join(app.getPath("userData"), "type-memory.json")`.
pub fn type_memory_filename(style: PathStyle, user_data: &str) -> String {
    join(style, user_data, "type-memory.json")
}

/// `path.join(app.getPath("userData"), "recent-units.json")`.
pub fn recent_units_filename(style: PathStyle, user_data: &str) -> String {
    join(style, user_data, "recent-units.json")
}

/// `APP_VERSION`: `app.getVersion()` in a packaged build, `"dev"` otherwise.
pub fn app_version(packaged: bool) -> &'static str {
    if packaged {
        asrs_model::build_constants::VERSION
    } else {
        "dev"
    }
}

/// `path.join(app.getPath("userData"), "autoAssets")`.
pub fn auto_assets_dir(style: PathStyle, user_data: &str) -> String {
    join(style, user_data, "autoAssets")
}

/// `path.join(app.getPath("userData"), "userAssets")`.
pub fn default_user_assets_dir(style: PathStyle, user_data: &str) -> String {
    join(style, user_data, "userAssets")
}

/// `path.join(app.getPath("temp"), "akit-log-path.txt")`: the log path AdvantageKit reads back.
pub fn akit_path_output(style: PathStyle, temp: &str) -> String {
    join(style, temp, "akit-log-path.txt")
}

/// `path.join(app.getPath("temp"), "ascope-log-path.txt")`: a log path AdvantageKit asks to open.
pub fn akit_path_input(style: PathStyle, temp: &str) -> String {
    join(style, temp, "ascope-log-path.txt")
}

/// `path.join(app.getPath("temp"), "advantagescope-video-cache")`.
pub fn video_cache_dir(style: PathStyle, temp: &str) -> String {
    join(style, temp, "advantagescope-video-cache")
}

/// `path.join(app.getPath("userData"), "video-cache")`.
pub fn video_cache_fallback_dir(style: PathStyle, user_data: &str) -> String {
    join(style, user_data, "video-cache")
}

/// `path.join(app.getPath("userData"), \`beta-${BETA_CONFIG.year}.json\`)`.
pub fn beta_filename(style: PathStyle, user_data: &str, year: &str) -> String {
    join(style, user_data, &format!("beta-{year}.json"))
}

/// The export dialog's `defaultPath`: `"AdvantageScope " + formatDate() + ".json"`.
pub fn layout_default_path(now_ms: f64, zone: &impl LocalZone) -> String {
    format!("AdvantageScope {}.json", format_date(now_ms, zone))
}

/// `NoopUnitConversion` from `shared/units.ts`.
pub fn noop_unit_conversion() -> JsValue {
    let mut preset = JsObject::new();
    preset.insert(JsString::from("type"), JsValue::Null);
    preset.insert(JsString::from("factor"), JsValue::Number(1.0));
    JsValue::Object(preset)
}
