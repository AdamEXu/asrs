use gpui_kit::{
    App, Bounds, ClickEvent, Div, ElementId, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, KeyboardButton, KeyboardClickEvent, Keystroke, Stateful,
    StatefulInteractiveElement, Styled, Window, actions, canvas, div, point, px, rgb,
};

use crate::input::skia;
use crate::selection::colors::page_active;
use crate::theme::{Appearance, FOCUS_RING_RADIUS, FOCUS_RING_WIDTH};

actions!(sequential_focus, [SelectContents]);

/// `:focus`, which no element of an inactive window matches.
pub fn has_focus(handle: &FocusHandle, window: &Window, cx: &App) -> bool {
    handle.is_focused(window) && page_active(window, cx)
}

/// `:focus-visible` of an element that takes no text.
pub fn focus_visible(handle: &FocusHandle, window: &Window, cx: &App) -> bool {
    has_focus(handle, window, cx) && window.focus_visible()
}

/// `global.css`'s 2 px `:focus-visible` outline, outside its parent's box.
pub fn outline(appearance: Appearance) -> Div {
    let outset = px(-FOCUS_RING_WIDTH);
    div()
        .absolute()
        .top(outset)
        .left(outset)
        .right(outset)
        .bottom(outset)
        .border(px(FOCUS_RING_WIDTH))
        .border_color(rgb(appearance.theme().focus_ring))
        .rounded(px(FOCUS_RING_RADIUS + FOCUS_RING_WIDTH))
}

/// html.css's `outline-offset` of a focused link.
pub const LINK_OUTLINE_OFFSET: f64 = 1.0;

/// The outline of an element a transform places at a fraction of a pixel, which the outline
/// keeps where GPUI would snap it.
pub struct TransformedOutline {
    /// The CSS size of the element's box, which covers the outline's parent.
    pub size: [f64; 2],
    /// `outline-offset`.
    pub offset: f64,
    /// The CSS position of the box less where GPUI's pixel snapping put it.
    pub subpixel: [f64; 2],
    /// What of that position is the transform's.
    pub translate: [f64; 2],
    pub appearance: Appearance,
}

impl TransformedOutline {
    pub fn element(self) -> impl IntoElement {
        let color = self.appearance.theme().focus_ring;
        canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                let scale = f64::from(window.scale_factor());
                let device = |v: f64| (v * scale + 0.5).floor();
                let [w, h] = self.size.map(device);
                let (stroke, offset) = (device(f64::from(FOCUS_RING_WIDTH)), device(self.offset));
                let margin = stroke + offset + 1.0;
                let origin = [f64::from(bounds.origin.x), f64::from(bounds.origin.y)];
                let [dx, dy] = [0, 1].map(|i| {
                    let layout = origin[i] + self.subpixel[i] - self.translate[i];
                    device(layout) + self.translate[i] * scale - origin[i] * scale
                });
                let inset = offset + stroke / 2.0;
                let radius = f64::from(FOCUS_RING_RADIUS) * scale + inset;
                let margin_px = px((margin / scale) as f32);
                let origin = point(bounds.origin.x - margin_px, bounds.origin.y - margin_px);
                let paint = format!(
                    "fill=\"none\" {} stroke-width=\"{stroke}\"",
                    skia::paint_attrs("stroke", color << 8 | 0xff)
                );
                let ring = skia::rect(
                    margin + dx - inset,
                    margin + dy - inset,
                    w + 2.0 * inset,
                    h + 2.0 * inset,
                    radius,
                    &paint,
                );
                let extent = |v: f64| (v + 2.0 * margin) as u32;
                skia::paint(
                    ("transformed-outline", &ring),
                    Bounds { origin, ..bounds },
                    || skia::svg(extent(w), extent(h), 1.0, &ring),
                    window,
                    cx,
                );
            },
        )
        .absolute()
        .inset_0()
    }
}

/// A button's focus and whether it takes `global.css`'s `:focus-visible` style.
pub struct FocusRing {
    pub handle: FocusHandle,
    pub visible: bool,
}

impl FocusRing {
    /// The state of the button `id`, which is a tab stop, focused through `given` or a handle of
    /// its own.
    pub fn of(
        id: impl Into<ElementId>,
        given: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut App,
    ) -> FocusRing {
        let handle = window.use_keyed_state(id, cx, |_, cx| {
            given.unwrap_or_else(|| cx.focus_handle()).tab_stop(true)
        });
        let handle = handle.read(cx).clone();
        FocusRing {
            visible: focus_visible(&handle, window, cx),
            handle,
        }
    }

    pub fn outline(&self, appearance: Appearance) -> Option<Div> {
        self.visible.then(|| outline(appearance))
    }
}

/// A key that moves the focus: whether it moves it back.
pub fn tab_key(keystroke: &Keystroke) -> Option<bool> {
    let modifiers = keystroke.modifiers;
    let taken =
        modifiers.control || modifiers.platform || (modifiers.alt && !cfg!(target_os = "macos"));
    (keystroke.key == "tab" && !taken).then_some(modifiers.shift)
}

/// The handle of an element that takes no focus, so that Tab goes on from a press on it.
pub fn place(id: impl Into<ElementId>, window: &mut Window, cx: &mut App) -> FocusHandle {
    let handle = window.use_keyed_state(id, cx, |_, cx| cx.focus_handle());
    handle.read(cx).clone()
}

