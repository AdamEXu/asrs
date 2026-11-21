//! Persisted file locations and window defaults from `main/electron/ElectronConstants.ts` and
//! `main.ts`. `Fs` paths are `path.join` results: `dir`, the platform separator, `name`.

use asrs_js::string::JsString;
use asrs_js::time::{LocalZone, format_date};
use asrs_js::value::{JsObject, JsValue};

pub const HUB_DEFAULT_WIDTH: f64 = 1100.0;
pub const HUB_DEFAULT_HEIGHT: f64 = 650.0;
pub const SATELLITE_DEFAULT_WIDTH: f64 = 900.0;
pub const SATELLITE_DEFAULT_HEIGHT: f64 = 500.0;
pub const SAVE_PERIOD_MS: f64 = 250.0;
pub const MAX_RECENT_UNITS: usize = 5;

fn join(dir: &str, name: &str) -> String {
    format!("{dir}{}{name}", std::path::MAIN_SEPARATOR)
}

/// `path.join(app.getPath("userData"), "prefs.json")`.
pub fn prefs_filename(user_data: &str) -> String {
    join(user_data, "prefs.json")
}

/// `path.join(app.getPath("userData"), \`state-${APP_VERSION.replaceAll(".", "_")}.json\`)`.
pub fn state_filename(user_data: &str, app_version: &str) -> String {
    join(
        user_data,
        &format!("state-{}.json", app_version.replace('.', "_")),
    )
}

/// `path.join(app.getPath("userData"), "type-memory.json")`.
pub fn type_memory_filename(user_data: &str) -> String {
    join(user_data, "type-memory.json")
}

/// `path.join(app.getPath("userData"), "recent-units.json")`.
pub fn recent_units_filename(user_data: &str) -> String {
    join(user_data, "recent-units.json")
}

/// `path.join(app.getPath("userData"), \`beta-${BETA_CONFIG.year}.json\`)`.
pub fn beta_filename(user_data: &str, year: &str) -> String {
    join(user_data, &format!("beta-{year}.json"))
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
