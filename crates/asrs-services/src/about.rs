//! `createAboutWindow()` in `src/main/electron/main.ts`.

use asrs_model::build_constants::Distribution;

use crate::dialog::MessageBox;

/// `process.versions.electron`, `.chrome` and `.node` of the Electron build upstream ships
/// (39.2.7), which the About text lists.
pub const ELECTRON_VERSION: &str = "39.2.7";
pub const CHROMIUM_VERSION: &str = "142.0.7444.235";
pub const NODE_VERSION: &str = "22.21.1";

/// The About box and the text "Copy & Close" puts on the clipboard.
#[derive(Clone, Debug, PartialEq)]
pub struct About {
    pub message_box: MessageBox,
    pub copy_text: String,
}

/// The facts the About text reads.
#[derive(Clone, Copy, Debug)]
pub struct AboutEnv<'a> {
    /// `AppIdentity::name`; upstream's message is "AdvantageScope".
    pub app_name: &'a str,
    pub distribution: Distribution,
    /// `process.platform`.
    pub platform: &'a str,
    /// `process.arch`.
    pub arch: &'a str,
    /// `app.isPackaged`.
    pub packaged: bool,
    /// `app.getVersion()`.
    pub version: &'a str,
}

/// `createAboutWindow()`'s box and copy text (`BUILD_DATE` and `COPYRIGHT` from
/// `asrs_model::build_constants`).
pub fn about(env: &AboutEnv<'_>) -> About {
    let _ = env;
    todo!("P07: createAboutWindow")
}

impl About {
    /// The clipboard text written once the box resolves with `response`, if any.
    pub fn response(&self, response: usize) -> Option<&str> {
        let _ = response;
        todo!("P07: About response")
    }
}
