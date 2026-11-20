//! The byte-level behaviour of jsonfile 6.1.0, which main uses for every JSON file it reads or
//! writes.

use asrs_js::json;
use asrs_js::string::{JsError, JsString, TextDecoder};
use asrs_js::value::JsValue;

/// `jsonfile.readFileSync` on file contents: `Buffer.toString("utf8")` (invalid sequences become
/// U+FFFD), one leading U+FEFF removed, then `JSON.parse`.
pub fn parse(bytes: &[u8]) -> Result<JsValue, JsError> {
    let decoder = TextDecoder::new(None, false, true).expect("utf-8 is supported");
    let text = decoder.decode(bytes).expect("lossy decoding cannot fail");
    let units = text.units();
    let units = units.strip_prefix(&[0xfeff]).unwrap_or(units);
    json::parse(&JsString::from(units.to_vec())).map_err(|e| JsError {
        name: e.name,
        message: e.message.to_string_lossy(),
    })
}

/// `jsonfile.writeFileSync(file, value)`: compact `JSON.stringify` plus `"\n"`.
pub fn stringify(value: &JsValue) -> Result<Vec<u8>, JsError> {
    stringify_gap(value, "")
}

/// `jsonfile.writeFile(file, value, { spaces: 2 })`.
pub fn stringify_pretty(value: &JsValue) -> Result<Vec<u8>, JsError> {
    stringify_gap(value, "  ")
}

fn stringify_gap(value: &JsValue, gap: &str) -> Result<Vec<u8>, JsError> {
    let text = json::stringify_gap(value, gap).map_err(|e| JsError {
        name: e.name,
        message: e.message.to_string_lossy(),
    })?;
    match text {
        Some(mut text) => {
            text.push('\n');
            Ok(text.into_bytes())
        }
        None => Err(JsError::type_error(
            "Cannot read properties of undefined (reading 'replace')",
        )),
    }
}
