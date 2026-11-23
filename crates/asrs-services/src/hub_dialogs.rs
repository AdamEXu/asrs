//! The dialogs `main.ts`'s `handleHubMessage` shows for `error` and `export-console`, and the
//! file the latter writes.

use asrs_js::ops;
use asrs_js::string::{JsError, JsString};
use asrs_js::time::{LocalZone, format_date};
use asrs_js::value::JsValue;

use crate::dialog::{FileFilter, MessageBox, SaveDialog};
use crate::fs::Fs;

/// `error`.
pub fn error(data: &JsValue) -> Result<MessageBox, JsError> {
    let text = |key: &str| Ok::<_, JsError>(ops::to_string(&ops::get(data, key)?));
    Ok(MessageBox {
        kind: "error",
        title: "Error",
        message: text("title")?,
        detail: text("content")?,
        buttons: None,
        default_id: None,
        cancel_id: None,
        icon: true,
        modal: true,
    })
}

/// `export-console`'s save dialog.
pub fn console_export(now_ms: f64, zone: &impl LocalZone) -> SaveDialog {
    SaveDialog {
        title: "Select export location for console log",
        default_path: format!("Console {}.txt", format_date(now_ms, zone)),
        properties: &[
            "createDirectory",
            "showOverwriteConfirmation",
            "dontAddToRecent",
        ],
        filters: FileFilter {
            name: "Text files",
            extensions: &["txt"],
        },
    }
}

/// `fs.writeFile(path, text)`: UTF-8, lone surrogates replaced.
pub fn write_console(fs: &dyn Fs, path: &str, text: &JsString) -> Result<(), JsError> {
    fs.write(path, text.to_string_lossy().as_bytes())
}
