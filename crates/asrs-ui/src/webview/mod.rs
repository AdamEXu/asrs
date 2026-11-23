//! A page of local files in the system's webview (WKWebView, WebView2, WebKitGTK), shown as a
//! native child view of a window. WebKitGTK runs in its own process, `asrs-webview`, so that
//! asrs starts where it is not installed.

#[cfg(not(target_os = "linux"))]
mod local;
#[cfg(not(target_os = "linux"))]
mod page;
#[cfg(target_os = "linux")]
mod remote;
#[allow(dead_code)]
mod wire;

use std::path::PathBuf;

#[cfg(not(target_os = "linux"))]
pub use local::Page;
#[cfg(target_os = "linux")]
pub use remote::Page;
pub use wire::PageEvent;

pub struct PageOptions {
    pub root: PathBuf,
    pub index: &'static str,
    /// What frames inside the page may load, as URL prefixes.
    pub embeds: &'static [&'static str],
    /// Where WebView2 keeps its profile, which is beside the executable by default.
    pub data_dir: Option<PathBuf>,
    /// The `asrs-webview` executable (Linux).
    pub host: PathBuf,
    pub dark: bool,
}

fn assignment(id: u64, hash: &str) -> String {
    let hash = hash.replace('\\', "\\\\").replace('\'', "\\'");
    format!("__asrsAssign({id}, '{hash}')")
}
