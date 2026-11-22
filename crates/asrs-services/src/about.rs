//! `createAboutWindow()` in `src/main/electron/main.ts`.

use asrs_js::string::JsString;
use asrs_model::build_constants::{self, Distribution};

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
    let version = if env.packaged {
        env.version
    } else {
        "Development"
    };
    let distribution = match env.distribution {
        Distribution::WpiLib => "WPILib",
        _ => "FRC 6328",
    };
    let copy_text = [
        format!("Version: {version}"),
        format!("Distribution: {distribution}"),
        format!("Platform: {}-{}", env.platform, env.arch),
        format!("Build Date: {}", build_constants::BUILD_DATE),
        format!("Electron: {ELECTRON_VERSION}"),
        format!("Chromium: {CHROMIUM_VERSION}"),
        format!("Node: {NODE_VERSION}"),
    ]
    .join("\n");
    About {
        message_box: MessageBox {
            kind: "info",
            title: "About",
            message: JsString::from(env.app_name),
            detail: JsString::from(format!("{}\n\n{copy_text}", build_constants::COPYRIGHT)),
            buttons: Some(vec![
                "Close",
                if env.platform == "win32" {
                    "Copy and Close"
                } else {
                    "Copy & Close"
                },
            ]),
            default_id: Some(0.0),
            cancel_id: None,
            icon: true,
            modal: false,
        },
        copy_text,
    }
}

impl About {
    /// The clipboard text written once the box resolves with `response`, if any.
    pub fn response(&self, response: usize) -> Option<&str> {
        (response == 1).then_some(self.copy_text.as_str())
    }
}
