//! An error box shown before the app runs, when there may be no terminal to print to.

#[cfg(target_os = "macos")]
pub(super) fn show(title: &str, detail: &str) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSAlert, NSAlertStyle, NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::NSString;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Critical);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(detail));
    alert.runModal();
}

#[cfg(windows)]
pub(super) fn show(title: &str, detail: &str) {
    use std::ffi::c_void;
    use std::ptr;

    const MB_ICONERROR: u32 = 0x10;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(owner: *mut c_void, text: *const u16, caption: *const u16, kind: u32)
        -> i32;
    }

    let wide = |text: &str| text.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (caption, text) = (wide(title), wide(detail));
    // SAFETY: both strings end in a NUL and outlive the call, and a null owner is allowed.
    unsafe {
        MessageBoxW(
            ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_ICONERROR,
        )
    };
}

/// GTK, which draws Electron's error box, is only linked by `asrs-webview`. Where that does not
/// start, the desktops' own dialog programs are tried.
#[cfg(target_os = "linux")]
pub(super) fn show(title: &str, detail: &str) {
    use std::process::{Command, Stdio};

    let exe = std::env::current_exe().unwrap_or_default();
    let mut webview = Command::new(exe.with_file_name("asrs-webview"));
    webview.args(["--error", title, detail]);
    let mut zenity = Command::new("zenity");
    zenity.args(["--error", "--no-markup", "--title", title, "--text", detail]);
    let mut kdialog = Command::new("kdialog");
    kdialog.args(["--title", title, "--error", detail]);
    for command in [&mut webview, &mut zenity, &mut kdialog] {
        let shown = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if shown {
            return;
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub(super) fn show(_title: &str, _detail: &str) {}
