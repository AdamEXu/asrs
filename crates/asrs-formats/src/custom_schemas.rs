//! `src/hub/dataSources/schema/CustomSchemas.ts`: raw types the wpilog worker decodes with a
//! dedicated parser after storing the raw value.

use asrs_js::string::JsString;
use asrs_log::log::{Log, LogError};

pub enum SchemaError {
    /// The parser threw. The worker catches it, so only the writes made before the throw remain.
    Threw,
    Log(LogError),
}

pub type SchemaParser = fn(&mut Log, &JsString, f64, &[u8]) -> Result<(), SchemaError>;

/// The keys of upstream's registry (`CustomSchemas.has`).
pub const NAMES: [&str; 3] = ["URCL", "URCLr2_periodic", "URCLr3_periodic"];

pub fn has(type_: &JsString) -> bool {
    NAMES.iter().any(|name| *type_ == JsString::from(*name))
}

/// `CustomSchemas.get`. `None` for a registered name means its parser is not ported yet
/// (URCL arrives with unit F08).
pub fn get(_type: &JsString) -> Option<SchemaParser> {
    None
}

pub fn unported() -> impl Iterator<Item = &'static str> {
    NAMES
        .into_iter()
        .filter(|name| get(&JsString::from(*name)).is_none())
}
