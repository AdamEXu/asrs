use std::cell::{Cell, RefCell};
use std::ffi::CStr;
use std::ptr::{self, NonNull};

use block2::RcBlock;
use gpui_kit::{App, ScrollEvent, Window};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, NSObject, Sel};
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, ffi, msg_send,
    sel,
};
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSBitmapImageRep, NSControlStateValueOff,
    NSControlStateValueOn, NSEvent, NSEventMask, NSEventModifierFlags, NSImage, NSMenu, NSMenuItem,
    NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSVisualEffectView, NSWindow, NSWindowOrderingMode,
};
use objc2_core_foundation::{CFData, CGRect};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, kCGColorSpaceSRGB,
};
use objc2_foundation::{NSArray, NSNumber, NSPoint, NSRunLoopCommonModes, NSSize, NSString};
use objc2_quartz_core::CAMetalLayer;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::probe::GpuiDelta;
use super::{MenuPath, PopupEntry, PopupItem, PopupMenu, SWATCH_SIZE, WheelDelta};

pub(super) mod dialog;
pub(super) mod window;

pub(super) fn init() {
    if MainThreadMarker::new().is_some() {
        install_wheel_monitor();
    }
}

fn ns_view(window: &Window) -> Option<Retained<NSView>> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    // SAFETY: GPUI's AppKit handle points at the window's live GPUIView, and retaining it keeps
    // it alive for as long as the caller holds it.
    unsafe { Retained::retain(handle.ns_view.as_ptr().cast::<NSView>()) }
}

fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
    ns_view(window)?.window()
}

pub(super) fn install_sidebar_vibrancy(window: &Window) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let Some(content) = ns_window(window).and_then(|w| w.contentView()) else {
        return false;
    };
    if find_vibrancy(&content).is_some() {
        return true;
    }
    let effect =
        NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), content.bounds());
    effect.setMaterial(NSVisualEffectMaterial::Sidebar);
    effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    effect.setState(NSVisualEffectState::FollowsWindowActiveState);
    effect.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    content.addSubview_positioned_relativeTo(&effect, NSWindowOrderingMode::Below, None);
    true
}

fn find_vibrancy(content: &NSView) -> Option<Retained<NSVisualEffectView>> {
    content
        .subviews()
        .iter()
        .find_map(|view| view.downcast::<NSVisualEffectView>().ok())
}

pub(super) fn keep_visible_when_inactive(window: &Window) -> bool {
    let Some(window) = ns_window(window) else {
        return false;
    };
    window.setHidesOnDeactivate(false);
    true
}

pub(super) fn tag_srgb_colorspace(window: &Window) -> bool {
    let Some(layer) = metal_layer(window) else {
        return false;
    };
    // SAFETY: an immutable CFString constant exported by CoreGraphics.
    let name = unsafe { kCGColorSpaceSRGB };
    let Some(srgb) = CGColorSpace::with_name(Some(name)) else {
        return false;
    };
    layer.setColorspace(Some(&srgb));
    true
}

fn metal_layer(window: &Window) -> Option<Retained<CAMetalLayer>> {
    ns_view(window)?.layer()?.downcast::<CAMetalLayer>().ok()
}

static ASPECT_RATIO_KEY: u8 = 0;

pub(super) fn set_aspect_ratio(window: &Window, aspect_ratio: f64) {
    let Some(window) = ns_window(window) else {
        return;
    };
    install_will_resize(&window);
    let value = NSNumber::new_f64(aspect_ratio);
    // SAFETY: the key is a unique static address and the policy retains the NSNumber, which is
    // released together with the window.
    unsafe {
        ffi::objc_setAssociatedObject(
            Retained::as_ptr(&window).cast_mut().cast(),
            (&raw const ASPECT_RATIO_KEY).cast(),
            Retained::as_ptr(&value).cast_mut().cast(),
            ffi::OBJC_ASSOCIATION_RETAIN_NONATOMIC,
        );
    }
    if aspect_ratio > 0.0 {
        window.setContentAspectRatio(NSSize::new(aspect_ratio, 1.0));
    } else {
        window.setResizeIncrements(NSSize::new(1.0, 1.0));
    }
}

fn stored_aspect_ratio(window: &NSWindow) -> f64 {
    // SAFETY: the associated object under this key is only ever an NSNumber set above.
    let value = unsafe {
        ffi::objc_getAssociatedObject(
            ptr::from_ref(window).cast(),
            (&raw const ASPECT_RATIO_KEY).cast(),
        )
    };
    let value = value.cast::<NSNumber>();
    if value.is_null() {
        0.0
    } else {
        // SAFETY: non-null and kept alive by the window's association.
        unsafe { &*value }.doubleValue()
    }
}

const WILL_RESIZE_TYPES: &CStr = c"{CGSize=dd}@:@{CGSize=dd}";

fn install_will_resize(window: &NSWindow) {
    // `-[NSObject respondsToSelector:]`, which AppKit asks before calling delegate methods, looks
    // in `[self class]`. Key-value observing makes that the class above the isa class.
    // SAFETY: `class` takes no arguments and returns a class pointer.
    let class: &AnyClass = unsafe { msg_send![window, class] };
    let imp: unsafe extern "C-unwind" fn(&NSWindow, Sel, &NSWindow, NSSize) -> NSSize =
        window_will_resize;
    // SAFETY: the implementation's signature matches `WILL_RESIZE_TYPES`, which is the
    // NSWindowDelegate signature of `windowWillResize:toSize:`. GPUI windows are their own
    // delegate and do not implement it, and `class_addMethod` leaves an existing one untouched.
    unsafe {
        ffi::class_addMethod(
            ptr::from_ref(class).cast_mut(),
            sel!(windowWillResize:toSize:),
            std::mem::transmute::<
                unsafe extern "C-unwind" fn(&NSWindow, Sel, &NSWindow, NSSize) -> NSSize,
                Imp,
            >(imp),
            WILL_RESIZE_TYPES.as_ptr(),
        );
    }
    let delegate = window.delegate();
    window.setDelegate(None);
    window.setDelegate(delegate.as_deref());
}

