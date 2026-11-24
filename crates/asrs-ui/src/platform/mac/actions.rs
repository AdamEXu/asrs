//! What AppKit asks of Electron's classes and not of GPUI's: the role items' actions on
//! `RenderWidgetHostViewCocoa` and `ElectronNSWindow`, `-[AtomApplication terminate:]`, and
//! `ElectronNSWindowDelegate`'s full screen notifications.

use std::cell::RefCell;
use std::ffi::CStr;
use std::ptr;
use std::rc::Rc;

use gpui_kit::{App, AsyncApp, Window};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, NSObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, ffi, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationTerminateReply, NSWindow, NSWindowButton,
    NSWindowDidEnterFullScreenNotification, NSWindowDidExitFullScreenNotification,
    NSWindowTitleVisibility, NSWindowWillEnterFullScreenNotification,
    NSWindowWillExitFullScreenNotification,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSString};

use super::super::EditCommand;
use super::{ns_view, ns_window};

type Action = unsafe extern "C-unwind" fn(&AnyObject, Sel, Option<&AnyObject>);

const ACTION_TYPES: &CStr = c"v@:@";

/// Adds a method to a class, overriding an inherited one but not one of the class's own, which
/// `class_addMethod` leaves as it is.
///
/// # Safety
///
/// `types` is the signature of `implementation`.
unsafe fn add_method(class: &AnyClass, selector: Sel, implementation: Imp, types: &CStr) {
    // SAFETY: as the caller promises.
    let added = unsafe {
        ffi::class_addMethod(
            ptr::from_ref(class).cast_mut(),
            selector,
            implementation,
            types.as_ptr(),
        )
    };
    if !added.as_bool()
        && class
            .instance_method(selector)
            .is_none_or(|method| !ptr::fn_addr_eq(method.implementation(), implementation))
    {
        eprintln!("asrs: {class:?} implements {selector:?} itself");
    }
}

fn add_action(class: &CStr, selector: Sel, action: Action) {
    let Some(class) = AnyClass::get(class) else {
        eprintln!("asrs: no class {class:?}");
        return;
    };
    // SAFETY: an action method's signature, and a function pointer as the runtime takes it.
    unsafe {
        add_method(
            class,
            selector,
            std::mem::transmute::<Action, Imp>(action),
            ACTION_TYPES,
        );
    }
}

type EditHandler = Rc<dyn Fn(EditCommand, &mut Window, &mut App)>;

thread_local! {
    static EDIT: RefCell<Option<(AsyncApp, EditHandler)>> = const { RefCell::new(None) };
}

/// `-[RenderWidgetHostViewCocoa undo:]` and the others.
fn edit_actions() -> [(Sel, EditCommand); 8] {
    [
        (sel!(undo:), EditCommand::Undo),
        (sel!(redo:), EditCommand::Redo),
        (sel!(cut:), EditCommand::Cut),
        (sel!(copy:), EditCommand::Copy),
        (sel!(paste:), EditCommand::Paste),
        (sel!(pasteAndMatchStyle:), EditCommand::PasteAndMatchStyle),
        (sel!(delete:), EditCommand::Delete),
        (sel!(selectAll:), EditCommand::SelectAll),
    ]
}

unsafe extern "C-unwind" fn edit_action(view: &AnyObject, action: Sel, _: Option<&AnyObject>) {
    let command = edit_actions().into_iter().find(|(s, _)| *s == action);
    let Some((_, command)) = command else {
        return;
    };
    let Some((cx, handler)) = EDIT.with_borrow(Clone::clone) else {
        return;
    };
    let view = ptr::from_ref(view) as usize;
    cx.spawn(async move |cx| {
        cx.update(|cx| {
            for handle in cx.windows() {
                let _ = handle.update(cx, |_, window, cx| {
                    if ns_view(window).is_some_and(|v| Retained::as_ptr(&v) as usize == view) {
                        handler(command, window, cx);
                    }
                });
            }
        });
    })
    .detach();
}

unsafe extern "C-unwind" fn no_action(_: &AnyObject, _: Sel, _: Option<&AnyObject>) {}

unsafe extern "C-unwind" fn always_valid(_: &AnyObject, _: Sel, _: Option<&AnyObject>) -> Bool {
    Bool::YES
}

