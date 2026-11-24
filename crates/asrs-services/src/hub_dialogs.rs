//! The dialogs `main.ts`'s `handleHubMessage` shows for `alert`, `error`, `export-console` and the
//! coordinate system warnings, and the files it writes after them.

use asrs_js::ops;
use asrs_js::string::{JsError, JsString};
use asrs_js::time::{LocalZone, format_date};
use asrs_js::value::JsValue;

use crate::dialog::{FileFilter, MessageBox, SaveDialog};
use crate::fs::Fs;

/// `alert`.
pub fn alert(data: &JsValue) -> Result<MessageBox, JsError> {
    titled("info", "Alert", data)
}

/// `error`.
pub fn error(data: &JsValue) -> Result<MessageBox, JsError> {
    titled("error", "Error", data)
}

fn titled(kind: &'static str, title: &'static str, data: &JsValue) -> Result<MessageBox, JsError> {
    let text = |key: &str| Ok::<_, JsError>(ops::to_string(&ops::get(data, key)?));
    Ok(MessageBox {
        kind,
        title,
        message: text("title")?,
        detail: text("content")?,
        buttons: None,
        default_id: None,
        cancel_id: None,
        icon: true,
        modal: true,
    })
}

/// The URL `open-link` opens: the page's, if it is `http:` or `https:`. Upstream hands whatever
/// an asset's `sourceUrl` says to `shell.openExternal` (#402).
pub fn link(data: &JsValue) -> Option<String> {
    let url = url::Url::parse(&data.as_str()?.to_string_lossy()).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.into())
}

/// The `checkboxLabel` of a [`CoordinateWarning`].
pub const DONT_SHOW_AGAIN: &str = "Don't Show Again";

/// `frc-coordinate-system-warning` and `ftc-coordinate-system-warning`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoordinateWarning {
    Frc,
    Ftc,
}

impl CoordinateWarning {
    /// The preference that skips it.
    fn skip(self) -> &'static str {
        match self {
            CoordinateWarning::Frc => "skipFRCCoordinateSystemWarning",
            CoordinateWarning::Ftc => "skipFTCCoordinateSystemWarning",
        }
    }

    /// Whether `prefs.json`'s `prefs` skip it.
    pub fn skipped(self, prefs: &JsValue) -> Result<bool, JsError> {
        Ok(ops::to_boolean(&ops::get(prefs, self.skip())?))
    }

    pub fn message_box(self) -> MessageBox {
        let (message, detail) = match self {
            CoordinateWarning::Frc => (
                "Legacy Coordinate System",
                "This field uses a legacy coordinate system by default for compatibility with \
                 pre-2027 WPILib. If your robot code uses 2027 WPILib, you can change the \
                 \"Coordinate System\" option in the AdvantageScope preferences. See the \
                 documentation for details.",
            ),
            CoordinateWarning::Ftc => (
                "Systemcore Coordinate System",
                "This field uses the Systemcore coordinate system by default. If your robot code \
                 uses the traditional FTC coordinate system, you can change the \"Coordinate \
                 System\" option in the AdvantageScope preferences. See the documentation for \
                 details.",
            ),
        };
        MessageBox {
            kind: "info",
            title: "Alert",
            message: message.into(),
            detail: detail.into(),
            buttons: Some(vec!["OK"]),
            default_id: None,
            cancel_id: None,
            icon: true,
            modal: true,
        }
    }

    /// What is written once the box closes with its checkbox checked: the preferences as they
    /// were when it opened, with the warning skipped.
    pub fn skip_in(self, prefs: &mut JsValue) {
        if let Some(prefs) = prefs.as_object_mut() {
            prefs.insert(self.skip(), JsValue::Bool(true));
        }
    }
}

/// `export-console`'s save dialog.
pub fn console_export(now_ms: f64, zone: &impl LocalZone) -> SaveDialog {
    SaveDialog {
        title: "Select export location for console log",
        default_path: Some(format!("Console {}.txt", format_date(now_ms, zone))),
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
        assert_eq!(
            dialog.default_path.as_deref(),
            Some("Console 2026-11-23.txt")
        );
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
