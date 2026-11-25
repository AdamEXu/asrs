//! The parts of Electron's `NativeWindowMac` GPUI's window options do not cover.

use gpui_kit::Window;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication, NSColor, NSScreen,
    NSWindow, NSWindowButton, NSWindowCollectionBehavior, NSWindowStyleMask, NSWindowTabbingMode,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use super::ns_window;

pub struct NativeWindow(Retained<NSWindow>);

pub fn native(window: &Window) -> Option<NativeWindow> {
    ns_window(window).map(NativeWindow)
}

pub fn titled_content_size(width: f64, height: f64) -> (f64, f64) {
    let Some(mtm) = MainThreadMarker::new() else {
        return (width, height);
    };
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height));
    let content = NSWindow::contentRectForFrameRect_styleMask(frame, style, mtm);
    (content.size.width, content.size.height)
}

pub fn title_bar_height() -> f64 {
    const FRAME: f64 = 1000.0;
    FRAME - titled_content_size(FRAME, FRAME).1
}

/// GPUI's own background is black, which the title bar's material lets through before macOS 26.
pub fn install_electron_frame(window: &Window) {
    let Some(window) = ns_window(window) else {
        return;
    };
    window.setBackgroundColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(
        1.0, 1.0, 1.0, 1.0,
    )));
    window.setTabbingMode(NSWindowTabbingMode::Disallowed);
}

pub fn set_app_appearance(dark: Option<bool>) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    // SAFETY: immutable NSString constants exported by AppKit.
    let name = dark.map(|dark| unsafe {
        if dark {
            NSAppearanceNameDarkAqua
        } else {
            NSAppearanceNameAqua
        }
    });
    let appearance = name.and_then(NSAppearance::appearanceNamed);
    NSApplication::sharedApplication(mtm).setAppearance(appearance.as_deref());
}

impl NativeWindow {
    pub fn content_size_for_frame(&self, width: f64, height: f64) -> (f64, f64) {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height));
        let content = self.0.contentRectForFrameRect(frame);
        (content.size.width, content.size.height)
    }

    pub fn content_size(&self) -> (f64, f64) {
        let content = self.0.contentRectForFrameRect(self.0.frame());
        (content.size.width, content.size.height)
    }

    pub fn set_content_size(&self, width: f64, height: f64) {
        self.0.setContentSize(NSSize::new(width, height));
    }

    pub fn set_content_origin(&self, _x: f64, _y: f64) {}

    pub fn center(&self, _parent: Option<&NativeWindow>) {}

    pub fn set_content_size_limits(&self, min: Option<(f64, f64)>, max: Option<(f64, f64)>) {
        if let Some((width, height)) = min {
            self.0.setContentMinSize(NSSize::new(width, height));
        }
        if let Some((width, height)) = max {
            self.0.setContentMaxSize(NSSize::new(width, height));
        }
    }

    /// `NativeWindowMac::SetFullScreenable`, then `UpdateZoomButton`.
    pub fn set_fullscreenable(&self, fullscreenable: bool) {
        let mut behavior = self.0.collectionBehavior();
        behavior.set(
            NSWindowCollectionBehavior::FullScreenPrimary,
            fullscreenable,
        );
        behavior.set(
            NSWindowCollectionBehavior::FullScreenAuxiliary,
            !fullscreenable,
        );
        self.0.setCollectionBehavior(behavior);
        if !self.0.styleMask().contains(NSWindowStyleMask::Resizable)
            && let Some(zoom) = self.0.standardWindowButton(NSWindowButton::ZoomButton)
        {
            zoom.setEnabled(false);
        }
    }

    pub fn begin_sheet(&self, sheet: &NativeWindow) {
        self.0.beginSheet_completionHandler(&sheet.0, None);
    }

    pub fn end_sheet(&self) {
        if let Some(parent) = self.0.sheetParent() {
            parent.endSheet(&self.0);
        }
    }

    pub fn is_visible(&self) -> bool {
        self.0.isVisible() && !self.0.isMiniaturized()
    }

    pub fn show(&self, focus: bool) {
        if focus {
            self.0.makeKeyAndOrderFront(None);
        } else {
            self.0.orderFront(None);
        }
    }
}

/// `NativeWindowMac::GetBounds`: the frame in top-left coordinates of the primary screen,
/// truncated to whole points.
pub fn bounds(window: &Window) -> Option<(f64, f64, f64, f64)> {
    let frame = ns_window(window)?.frame();
    let screen = NSScreen::screens(MainThreadMarker::new()?).firstObject()?;
    let top = screen.frame().size.height - (frame.origin.y + frame.size.height);
    Some((
        frame.origin.x.trunc(),
        top.trunc(),
        frame.size.width.trunc(),
        frame.size.height.trunc(),
    ))
}