unsafe extern "C-unwind" fn window_will_resize(
    _this: &NSWindow,
    _cmd: Sel,
    sender: &NSWindow,
    frame_size: NSSize,
) -> NSSize {
    let aspect_ratio = stored_aspect_ratio(sender);
    if aspect_ratio > 0.0 {
        electron_aspect_resize(sender, aspect_ratio, frame_size)
    } else {
        frame_size
    }
}

/// `-[ElectronNSWindowDelegate windowWillResize:toSize:]` for a framed window with no extra size,
/// as of Electron 39, which upstream pins. Later Electron versions changed the min and max clamps.
fn electron_aspect_resize(window: &NSWindow, aspect_ratio: f64, frame_size: NSSize) -> NSSize {
    let frame = window.frame();
    let content = window.contentRectForFrameRect(frame);
    let window_size = (frame.size.width as i32, frame.size.height as i32);
    let content_size = (content.size.width as i32, content.size.height as i32);
    let title_bar_height = f64::from(window_size.1 - content_size.1);
    let extra_width_plus_frame = f64::from(window_size.0 - content_size.0);
    let extra_height_plus_frame = title_bar_height;

    let mut width = roundf(
        (frame_size.height - extra_height_plus_frame) * aspect_ratio + extra_width_plus_frame,
    );
    let mut height =
        roundf((width - extra_width_plus_frame) / aspect_ratio + extra_height_plus_frame);

    let min_size = window.minSize();
    let zero_size = NSSize::new(0.0, title_bar_height);
    if min_size != zero_size {
        let min_width_for_aspect_ratio = (min_size.height - title_bar_height) * aspect_ratio;
        let at_min_height = min_size.height > zero_size.height && height <= min_size.height;
        width = if at_min_height {
            min_width_for_aspect_ratio
        } else {
            cpp_max(width, min_size.width)
        };
        let min_height_for_aspect_ratio = min_size.width / aspect_ratio;
        let at_min_width = min_size.width > zero_size.width && width <= min_size.width;
        height = if at_min_width {
            min_height_for_aspect_ratio
        } else {
            cpp_max(height, min_size.height)
        };
    }

    let max_size = window.maxSize();
    let flt_max = f64::from(f32::MAX);
    if max_size != NSSize::new(flt_max, flt_max) {
        let max_width_for_aspect_ratio = max_size.height * aspect_ratio;
        let at_max_height = max_size.height < flt_max && height >= max_size.height;
        width = if at_max_height {
            max_width_for_aspect_ratio
        } else {
            cpp_min(width, max_size.width)
        };
        let max_height_for_aspect_ratio = max_size.width / aspect_ratio;
        let at_max_width = max_size.width < flt_max && width >= max_size.width;
        height = if at_max_width {
            max_height_for_aspect_ratio
        } else {
            cpp_min(height, max_size.height)
        };
    }
    NSSize::new(width, height)
}

fn roundf(value: f64) -> f64 {
    f64::from((value as f32).round())
}

fn cpp_max(a: f64, b: f64) -> f64 {
    if a < b { b } else { a }
}

fn cpp_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

pub(super) fn native_menu_label(label: &str) -> String {
    let units: Vec<u16> = label.encode_utf16().collect();
    let mut out = Vec::with_capacity(units.len());
    let len = units.len();
    let mut i = 0;
    while i < len {
        let c = units[i];
        if c == u16::from(b'(') && i + 3 < len && units[i + 1] == u16::from(b'&') {
            if units[i + 3] == u16::from(b')') {
                i += 4;
                continue;
            }
            out.push(c);
        } else if c == u16::from(b'&') {
            if i + 1 < len && units[i + 1] == u16::from(b'&') {
                out.push(c);
                i += 1;
            }
        } else if c == u16::from(b'.')
            && i + 2 < len
            && units[i + 1] == u16::from(b'.')
            && units[i + 2] == u16::from(b'.')
        {
            out.push(0x2026);
            i += 2;
        } else {
            out.push(c);
        }
        i += 1;
    }
    String::from_utf16_lossy(&out)
}

struct PopupTargetIvars {
    paths: RefCell<Vec<MenuPath>>,
    selected: Cell<Option<usize>>,
    on_select: Option<Box<dyn Fn(MenuPath)>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AsrsPopupMenuTarget"]
    #[ivars = PopupTargetIvars]
    struct PopupTarget;

    impl PopupTarget {
        #[unsafe(method(itemSelected:))]
        fn item_selected(&self, sender: &NSMenuItem) {
            let index = usize::try_from(sender.tag()).ok();
            self.ivars().selected.set(index);
            if let Some(on_select) = &self.ivars().on_select {
                let path = index.and_then(|i| self.ivars().paths.borrow().get(i).cloned());
                if let Some(path) = path {
                    on_select(path);
                }
            }
        }
    }
);

impl PopupTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        Self::with_callback(mtm, None)
    }

    fn with_callback(
        mtm: MainThreadMarker,
        on_select: Option<Box<dyn Fn(MenuPath)>>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PopupTargetIvars {
            paths: RefCell::new(Vec::new()),
            selected: Cell::new(None),
            on_select,
        });
        // SAFETY: NSObject's designated initializer on a freshly allocated instance.
        unsafe { msg_send![super(this), init] }
    }

    fn selection(&self) -> Option<MenuPath> {
        let index = self.ivars().selected.get()?;
        self.ivars().paths.borrow().get(index).cloned()
    }
}

