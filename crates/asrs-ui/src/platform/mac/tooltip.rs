//! Chromium's `ToolTipBaseView`: a tooltip that follows what the page has under the pointer. AppKit
//! shows a view's tooltip as the pointer enters one of its tracking rects, so the view keeps the
//! owner of the tooltip's rect and tells it the pointer entered.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use gpui_kit::Window;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSEvent, NSEventType, NSView, NSWindow,
};
use objc2_foundation::{NSInteger, NSPoint, NSRect, NSSize, NSString};

use super::ns_view;

const TRACKING_RECT: NSInteger = 0xBAD_FACE;

#[derive(Default)]
struct Ivars {
    /// Not retained: AppKit's tooltip manager, which lives as long as the app.
    owner: Cell<Option<*const AnyObject>>,
    user_data: Cell<usize>,
    last_tag: Cell<NSInteger>,
    text: RefCell<Option<Retained<NSString>>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "AsrsToolTipView"]
    #[ivars = Ivars]
    struct ToolTipView;

    impl ToolTipView {
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> Option<Retained<NSView>> {
            None
        }

        #[unsafe(method(addTrackingRect:owner:userData:assumeInside:))]
        fn add_tracking_rect(
            &self,
            _rect: NSRect,
            owner: &AnyObject,
            data: *mut c_void,
            _inside: Bool,
        ) -> NSInteger {
            self.track(owner, data)
        }

        #[unsafe(method(_addTrackingRect:owner:userData:assumeInside:useTrackingNum:))]
        fn add_tracking_rect_numbered(
            &self,
            _rect: NSRect,
            owner: &AnyObject,
            data: *mut c_void,
            _inside: Bool,
            _tag: i32,
        ) -> NSInteger {
            self.track(owner, data)
        }

        #[unsafe(method(_addTrackingRects:owner:userDataList:assumeInsideList:trackingNums:count:))]
        fn add_tracking_rects(
            &self,
            _rects: *mut NSRect,
            owner: &AnyObject,
            data: *mut *mut c_void,
            _inside: *mut Bool,
            tags: *mut NSInteger,
            count: i32,
        ) {
            for i in 0..usize::try_from(count).unwrap_or(0) {
                // SAFETY: both arrays have `count` elements.
                unsafe { *tags.add(i) = self.track(owner, *data.add(i)) };
            }
        }

        #[unsafe(method(removeTrackingRect:))]
        fn remove_tracking_rect(&self, tag: NSInteger) {
            if tag == TRACKING_RECT {
                self.ivars().owner.set(None);
            } else if tag != 0 {
                if tag == self.ivars().last_tag.get() {
                    self.ivars().last_tag.set(0);
                }
                // SAFETY: NSView's own method with its own argument.
                let () = unsafe { msg_send![super(self), removeTrackingRect: tag] };
            }
        }

        #[unsafe(method(_removeTrackingRects:count:))]
        fn remove_tracking_rects(&self, _tags: *mut NSInteger, _count: i32) {
            self.ivars().owner.set(None);
        }

        #[unsafe(method_id(view:stringForToolTip:point:userData:))]
        fn string_for_tool_tip(
            &self,
            _view: &NSView,
            _tag: NSInteger,
            _point: NSPoint,
            _data: *mut c_void,
        ) -> Option<Retained<NSString>> {
            self.ivars().text.borrow().clone()
        }
    }
);

impl ToolTipView {
    fn new(parent: &NSView, mtm: MainThreadMarker) -> Retained<ToolTipView> {
        let view = ToolTipView::alloc(mtm).set_ivars(Ivars::default());
        // SAFETY: NSView's designated initializer on a freshly allocated instance.
        let view: Retained<ToolTipView> =
            unsafe { msg_send![super(view), initWithFrame: parent.bounds()] };
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        parent.addSubview(&view);
        view
    }

    fn track(&self, owner: &AnyObject, data: *mut c_void) -> NSInteger {
        self.ivars().owner.set(Some(owner));
        self.ivars().user_data.set(data as usize);
        TRACKING_RECT
    }

    /// `_toolTipOwnerForSendingMouseEvents`.
    fn owner(&self) -> Option<Retained<AnyObject>> {
        if let Some(owner) = self.ivars().owner.get() {
            // SAFETY: the tooltip manager, which AppKit does not deallocate.
            return unsafe { Retained::retain(owner.cast_mut()) };
        }
        self.trackingAreas().iter().find_map(|area| {
            let owner = area.owner()?;
            (owner.class().name() == c"NSToolTipManager").then_some(owner)
        })
    }

