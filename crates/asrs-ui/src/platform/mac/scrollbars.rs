//! `ThemeHelperMac`'s notifications of the scrollbar preferences.

use std::rc::Rc;

use gpui_kit::{App, AsyncApp};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, NSObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSString};

type OnChange = Rc<dyn Fn(&mut App)>;

/// When `preferredScrollerStyle` changes, as it does when a mouse is plugged in.
const STYLE: &str = "NSPreferredScrollerStyleDidChangeNotification";
/// When the preference for a press on a track does.
const BEHAVIOR: &str = "AppleNoRedisplayAppearancePreferenceChanged";

pub struct Ivars {
    cx: AsyncApp,
    on_change: OnChange,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AsrsScrollbarObserver"]
    #[ivars = Ivars]
    struct Observer;

    impl Observer {
        #[unsafe(method(preferencesDidChange:))]
        fn preferences_did_change(&self, _: &NSNotification) {
            let Ivars { cx, on_change } = self.ivars();
            let on_change = on_change.clone();
            cx.spawn(async move |cx| cx.update(|cx| on_change(cx)))
                .detach();
        }
    }
);

/// Calls `on_change` for the rest of the app's life.
pub fn observe(cx: &App, on_change: impl Fn(&mut App) + 'static) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let observer = Observer::alloc(mtm).set_ivars(Ivars {
        cx: cx.to_async(),
        on_change: Rc::new(on_change),
    });
    // SAFETY: NSObject's designated initializer on a freshly allocated instance.
    let observer: Retained<Observer> = unsafe { msg_send![super(observer), init] };
    let distributed = AnyClass::get(c"NSDistributedNotificationCenter").and_then(|class| {
        // SAFETY: a class method without arguments, whose result is an NSNotificationCenter.
        let center: Option<Retained<NSNotificationCenter>> =
            unsafe { msg_send![class, defaultCenter] };
        center
    });
    let centers = [
        (Some(NSNotificationCenter::defaultCenter()), STYLE),
        (distributed, BEHAVIOR),
    ];
    for (center, name) in centers {
        let Some(center) = center else {
            continue;
        };
        let (name, selector) = (NSString::from_str(name), sel!(preferencesDidChange:));
        // SAFETY: the observer implements the selector with the notification as its argument,
        // and it is never released.
        unsafe { center.addObserver_selector_name_object(&observer, selector, Some(&name), None) };
    }
    std::mem::forget(observer);
}

pub fn post_style_change() {
    let name = NSString::from_str(STYLE);
    // SAFETY: AppKit posts it without an object.
    unsafe { NSNotificationCenter::defaultCenter().postNotificationName_object(&name, None) };
}
