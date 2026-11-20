//! `src/hub/dataSources/schema/CustomSchemas.ts`: raw types the wpilog worker decodes with a
//! dedicated parser after storing the raw value.

use asrs_js::string::JsString;
use asrs_log::log::{Log, LogError};

use crate::urcl;

#[derive(Debug)]
pub enum SchemaError {
    /// The parser threw. The worker catches it, so only the writes made before the throw remain.
    Threw,
    Log(LogError),
}

impl From<LogError> for SchemaError {
    fn from(error: LogError) -> Self {
        SchemaError::Log(error)
    }
}

pub type SchemaParser = fn(&mut Log, &JsString, f64, &[u8]) -> Result<(), SchemaError>;

const REGISTRY: [(&str, SchemaParser); 3] = [
    ("URCL", urcl::parse_urcl_r1),
    ("URCLr2_periodic", urcl::parse_urcl_r2),
    ("URCLr3_periodic", urcl::parse_urcl_r3),
];

/// `CustomSchemas.get`; `None` where `CustomSchemas.has` is false.
pub fn get(type_: &JsString) -> Option<SchemaParser> {
    REGISTRY
        .iter()
        .find(|(name, _)| *type_ == JsString::from(*name))
        .map(|&(_, parser)| parser)
}
