//! Linux. Menus and select lists are drawn (`crate::menu`): on X11 in override-redirect windows
//! with the pointer grabbed, on Wayland in popup surfaces that the compositor places and grabs
//! for. Window work goes through the X11 connection; on Wayland it has no equivalent.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{
    AnyWindowHandle, App, AsyncApp, Bounds, Pixels, ScrollDelta, ScrollEvent, Subscription,
    WeakEntity, Window, WindowId, point, px, size,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

pub(super) use super::fallback::*;
use super::{
    MenuPath, PIXELS_PER_WHEEL_TICK, PopupEntry, PopupMenu, SelectMenu, SelectMenuItemKind,
    WheelDelta,
};
use crate::menu::{CloseHandler, ListStyle, MenuView, Placement};

pub(super) mod probe;
pub(super) mod window;
mod x11;

use x11::X11Window;

const GRAB_ATTEMPTS: usize = 20;
const GRAB_RETRY: Duration = Duration::from_millis(10);

struct OpenPopup {
    parent: WindowId,
    view: WeakEntity<MenuView>,
    window: AnyWindowHandle,
    cx: AsyncApp,
    _keys: Subscription,
}

thread_local! {
    static OPEN: RefCell<Option<OpenPopup>> = const { RefCell::new(None) };
}

/// `Window` has a method of this name in two traits.
fn handle_of(window: &Window) -> AnyWindowHandle {
    Window::window_handle(window)
}

fn is_wayland(window: &Window) -> bool {
    HasWindowHandle::window_handle(window)
        .is_ok_and(|handle| matches!(handle.as_raw(), RawWindowHandle::Wayland(_)))
}

/// Chromium reads X11 scroll valuators as 120 px a notch and GPUI as 3 lines a notch, fractions
/// of a notch included.
pub(super) fn wheel_delta(event: &ScrollEvent) -> Option<WheelDelta> {
    match event.delta {
        ScrollDelta::Lines(lines) => Some(WheelDelta {
            x: lines.x * PIXELS_PER_WHEEL_TICK,
            y: lines.y * PIXELS_PER_WHEEL_TICK,
        }),
        ScrollDelta::Pixels(_) => None,
    }
}

/// `gfx::RemoveAccelerator`: the `&` that marks a mnemonic is not shown, `&&` is one `&`.
pub(super) fn native_menu_label(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut chars = label.chars();
    while let Some(c) = chars.next() {
        match c {
            '&' => out.extend(chars.next()),
            c => out.push(c),
        }
    }
    out
}

/// Chromium's `alwaysOnTop`, `_NET_WM_STATE_ABOVE`.
pub(super) fn keep_visible_when_inactive(window: &Window) -> bool {
    X11Window::of(window).is_some_and(|window| window.keep_above().is_ok())
}

pub(super) fn set_aspect_ratio(window: &Window, aspect_ratio: f64) {
    if let Some(window) = X11Window::of(window) {
        window.set_aspect_ratio(aspect_ratio).ok();
    }
}

fn placement(window: &Window, bounds: Bounds<Pixels>) -> Placement {
    if is_wayland(window) {
        return Placement::Anchored(handle_of(window), bounds);
    }
    let origin = match X11Window::of(window) {
        Some(x11) => x11.origin().unwrap_or(window.bounds().origin),
        None => window.bounds().origin,
    };
    Placement::Screen(Bounds::new(origin + bounds.origin, bounds.size))
}

pub(super) fn popup_menu(
    window: &Window,
    cx: &mut App,
    menu: PopupMenu,
    x: i32,
    y: i32,
    on_close: impl FnOnce(Option<MenuPath>, &mut App) + 'static,
) {
    let origin = if x == -1 || y == -1 {
        window.mouse_position()
    } else {
        point(px(x as f32), px(y as f32))
    };
    let anchor = Bounds::new(origin, size(px(0.0), px(0.0)));
    show(window, cx, menu, anchor, None, on_close);
}

pub(super) fn select_popup(
    window: &Window,
    cx: &mut App,
    menu: SelectMenu,
    [x, y, width, height]: [f64; 4],
    on_close: impl FnOnce(Option<usize>, &mut App) + 'static,
) {
    let px = |v: f64| px(v as f32);
    let anchor = Bounds::new(point(px(x), px(y)), size(px(width), px(height)));
    let list = ListStyle {
        min_width: width as f32,
        selected: menu.selected,
        groups: menu
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.kind == SelectMenuItemKind::Group)
            .map(|(i, _)| i)
            .collect(),
    };
    let mut popup = PopupMenu::new();
    for (i, item) in menu.items.into_iter().enumerate() {
        popup = match item.kind {
            SelectMenuItemKind::Separator => popup.separator(),
            kind => popup.entry(
                // The label is text, in which `&` marks nothing.
                PopupEntry::new(item.label.replace('&', "&&"))
                    .enabled(item.enabled && kind == SelectMenuItemKind::Option)
                    .checked(menu.selected == Some(i)),
            ),
        };
    }
    show(window, cx, popup, anchor, Some(list), move |path, cx| {
        on_close(path.and_then(|path| path.first().copied()), cx);
    });
}

