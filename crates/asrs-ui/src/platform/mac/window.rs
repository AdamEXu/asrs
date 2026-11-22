//! The parts of Electron's `NativeWindowMac` GPUI's window options do not cover.

use gpui_kit::Window;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSToolbar, NSWindowCollectionBehavior};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use super::ns_window;

pub(in super::super) fn hidden_inset_titlebar(window: &Window) -> bool {
    let (Some(mtm), Some(window)) = (MainThreadMarker::new(), ns_window(window)) else {
        return false;
    };
    let toolbar = NSToolbar::initWithIdentifier(
        NSToolbar::alloc(mtm),
        &NSString::from_str("titlebarStylingToolbar"),
    );
    #[allow(deprecated)]
    toolbar.setShowsBaselineSeparator(false);
    window.setToolbar(Some(&toolbar));
    true
}

pub(in super::super) fn content_size_for_frame(
    window: &Window,
    width: f64,
    height: f64,
) -> Option<(f64, f64)> {
    let window = ns_window(window)?;
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height));
    let content = window.contentRectForFrameRect(frame);
    Some((content.size.width, content.size.height))
}

pub(in super::super) fn set_content_size_limits(
    window: &Window,
    min: Option<(f64, f64)>,
    max: Option<(f64, f64)>,
) {
    let Some(window) = ns_window(window) else {
        return;
    };
    if let Some((width, height)) = min {
        window.setContentMinSize(NSSize::new(width, height));
    }
    if let Some((width, height)) = max {
        window.setContentMaxSize(NSSize::new(width, height));
    }
}

/// `NativeWindowMac::SetFullScreenable`.
pub(in super::super) fn set_fullscreenable(window: &Window, fullscreenable: bool) {
    let Some(window) = ns_window(window) else {
        return;
    };
    let mut behavior = window.collectionBehavior();
    behavior.set(
        NSWindowCollectionBehavior::FullScreenPrimary,
        fullscreenable,
    );
    behavior.set(
        NSWindowCollectionBehavior::FullScreenAuxiliary,
        !fullscreenable,
    );
    window.setCollectionBehavior(behavior);
}

pub(in super::super) fn center(window: &Window) {
    if let Some(window) = ns_window(window) {
        window.center();
    }
}
