//! The tooltips of HTML's `title` attributes. [`titled`] gives an element one; which text is under
//! the pointer follows Blink's hit testing, and what the platform makes of it follows Chromium: an
//! AppKit tooltip on macOS, `TooltipAura` elsewhere.

pub mod aura;
mod blink;
mod popup;

use std::collections::HashMap;

use gpui_kit::{
    AnyElement, App, AppContext, Bounds, DispatchPhase, Element, ElementId, Entity, Global,
    GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, IntoElement, KeyDownEvent,
    LayoutId, MouseDownEvent, MouseExitEvent, MouseMoveEvent, MouseUpEvent, Pixels, ScrollEvent,
    SharedString, Task, Window, WindowId,
};

use crate::platform;

/// `kMaxTooltipLength` of `RenderWidgetHostNSViewBridge::SetTooltipText`, in UTF-16 units.
const MAC_MAX_LENGTH: usize = 1024;

/// What shows the tooltips of a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    /// `RenderWidgetHostViewCocoa` and AppKit.
    Native,
    /// `TooltipController` and `TooltipAura`.
    Aura,
}

/// The host in place of the platform's own.
#[cfg(feature = "parity")]
pub struct ForcedHost(pub Host);

#[cfg(feature = "parity")]
impl Global for ForcedHost {}

fn host(cx: &App) -> Host {
    #[cfg(feature = "parity")]
    if let Some(forced) = cx.try_global::<ForcedHost>() {
        return forced.0;
    }
    let _ = cx;
    if cfg!(target_os = "macos") {
        Host::Native
    } else {
        Host::Aura
    }
}

struct Entry {
    hitbox: Hitbox,
    title: Option<SharedString>,
}

#[derive(Default)]
struct WindowTooltips {
    entries: Vec<Entry>,
    /// Whether the window's elements are painting, which the next frame's then replace.
    painting: bool,
    inside: bool,
    page: blink::Page,
    text: String,
    aura: aura::Controller,
    timer: Option<Task<()>>,
    popup: Option<popup::Popup>,
    shown: Option<Shown>,
    activation: Option<Entity<Activation>>,
}

/// Holds the observer of a window's activation.
struct Activation;

/// A tooltip [`Host::Aura`] shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub text: String,
    /// Where the pointer is in the window.
    pub pointer: gpui_kit::Point<Pixels>,
}

#[derive(Default)]
struct Tooltips(HashMap<WindowId, WindowTooltips>);

impl Global for Tooltips {}

fn state<R>(window: &Window, cx: &mut App, f: impl FnOnce(&mut WindowTooltips) -> R) -> R {
    let id = window.window_handle().window_id();
    f(cx.default_global::<Tooltips>().0.entry(id).or_default())
}

/// The text the host was last given for the window.
pub fn text(window: &Window, cx: &mut App) -> String {
    state(window, cx, |state| state.text.clone())
}

/// The tooltip [`Host::Aura`] shows for the window.
pub fn shown(window: &Window, cx: &mut App) -> Option<Shown> {
    state(window, cx, |state| state.shown.clone())
}

/// Every `title` in the window that the pointer could be over at the middle of its box, with its
/// box in window coordinates.
pub fn titles(window: &Window, cx: &mut App) -> Vec<(SharedString, Bounds<Pixels>)> {
    state(window, cx, |state| {
        let titled = |entry: &Entry| entry.title.as_ref().is_some_and(|t| !t.is_empty());
        (state.entries.iter().filter(|e| titled(e)))
            .filter_map(|e| Some((e.title.clone()?, e.hitbox.bounds)))
            .collect()
    })
}

/// An element with the `title` attribute, which the empty string is one of: no tooltip shows over
/// it, not even an ancestor's.
pub fn titled(title: impl Into<SharedString>, child: impl IntoElement) -> Titled {
    Titled {
        title: Some(title.into()),
        child: child.into_any_element(),
    }
}

/// An element without the attribute inside a [`titled`] one. The tooltip's delay starts over as
/// the pointer moves between the two.
pub fn untitled(child: impl IntoElement) -> Titled {
    Titled {
        title: None,
        child: child.into_any_element(),
    }
}

pub struct Titled {
    title: Option<SharedString>,
    child: AnyElement,
}

impl IntoElement for Titled {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Titled {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        self.child.prepaint(window, cx);
        // After the child, whose own hitboxes may hide the ones before them from the pointer.
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        register(
            Entry {
                hitbox: hitbox.clone(),
                title: self.title.clone(),
            },
            window,
            cx,
        );
        self.child.paint(window, cx);
    }
}

fn register(entry: Entry, window: &mut Window, cx: &mut App) {
    let first = state(window, cx, |state| {
        let first = !state.painting;
        if first {
            state.painting = true;
            state.entries.clear();
        }
        state.entries.push(entry);
        first
    });
    if first {
        listen(window, cx);
        window.defer(cx, |window, cx| {
            state(window, cx, |state| state.painting = false);
            update(window, cx);
        });
    }
}