/// `kRolesMap` in `electron_menu_controller.mm`: roles AppKit performs through the responder
/// chain instead of the item's target.
const ROLE_SELECTORS: &[(&str, &CStr)] = &[
    ("about", c"orderFrontStandardAboutPanel:"),
    ("hide", c"hide:"),
    ("hideothers", c"hideOtherApplications:"),
    ("unhide", c"unhideAllApplications:"),
    ("front", c"arrangeInFront:"),
    ("undo", c"undo:"),
    ("redo", c"redo:"),
    ("cut", c"cut:"),
    ("copy", c"copy:"),
    ("paste", c"paste:"),
    ("delete", c"delete:"),
    ("pasteandmatchstyle", c"pasteAndMatchStyle:"),
    ("selectall", c"selectAll:"),
    ("showsubstitutions", c"orderFrontSubstitutionsPanel:"),
    ("togglesmartquotes", c"toggleAutomaticQuoteSubstitution:"),
    ("togglesmartdashes", c"toggleAutomaticDashSubstitution:"),
    ("toggletextreplacement", c"toggleAutomaticTextReplacement:"),
    ("startspeaking", c"startSpeaking:"),
    ("stopspeaking", c"stopSpeaking:"),
    ("minimize", c"performMiniaturize:"),
    ("close", c"performClose:"),
    ("zoom", c"performZoom:"),
    ("quit", c"terminate:"),
    ("togglefullscreen", c"toggleFullScreenMode:"),
    ("toggletabbar", c"toggleTabBar:"),
    ("selectnexttab", c"selectNextTab:"),
    ("showalltabs", c"toggleTabOverview:"),
    ("selectprevioustab", c"selectPreviousTab:"),
    ("mergeallwindows", c"mergeAllWindows:"),
    ("movetabtonewwindow", c"moveTabToNewWindow:"),
    ("clearrecentdocuments", c"clearRecentDocuments:"),
];

/// `-[ElectronMenuController menuFromModel:]`.
fn build_menu(
    mtm: MainThreadMarker,
    menu: &PopupMenu,
    target: &PopupTarget,
    prefix: &[usize],
) -> Retained<NSMenu> {
    let ns_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::new());
    ns_menu.setAutoenablesItems(false);
    retain_target(&ns_menu, target);
    for (index, item) in menu.items.iter().enumerate() {
        let ns_item = match item {
            PopupItem::Separator => NSMenuItem::separatorItem(mtm),
            PopupItem::Entry(entry) => {
                let mut path = prefix.to_vec();
                path.push(index);
                build_item(mtm, entry, target, path)
            }
        };
        mark_built(&ns_item);
        ns_menu.addItem(&ns_item);
    }
    ns_menu
}

static TARGET_KEY: u8 = 0;

/// `setTarget:` does not retain, and AppKit can keep a menu (or one of its submenus) past our
/// last reference, such as a menu bar replaced while it is being tracked.
fn retain_target(menu: &NSMenu, target: &PopupTarget) {
    // SAFETY: both are live objects; the key is a static's address, unique to this association.
    unsafe {
        ffi::objc_setAssociatedObject(
            ptr::from_ref(menu).cast_mut().cast(),
            (&raw const TARGET_KEY).cast(),
            ptr::from_ref(target).cast_mut().cast(),
            ffi::OBJC_ASSOCIATION_RETAIN_NONATOMIC,
        );
    }
}

const BUILT_MARKER: &str = "asrs.menu";

/// Tags an item as ours, so a read-back can tell it from items AppKit adds to menus on its own
/// (dictation, emoji and writing tools in a menu with editing selectors).
fn mark_built(item: &NSMenuItem) {
    // SAFETY: an immutable NSString is a valid represented object.
    unsafe { item.setRepresentedObject(Some(&NSString::from_str(BUILT_MARKER))) };
}

/// `-[ElectronMenuController makeMenuItemForIndex:fromModel:]` followed by
/// `applyStateToMenuItem:`.
fn build_item(
    mtm: MainThreadMarker,
    entry: &PopupEntry,
    target: &PopupTarget,
    path: MenuPath,
) -> Retained<NSMenuItem> {
    let title = NSString::from_str(&native_menu_label(&entry.label));
    // SAFETY: `itemSelected:` is implemented by `PopupTarget`, the only target these items get.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &title,
            Some(sel!(itemSelected:)),
            &NSString::new(),
        )
    };
    if let Some(rgb) = entry.swatch {
        item.setImage(swatch_image(rgb).as_deref());
    }
    item.setToolTip(Some(&NSString::new()));
    let role = entry.role.as_deref();
    if role == Some("services") {
        // SAFETY: clearing the target and action of a submenu item.
        unsafe {
            item.setTarget(None);
            item.setAction(None);
        }
        let submenu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Services"));
        item.setSubmenu(Some(&submenu));
        NSApplication::sharedApplication(mtm).setServicesMenu(Some(&submenu));
        return item;
    }
    match &entry.submenu {
        Some(submenu) if entry.visible => {
            // SAFETY: clearing the target and action of a submenu item.
            unsafe {
                item.setTarget(None);
                item.setAction(None);
            }
            let ns_submenu = if submenu.has_visible_items() {
                build_menu(mtm, submenu, target, &path)
            } else {
                empty_submenu(mtm)
            };
            ns_submenu.setTitle(&item.title());
            item.setSubmenu(Some(&ns_submenu));
            let app = NSApplication::sharedApplication(mtm);
            if matches!(role, Some("window" | "windowmenu")) && ns_submenu.numberOfItems() > 0 {
                app.setWindowsMenu(Some(&ns_submenu));
            } else if role == Some("help") {
                app.setHelpMenu(Some(&ns_submenu));
            }
        }
        _ => {
            let mut paths = target.ivars().paths.borrow_mut();
            item.setTag(paths.len() as isize);
            paths.push(path);
            if let Some((key, modifiers)) = &entry.key_equivalent {
                item.setKeyEquivalent(&NSString::from_str(key));
                item.setKeyEquivalentModifierMask(NSEventModifierFlags::from_bits_retain(
                    *modifiers as usize,
                ));
            }
            // SAFETY: `setAllowsKeyEquivalentWhenHidden:` takes a BOOL; Electron sends it the
            // same way on every macOS version it supports.
            unsafe {
                let _: () = msg_send![
                    &*item,
                    setAllowsKeyEquivalentWhenHidden: Bool::new(entry.works_when_hidden)
                ];
            }
            let selector = role.and_then(|role| {
                ROLE_SELECTORS
                    .iter()
                    .find(|(name, _)| *name == role)
                    .map(|(_, selector)| Sel::register(selector))
            });
            match selector {
                // SAFETY: a role item goes to the first responder that implements its
                // selector, as AppKit's own menu items do.
                Some(selector) => unsafe {
                    item.setTarget(None);
                    item.setAction(Some(selector));
                },
                // SAFETY: `build_menu` ties the target's lifetime to this item's menu.
                None => unsafe { item.setTarget(Some(target)) },
            }
        }
    }
    item.setHidden(!entry.visible);
    item.setEnabled(entry.enabled);
    item.setState(if entry.checked {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    item
}

/// Chromium's `MakeEmptySubmenu` with the en-US `IDS_APP_MENU_EMPTY_SUBMENU`.
fn empty_submenu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::new());
    menu.setAutoenablesItems(false);
    // SAFETY: an item with no action.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("(empty)"),
            None,
            &NSString::new(),
        )
    };
    item.setEnabled(false);
    mark_built(&item);
    menu.addItem(&item);
    menu
}

