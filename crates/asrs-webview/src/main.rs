#![allow(clippy::disallowed_methods)]

//! The Documentation tab's page on Linux: a WebKitGTK view inside an X11 window of asrs, told
//! what to do on stdin and reporting on stdout. It is a process of its own so that asrs itself
//! does not link WebKitGTK, and ends when its stdin does.

#[cfg(target_os = "linux")]
mod host;
#[cfg(target_os = "linux")]
#[path = "../../asrs-ui/src/webview/page.rs"]
mod page;
#[cfg(target_os = "linux")]
mod parent;
#[cfg(target_os = "linux")]
#[allow(dead_code)]
#[path = "../../asrs-ui/src/webview/wire.rs"]
mod wire;

fn main() {
    #[cfg(target_os = "linux")]
    host::run();
}
