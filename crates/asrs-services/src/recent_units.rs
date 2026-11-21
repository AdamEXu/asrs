//! `updateRecents(newUnitConversion)` from `main.ts` and the `recent-units.json` file it keeps.

use asrs_js::json::{self};
use asrs_js::string::JsError;
use asrs_js::value::JsValue;
use asrs_model::json_file;

use crate::constants::{MAX_RECENT_UNITS, noop_unit_conversion};
use crate::fs::Fs;

fn stringify(value: &JsValue) -> Result<Option<String>, JsError> {
    json::stringify(value).map_err(|e| JsError::new(e.name, e.message.to_string_lossy()))
}

/// `ipcMain.on("update-recents", ...)`.
pub fn update(fs: &dyn Fs, recent_units_file: &str, preset: &JsValue) -> Result<(), JsError> {
    if stringify(preset)? == stringify(&noop_unit_conversion())? {
        return Ok(());
    }
    let mut recents = if fs.exists(recent_units_file) {
        json_file::parse(&fs.read(recent_units_file)?)?
    } else {
        JsValue::Array(Vec::new())
    };
    let new_str = stringify(preset)?;
    let Some(items) = recents.as_array_mut() else {
        return Err(match recents {
            JsValue::Null => {
                JsError::type_error("Cannot read properties of null (reading 'filter')")
            }
            JsValue::Undefined => {
                JsError::type_error("Cannot read properties of undefined (reading 'filter')")
            }
            _ => JsError::type_error("recentUnits.filter is not a function"),
        });
    };
    let mut kept = Vec::with_capacity(items.len());
    for x in items.iter() {
        if stringify(x)? != new_str {
            kept.push(x.clone());
        }
    }
    *items = kept;
    items.insert(0, preset.clone());
    items.truncate(MAX_RECENT_UNITS);
    fs.write(recent_units_file, &json_file::stringify(&recents)?)
}