/// `nativeImage.createFromBitmap` of a solid 15x15 BGRA buffer at scale 1, converted the way
/// Chromium's `NSImageFromImageSkia` does: one sRGB bitmap rep, image size equal to the pixel
/// size, not a template.
fn swatch_image(rgb: [u8; 3]) -> Option<Retained<NSImage>> {
    let size = SWATCH_SIZE as usize;
    let pixels: Vec<u8> = std::iter::repeat_n([rgb[2], rgb[1], rgb[0], 255], size * size)
        .flatten()
        .collect();
    let data = CFData::from_bytes(&pixels);
    let provider = CGDataProvider::with_cf_data(Some(&data))?;
    // SAFETY: an immutable CFString constant exported by CoreGraphics.
    let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))?;
    let info = CGBitmapInfo::from_bits_retain(
        CGImageByteOrderInfo::Order32Little.0 | CGImageAlphaInfo::PremultipliedFirst.0,
    );
    // SAFETY: the provider holds exactly `size * size * 4` bytes laid out as described, and a
    // null decode array is allowed.
    let image = unsafe {
        CGImage::new(
            size,
            size,
            8,
            32,
            size * 4,
            Some(&srgb),
            info,
            Some(&provider),
            ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }?;
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &image);
    let ns_image = NSImage::init(NSImage::alloc());
    ns_image.addRepresentation(&rep);
    ns_image.setSize(NSSize::new(f64::from(SWATCH_SIZE), f64::from(SWATCH_SIZE)));
    Some(ns_image)
}

thread_local! {
    static OPEN_POPUPS: RefCell<Vec<(usize, Retained<NSMenu>)>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn popup_menu(
    window: &Window,
    cx: &mut App,
    menu: PopupMenu,
    x: i32,
    y: i32,
    on_close: impl FnOnce(Option<MenuPath>, &mut App) + 'static,
) {
    let ns_window = ns_window(window);
    let cx = cx.to_async();
    perform_on_run_loop(move || {
        let selection = ns_window.and_then(|w| show_popup(&w, &menu, x, y));
        cx.spawn(async move |cx| cx.update(|cx| on_close(selection, cx)))
            .detach();
    });
}

/// `Menu::SetApplicationMenu` with `useDefaultAccelerator:YES`; the caller has already resolved
/// default accelerators into the entries' key equivalents.
pub(super) fn set_application_menu(
    cx: &mut App,
    menu: &PopupMenu,
    on_select: impl Fn(MenuPath, &mut App) + 'static,
) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let cx = cx.to_async();
    let on_select = std::rc::Rc::new(on_select);
    let target = PopupTarget::with_callback(
        mtm,
        Some(Box::new(move |path| {
            let on_select = on_select.clone();
            cx.spawn(async move |cx| cx.update(|cx| on_select(path, cx)))
                .detach();
        })),
    );
    let ns_menu = build_menu(mtm, menu, &target, &[]);
    NSApplication::sharedApplication(mtm).setMainMenu(Some(&ns_menu));
    true
}

fn perform_on_run_loop(job: impl FnOnce() + 'static) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let runner = PopupRunner::alloc(mtm).set_ivars(PopupRunnerIvars {
        job: RefCell::new(Some(Box::new(job))),
    });
    // SAFETY: NSObject's designated initializer on a freshly allocated instance.
    let runner: Retained<PopupRunner> = unsafe { msg_send![super(runner), init] };
    // SAFETY: an immutable NSString constant exported by Foundation.
    let modes = NSArray::from_slice(&[unsafe { NSRunLoopCommonModes }]);
    // SAFETY: `run:` takes one object argument, and the run loop retains the runner until it
    // fires.
    unsafe {
        let _: () = msg_send![
            &*runner,
            performSelector: sel!(run:),
            withObject: None::<&AnyObject>,
            afterDelay: 0.0f64,
            inModes: &*modes
        ];
    }
}

struct PopupRunnerIvars {
    job: RefCell<Option<Box<dyn FnOnce()>>>,
}

// Menu tracking and modal panels run a nested run loop. Starting one from a GCD main-queue block
// (where GPUI runs its foreground tasks) would stall every other main-queue block until it
// closes, so they start from a run loop perform instead, as Chromium's posted task does.
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AsrsPopupMenuRunner"]
    #[ivars = PopupRunnerIvars]
    struct PopupRunner;

    impl PopupRunner {
        #[unsafe(method(run:))]
        fn run(&self, _argument: Option<&AnyObject>) {
            let job = self.ivars().job.borrow_mut().take();
            if let Some(job) = job {
                job();
            }
        }
    }
);