/// GPUI's app delegate takes the editing actions for menus GPUI built. From any other menu its
/// `handle_menu_item` and `validate_menu_item` lock the platform twice and never return, so where
/// no responder before it takes an action, as in an alert or without windows, it must not either.
fn disable_delegate_actions() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(delegate) = NSApplication::sharedApplication(mtm).delegate() else {
        return;
    };
    // SAFETY: `class` takes no arguments and returns a class pointer.
    let class: &AnyClass = unsafe { msg_send![&*delegate, class] };
    for (selector, _) in edit_actions() {
        // SAFETY: both are action methods.
        unsafe {
            replace_own_method(
                class,
                selector,
                std::mem::transmute::<Action, Imp>(no_action),
            );
        }
    }
    type Validate = unsafe extern "C-unwind" fn(&AnyObject, Sel, Option<&AnyObject>) -> Bool;
    // SAFETY: both have NSMenuItemValidation's signature.
    unsafe {
        replace_own_method(
            class,
            sel!(validateMenuItem:),
            std::mem::transmute::<Validate, Imp>(always_valid),
        );
    }
}

/// Leaves an inherited method alone, which other classes share.
///
/// # Safety
///
/// `implementation` has the signature of the method.
unsafe fn replace_own_method(class: &AnyClass, selector: Sel, implementation: Imp) {
    let inherited = class
        .superclass()
        .and_then(|above| above.instance_method(selector));
    if let Some(method) = class.instance_method(selector)
        && !inherited.is_some_and(|inherited| ptr::eq(inherited, method))
    {
        // SAFETY: as the caller promises.
        unsafe { method.set_implementation(implementation) };
    }
}

pub(in super::super) fn install_edit_commands(
    cx: &mut App,
    on_command: impl Fn(EditCommand, &mut Window, &mut App) + 'static,
) {
    EDIT.set(Some((cx.to_async(), Rc::new(on_command))));
    for (selector, _) in edit_actions() {
        add_action(c"GPUIView", selector, edit_action);
    }
    disable_delegate_actions();
    for class in [c"GPUIWindow", c"GPUIPanel"] {
        add_action(class, sel!(toggleFullScreenMode:), toggle_full_screen_mode);
        add_action(
            class,
            sel!(windowDidFailToEnterFullScreen:),
            did_fail_to_enter,
        );
    }
}

/// `-[ElectronNSWindow toggleFullScreenMode:]`.
unsafe extern "C-unwind" fn toggle_full_screen_mode(
    window: &AnyObject,
    _: Sel,
    sender: Option<&AnyObject>,
) {
    let Some(window) = window.downcast_ref::<NSWindow>() else {
        return;
    };
    if !window.isVisible() {
        window.toggleFullScreen(sender);
        return;
    }
    // SAFETY: `toggleFullScreen:` takes a sender, and the run loop retains the window.
    unsafe {
        let _: () = msg_send![
            window,
            performSelector: sel!(toggleFullScreen:),
            withObject: None::<&AnyObject>,
            afterDelay: 0.0f64
        ];
    }
}

const DID_FAIL_TO_ENTER: &str = "asrs.windowDidFailToEnterFullScreen";

/// A delegate method AppKit has no notification for; GPUI's windows are their own delegates.
unsafe extern "C-unwind" fn did_fail_to_enter(window: &AnyObject, _: Sel, _: Option<&AnyObject>) {
    let name = NSString::from_str(DID_FAIL_TO_ENTER);
    let center = NSNotificationCenter::defaultCenter();
    // SAFETY: a notification with the window as its object, as AppKit's own have.
    unsafe { center.postNotificationName_object(&name, Some(window)) };
}

type MayQuit = Rc<dyn Fn() -> bool>;
type Ask = Rc<dyn Fn(&mut App)>;

#[derive(Clone)]
struct QuitHandler {
    cx: AsyncApp,
    may_quit: MayQuit,
    ask: Ask,
}

thread_local! {
    static QUIT: RefCell<Option<QuitHandler>> = const { RefCell::new(None) };
}

/// AppKit may ask from inside an update of the app, so only `may_quit` answers it.
unsafe extern "C-unwind" fn should_terminate(
    _: &AnyObject,
    _: Sel,
    _: Option<&AnyObject>,
) -> NSApplicationTerminateReply {
    let Some(QuitHandler { cx, may_quit, ask }) = QUIT.with_borrow(Clone::clone) else {
        return NSApplicationTerminateReply::TerminateNow;
    };
    if may_quit() {
        return NSApplicationTerminateReply::TerminateNow;
    }
    cx.spawn(async move |cx| cx.update(|cx| ask(cx))).detach();
    NSApplicationTerminateReply::TerminateCancel
}

