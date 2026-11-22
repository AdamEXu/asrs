//! Electron's `dialog.showMessageBox`, `showOpenDialog` and `showSaveDialog`, with the option
//! handling Electron's `lib/browser/api/dialog.ts` does before reaching the native code.

use std::path::PathBuf;

use gpui_kit::{App, Window};

use super::imp;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MessageBoxType {
    #[default]
    None,
    Info,
    Warning,
    Error,
    Question,
}

/// `MessageBoxOptions`. `default_id: None` is Electron's `-1`; `cancel_id: None` lets
/// [`MessageBoxOptions::cancel_id`] pick one as Electron does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MessageBoxOptions {
    pub kind: MessageBoxType,
    pub title: String,
    pub message: String,
    pub detail: String,
    pub buttons: Vec<String>,
    pub default_id: Option<usize>,
    pub cancel_id: Option<usize>,
    pub checkbox_label: String,
    pub checkbox_checked: bool,
}

impl MessageBoxOptions {
    /// The button Escape picks: the given one, else the first button labelled "cancel" or "no"
    /// in any case, else 1 when the default is 0 and there are two or more buttons, else 0.
    pub fn cancel_id(&self) -> usize {
        if let Some(id) = self.cancel_id {
            return id;
        }
        self.buttons
            .iter()
            .position(|b| {
                let lower = b.to_lowercase();
                lower == "cancel" || lower == "no"
            })
            .unwrap_or(if self.default_id == Some(0) && self.buttons.len() > 1 {
                1
            } else {
                0
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MessageBoxResult {
    pub response: usize,
    pub checkbox_checked: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileFilter {
    pub name: String,
    pub extensions: Vec<String>,
}

/// `OpenDialogOptions` with `properties` as flags.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpenDialogOptions {
    pub title: String,
    pub message: String,
    pub button_label: String,
    pub default_path: Option<PathBuf>,
    pub filters: Vec<FileFilter>,
    pub open_file: bool,
    pub open_directory: bool,
    pub multi_selections: bool,
    pub create_directory: bool,
}

/// `SaveDialogOptions` with `properties` as flags. `showOverwriteConfirmation` and
/// `dontAddToRecent` have no effect on macOS, where the panel always confirms.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SaveDialogOptions {
    pub title: String,
    pub message: String,
    pub button_label: String,
    pub default_path: Option<PathBuf>,
    pub filters: Vec<FileFilter>,
    pub create_directory: bool,
}

/// Shows a message box, as a sheet on `window` when given. `on_done` runs once it closes.
pub fn message_box(
    window: Option<&mut Window>,
    cx: &mut App,
    options: MessageBoxOptions,
    on_done: impl FnOnce(MessageBoxResult, &mut App) + 'static,
) {
    imp::dialog::message_box(window, cx, options, on_done);
}

/// Shows an open panel, as a sheet on `window` when given. `on_done` gets the chosen paths,
/// empty when cancelled.
pub fn open_dialog(
    window: Option<&mut Window>,
    cx: &mut App,
    options: OpenDialogOptions,
    on_done: impl FnOnce(Vec<PathBuf>, &mut App) + 'static,
) {
    imp::dialog::open_dialog(window, cx, options, on_done);
}

/// Shows a save panel, as a sheet on `window` when given. `on_done` gets the chosen path, `None`
/// when cancelled.
pub fn save_dialog(
    window: Option<&mut Window>,
    cx: &mut App,
    options: SaveDialogOptions,
    on_done: impl FnOnce(Option<PathBuf>, &mut App) + 'static,
) {
    imp::dialog::save_dialog(window, cx, options, on_done);
}
