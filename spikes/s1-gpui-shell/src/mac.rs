use std::cell::RefCell;
use std::fmt::Write as _;

use gpui_kit::{Pixels, Point, Window};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, Sel};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSEvent, NSImage, NSMenu, NSMenuItem, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindow, NSWindowOrderingMode,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGImage, CGScrollEventUnit, CGWindowImageOption, CGWindowListOption,
};
use objc2_foundation::{NSArray, NSData, NSPoint, NSSize, NSString, NSRunLoopCommonModes};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

pub fn ns_view(window: &Window) -> Option<Retained<NSView>> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return None;
    };
    let ptr = h.ns_view.as_ptr() as *mut NSView;
    unsafe { Retained::retain(ptr) }
}

pub fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
    ns_view(window)?.window()
}

pub fn install_sidebar_vibrancy(window: &Window) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let Some(nswin) = ns_window(window) else {
        return false;
    };
    let Some(content) = nswin.contentView() else {
        return false;
    };
    let effect = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), content.bounds());
    effect.setMaterial(NSVisualEffectMaterial::Sidebar);
    effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    effect.setState(NSVisualEffectState::FollowsWindowActiveState);
    effect.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    content.addSubview_positioned_relativeTo(&effect, NSWindowOrderingMode::Below, None);
    true
}

pub fn window_facts(window: &Window) -> String {
    let Some(w) = ns_window(window) else {
        return "no NSWindow".into();
    };
    let frame = w.frame();
    let content = w.contentRectForFrameRect(frame);
    let sheet: Option<Retained<NSWindow>> = w.attachedSheet();
    let is_sheet = w.isSheet();
    let hides: bool = unsafe { msg_send![&*w, hidesOnDeactivate] };
    let class = w.class().name().to_string_lossy().into_owned();
    format!(
        "class={class} number={} level={} frame=({:.0},{:.0} {:.0}x{:.0}) content={:.0}x{:.0} isSheet={is_sheet} attachedSheet={} hidesOnDeactivate={hides} backing={}",
        w.windowNumber(),
        w.level(),
        frame.origin.x,
        frame.origin.y,
        frame.size.width,
        frame.size.height,
        content.size.width,
        content.size.height,
        sheet.is_some(),
        w.backingScaleFactor(),
    )
}

pub fn dump_main_menu() -> String {
    let mtm = MainThreadMarker::new().expect("main thread");
    let app = NSApplication::sharedApplication(mtm);
    let mut out = String::new();
    if let Some(menu) = app.mainMenu() {
        dump_menu(&menu, 0, &mut out);
    }
    out
}

fn dump_menu(menu: &NSMenu, depth: usize, out: &mut String) {
    for i in 0..menu.numberOfItems() {
        let Some(item) = menu.itemAtIndex(i) else { continue };
        let indent = "  ".repeat(depth);
        if item.isSeparatorItem() {
            let _ = writeln!(out, "{indent}----");
            continue;
        }
        let mods = item.keyEquivalentModifierMask();
        let key = item.keyEquivalent().to_string();
        let mut accel = String::new();
        if !key.is_empty() {
            use objc2_app_kit::NSEventModifierFlags as M;
            for (flag, s) in [
                (M::Control, "ctrl-"),
                (M::Option, "alt-"),
                (M::Shift, "shift-"),
                (M::Command, "cmd-"),
            ] {
                if mods.contains(flag) {
                    accel.push_str(s);
                }
            }
            let printable: String = key
                .chars()
                .map(|c| if c.is_control() || (c as u32) >= 0xF700 { '?' } else { c })
                .collect();
            let _ = write!(accel, "{printable}(U+{:04X})", key.chars().next().unwrap() as u32);
        }
        let _ = writeln!(
            out,
            "{indent}{:?} accel=[{accel}] state={} enabled={}",
            item.title().to_string(),
            item.state(),
            item.isEnabled()
        );
        if let Some(sub) = item.submenu() {
            dump_menu(&sub, depth + 1, out);
        }
    }
}