pub(in super::super) fn on_quit_request(
    cx: &mut App,
    may_quit: impl Fn() -> bool + 'static,
    ask: impl Fn(&mut App) + 'static,
) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    QUIT.set(Some(QuitHandler {
        cx: cx.to_async(),
        may_quit: Rc::new(may_quit),
        ask: Rc::new(ask),
    }));
    let Some(delegate) = NSApplication::sharedApplication(mtm).delegate() else {
        return;
    };
    // SAFETY: `class` takes no arguments and returns a class pointer.
    let class: &AnyClass = unsafe { msg_send![&*delegate, class] };
    type Reply = unsafe extern "C-unwind" fn(
        &AnyObject,
        Sel,
        Option<&AnyObject>,
    ) -> NSApplicationTerminateReply;
    // SAFETY: NSApplicationDelegate's signature, and a function pointer as the runtime takes it.
    unsafe {
        add_method(
            class,
            sel!(applicationShouldTerminate:),
            std::mem::transmute::<Reply, Imp>(should_terminate),
            c"Q@:@",
        );
    }
}

type OnChange = Rc<dyn Fn(bool, &mut App)>;

struct FullScreenIvars {
    cx: AsyncApp,
    on_change: OnChange,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AsrsFullScreenObserver"]
    #[ivars = FullScreenIvars]
    struct Observer;

    impl Observer {
        /// A toolbar stays on screen in full screen, where Electron's hidden inset title bar has
        /// none.
        #[unsafe(method(willEnter:))]
        fn will_enter(&self, notification: &NSNotification) {
            if let Some(window) = window_of(notification) {
                window.setToolbar(None);
            }
        }

        #[unsafe(method(didEnter:))]
        fn did_enter(&self, notification: &NSNotification) {
            if let Some(window) = window_of(notification) {
                window.setTitleVisibility(NSWindowTitleVisibility::Visible);
            }
            self.changed(true);
        }

        #[unsafe(method(didFailToEnter:))]
        fn did_fail_to_enter(&self, notification: &NSNotification) {
            if let Some(window) = window_of(notification) {
                super::window::add_hidden_inset_toolbar(&window);
            }
        }

        /// `NativeWindowMac::NotifyWindowWillLeaveFullScreen`.
        #[unsafe(method(willExit:))]
        fn will_exit(&self, notification: &NSNotification) {
            if let Some(window) = window_of(notification) {
                window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
                show_buttons(&window, false);
            }
        }

        /// `NativeWindowMac::NotifyWindowLeaveFullScreen`.
        #[unsafe(method(didExit:))]
        fn did_exit(&self, notification: &NSNotification) {
            if let Some(window) = window_of(notification) {
                super::window::add_hidden_inset_toolbar(&window);
                show_buttons(&window, true);
            }
            self.changed(false);
        }
    }
);

impl Observer {
    fn changed(&self, full_screen: bool) {
        let FullScreenIvars { cx, on_change } = self.ivars();
        let on_change = on_change.clone();
        cx.spawn(async move |cx| cx.update(|cx| on_change(full_screen, cx)))
            .detach();
    }
}

fn window_of(notification: &NSNotification) -> Option<Retained<NSWindow>> {
    notification.object()?.downcast::<NSWindow>().ok()
}

fn show_buttons(window: &NSWindow, show: bool) {
    for button in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        if let Some(button) = window.standardWindowButton(button) {
            button.setHidden(!show);
        }
    }
}

/// `enter-full-screen` and `leave-full-screen`, until it is dropped.
pub struct FullScreenObserver(#[allow(dead_code)] Retained<Observer>);

pub(in super::super) fn observe_full_screen(
    window: &Window,
    cx: &App,
    on_change: impl Fn(bool, &mut App) + 'static,
) -> Option<FullScreenObserver> {
    let window = ns_window(window)?;
    let observer = Observer::alloc(MainThreadMarker::new()?).set_ivars(FullScreenIvars {
        cx: cx.to_async(),
        on_change: Rc::new(on_change),
    });
    // SAFETY: NSObject's designated initializer on a freshly allocated instance.
    let observer: Retained<Observer> = unsafe { msg_send![super(observer), init] };
    let center = NSNotificationCenter::defaultCenter();
    let failed = NSString::from_str(DID_FAIL_TO_ENTER);
    // SAFETY: AppKit's notification names.
    let names = unsafe {
        [
            (NSWindowWillEnterFullScreenNotification, sel!(willEnter:)),
            (NSWindowDidEnterFullScreenNotification, sel!(didEnter:)),
            (&*failed, sel!(didFailToEnter:)),
            (NSWindowWillExitFullScreenNotification, sel!(willExit:)),
            (NSWindowDidExitFullScreenNotification, sel!(didExit:)),
        ]
    };
    for (name, selector) in names {
        // SAFETY: the observer implements the selectors with the notification as their argument,
        // and the center lets go of an observer that is gone.
        unsafe {
            center.addObserver_selector_name_object(&observer, selector, Some(name), Some(&window));
        }
    }
    Some(FullScreenObserver(observer))
}