fn show(
    window: &Window,
    cx: &mut App,
    menu: PopupMenu,
    anchor: Bounds<Pixels>,
    list: Option<ListStyle>,
    on_close: impl FnOnce(Option<MenuPath>, &mut App) + 'static,
) {
    let parent = handle_of(window);
    cx.defer(move |cx| {
        let opened = parent.update(cx, |_, window, _| {
            (placement(window, anchor), window.scale_factor())
        });
        let Ok((placement, scale)) = opened else {
            return on_close(None, cx);
        };
        close_open_popup();
        let on_close = Rc::new(std::cell::Cell::new(Some(on_close)));
        let close = on_close.clone();
        let handler: CloseHandler = Rc::new(move |path, _, cx| {
            OPEN.take();
            if let Some(on_close) = close.take() {
                cx.defer(move |cx| on_close(path, cx));
            }
        });
        let Some((popup, view)) = MenuView::open(menu, placement, list, scale, cx, handler) else {
            if let Some(on_close) = on_close.take() {
                on_close(None, cx);
            }
            return;
        };
        if matches!(placement, Placement::Screen(_)) {
            parent
                .update(cx, |_, window, cx| {
                    view.update(cx, |view, cx| view.dismiss_with(window, cx));
                })
                .ok();
        }
        let keys = intercept_keys(&view, cx);
        OPEN.set(Some(OpenPopup {
            parent: parent.window_id(),
            view: view.downgrade(),
            window: popup,
            cx: cx.to_async(),
            _keys: keys,
        }));
    });
}

/// The focus stays in the window the menu popped up for, as it does under Chromium's menus, so
/// its keys go to the menu.
fn intercept_keys(view: &gpui_kit::Entity<MenuView>, cx: &mut App) -> Subscription {
    let view = view.downgrade();
    cx.intercept_keystrokes(move |event, window, cx| {
        let Some(view) = view.upgrade() else {
            return;
        };
        let view = view.read(cx);
        if view.owns_window(handle_of(window), cx) {
            return;
        }
        if let Some(menu) = view.innermost_window(cx) {
            let keystroke = event.keystroke.clone();
            cx.defer(move |cx| {
                menu.update(cx, |_, window, cx| window.dispatch_keystroke(keystroke, cx))
                    .ok();
            });
        }
        cx.stop_propagation();
    })
}

fn close_open_popup() {
    if let Some(open) = OPEN.take() {
        let OpenPopup {
            view, window, cx, ..
        } = open;
        cx.spawn(async move |cx| {
            window
                .update(cx, |_, window, cx| {
                    view.update(cx, |view, cx| view.cancel(window, cx))
                })
                .ok();
        })
        .detach();
    }
}

pub(super) fn close_popup_menu(window: &Window) {
    let id = handle_of(window).window_id();
    if OPEN.with_borrow(|open| open.as_ref().is_some_and(|open| open.parent == id)) {
        close_open_popup();
    }
}

/// GPUI shifts new X11 windows by 2 px; `bounds` is where the menu belongs. The outermost menu
/// takes the pointer.
pub(super) fn popup_window_opened(
    window: &Window,
    bounds: Bounds<Pixels>,
    root: bool,
    cx: &mut App,
) {
    let Some(x11) = X11Window::of(window) else {
        return;
    };
    x11.move_to(bounds.origin).ok();
    if !root {
        return;
    }
    let handle = handle_of(window);
    cx.spawn(async move |cx| {
        for _ in 0..GRAB_ATTEMPTS {
            let grabbed = handle.update(cx, |_, window, _| {
                X11Window::of(window).is_none_or(|x11| x11.grab_pointer().unwrap_or(false))
            });
            if grabbed.unwrap_or(true) {
                return;
            }
            cx.background_executor().timer(GRAB_RETRY).await;
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::native_menu_label;

    #[test]
    fn mnemonics_are_not_shown() {
        for (label, shown) in [
            ("&File", "File"),
            ("Save && Close", "Save & Close"),
            ("Trailing&", "Trailing"),
            ("Plain", "Plain"),
        ] {
            assert_eq!(native_menu_label(label), shown);
        }
    }
}
