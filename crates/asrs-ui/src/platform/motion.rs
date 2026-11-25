//! `(prefers-reduced-motion: reduce)`: Chromium's `gfx::Animation::PrefersReducedMotion` per
//! platform. Linux, where it is GTK's `gtk-enable-animations`, always animates.

use gpui_kit::{App, Global};

/// Stands in for the platform's setting.
pub struct ForcedReducedMotion(pub bool);

impl Global for ForcedReducedMotion {}

pub fn prefers_reduced_motion(cx: &App) -> bool {
    match cx.try_global::<ForcedReducedMotion>() {
        Some(forced) => forced.0,
        None => source::prefers_reduced_motion(),
    }
}

#[cfg(target_os = "macos")]
mod source {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    pub fn prefers_reduced_motion() -> bool {
        // SAFETY: both are plain AppKit getters with these types.
        unsafe {
            let workspace: Retained<AnyObject> = msg_send![class!(NSWorkspace), sharedWorkspace];
            msg_send![&*workspace, accessibilityDisplayShouldReduceMotion]
        }
    }
}

#[cfg(target_os = "windows")]
mod source {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    use windows::core::BOOL;

    pub fn prefers_reduced_motion() -> bool {
        let mut animates = BOOL(1);
        // SAFETY: the action writes one BOOL to the pointer.
        let read = unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                Some((&raw mut animates).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        };
        read.is_ok() && !animates.as_bool()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod source {
    pub fn prefers_reduced_motion() -> bool {
        false
    }
}
