//! `src/hub/dataSources/schema/CustomSchemas.ts`: raw types the wpilog worker decodes with a
//! dedicated parser after storing the raw value.

use asrs_js::string::JsString;
use asrs_log::log::Log;

/// The parser threw. The worker catches it, so only the writes made before the throw remain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaThrew;

pub type SchemaParser = fn(&mut Log, &JsString, f64, &[u8]) -> Result<(), SchemaThrew>;

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
