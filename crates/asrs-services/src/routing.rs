//! `main.ts`'s file routing: `isDsLog`, `isDsWindow`, `openFilesSmart` and the `open-file` event.
//! They decide from the files, the open hubs and the renderer states; the caller carries out the
//! [`Action`].

use asrs_js::json;
use asrs_js::ops;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsValue, ToJs};

use crate::dialog::MessageBox;
use crate::ds_layout::default_ds_layout;
use crate::fs::Fs;
use crate::window_manager::WindowId;

const DS_EXTRA_HEADER: &[u8] = b"DS Log File";
const DS_HEADER_LENGTH: usize = 12 + DS_EXTRA_HEADER.len();

/// `isDsLog`: whether the file starts with the WPILOG header the Driver Station writes.
pub fn is_ds_log(fs: &dyn Fs, path: &str) -> bool {
    let Ok(header) = fs.read_prefix(path, DS_HEADER_LENGTH) else {
        return false;
    };
    header.len() == DS_HEADER_LENGTH
        // `toString("ascii")` drops every byte's high bit.
        && header[..6].iter().map(|b| b & 0x7f).eq(*b"WPILOG")
        && header[7] == 1
        && header[8..12] == (DS_EXTRA_HEADER.len() as u32).to_le_bytes()
        && &header[12..] == DS_EXTRA_HEADER
}

fn signature(tabs: &[JsValue]) -> Result<Option<String>, JsError> {
    let pairs = tabs
        .iter()
        .map(|tab| {
            Ok(JsValue::Array(vec![
                ops::get(tab, "type")?,
                ops::get(tab, "title")?,
            ]))
        })
        .collect::<Result<_, JsError>>()?;
    json::stringify(&JsValue::Array(pairs))
        .map_err(|e| JsError::new(e.name, e.message.to_string_lossy()))
}

/// `isDsWindow` on the window's `getRendererState`. Saved tabs that are no array throw.
pub fn is_ds_window(state: &JsValue) -> Result<bool, JsError> {
    let optional = |value: &JsValue, key: &str| match value {
        JsValue::Undefined | JsValue::Null => Ok(JsValue::Undefined),
        value => ops::get(value, key),
    };
    match &optional(&optional(state, "tabs")?, "tabs")? {
        JsValue::Undefined => Ok(false),
        JsValue::Array(tabs) => {
            let layout: Vec<JsValue> = default_ds_layout(true)
                .tabs
                .tabs
                .iter()
                .map(ToJs::to_js)
                .collect();
            Ok(signature(tabs)? == signature(&layout)?)
        }
        _ => Err(JsError::type_error("tabs.map is not a function")),
    }
}

/// Where the files go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// `sendMessage(window, "open-files", { files, merge: false })`.
    Send(WindowId),
    /// `window.focus()`, then the same.
    FocusAndSend(WindowId),
    /// `createHubWindow()`, with the message sent on its `dom-ready`.
    NewHub,
    /// `createDsWindow(files)`.
    NewDsHub,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Route {
    Open(Action),
    /// `dialog.showMessageBoxSync`, on the source window when there is one. A response past the
    /// actions is Cancel.
    Ask {
        window: Option<WindowId>,
        options: MessageBox,
        actions: Vec<Action>,
    },
}

/// What routing reads from main.
pub struct Hubs<'a> {
    pub fs: &'a dyn Fs,
    /// `hubWindows`: the live ones, most recently focused first.
    pub windows: &'a [WindowId],
    /// `stateTracker.getRendererState(window)`.
    pub state: &'a dyn Fn(WindowId) -> JsValue,
}

impl Hubs<'_> {
    /// `openFilesSmart(files, sourceWindow)`.
    pub fn open_files_smart(
        &self,
        files: &[String],
        source: Option<WindowId>,
    ) -> Result<Route, JsError> {
        let source = source.filter(|source| self.windows.contains(source));
        let [file] = files else {
            return Ok(Route::Open(source.map_or(Action::NewHub, Action::Send)));
        };
        if !is_ds_log(self.fs, file) {
            return Ok(Route::Open(source.map_or(Action::NewHub, Action::Send)));
        }

        let is_ds = |window: WindowId| is_ds_window(&(self.state)(window));
        let mut ds_window = None;
        for window in self.windows {
            if is_ds(*window)? {
                ds_window = Some(*window);
                break;
            }
        }
        if source.is_none() && self.windows.is_empty() {
            return Ok(Route::Open(Action::NewDsHub));
        }
        if let Some(source) = source
            && is_ds(source)?
        {
            return Ok(Route::Open(Action::Send(source)));
        }

        let mut buttons = Vec::new();
        let mut actions = Vec::new();
        match ds_window {
            Some(target) => {
                buttons.push("Open in DS Window");
                actions.push(Action::FocusAndSend(target));
            }
            None => {
                buttons.push("Open in New Window");
                actions.push(Action::NewDsHub);
            }
        }
        if let Some(source) = source {
            buttons.push("Open in This Window");
            actions.push(Action::Send(source));
        }
        buttons.push("Cancel");
        Ok(Route::Ask {
            window: source,
            options: MessageBox {
                kind: "question",
                title: "Open DS log",
                message: JsString::from("Open this log using the DS layout?"),
                detail: JsString::from(
                    "The Driver Station layout can be opened in a separate window without changing your current layout.",
                ),
                default_id: Some(0.0),
                cancel_id: Some((buttons.len() - 1) as f64),
                buttons: Some(buttons),
                icon: true,
                modal: source.is_some(),
            },
            actions,
        })
    }

    /// The `open-file` event once the app is ready.
    pub fn open_file(&self, path: &str) -> Result<Route, JsError> {
        if is_ds_log(self.fs, path) {
            self.open_files_smart(&[path.to_owned()], self.windows.first().copied())
        } else {
            Ok(Route::Open(Action::NewHub))
        }
    }
}