/// `MenuMac::PopupOnUI` with no positioning item and no frame, which is what upstream's
/// `menu.popup({ window, x, y })` reaches. Blocks in AppKit's menu tracking loop.
fn show_popup(window: &NSWindow, menu: &PopupMenu, x: i32, y: i32) -> Option<MenuPath> {
    let mtm = MainThreadMarker::new()?;
    let view = window.contentView()?;
    let target = PopupTarget::new(mtm);
    let ns_menu = build_menu(mtm, menu, &target, &[]);

    let mut position = if x == -1 || y == -1 {
        view.convertPoint_fromView(window.mouseLocationOutsideOfEventStream(), None)
    } else {
        NSPoint::new(f64::from(x), view.frame().size.height - f64::from(y))
    };
    let visible_frame = window
        .screen()
        .map(|screen| screen.visibleFrame())
        .unwrap_or_default();
    let menu_size = ns_menu.size();
    let window_frame = window.frame();
    let lowest_menu_point = min_y(window_frame) + position.y - menu_size.height;
    let distance_from_bottom = lowest_menu_point - min_y(visible_frame);
    if distance_from_bottom < 0.0 {
        position.y = position.y - distance_from_bottom + 4.0;
    }
    let rightmost_menu_point = min_x(window_frame) + position.x + menu_size.width;
    if rightmost_menu_point > max_x(visible_frame) {
        position.x -= menu_size.width;
    }

    let key = ptr::from_ref(window) as usize;
    OPEN_POPUPS.with_borrow_mut(|open| open.push((key, ns_menu.clone())));
    ns_menu.popUpMenuPositioningItem_atLocation_inView(None, position, Some(&view));
    OPEN_POPUPS.with_borrow_mut(|open| {
        if let Some(i) = open.iter().position(|(_, m)| *m == ns_menu) {
            open.remove(i);
        }
    });
    target.selection()
}

fn min_x(rect: CGRect) -> f64 {
    rect.origin.x.min(rect.origin.x + rect.size.width)
}

fn max_x(rect: CGRect) -> f64 {
    rect.origin.x.max(rect.origin.x + rect.size.width)
}

fn min_y(rect: CGRect) -> f64 {
    rect.origin.y.min(rect.origin.y + rect.size.height)
}

pub(super) fn close_popup_menu(window: &Window) {
    let Some(window) = ns_window(window) else {
        return;
    };
    let key = Retained::as_ptr(&window) as usize;
    let menus: Vec<Retained<NSMenu>> = OPEN_POPUPS.with_borrow(|open| {
        open.iter()
            .filter(|(k, _)| *k == key)
            .map(|(_, m)| m.clone())
            .collect()
    });
    for menu in menus {
        menu.cancelTracking();
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RawWheel {
    continuous: bool,
    point_delta: [i64; 2],
    delta: [f64; 2],
    scrolling_delta: [f64; 2],
    precise: bool,
}

impl RawWheel {
    fn from_event(event: &NSEvent) -> Option<Self> {
        use objc2_core_graphics::{CGEvent, CGEventField};
        let cg = event.CGEvent()?;
        let field = |f| CGEvent::integer_value_field(Some(&cg), f);
        Some(Self {
            continuous: field(CGEventField::ScrollWheelEventIsContinuous) != 0,
            point_delta: [
                field(CGEventField::ScrollWheelEventPointDeltaAxis2),
                field(CGEventField::ScrollWheelEventPointDeltaAxis1),
            ],
            delta: [event.deltaX(), event.deltaY()],
            scrolling_delta: [event.scrollingDeltaX(), event.scrollingDeltaY()],
            precise: event.hasPreciseScrollingDeltas(),
        })
    }

    /// The deltas of `WebMouseWheelEventBuilder::Build` in Chromium's
    /// `components/input/web_input_event_builders_mac.mm`.
    fn chromium(&self) -> WheelDelta {
        if self.continuous {
            WheelDelta {
                x: self.point_delta[0] as f32,
                y: self.point_delta[1] as f32,
            }
        } else {
            let tick = f64::from(super::PIXELS_PER_WHEEL_TICK);
            WheelDelta {
                x: (self.delta[0] * tick) as f32,
                y: (self.delta[1] * tick) as f32,
            }
        }
    }

    /// The delta GPUI derives from the same event.
    fn gpui(&self) -> GpuiDelta {
        let delta = [
            self.scrolling_delta[0] as f32,
            self.scrolling_delta[1] as f32,
        ];
        if self.precise {
            GpuiDelta::Pixels(delta)
        } else {
            GpuiDelta::Lines(delta)
        }
    }
}

thread_local! {
    static LAST_WHEEL: Cell<Option<RawWheel>> = const { Cell::new(None) };
    static WHEEL_MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
}

fn install_wheel_monitor() {
    WHEEL_MONITOR.with_borrow_mut(|monitor| {
        if monitor.is_some() {
            return;
        }
        let block = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit passes a live event for the duration of the handler.
            let raw = RawWheel::from_event(unsafe { event.as_ref() });
            LAST_WHEEL.set(raw);
            event.as_ptr()
        });
        // SAFETY: the handler returns the event it was given, so dispatch continues unchanged.
        *monitor = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::ScrollWheel, &block)
        };
    });
}

pub(super) fn wheel_delta(event: &ScrollEvent) -> Option<WheelDelta> {
    let raw = LAST_WHEEL.get()?;
    (raw.gpui() == GpuiDelta::of(&event.delta)).then(|| raw.chromium())
}

pub(super) mod probe {
    use std::ptr;

    use gpui_kit::Window;
    use objc2::rc::Retained;
    use objc2::runtime::Bool;
    use objc2::{MainThreadMarker, msg_send};
    use objc2_app_kit::{
        NSApplication, NSBitmapImageRep, NSControlStateValueOn, NSEvent, NSImage, NSMenu,
        NSMenuItem, NSScreen,
    };
    use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
    #[allow(deprecated)]
    use objc2_core_graphics::CGWindowListCreateImage;
    use objc2_core_graphics::{
        CGDataProvider, CGEvent, CGEventField, CGEventTapLocation, CGEventType, CGImage,
        CGMouseButton, CGPreflightPostEventAccess, CGPreflightScreenCaptureAccess,
        CGScrollEventUnit, CGWarpMouseCursorPosition, CGWindowImageOption, CGWindowListOption,
    };
    use objc2_foundation::NSSize;

    use super::super::probe::{Frame, MouseAction, WheelReading, WheelSample, WindowImage};
    use super::super::{PopupEntry, PopupItem, PopupMenu};
    use super::{PopupTarget, RawWheel, build_menu, find_vibrancy, metal_layer, ns_window};

    pub fn native_menu_roundtrip(menu: &PopupMenu) -> Option<PopupMenu> {
        let mtm = MainThreadMarker::new()?;
        let target = PopupTarget::new(mtm);
        read_menu(&build_menu(mtm, menu, &target, &[]))
    }

