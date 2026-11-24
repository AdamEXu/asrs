use gpui_kit::{
    App, Bounds, Div, ElementId, FocusHandle, IntoElement, Styled, Window, canvas, div, point, px,
    rgb,
};

use crate::input::skia;
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
