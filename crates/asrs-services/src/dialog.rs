//! `dialog.showMessageBox`, `showOpenDialog` and `showSaveDialog` options as main passes them.

use asrs_js::string::JsString;

/// One `showMessageBox` call. `None` fields are keys main leaves out; `icon` is whether main
/// passes ElectronConstants' `WINDOW_ICON` (the key is present even where that is `undefined`);
/// `modal` is whether the box is parented to a window (`showMessageBox(window, options)`).
#[derive(Clone, Debug, PartialEq)]
pub struct MessageBox {
    pub kind: &'static str,
    pub title: &'static str,
    pub message: JsString,
    pub detail: JsString,
    pub buttons: Option<Vec<&'static str>>,
    pub default_id: Option<f64>,
    pub cancel_id: Option<f64>,
    pub icon: bool,
    pub modal: bool,
}

/// One `showOpenDialog` call. `None` fields are keys main leaves out.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenDialog {
    pub title: String,
    pub message: Option<&'static str>,
    pub button_label: Option<&'static str>,
    pub properties: &'static [&'static str],
    pub filters: Option<FileFilter>,
}

/// One `showSaveDialog` call.
#[derive(Clone, Debug, PartialEq)]
pub struct SaveDialog {
    pub title: &'static str,
    pub default_path: String,
    pub properties: &'static [&'static str],
    pub filters: FileFilter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileFilter {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
}