/// Makes `element` the `index`th part of the page's order, whatever order it is drawn in. Tab goes
/// on from `place` after a press on it that focuses nothing.
pub fn section<E: InteractiveElement>(element: E, index: isize, place: &FocusHandle) -> E {
    element.track_focus(place).tab_group().tab_index(index)
}

/// `place` after the other elements of its section, whatever order it is drawn in.
pub fn last(place: &FocusHandle) -> FocusHandle {
    place.clone().tab_index(isize::MAX)
}

/// An element of the order that is not displayed.
pub fn hidden(handle: &FocusHandle) -> Div {
    div()
        .absolute()
        .track_focus(&handle.clone().tab_stop(false))
}

/// Where Tab goes from in a page: Blink's sequential focus navigation starting point, which
/// outlives the focus.
#[derive(Default)]
pub struct Traversal {
    /// What had the focus last, or was pressed since.
    last: Option<FocusHandle>,
    /// The page's order when `last` was in it.
    order: Vec<FocusHandle>,
}

impl Traversal {
    /// A press, before it focuses what it is on.
    pub fn pressed(&mut self) {
        self.last = None;
    }

    /// Follows the focus in a page that was just drawn. `body` has it while no element does, and
    /// takes it from a [`place`] and from an element that is gone.
    pub fn drawn(&mut self, body: &FocusHandle, window: &mut Window, cx: &mut App) {
        let order = window.tab_order();
        let focused = order
            .iter()
            .find(|handle| *handle != body && handle.is_focused(window))
            .cloned();
        match focused {
            Some(handle) => {
                self.order = order;
                if !handle.tab_stop {
                    body.focus(window, cx);
                }
                self.last = Some(handle);
            }
            None => {
                if self.last.as_ref().is_some_and(|last| order.contains(last)) {
                    self.order = order;
                }
                if !body.contains_focused(window, cx) {
                    body.focus(window, cx);
                }
            }
        }
    }

    /// `gone` were removed, all from before the rest of what `place` holds.
    pub fn removed(&mut self, gone: &[FocusHandle], place: &FocusHandle) {
        if self.last.as_ref().is_some_and(|last| gone.contains(last)) {
            self.last = Some(place.clone());
        }
    }

    /// Focuses the next or previous element of the page, beginning again at its end.
    pub fn advance(
        &mut self,
        back: bool,
        body: &FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<FocusHandle> {
        let order = window.tab_order();
        let focused = order
            .iter()
            .find(|handle| *handle != body && handle.is_focused(window))
            .cloned();
        let start = focused.or(self.last.take());
        let passed = |handle: &&FocusHandle| handle.tab_stop;
        let current = start
            .as_ref()
            .and_then(|s| order.iter().position(|h| h == s));
        let next = match (current, start) {
            (Some(i), _) if back => order[..i].iter().rfind(passed),
            (Some(i), _) => order[i + 1..].iter().find(passed),
            (None, Some(start)) => {
                let kept = |handle: &&FocusHandle| order.contains(handle) && handle.tab_stop;
                let i = self.order.iter().position(|handle| *handle == start);
                i.and_then(|i| match back {
                    true => self.order[..i].iter().rfind(kept),
                    false => self.order[i + 1..].iter().find(kept),
                })
            }
            (None, None) => None,
        };
        let next = next.or_else(|| match back {
            true => order.iter().rfind(passed),
            false => order.iter().find(passed),
        })?;
        let next = next.clone();
        window.focus(&next, cx);
        next.dispatch_action(&SelectContents, window, cx);
        self.last = Some(next.clone());
        self.order = order;
        Some(next)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollAxis {
    Horizontal,
    Vertical,
}

/// `ScrollAlignment::CenterIfNeeded` as `Element::UpdateSelectionOnFocus` changes it, which leaves
/// what shows in part horizontally: how far a scroller that shows `visible` moves to show the
/// focused `[start, length]` at that offset from its start.
pub fn reveal(axis: ScrollAxis, visible: f64, [start, length]: [f64; 2]) -> f64 {
    let end = start + length;
    let intersect = end.min(visible) - start.max(0.0);
    let shown = intersect >= length
        || intersect >= visible
        || (axis == ScrollAxis::Horizontal && intersect > 0.0);
    if shown {
        0.0
    } else if intersect <= 0.0 {
        start + (length - visible) / 2.0
    } else if end > visible {
        end - visible
    } else {
        start
    }
}

/// Enter activates a focused element as it is pressed, in HTML, where GPUI waits for its release.
pub trait Activate: StatefulInteractiveElement + Sized {
    fn on_activate(self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self;
}

impl Activate for Stateful<Div> {
    fn on_activate(self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        let handler = std::rc::Rc::new(handler);
        let pressed = handler.clone();
        self.on_click(move |event, window, cx| {
            if !matches!(event, ClickEvent::Keyboard(key) if key.button == KeyboardButton::Enter) {
                handler(event, window, cx);
            }
        })
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            let modifiers = event.keystroke.modifiers;
            if event.keystroke.key == "enter"
                && !modifiers.control
                && !modifiers.platform
                && !window.default_prevented()
            {
                let click = KeyboardClickEvent {
                    button: KeyboardButton::Enter,
                    ..Default::default()
                };
                pressed(&ClickEvent::Keyboard(click), window, cx);
            }
        })
    }
}
