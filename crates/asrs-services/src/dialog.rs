//! `dialog.showMessageBox` options as main passes them.

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