    fn read_menu(menu: &NSMenu) -> Option<PopupMenu> {
        let mut items = Vec::new();
        for item in menu.itemArray().iter() {
            let built = item
                .representedObject()
                .and_then(|o| o.downcast::<objc2_foundation::NSString>().ok())
                .is_some_and(|marker| marker.to_string() == super::BUILT_MARKER);
            if !built {
                ADDED_BY_APPKIT.with_borrow_mut(|added| added.push(item.title().to_string()));
                continue;
            }
            if item.isSeparatorItem() {
                items.push(PopupItem::Separator);
                continue;
            }
            // macOS 26 gives items with standard selectors a system symbol image of its own;
            // only a swatch reads back as one.
            let swatch = item.image().and_then(|image| read_swatch(&image));
            let submenu = match item.submenu() {
                Some(sub) => Some(read_menu(&sub)?),
                None => None,
            };
            let key = item.keyEquivalent().to_string();
            // SAFETY: a BOOL getter with no arguments.
            let works_when_hidden: Bool =
                unsafe { msg_send![&*item, allowsKeyEquivalentWhenHidden] };
            items.push(PopupItem::Entry(PopupEntry {
                label: item.title().to_string(),
                enabled: item.isEnabled(),
                visible: !item.isHidden(),
                checked: item.state() == NSControlStateValueOn,
                swatch,
                works_when_hidden: key.is_empty() || works_when_hidden.as_bool(),
                key_equivalent: (!key.is_empty())
                    .then(|| (key, item.keyEquivalentModifierMask().bits() as u32)),
                role: read_role(&item),
                submenu,
            }));
        }
        Some(PopupMenu { items })
    }

    /// The role an item's AppKit wiring stands for: a role selector, or the system menu its
    /// submenu was registered as. `window` and `windowmenu` both read as `windowmenu`.
    fn read_role(item: &NSMenuItem) -> Option<String> {
        let mtm = MainThreadMarker::new()?;
        let app = NSApplication::sharedApplication(mtm);
        if let Some(submenu) = item.submenu() {
            let is = |menu: Option<Retained<NSMenu>>| menu.is_some_and(|m| m == submenu);
            return if is(app.servicesMenu()) {
                Some("services".to_owned())
            } else if is(app.windowsMenu()) {
                Some("windowmenu".to_owned())
            } else if is(app.helpMenu()) {
                Some("help".to_owned())
            } else {
                None
            };
        }
        let action = item.action()?;
        super::ROLE_SELECTORS
            .iter()
            .find(|(_, selector)| *selector == action.name())
            .map(|(role, _)| (*role).to_owned())
    }

