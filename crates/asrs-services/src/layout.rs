//! The "Export Layout..."/"Import Layout..." menu handlers from `main.ts`: the file format
//! (`version`, `hubs`, `satellites`), the older-format detection, the additional-file merge,
//! and the packaged-version warning gate.

use asrs_js::ops;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::JsValue;
use asrs_model::json_file;

use crate::fs::Fs;

/// `state.version = APP_VERSION` plus `jsonfile.writeFile(path, state, { spaces: 2 })` minus the
/// dialog; returns the file bytes.
pub fn export_bytes(state: &mut JsValue, app_version: &str) -> Result<Vec<u8>, JsError> {
    ops::set_key(
        state,
        &JsString::from("version"),
        JsValue::string(app_version),
    )?;
    json_file::stringify_pretty(state)
}

/// The import outcome once the primary file is read and validated.
pub enum Import {
    /// Windows are (re)created from this state; `bool` is the version-mismatch warning the
    /// packaged build shows, whose Cancel aborts before windows are closed.
    Windows { state: JsValue, version_mismatch: bool },
    /// `layout` array present: "uses an older format" error.
    OldFormat,
    /// "was not a recognized format" error.
    Unrecognized,
}

fn valid_layout(data: &JsValue) -> Result<bool, JsError> {
    Ok(ops::has(data, "version")?
        && ops::has(data, "hubs")?
        && matches!(ops::get(data, "hubs")?, JsValue::Array(_))
        && ops::has(data, "satellites")?
        && matches!(ops::get(data, "satellites")?, JsValue::Array(_)))
}

/// The import handler after `filePaths` is nonempty.
pub fn import(
    fs: &dyn Fs,
    file_paths: &[String],
    is_packaged: bool,
    app_version: &str,
) -> Result<Import, JsError> {
    let mut data = json_file::parse(&fs.read(&file_paths[0])?)?;
    if !valid_layout(&data)? {
        let old = ops::has(&data, "layout")?
            && matches!(ops::get(&data, "layout")?, JsValue::Array(_));
        return Ok(if old {
            Import::OldFormat
        } else {
            Import::Unrecognized
        });
    }
    for file in &file_paths[1..] {
        let additional = json_file::parse(&fs.read(file)?)?;
        if valid_layout(&additional)?
            && ops::strict_equals(
                &ops::get(&additional, "version")?,
                &ops::get(&data, "version")?,
            )
        {
            concat(&mut data, "hubs", &additional)?;
            concat(&mut data, "satellites", &additional)?;
        }
    }
    let version_mismatch = is_packaged
        && !ops::strict_equals(
            &ops::get(&data, "version")?,
            &JsValue::string(app_version),
        );
    Ok(Import::Windows {
        state: data,
        version_mismatch,
    })
}

/// `data[field] = data[field].concat(additional[field])` for validated array fields.
fn concat(data: &mut JsValue, field: &str, additional: &JsValue) -> Result<(), JsError> {
    let base = ops::get(data, field)?;
    let mut merged = match &base {
        JsValue::Array(items) => items.clone(),
        _ => Vec::new(),
    };
    if let JsValue::Array(items) = &ops::get(additional, field)? {
        merged.extend(items.iter().cloned());
    }
    ops::set_key(data, &JsString::from(field), JsValue::Array(merged))
}
