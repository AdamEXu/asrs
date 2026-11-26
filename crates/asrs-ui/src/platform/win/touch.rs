//! Touch and pen as Chromium takes them on Windows. A contact that starts over a scroller goes to
//! GPUI's recognizer: a drag pans it and flings, a tap clicks, and a press held still opens the
//! context menu when it lifts. Any other contact is left to the system, which turns it into a
//! mouse, so the drags of handles, sliders and the 3D view keep working.
//!
//! Chromium does nothing with a pinch over a scroller, but Windows sent the window Ctrl and the
//! wheel for it while it had every touch, which zooms the line graph and the timeline; a second
//! finger over a scroller still does.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use gpui_kit::{
    App, AppContext, Bounds, Context, DispatchPhase, Entity, IntoElement, LongPressEvent,
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, PlatformInput,
    Point, Render, TouchPhase, Window, point, px,
};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Input::Pointer::{GetPointerInfo, GetPointerType, POINTER_INFO};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, POINTER_INPUT_TYPE, PT_PEN, PT_TOUCH, PostMessageW, WHEEL_DELTA,
    WM_MOUSEWHEEL, WM_NCDESTROY, WM_POINTERCAPTURECHANGED, WM_POINTERDOWN, WM_POINTERUP,
    WM_POINTERUPDATE,
};

use super::window::scale;

const ID: usize = 0x7463_6873;

/// `GestureConfigurationAura`'s `max_touch_move_in_pixels_for_click`: a held press that moved
/// further is no long tap.
const SLOP: f32 = 15.0;

/// `MK_CONTROL`.
const CONTROL: usize = 0x0008;

/// Wheel units a pinch sends as its fingers go `e` times as far apart.
const WHEEL_PER_E: f64 = WHEEL_DELTA as f64 / std::f64::consts::LN_2;

struct Pinch {
    fingers: [u32; 2],
    /// How far apart the fingers were, in device px.
    apart: f64,
    /// Wheel units not sent yet.
    rest: f64,
}

enum Route {
    Gpui,
    System,
    Taken,
}

#[derive(Default)]
struct Touch {
    /// Bumped as each frame of the window starts.
    frame: u64,
    /// The scrollers of the last frame, and the boxes over them that take touches as a mouse, in
    /// the order they were drawn.
    regions: Vec<(Bounds<Pixels>, bool)>,
    /// The frame whose long press listener is in.
    listening: Option<u64>,
    /// Contacts the recognizer has, by pointer id.
    recognized: HashSet<u32>,
    pinch: Option<Pinch>,
}

thread_local! {
    static WINDOWS: RefCell<HashMap<isize, Touch>> = RefCell::default();
}

fn with<R>(hwnd: HWND, f: impl FnOnce(&mut Touch) -> R) -> R {
    WINDOWS.with_borrow_mut(|windows| f(windows.entry(hwnd.0 as isize).or_default()))
}

/// The root of a window's views, which marks where each of its frames starts.
pub struct TouchRoot<V: Render>(Entity<V>);

impl<V: Render> Render for TouchRoot<V> {
    fn render(&mut self, window: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        if let Some(hwnd) = super::hwnd(window) {
            with(hwnd, |touch| {
                touch.frame += 1;
                touch.regions.clear();
            });
        }
        self.0.clone()
    }
}

pub fn root<V: Render>(view: Entity<V>, window: &mut Window, cx: &mut App) -> Entity<TouchRoot<V>> {
    if let Some(hwnd) = super::hwnd(window) {
        // SAFETY: the window belongs to this thread; installing the procedure twice updates it.
        let _ = unsafe { SetWindowSubclass(hwnd, Some(procedure), ID, 0) };
    }
    cx.new(|_| TouchRoot(view))
}

/// `bounds`, a box of the frame being drawn, takes the touches that start over it to pan if
/// `pans`, or else as a mouse.
pub fn region(window: &Window, bounds: Bounds<Pixels>, pans: bool) {
    let Some(hwnd) = super::hwnd(window) else {
        return;
    };
    let shown = bounds.intersect(&window.content_mask().bounds);
    if shown.size.width <= px(0.0) || shown.size.height <= px(0.0) {
        return;
    }
    with(hwnd, |touch| touch.regions.push((shown, pans)));
}

/// Chromium's long tap: a press held still opens the context menu as it lifts, and clicks nothing.
pub fn listen(window: &mut Window) {
    let Some(hwnd) = super::hwnd(window) else {
        return;
    };
    let first = with(hwnd, |touch| {
        let first = touch.listening != Some(touch.frame);
        touch.listening = Some(touch.frame);
        first
    });
    if !first {
        return;
    }
    window.on_mouse_event(|event: &LongPressEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble {
            return;
        }
        match event.phase {
            TouchPhase::Started => window.prevent_default(),
            TouchPhase::Ended => {
                let moved = event.position - event.start_position;
                if moved.magnitude() <= f64::from(SLOP) {
                    let at = event.start_position;
                    window.defer(cx, move |window, cx| context_click(window, at, cx));
                }
            }
            TouchPhase::Moved | TouchPhase::Cancelled => {}
        }
    });
}

fn context_click(window: &mut Window, position: Point<Pixels>, cx: &mut App) {
    let modifiers = Modifiers::default();
    let events = [
        PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers,
        }),
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Right,
            position,
            modifiers,
            click_count: 1,
            first_mouse: false,
        }),
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Right,
            position,
            modifiers,
            click_count: 1,
        }),
    ];
    for event in events {
        window.dispatch_event(event, cx);
    }
}

fn screen(id: u32) -> Option<POINT> {
    let mut info = POINTER_INFO::default();
    // SAFETY: `info` outlives the call.
    unsafe { GetPointerInfo(id, &mut info) }.ok()?;
    Some(info.ptPixelLocation)
}

