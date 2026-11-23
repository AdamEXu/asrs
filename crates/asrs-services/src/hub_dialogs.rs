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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::MemFs;
    use asrs_js::time::FixedZone;

    /// The dialogs `cargo xtask app run console` records (`run.json`), for 2026-11-23.
    #[test]
    fn dialogs_match_the_console_scenario() {
        let dialog = console_export(1_795_392_000_000.0, &FixedZone::UTC);
        assert_eq!(dialog.title, "Select export location for console log");
        assert_eq!(dialog.default_path, "Console 2026-11-23.txt");
        assert_eq!(
            dialog.properties,
            [
                "createDirectory",
                "showOverwriteConfirmation",
                "dontAddToRecent"
            ]
        );
        assert_eq!(dialog.filters.name, "Text files");
        assert_eq!(dialog.filters.extensions, ["txt"]);

        let data = ops::object([
            ("title", JsValue::string("Cannot export console log")),
            ("content", JsValue::string("Please add a field")),
        ]);
        let shown = error(&data).unwrap();
        assert_eq!((shown.kind, shown.title), ("error", "Error"));
        assert_eq!(shown.message, JsString::from("Cannot export console log"));
        assert_eq!(shown.detail, JsString::from("Please add a field"));
        assert!(shown.buttons.is_none() && shown.modal);
    }

    #[test]
    fn lone_surrogates_are_replaced() {
        let fs = MemFs::default();
        let text = JsString::from_units(vec![0x61, 0xd800, 0x0a, 0xe9]);
        write_console(&fs, "/out.txt", &text).unwrap();
        assert_eq!(fs.read("/out.txt").unwrap(), "a\u{fffd}\n\u{e9}".as_bytes());
    }
}
