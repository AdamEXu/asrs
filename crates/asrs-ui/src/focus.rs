use gpui_kit::{
    App, Div, ElementId, FocusHandle, InteractiveElement, ParentElement, Styled, Window, div, px,
    rgb,
};

use crate::selection::colors::page_active;
use crate::theme::{Appearance, FOCUS_RING_RADIUS, FOCUS_RING_WIDTH};

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
            given
                .unwrap_or_else(|| cx.focus_handle())
                .tab_stop(true)
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
