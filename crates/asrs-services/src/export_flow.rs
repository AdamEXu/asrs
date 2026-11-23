//! `main.ts`'s export flow: `prompt-export`, the window of `createExportWindow`, `write-export`
//! and the warning a hub closing during an export shows. Each step returns the [`Effect`]s main
//! has, in order.

use asrs_js::ops;
use asrs_js::string::{JsError, JsString, text_encode};
use asrs_js::value::JsValue;
use asrs_model::app_identity::AppIdentity;
use asrs_model::messages::MainToHub;

use crate::dialog::{FileFilter, MessageBox, SaveDialog};
use crate::fs::Fs;

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// `dialog.showMessageBox` on the hub; its response goes to [`warned`] with the request.
    Warn(MessageBox),
    /// `createExportWindow(hub, supportsAkit, currentLogPath)`.
    OpenPrompt {
        supports_akit: JsValue,
        path: JsValue,
    },
    /// `sendMessage(hub, name, data)`.
    Send { name: MainToHub, data: JsValue },
    /// `exportWindow.destroy()`.
    DestroyPrompt,
    /// `shell.openExternal(url)`.
    OpenExternal(JsString),
    /// `dialog.showSaveDialog` on the export window; its reply goes to [`ExportPrompt::saved`].
    Save {
        options: SaveDialog,
        export: JsValue,
    },
}

const CANCEL: Effect = Effect::Send {
    name: MainToHub::CancelExport,
    data: JsValue::Undefined,
};

fn open_prompt(data: &JsValue) -> Result<Effect, JsError> {
    Ok(Effect::OpenPrompt {
        supports_akit: ops::get(data, "supportsAkit")?,
        path: ops::get(data, "path")?,
    })
}

/// `prompt-export`.
pub fn prompt_export(data: &JsValue, identity: AppIdentity) -> Result<Effect, JsError> {
    if !ops::to_boolean(&ops::get(data, "incompleteWarning")?) {
        return open_prompt(data);
    }
    let detail = "Some fields will not be available in the exported data. To save all fields from the server, the \"Logging\" live mode must be selected. Check the AdvantageScope documentation for details.";
    Ok(Effect::Warn(MessageBox {
        kind: "info",
        title: "Warning",
        message: JsString::from("Incomplete data for export"),
        detail: JsString::from(identity.text(detail)),
        buttons: Some(vec!["Continue", "Cancel"]),
        default_id: None,
        cancel_id: None,
        icon: true,
        modal: true,
    }))
}

/// The response to the incomplete data warning of the `prompt-export` request `data`.
pub fn warned(data: &JsValue, response: usize) -> Result<Effect, JsError> {
    if response == 0 {
        open_prompt(data)
    } else {
        Ok(CANCEL)
    }
}

/// Main's side of one export window.
#[derive(Clone, Debug)]
pub struct ExportPrompt {
    /// `currentLogPath`.
    path: JsValue,
    preparing: bool,
    closed: bool,
}

impl ExportPrompt {
    pub fn new(path: JsValue) -> ExportPrompt {
        ExportPrompt {
            path,
            preparing: false,
            closed: false,
        }
    }

    /// A message from the window's page: `null` from the exit button, the help URL or the
    /// chosen `ExportOptions`.
    pub fn message(&mut self, data: &JsValue) -> Result<Vec<Effect>, JsError> {
        match data {
            JsValue::Null => {
                let mut effects = self.destroy();
                effects.push(CANCEL);
                Ok(effects)
            }
            JsValue::String(url) => Ok(vec![Effect::OpenExternal(url.clone())]),
            JsValue::Object(_) => Ok(vec![Effect::Save {
                options: self.save_dialog(data)?,
                export: data.clone(),
            }]),
            _ => Ok(Vec::new()),
        }
    }

    fn save_dialog(&self, export: &JsValue) -> Result<SaveDialog, JsError> {
        let format = ops::get(export, "format")?;
        let Some(format) = format.as_str().map(JsString::to_string_lossy) else {
            return Err(JsError::type_error(
                "exportOptions.format.startsWith is not a function",
            ));
        };
        let (extension, filters) = match format.as_str() {
            csv if csv.starts_with("csv") => (
                "csv",
                FileFilter {
                    name: "Comma-separated values",
                    extensions: &["csv"],
                },
            ),
            "wpilog" => (
                "wpilog",
                FileFilter {
                    name: "WPILib robot log",
                    extensions: &["wpilog"],
                },
            ),
            "mcap" => (
                "mcap",
                FileFilter {
                    name: "MCAP log",
                    extensions: &["mcap"],
                },
            ),
            other => unreachable!("export.html offers no {other} format"),
        };
        Ok(SaveDialog {
            title: "Select export location for robot log",
            default_path: match &self.path {
                JsValue::String(path) => Some(with_extension(&path.to_string_lossy(), extension)),
                _ => None,
            },
            properties: &[
                "createDirectory",
                "showOverwriteConfirmation",
                "dontAddToRecent",
            ],
            filters,
        })
    }

    /// The save dialog closed with `path`, or `None` when cancelled.
    pub fn saved(&mut self, export: JsValue, path: Option<&str>) -> Vec<Effect> {
        let Some(path) = path else {
            return Vec::new();
        };
        self.preparing = true;
        let mut effects = self.destroy();
        effects.push(Effect::Send {
            name: MainToHub::PrepareExport,
            data: ops::object([("path", JsValue::string(path)), ("options", export)]),
        });
        effects
    }

    /// `exportWindow.destroy()`, which fires `closed` before it returns.
    fn destroy(&mut self) -> Vec<Effect> {
        let mut effects = vec![Effect::DestroyPrompt];
        effects.extend(self.closed());
        effects
    }

    /// The window's `closed` event, which fires once.
    pub fn closed(&mut self) -> Option<Effect> {
        let first = !std::mem::replace(&mut self.closed, true);
        (first && !self.preparing).then_some(CANCEL)
    }
}

/// `path.split(".")` without its last component, joined again, with `extension` in its place.
fn with_extension(path: &str, extension: &str) -> String {
    let stem = path.rsplit_once('.').map_or("", |(stem, _)| stem);
    format!("{stem}.{extension}")
}

/// What `write-export` writes: the path, and a string as UTF-8 or bytes as they are.
pub fn export_file(data: &JsValue) -> Result<(String, Vec<u8>), JsError> {
    let path = ops::to_string(&ops::get(data, "path")?).to_string_lossy();
    match data.get("content") {
        Some(JsValue::String(text)) => Ok((path, text_encode(text))),
        Some(JsValue::Bytes(bytes)) => Ok((path, bytes.clone())),
        _ => Err(JsError::type_error(
            "The \"data\" argument must be of type string or an instance of Buffer",
        )),
    }
}

/// `write-export`'s `fs.writeFile`. An error is its callback's throw.
pub fn write_export(fs: &dyn Fs, path: &str, content: &[u8]) -> Result<Effect, JsError> {
    fs.write(path, content)?;
    Ok(Effect::Send {
        name: MainToHub::FinishExport,
        data: JsValue::Undefined,
    })
}

/// What a hub in `hubExportingIds` asks before it closes; response 0 keeps it open.
pub fn closing_warning() -> MessageBox {
    MessageBox {
        kind: "info",
        title: "Warning",
        message: JsString::from("Export in progress"),
        detail: JsString::from(
            "Are you sure you want to close the window while an export is in progress? Data will NOT be saved.",
        ),
        buttons: Some(vec!["Don't Close", "Close"]),
        default_id: Some(0.0),
        cancel_id: None,
        icon: false,
        modal: true,
    }
}