pub struct MenuProbeIvars {
    menu: RefCell<Option<Retained<NSMenu>>>,
    report: RefCell<String>,
    selected: std::cell::Cell<isize>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "S1MenuProbe"]
    #[ivars = MenuProbeIvars]
    pub struct MenuProbe;

    impl MenuProbe {
        #[unsafe(method(probe:))]
        fn probe(&self, _sender: Option<&AnyObject>) {
            let mtm = MainThreadMarker::new().unwrap();
            let app = NSApplication::sharedApplication(mtm);
            let mut report = String::new();
            let windows: Retained<NSArray<NSWindow>> = app.windows();
            for w in windows.iter() {
                let f = w.frame();
                let _ = writeln!(
                    report,
                    "  window class={} level={} visible={} frame=({:.1},{:.1} {:.1}x{:.1})",
                    w.class().name().to_string_lossy(),
                    w.level(),
                    w.isVisible(),
                    f.origin.x,
                    f.origin.y,
                    f.size.width,
                    f.size.height
                );
            }
            *self.ivars().report.borrow_mut() = report;
            if let Some(menu) = self.ivars().menu.borrow().as_ref() {
                menu.cancelTracking();
            }
        }

        #[unsafe(method(itemClicked:))]
        fn item_clicked(&self, sender: &NSMenuItem) {
            self.ivars().selected.set(sender.tag());
        }
    }
);

impl MenuProbe {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(MenuProbeIvars {
            menu: RefCell::new(None),
            report: RefCell::new(String::new()),
            selected: std::cell::Cell::new(-1),
        });
        unsafe { msg_send![super(this), init] }
    }
}

fn swatch_png(rgb: [u8; 3]) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(30, 30, |_, _| image::Rgba([rgb[0], rgb[1], rgb[2], 255]));
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

fn item(
    mtm: MainThreadMarker,
    title: &str,
    target: &MenuProbe,
    tag: isize,
) -> Retained<NSMenuItem> {
    let it = NSMenuItem::new(mtm);
    it.setTitle(&NSString::from_str(title));
    it.setTag(tag);
    unsafe {
        it.setTarget(Some(target));
        it.setAction(Some(sel!(itemClicked:)));
    }
    it
}

pub fn native_popup_probe(view: &NSView, position: Point<Pixels>) -> String {
    let mtm = MainThreadMarker::new().unwrap();
    let probe = MenuProbe::new(mtm);
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    menu.addItem(&item(mtm, "Copy Key", &probe, 0));
    let checked = item(mtm, "Loop Visible Range", &probe, 1);
    checked.setState(1);
    menu.addItem(&checked);
    let disabled = item(mtm, "(No Unit Metadata)", &probe, 2);
    disabled.setEnabled(false);
    menu.addItem(&disabled);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    for (i, (name, rgb)) in [("Blue", [0x2b, 0x66, 0xa2]), ("Red", [0xe4, 0x1a, 0x1c])]
        .into_iter()
        .enumerate()
    {
        let it = item(mtm, name, &probe, 10 + i as isize);
        let data = NSData::with_bytes(&swatch_png(rgb));
        if let Some(img) = NSImage::initWithData(NSImage::alloc(), &data) {
            img.setSize(NSSize::new(15.0, 15.0));
            img.setTemplate(false);
            it.setImage(Some(&img));
        }
        menu.addItem(&it);
    }
    let sub = NSMenu::new(mtm);
    sub.addItem(&item(mtm, "Blue 1", &probe, 20));
    sub.addItem(&item(mtm, "Blue 2", &probe, 21));
    let sub_item = NSMenuItem::new(mtm);
    sub_item.setTitle(&NSString::from_str("Driver Station"));
    sub_item.setSubmenu(Some(&sub));
    menu.addItem(&sub_item);

    *probe.ivars().menu.borrow_mut() = Some(menu.clone());
    unsafe {
        let modes = NSArray::from_slice(&[NSRunLoopCommonModes]);
        let _: () = msg_send![
            &*probe,
            performSelector: sel!(probe:),
            withObject: None::<&AnyObject>,
            afterDelay: 0.6f64,
            inModes: &*modes
        ];
    }
    let height = view.bounds().size.height;
    let location = NSPoint::new(f32::from(position.x) as f64, height - f32::from(position.y) as f64);
    let win_frame = view.window().map(|w| w.frame());
    let t0 = std::time::Instant::now();
    let _ = menu.popUpMenuPositioningItem_atLocation_inView(None, location, Some(view));
    let tracked = t0.elapsed();
    let mut out = format!(
        "popUp returned after {:?}; requested window point ({:.0},{:.0}); window frame {:?}\n",
        tracked,
        f32::from(position.x),
        f32::from(position.y),
        win_frame.map(|f| (f.origin.x, f.origin.y, f.size.width, f.size.height))
    );
    out.push_str(&probe.ivars().report.borrow());
    out
}