    pub fn perform_application_menu_item(path: &[usize]) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let Some(mut menu) = NSApplication::sharedApplication(mtm).mainMenu() else {
            return false;
        };
        let Some((&last, parents)) = path.split_last() else {
            return false;
        };
        for &index in parents {
            let Some(submenu) = menu
                .itemAtIndex(index as isize)
                .and_then(|item| item.submenu())
            else {
                return false;
            };
            menu = submenu;
        }
        if last as isize >= menu.numberOfItems() {
            return false;
        }
        menu.performActionForItemAtIndex(last as isize);
        true
    }

    pub fn perform_key_equivalent(key: &str, modifiers: u32) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let Some(menu) = NSApplication::sharedApplication(mtm).mainMenu() else {
            return false;
        };
        let characters = objc2_foundation::NSString::from_str(key);
        // SAFETY: NSEvent's documented key event constructor; a nil graphics context is the
        // value AppKit requires.
        let event: Option<Retained<NSEvent>> = unsafe {
            msg_send![
                <NSEvent as objc2::ClassType>::class(),
                keyEventWithType: objc2_app_kit::NSEventType::KeyDown,
                location: objc2_foundation::NSPoint::ZERO,
                modifierFlags: objc2_app_kit::NSEventModifierFlags::from_bits_retain(
                    modifiers as usize
                ),
                timestamp: 0.0f64,
                windowNumber: 0isize,
                context: None::<&objc2::runtime::AnyObject>,
                characters: &*characters,
                charactersIgnoringModifiers: &*characters,
                isARepeat: false,
                keyCode: 0u16
            ]
        };
        event.is_some_and(|event| menu.performKeyEquivalent(&event))
    }

    thread_local! {
        static ADDED_BY_APPKIT: std::cell::RefCell<Vec<String>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    /// The application's main menu as AppKit holds it, without the items AppKit added itself,
    /// whose titles come second.
    pub fn read_application_menu() -> Option<(PopupMenu, Vec<String>)> {
        let mtm = MainThreadMarker::new()?;
        let menu = NSApplication::sharedApplication(mtm).mainMenu()?;
        ADDED_BY_APPKIT.with_borrow_mut(Vec::clear);
        let read = read_menu(&menu)?;
        Some((read, ADDED_BY_APPKIT.with_borrow_mut(std::mem::take)))
    }

    fn read_swatch(image: &NSImage) -> Option<[u8; 3]> {
        let side = f64::from(super::SWATCH_SIZE);
        if image.isTemplate() || image.size() != NSSize::new(side, side) {
            return None;
        }
        let reps = image.representations();
        if reps.count() != 1 {
            return None;
        }
        let rep = reps.firstObject()?.downcast::<NSBitmapImageRep>().ok()?;
        let image = rep.CGImage()?;
        let pixels = cg_image_rgba(&image)?;
        if (pixels.width, pixels.height) != (super::SWATCH_SIZE, super::SWATCH_SIZE) {
            return None;
        }
        let first = pixels.pixel(0, 0);
        let solid = pixels.rgba.as_chunks::<4>().0.iter().all(|p| *p == first);
        (solid && first[3] == 255).then_some([first[0], first[1], first[2]])
    }

    fn scroll_event(sample: &WheelSample) -> Option<CFRetained<CGEvent>> {
        let unit = if sample.continuous {
            CGScrollEventUnit::Pixel
        } else {
            CGScrollEventUnit::Line
        };
        let event =
            CGEvent::new_scroll_wheel_event2(None, unit, 2, sample.delta[1], sample.delta[0], 0)?;
        let ev = Some(&*event);
        let int = |field, value| CGEvent::set_integer_value_field(ev, field, value);
        int(
            CGEventField::ScrollWheelEventIsContinuous,
            i64::from(sample.continuous),
        );
        int(
            CGEventField::ScrollWheelEventDeltaAxis1,
            i64::from(sample.delta[1]),
        );
        int(
            CGEventField::ScrollWheelEventDeltaAxis2,
            i64::from(sample.delta[0]),
        );
        int(
            CGEventField::ScrollWheelEventPointDeltaAxis1,
            sample.point_delta[1],
        );
        int(
            CGEventField::ScrollWheelEventPointDeltaAxis2,
            sample.point_delta[0],
        );
        int(
            CGEventField::ScrollWheelEventScrollPhase,
            sample.scroll_phase,
        );
        int(
            CGEventField::ScrollWheelEventMomentumPhase,
            sample.momentum_phase,
        );
        let double = |field, value| CGEvent::set_double_value_field(ev, field, value);
        double(
            CGEventField::ScrollWheelEventFixedPtDeltaAxis1,
            sample.fixed_delta[1],
        );
        double(
            CGEventField::ScrollWheelEventFixedPtDeltaAxis2,
            sample.fixed_delta[0],
        );
        Some(event)
    }

    pub fn read_wheel_sample(sample: &WheelSample) -> Option<WheelReading> {
        let cg_event = scroll_event(sample)?;
        let event = NSEvent::eventWithCGEvent(&cg_event)?;
        let raw = RawWheel::from_event(&event)?;
        Some(WheelReading {
            chromium: raw.chromium(),
            gpui: raw.gpui(),
        })
    }

    fn primary_screen_height() -> f64 {
        MainThreadMarker::new()
            .and_then(|mtm| NSScreen::screens(mtm).firstObject())
            .map_or(0.0, |screen| screen.frame().size.height)
    }

    pub fn post_wheel(window: &Window, sample: &WheelSample) -> bool {
        let Some(window) = ns_window(window) else {
            return false;
        };
        let frame = window.contentRectForFrameRect(window.frame());
        let x = frame.origin.x + frame.size.width / 2.0;
        let y = primary_screen_height() - (frame.origin.y + frame.size.height / 2.0);
        post_wheel_at(x, y, sample)
    }

    pub fn post_wheel_at(x: f64, y: f64, sample: &WheelSample) -> bool {
        let Some(event) = scroll_event(sample) else {
            return false;
        };
        warp_cursor(x, y);
        CGEvent::set_location(Some(&event), CGPoint::new(x, y));
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
        true
    }

    pub fn post_mouse(action: MouseAction, x: f64, y: f64) -> bool {
        let kind = match action {
            MouseAction::Down => CGEventType::LeftMouseDown,
            MouseAction::Drag => CGEventType::LeftMouseDragged,
            MouseAction::Up => CGEventType::LeftMouseUp,
        };
        let Some(event) =
            CGEvent::new_mouse_event(None, kind, CGPoint::new(x, y), CGMouseButton::Left)
        else {
            return false;
        };
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
        true
    }

    pub fn cursor_location() -> (f64, f64) {
        let location = CGEvent::new(None).map_or(CGPoint::new(0.0, 0.0), |event| {
            CGEvent::location(Some(&event))
        });
        (location.x, location.y)
    }

    pub fn warp_cursor(x: f64, y: f64) {
        CGWarpMouseCursorPosition(CGPoint::new(x, y));
    }

    fn frame_of(rect: CGRect) -> Frame {
        Frame {
            x: rect.origin.x,
            y: rect.origin.y,
            width: rect.size.width,
            height: rect.size.height,
        }
    }

    pub fn window_frame(window: &Window) -> Option<Frame> {
        Some(frame_of(ns_window(window)?.frame()))
    }

    pub fn content_size(window: &Window) -> Option<(f64, f64)> {
        let window = ns_window(window)?;
        let content = window.contentRectForFrameRect(window.frame());
        Some((content.size.width, content.size.height))
    }

    pub fn window_number(window: &Window) -> Option<u32> {
        u32::try_from(ns_window(window)?.windowNumber()).ok()
    }

    pub fn menu_window_frames() -> Vec<Frame> {
        let Some(mtm) = MainThreadMarker::new() else {
            return Vec::new();
        };
        NSApplication::sharedApplication(mtm)
            .windows()
            .iter()
            .filter(|w| w.isVisible() && w.class().name().to_string_lossy().contains("Menu"))
            .map(|w| frame_of(w.frame()))
            .collect()
    }

    pub fn is_visible(window: &Window) -> Option<bool> {
        let window = ns_window(window)?;
        Some(
            window.isVisible()
                && window
                    .occlusionState()
                    .contains(objc2_app_kit::NSWindowOcclusionState::Visible),
        )
    }

    pub fn app_is_active() -> Option<bool> {
        Some(NSApplication::sharedApplication(MainThreadMarker::new()?).isActive())
    }

    pub fn is_sheet_of(window: &Window, parent: &Window) -> bool {
        let (Some(window), Some(parent)) = (ns_window(window), ns_window(parent)) else {
            return false;
        };
        window.isSheet() && parent.attachedSheet().is_some_and(|s| s == window)
    }

    pub fn hides_on_deactivate(window: &Window) -> Option<bool> {
        Some(ns_window(window)?.hidesOnDeactivate())
    }

    use objc2_app_kit::NSView;
    fn find_button(view: &NSView, title: &str) -> Option<Retained<objc2_app_kit::NSButton>> {
        for child in view.subviews().iter() {
            if let Ok(button) = child.clone().downcast::<objc2_app_kit::NSButton>()
                && button.title().to_string() == title
            {
                return Some(button);
            }
            if let Some(found) = find_button(&child, title) {
                return Some(found);
            }
        }
        None
    }

    pub fn click_sheet_button(parent: &Window, title: &str) -> bool {
        let Some(sheet) = ns_window(parent).and_then(|w| w.attachedSheet()) else {
            return false;
        };
        let Some(button) = sheet.contentView().and_then(|v| find_button(&v, title)) else {
            return false;
        };
        // SAFETY: `performClick:` takes an optional sender.
        unsafe { button.performClick(None) };
        true
    }

    fn sheet_panel(parent: &Window) -> Option<Retained<objc2_app_kit::NSSavePanel>> {
        ns_window(parent)?
            .attachedSheet()?
            .downcast::<objc2_app_kit::NSSavePanel>()
            .ok()
    }

    #[allow(deprecated)]
    pub fn sheet_panel_state(parent: &Window) -> Option<super::super::probe::PanelState> {
        let panel = sheet_panel(parent)?;
        let open = panel.clone().downcast::<objc2_app_kit::NSOpenPanel>().ok();
        Some(super::super::probe::PanelState {
            open: open.is_some(),
            title: panel.title().to_string(),
            message: panel.message().to_string(),
            prompt: panel.prompt().to_string(),
            allowed_file_types: panel
                .allowedFileTypes()
                .map(|types| types.iter().map(|t| t.to_string()).collect())
                .unwrap_or_default(),
            directory: panel
                .directoryURL()
                .and_then(|url| url.path())
                .map(|p| p.to_string()),
            name: panel.nameFieldStringValue().to_string(),
            can_create_directories: panel.canCreateDirectories(),
            can_choose_files: open.as_ref().is_some_and(|o| o.canChooseFiles()),
            can_choose_directories: open.as_ref().is_some_and(|o| o.canChooseDirectories()),
            allows_multiple_selection: open.as_ref().is_some_and(|o| o.allowsMultipleSelection()),
        })
    }

    pub fn cancel_sheet_panel(parent: &Window) -> bool {
        let Some(panel) = sheet_panel(parent) else {
            return false;
        };
        // SAFETY: `cancel:` takes an optional sender.
        unsafe { panel.cancel(None) };
        true
    }

    pub fn has_attached_sheet(window: &Window) -> bool {
        ns_window(window).is_some_and(|w| w.attachedSheet().is_some())
    }

    pub fn has_sidebar_vibrancy(window: &Window) -> bool {
        use objc2_app_kit::{
            NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
        };
        let Some(content) = ns_window(window).and_then(|w| w.contentView()) else {
            return false;
        };
        find_vibrancy(&content).is_some_and(|effect| {
            effect.material() == NSVisualEffectMaterial::Sidebar
                && effect.blendingMode() == NSVisualEffectBlendingMode::BehindWindow
                && effect.state() == NSVisualEffectState::FollowsWindowActiveState
                && content
                    .subviews()
                    .firstObject()
                    .is_some_and(|first| ptr::eq(&*first, &**effect))
        })
    }

    pub fn untag_colorspace(window: &Window) -> bool {
        let Some(layer) = metal_layer(window) else {
            return false;
        };
        layer.setColorspace(None);
        true
    }

    pub fn screen_height() -> f64 {
        primary_screen_height()
    }

    pub fn is_tagged_srgb(window: &Window) -> Option<bool> {
        let layer = metal_layer(window)?;
        let name = layer
            .colorspace()
            .and_then(|space| objc2_core_graphics::CGColorSpace::name(Some(&space)));
        Some(name.is_some_and(|n| n.to_string() == "kCGColorSpaceSRGB"))
    }

    pub fn proposed_resize(window: &Window, width: f64, height: f64) -> Option<(f64, f64)> {
        let window = ns_window(window)?;
        let delegate = window.delegate()?;
        // SAFETY: `windowWillResize:toSize:` is an NSWindowDelegate method with this signature;
        // the delegate is checked to implement it first.
        let size: NSSize = unsafe {
            let responds: bool =
                msg_send![&*delegate, respondsToSelector: objc2::sel!(windowWillResize:toSize:)];
            if !responds {
                return Some((width, height));
            }
            msg_send![&*delegate, windowWillResize: &*window, toSize: NSSize::new(width, height)]
        };
        Some((size.width, size.height))
    }

    #[allow(deprecated)]
    pub fn capture_window(window_number: u32) -> Option<WindowImage> {
        let null_rect = CGRect::new(
            CGPoint::new(f64::INFINITY, f64::INFINITY),
            CGSize::new(0.0, 0.0),
        );
        let image = CGWindowListCreateImage(
            null_rect,
            CGWindowListOption::OptionIncludingWindow,
            window_number,
            CGWindowImageOption::BoundsIgnoreFraming | CGWindowImageOption::BestResolution,
        )?;
        cg_image_rgba(&image)
    }

    fn cg_image_rgba(image: &CGImage) -> Option<WindowImage> {
        let width = CGImage::width(Some(image));
        let height = CGImage::height(Some(image));
        let bytes_per_row = CGImage::bytes_per_row(Some(image));
        if CGImage::bits_per_pixel(Some(image)) != 32 {
            return None;
        }
        let provider = CGImage::data_provider(Some(image))?;
        let bytes = CGDataProvider::data(Some(&provider))?.to_vec();
        let info = CGImage::bitmap_info(Some(image)).0;
        let little_endian = (info & 0x7000) == 0x2000;
        let alpha_first = matches!(info & 0x1f, 2 | 4 | 6);
        let mut rgba = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                let o = y * bytes_per_row + x * 4;
                let p = &bytes[o..o + 4];
                let px = match (little_endian, alpha_first) {
                    (true, true) => [p[2], p[1], p[0], p[3]],
                    (true, false) => [p[3], p[2], p[1], p[0]],
                    (false, true) => [p[1], p[2], p[3], p[0]],
                    (false, false) => [p[0], p[1], p[2], p[3]],
                };
                rgba.extend_from_slice(&px);
            }
        }
        Some(WindowImage {
            width: u32::try_from(width).ok()?,
            height: u32::try_from(height).ok()?,
            rgba,
        })
    }

    pub fn can_post_events() -> bool {
        CGPreflightPostEventAccess()
    }

    pub fn can_capture_screen() -> bool {
        CGPreflightScreenCaptureAccess()
    }
}
