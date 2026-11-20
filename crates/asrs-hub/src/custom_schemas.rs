//! `src/hub/dataSources/schema/CustomSchemas.ts`: schemas that require custom handling because
//! they can't be decoded using just the log data.

use asrs_js::string::JsString;

const CUSTOM_SCHEMAS: &[&str] = &["URCL", "URCLr2_periodic", "URCLr3_periodic"];

pub fn has(key: &JsString) -> bool {
    CUSTOM_SCHEMAS.iter().any(|k| key == &JsString::from(*k))
}