pub fn cg_capture_window(window: &Window) -> anyhow::Result<image::RgbaImage> {
    let w = ns_window(window).ok_or_else(|| anyhow::anyhow!("no window"))?;
    let id = w.windowNumber() as u32;
    let null_rect = CGRect::new(
        CGPoint::new(f64::INFINITY, f64::INFINITY),
        CGSize::new(0.0, 0.0),
    );
    #[allow(deprecated)]
    let img = objc2_core_graphics::CGWindowListCreateImage(
        null_rect,
        CGWindowListOption::OptionIncludingWindow,
        id,
        CGWindowImageOption::BoundsIgnoreFraming | CGWindowImageOption::BestResolution,
    )
    .ok_or_else(|| anyhow::anyhow!("CGWindowListCreateImage returned null"))?;
    cg_image_to_rgba(&img)
}

fn cg_image_to_rgba(img: &CGImage) -> anyhow::Result<image::RgbaImage> {
    let width = CGImage::width(Some(img));
    let height = CGImage::height(Some(img));
    let bpr = CGImage::bytes_per_row(Some(img));
    let bpp = CGImage::bits_per_pixel(Some(img));
    anyhow::ensure!(bpp == 32, "unexpected bpp {bpp}");
    let provider = CGImage::data_provider(Some(img)).ok_or_else(|| anyhow::anyhow!("no provider"))?;
    let data = objc2_core_graphics::CGDataProvider::data(Some(&provider))
        .ok_or_else(|| anyhow::anyhow!("no data"))?;
    let bytes = data.to_vec();
    let info = CGImage::bitmap_info(Some(img)).0;
    let little = (info & 0x7000) == 0x2000;
    let mut out = image::RgbaImage::new(width as u32, height as u32);
    for y in 0..height {
        for x in 0..width {
            let o = y * bpr + x * 4;
            let p = &bytes[o..o + 4];
            let px = if little {
                [p[2], p[1], p[0], p[3]]
            } else {
                [p[1], p[2], p[3], p[0]]
            };
            out.put_pixel(x as u32, y as u32, image::Rgba(px));
        }
    }
    Ok(out)
}

pub fn make_scroll(
    w: &NSWindow,
    pixel_units: bool,
    lines_or_px: i32,
    continuous: bool,
) -> Option<(String, Retained<NSEvent>)> {
    let units = if pixel_units {
        CGScrollEventUnit::Pixel
    } else {
        CGScrollEventUnit::Line
    };
    let ev = CGEvent::new_scroll_wheel_event2(None, units, 1, lines_or_px, 0, 0)?;
    let frame = w.frame();
    let mtm = MainThreadMarker::new().unwrap();
    let screen_h = objc2_app_kit::NSScreen::screens(mtm)
        .firstObject()
        .map(|s| s.frame().size.height)
        .unwrap_or(0.0);
    let center = CGPoint::new(
        frame.origin.x + frame.size.width / 2.0,
        screen_h - (frame.origin.y + frame.size.height / 2.0),
    );
    CGEvent::set_location(Some(&ev), center);
    CGEvent::set_integer_value_field(
        Some(&ev),
        CGEventField::ScrollWheelEventIsContinuous,
        continuous as i64,
    );
    CGEvent::set_integer_value_field(
        Some(&ev),
        CGEventField::MouseEventWindowUnderMousePointer,
        w.windowNumber() as i64,
    );
    CGEvent::set_integer_value_field(
        Some(&ev),
        CGEventField::MouseEventWindowUnderMousePointerThatCanHandleThisEvent,
        w.windowNumber() as i64,
    );
    let ns_ev = NSEvent::eventWithCGEvent(&ev)?;
    let desc = format!(
        "injected units={} value={lines_or_px} continuous={continuous}: NSEvent deltaY={} scrollingDeltaY={} precise={} locInWin=({:.0},{:.0}) winNum={}",
        if pixel_units { "pixel" } else { "line" },
        ns_ev.deltaY(),
        ns_ev.scrollingDeltaY(),
        ns_ev.hasPreciseScrollingDeltas(),
        ns_ev.locationInWindow().x,
        ns_ev.locationInWindow().y,
        ns_ev.windowNumber(),
    );
    Some((desc, ns_ev))
}

#[allow(dead_code)]
pub fn sel_name(s: Sel) -> String {
    s.name().to_string_lossy().into_owned()
}