    fn send(&self, entered: bool) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(window) = self.window() else {
            return;
        };
        let number = window.windowNumber();
        if entered
            && NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(
                NSEvent::mouseLocation(),
                0,
                mtm,
            ) != number
        {
            return;
        }
        let event_type = if entered {
            NSEventType::MouseEntered
        } else {
            NSEventType::MouseExited
        };
        let timestamp = NSApplication::sharedApplication(mtm)
            .currentEvent()
            .map_or(0.0, |event| event.timestamp());
        // SAFETY: the event carries the user data AppKit gave for the tracking rect.
        let event: Option<Retained<NSEvent>> = unsafe {
            msg_send![
                <NSEvent as objc2::ClassType>::class(),
                enterExitEventWithType: event_type,
                location: NSPoint::ZERO,
                modifierFlags: objc2_app_kit::NSEventModifierFlags::empty(),
                timestamp: timestamp,
                windowNumber: number,
                context: None::<&AnyObject>,
                eventNumber: 0 as NSInteger,
                trackingNumber: TRACKING_RECT,
                userData: self.ivars().user_data.get() as *mut c_void,
            ]
        };
        let (Some(event), Some(owner)) = (event, self.owner()) else {
            return;
        };
        // SAFETY: both take an NSEvent; the tooltip manager implements them.
        unsafe {
            if entered {
                let () = msg_send![&*owner, mouseEntered: &*event];
            } else {
                let () = msg_send![&*owner, mouseExited: &*event];
            }
        }
    }

    /// `setToolTipAtMousePoint:`.
    fn set(&self, text: &str) {
        let text = (!text.is_empty()).then(|| NSString::from_str(text));
        if *self.ivars().text.borrow() == text {
            return;
        }
        if self.ivars().text.borrow().is_some() {
            self.send(false);
        }
        let shows = text.is_some();
        self.ivars().text.replace(text);
        self.removeAllToolTips();
        if shows {
            let wide_open = NSRect::new(
                NSPoint::new(-100_000.0, -100_000.0),
                NSSize::new(200_000.0, 200_000.0),
            );
            // SAFETY: the owner is this view, which outlives its tooltip rect, and the user data
            // is null.
            let tag: NSInteger = unsafe {
                msg_send![
                    self,
                    addToolTipRect: wide_open,
                    owner: self,
                    userData: std::ptr::null_mut::<c_void>(),
                ]
            };
            self.ivars().last_tag.set(tag);
            self.send(true);
        }
    }
}

fn tool_tip_view(view: &NSView) -> Option<Retained<ToolTipView>> {
    (view.subviews().iter()).find_map(|view| view.downcast::<ToolTipView>().ok())
}

pub(in crate::platform) fn set_tooltip(window: &Window, text: &str) {
    let (Some(mtm), Some(parent)) = (MainThreadMarker::new(), ns_view(window)) else {
        return;
    };
    let view = tool_tip_view(&parent).unwrap_or_else(|| ToolTipView::new(&parent, mtm));
    view.set(text);
}

/// What AppKit gets for each of `texts`, set one after the other on one view, and whether its
/// tooltip manager then owns the view's tracking rect.
pub(super) fn roundtrip(texts: &[&str]) -> Vec<(Option<String>, bool)> {
    let mtm = MainThreadMarker::new().expect("the main thread");
    let frame = NSRect::new(NSPoint::ZERO, NSSize::new(100.0, 100.0));
    // SAFETY: the window is never shown, and not released by closing it.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            objc2_app_kit::NSWindowStyleMask::Borderless,
            objc2_app_kit::NSBackingStoreType::Buffered,
            true,
        )
    };
    let parent = window.contentView().expect("a content view");
    let view = ToolTipView::new(&parent, mtm);
    (texts.iter())
        .map(|text| {
            view.set(text);
            // SAFETY: the view's own method; nothing but its text goes into the answer.
            let shown: Option<Retained<NSString>> = unsafe {
                msg_send![
                    &*view,
                    view: &*view,
                    stringForToolTip: 0 as NSInteger,
                    point: NSPoint::ZERO,
                    userData: std::ptr::null_mut::<c_void>(),
                ]
            };
            (shown.map(|s| s.to_string()), view.owner().is_some())
        })
        .collect()
}