fn listen(window: &mut Window, cx: &mut App) {
    if state(window, cx, |state| state.activation.is_none()) {
        let open: Vec<_> = cx.windows().iter().map(|w| w.window_id()).collect();
        let this = window.window_handle().window_id();
        let tooltips = &mut cx.default_global::<Tooltips>().0;
        tooltips.retain(|id, _| *id == this || open.contains(id));
        let activation = cx.new(|cx| {
            cx.observe_window_activation(window, |_, window, cx| {
                if !window.is_window_active() {
                    control(window, cx, aura::Controller::reset);
                }
            })
            .detach();
            Activation
        });
        state(window, cx, |state| state.activation = Some(activation));
    }
    window.on_mouse_event(|event: &MouseMoveEvent, phase, window, cx| {
        if phase == DispatchPhase::Capture {
            let (position, pressed) = (event.position, event.pressed_button.is_some());
            state(window, cx, |state| state.inside = true);
            control(window, cx, |aura| aura.mouse_moved(position, pressed));
            update(window, cx);
        }
    });
    window.on_mouse_event(|_: &MouseExitEvent, phase, window, cx| {
        if phase == DispatchPhase::Capture {
            state(window, cx, |state| state.inside = false);
            match host(cx) {
                Host::Native => set_text("", window, cx),
                Host::Aura => control(window, cx, aura::Controller::mouse_exited),
            }
        }
    });
    window.on_mouse_event(|_: &MouseDownEvent, phase, window, cx| {
        if phase == DispatchPhase::Capture {
            control(window, cx, aura::Controller::mouse_pressed);
        }
    });
    window.on_mouse_event(|_: &MouseUpEvent, phase, window, cx| {
        if phase == DispatchPhase::Capture {
            state(window, cx, |state| state.aura.mouse_released());
        }
    });
    window.on_mouse_event(|_: &ScrollEvent, phase, window, cx| {
        if phase == DispatchPhase::Capture {
            control(window, cx, aura::Controller::wheel);
        }
    });
    window.on_key_event(|_: &KeyDownEvent, phase, window, cx| {
        if phase == DispatchPhase::Capture {
            control(window, cx, aura::Controller::reset);
        }
    });
}

fn update(window: &mut Window, cx: &mut App) {
    if !state(window, cx, |state| state.inside) {
        return;
    }
    let position = window.mouse_position();
    let mut texts = Vec::new();
    let id = window.window_handle().window_id();
    if let Some(state) = cx.default_global::<Tooltips>().0.get_mut(&id) {
        let hovered: Vec<&Entry> = (state.entries.iter())
            .filter(|entry| entry.hitbox.is_hovered_at(position, window))
            .collect();
        let node = hovered.last().map(|entry| entry.hitbox.bounds);
        let title = hovered.iter().rev().find_map(|e| e.title.as_deref());
        state.page.moved(position, node, title, |text| {
            texts.push(text.to_owned());
        });
    }
    for text in texts {
        set_text(&text, window, cx);
    }
}

/// `RenderWidgetHostView::UpdateTooltip`.
fn set_text(text: &str, window: &mut Window, cx: &mut App) {
    match host(cx) {
        Host::Native => {
            if state(window, cx, |state| state.text == text) || !platform::is_key_window(window) {
                return;
            }
            state(window, cx, |state| text.clone_into(&mut state.text));
            let units: Vec<u16> = text.encode_utf16().take(MAC_MAX_LENGTH).collect();
            platform::set_tooltip(window, &String::from_utf16_lossy(&units));
        }
        Host::Aura => {
            state(window, cx, |state| text.clone_into(&mut state.text));
            control(window, cx, |aura| aura.set_text(text));
        }
    }
}

fn control(
    window: &mut Window,
    cx: &mut App,
    f: impl FnOnce(&mut aura::Controller) -> Vec<aura::Command>,
) {
    if host(cx) != Host::Aura {
        return;
    }
    for command in state(window, cx, |state| f(&mut state.aura)) {
        match command {
            aura::Command::Hide => hide(window, cx),
            aura::Command::Start => {
                let handle = window.window_handle();
                let timer = cx.background_executor().timer(aura::SHOW_DELAY);
                let timer = cx.spawn(async move |cx| {
                    timer.await;
                    handle.update(cx, |_, window, cx| show(window, cx)).ok();
                });
                state(window, cx, |state| state.timer = Some(timer));
            }
        }
    }
}

fn hide(window: &mut Window, cx: &mut App) {
    let popup = state(window, cx, |state| {
        state.timer = None;
        state.shown = None;
        state.popup.take()
    });
    if let Some(popup) = popup {
        popup.close(cx);
    }
}

fn show(window: &mut Window, cx: &mut App) {
    let Some((text, pointer)) = state(window, cx, |state| state.aura.show_now()) else {
        return;
    };
    let popup = popup::Popup::open(&text, pointer, window, cx);
    state(window, cx, |state| {
        state.popup = popup;
        state.shown = Some(Shown { text, pointer });
    });
}