/// Where the contact `id` is, in the window's logical px.
fn position(hwnd: HWND, id: u32) -> Option<Point<Pixels>> {
    let mut at = screen(id)?;
    // SAFETY: `at` outlives the call.
    if !unsafe { ScreenToClient(hwnd, &mut at) }.as_bool() {
        return None;
    }
    let scale = scale(hwnd) as f32;
    Some(point(px(at.x as f32 / scale), px(at.y as f32 / scale)))
}

fn is_contact(id: u32) -> bool {
    let mut kind = POINTER_INPUT_TYPE::default();
    // SAFETY: `kind` outlives the call.
    unsafe { GetPointerType(id, &mut kind) }.is_ok() && (kind == PT_TOUCH || kind == PT_PEN)
}

fn apart(a: POINT, b: POINT) -> f64 {
    f64::from(a.x - b.x).hypot(f64::from(a.y - b.y))
}

fn route(hwnd: HWND, message: u32, id: u32) -> Route {
    match message {
        WM_POINTERDOWN if is_contact(id) => press(hwnd, id),
        WM_POINTERDOWN => Route::Gpui,
        WM_POINTERUPDATE => moved(hwnd, id),
        _ => lift(hwnd, id),
    }
}

fn press(hwnd: HWND, id: u32) -> Route {
    let at = position(hwnd, id);
    let alone = with(hwnd, |touch| {
        if touch.pinch.is_some() {
            touch.recognized.insert(id);
            return Err(Route::Taken);
        }
        let mut recognized = touch.recognized.iter();
        Ok(recognized.next().copied().filter(|_| touch.recognized.len() == 1))
    });
    let first = match alone {
        Ok(first) => first,
        Err(route) => return route,
    };
    if let Some(first) = first {
        if let (Some(a), Some(b)) = (screen(first), screen(id)) {
            with(hwnd, |touch| {
                touch.recognized.insert(id);
                touch.pinch = Some(Pinch {
                    fingers: [first, id],
                    apart: apart(a, b),
                    rest: 0.0,
                });
            });
            // SAFETY: the window belongs to this thread; GPUI ends the first finger's gesture.
            unsafe {
                DefSubclassProc(
                    hwnd,
                    WM_POINTERCAPTURECHANGED,
                    WPARAM(first as usize),
                    LPARAM(0),
                )
            };
            return Route::Taken;
        }
    }
    with(hwnd, |touch| {
        let over = at.is_some_and(|at| {
            let mut under = touch.regions.iter().rev();
            under.find(|(bounds, _)| bounds.contains(&at)).is_some_and(|r| r.1)
        });
        if over || !touch.recognized.is_empty() {
            touch.recognized.insert(id);
            Route::Gpui
        } else {
            Route::System
        }
    })
}

fn moved(hwnd: HWND, id: u32) -> Route {
    let (recognized, fingers) = with(hwnd, |touch| {
        let fingers = touch.pinch.as_ref().map(|pinch| pinch.fingers);
        (touch.recognized.contains(&id), fingers)
    });
    let Some(fingers) = fingers else {
        return if recognized { Route::Gpui } else { Route::System };
    };
    if !fingers.contains(&id) {
        return if recognized { Route::Taken } else { Route::System };
    }
    let (Some(a), Some(b)) = (screen(fingers[0]), screen(fingers[1])) else {
        return Route::Taken;
    };
    let units = with(hwnd, |touch| {
        let pinch = touch.pinch.as_mut()?;
        let now = apart(a, b);
        if pinch.apart <= 0.0 || now <= 0.0 {
            return None;
        }
        let units = WHEEL_PER_E * (now / pinch.apart).ln() + pinch.rest;
        pinch.apart = now;
        pinch.rest = units - units.trunc();
        Some(units.trunc() as i16)
    });
    if let Some(units) = units.filter(|units| *units != 0) {
        let wheel = (usize::from(units as u16) << 16) | CONTROL;
        let x = ((a.x + b.x) / 2) as u16 as u32;
        let y = ((a.y + b.y) / 2) as u16 as u32;
        let at = LPARAM(((y << 16) | x) as isize);
        // SAFETY: no preconditions.
        let _ = unsafe { PostMessageW(Some(hwnd), WM_MOUSEWHEEL, WPARAM(wheel), at) };
    }
    Route::Taken
}

fn lift(hwnd: HWND, id: u32) -> Route {
    with(hwnd, |touch| {
        let pinched = touch.pinch.as_ref().is_some_and(|p| p.fingers.contains(&id));
        if pinched {
            touch.pinch = None;
        }
        match touch.recognized.remove(&id) {
            true if pinched || touch.pinch.is_some() => Route::Taken,
            true => Route::Gpui,
            false => Route::System,
        }
    })
}

/// SAFETY: called by the system with a live window of this thread and the message's own
/// parameters.
unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // SAFETY: as above.
    unsafe {
        match message {
            WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP | WM_POINTERCAPTURECHANGED => {
                let id = (wparam.0 & 0xffff) as u32;
                match route(hwnd, message, id) {
                    Route::Gpui => DefSubclassProc(hwnd, message, wparam, lparam),
                    Route::System => DefWindowProcW(hwnd, message, wparam, lparam),
                    Route::Taken => LRESULT(0),
                }
            }
            WM_NCDESTROY => {
                WINDOWS.with_borrow_mut(|windows| windows.remove(&(hwnd.0 as isize)));
                let _ = RemoveWindowSubclass(hwnd, Some(procedure), ID);
                DefSubclassProc(hwnd, message, wparam, lparam)
            }
            _ => DefSubclassProc(hwnd, message, wparam, lparam),
        }
    }
}
