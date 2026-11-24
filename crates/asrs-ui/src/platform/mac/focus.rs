//! `ElectronNSWindowDelegate`'s main window notifications.

use std::rc::Rc;

use gpui_kit::{App, AsyncApp, Window};
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSWindowDidBecomeMainNotification, NSWindowDidResignMainNotification};
use objc2_foundation::{NSNotification, NSNotificationCenter};

use super::ns_window;

type OnChange = Rc<dyn Fn(bool, &mut App)>;

pub struct Ivars {
    cx: AsyncApp,
    on_change: OnChange,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AsrsFocusObserver"]
    #[ivars = Ivars]
    pub struct FocusObserver;

    impl FocusObserver {
        #[unsafe(method(didBecomeMain:))]
        fn did_become_main(&self, _: &NSNotification) {
            self.changed(true);
        }

        #[unsafe(method(didResignMain:))]
        fn did_resign_main(&self, _: &NSNotification) {
            self.changed(false);
        }
    }
);

impl FocusObserver {
    fn changed(&self, focused: bool) {
        let Ivars { cx, on_change } = self.ivars();
        let on_change = on_change.clone();
        cx.spawn(async move |cx| cx.update(|cx| on_change(focused, cx)))
            .detach();
    }
}

pub fn observe(
    window: &Window,
    cx: &App,
    on_change: impl Fn(bool, &mut App) + 'static,
) -> Option<Retained<FocusObserver>> {
    let window = ns_window(window)?;
    let observer = FocusObserver::alloc(MainThreadMarker::new()?).set_ivars(Ivars {
        cx: cx.to_async(),
        on_change: Rc::new(on_change),
    });
    // SAFETY: NSObject's designated initializer on a freshly allocated instance.
    let observer: Retained<FocusObserver> = unsafe { msg_send![super(observer), init] };
    let center = NSNotificationCenter::defaultCenter();
    // SAFETY: AppKit's notification names.
    let names = unsafe {
        [
            (NSWindowDidBecomeMainNotification, sel!(didBecomeMain:)),
            (NSWindowDidResignMainNotification, sel!(didResignMain:)),
        ]
    };
    for (name, selector) in names {
        // SAFETY: the observer implements the selectors with the notification as their argument,
        // and the center lets go of an observer that is gone.
        unsafe {
            center.addObserver_selector_name_object(&observer, selector, Some(name), Some(&window));
        }
    }
    Some(observer)
}
