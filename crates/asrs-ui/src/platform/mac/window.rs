//! The parts of Electron's `NativeWindowMac` GPUI's window options do not cover.

use gpui_kit::Window;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSScreen, NSToolbar, NSWindow, NSWindowCollectionBehavior};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use super::ns_window;

pub struct NativeWindow(Retained<NSWindow>);

pub fn native(window: &Window) -> Option<NativeWindow> {
    ns_window(window).map(NativeWindow)
}

#[cfg(feature = "parity")]
pub fn titled_content_size(width: f64, height: f64) -> (f64, f64) {
    use objc2_app_kit::NSWindowStyleMask;

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

impl NativeWindow {
    pub fn hidden_inset_titlebar(&self) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let toolbar = NSToolbar::initWithIdentifier(
            NSToolbar::alloc(mtm),
            &NSString::from_str("titlebarStylingToolbar"),
        );
        #[allow(deprecated)]
        toolbar.setShowsBaselineSeparator(false);
        self.0.setToolbar(Some(&toolbar));
    }

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

    pub fn set_content_size_limits(&self, min: Option<(f64, f64)>, max: Option<(f64, f64)>) {
        if let Some((width, height)) = min {
            self.0.setContentMinSize(NSSize::new(width, height));
        }
        if let Some((width, height)) = max {
            self.0.setContentMaxSize(NSSize::new(width, height));
        }
    }

    /// `NativeWindowMac::SetFullScreenable`.
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
