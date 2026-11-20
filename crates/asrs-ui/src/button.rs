use gpui_kit::{
    App, ClickEvent, ElementId, FocusHandle, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, StatefulInteractiveElement, Styled, Window, div, px, rgb,
};

use crate::icon::{Icon, IconFit, IconSource};
use crate::theme::{Appearance, BUTTON_RADIUS, FOCUS_RING_RADIUS, FOCUS_RING_WIDTH, IconTint};

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// `<button><img src="symbols/..."></button>` styled by `global.css`: transparent, 6 px radius,
/// hover and active backgrounds unless disabled, and the icon tinted by a filter chain that
/// changes when the window is blurred or the button is disabled.
///
/// `blurred` mirrors upstream's `button.blurred` class, which the hub toggles on every existing
/// button from the main process's `set-focused` message, not live window focus.
///
/// An enabled button is a tab stop. Focused from the keyboard it takes `global.css`'s
/// `:focus-visible` style: a 2 px `#446ce3` outline outside the box, which follows the 3 px
/// border radius that rule also sets.
#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: IconSource,
    width: f32,
    height: f32,
    tint: IconTint,
    fit: IconFit,
    disabled: bool,
    blurred: bool,
    appearance: Option<Appearance>,
    on_click: Option<ClickHandler>,
}

impl IconButton {
    pub fn new(id: impl Into<ElementId>, icon: impl Into<IconSource>) -> Self {
        IconButton {
            id: id.into(),
            icon: icon.into(),
            width: 28.0,
            height: 28.0,
            tint: IconTint::BUTTON,
            fit: IconFit::BUTTON,
            disabled: false,
            blurred: false,
            appearance: None,
            on_click: None,
        }
    }

    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    pub fn tint(mut self, tint: IconTint) -> Self {
        self.tint = tint;
        self
    }

    pub fn fit(mut self, fit: IconFit) -> Self {
        self.fit = fit;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn blurred(mut self, blurred: bool) -> Self {
        self.blurred = blurred;
        self
    }

    pub fn appearance(mut self, appearance: Appearance) -> Self {
        self.appearance = Some(appearance);
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for IconButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let appearance = self
            .appearance
            .unwrap_or_else(|| Appearance::of(window, cx));
        let theme = appearance.theme();
        let chain = self.tint.chain(appearance, self.blurred, self.disabled);
        let focus = (!self.disabled).then(|| {
            window
                .use_keyed_state(self.id.clone(), cx, |_, cx| {
                    cx.focus_handle().tab_stop(true)
                })
                .read(cx)
                .clone()
        });
        let focus_visible = focus.as_ref().is_some_and(|f: &FocusHandle| {
            f.is_focused(window) && window.last_input_was_keyboard()
        });
        let radius = if focus_visible {
            FOCUS_RING_RADIUS
        } else {
            BUTTON_RADIUS
        };
        let mut button = div()
            .id(self.id)
            .relative()
            .flex_none()
            .w(px(self.width))
            .h(px(self.height))
            .rounded(px(radius))
            .child(Icon::new(self.icon, chain).fit(self.fit));
        if let Some(focus) = &focus {
            button = button.track_focus(focus);
        }
        if focus_visible {
            let outset = px(-FOCUS_RING_WIDTH);
            button = button.child(
                div()
                    .absolute()
                    .top(outset)
                    .left(outset)
                    .right(outset)
                    .bottom(outset)
                    .border(px(FOCUS_RING_WIDTH))
                    .border_color(rgb(theme.focus_ring))
                    .rounded(px(FOCUS_RING_RADIUS + FOCUS_RING_WIDTH)),
            );
        }
        if !self.disabled {
            let (hover, active) = (theme.button_hover, theme.button_active);
            button = button
                .hover(move |s| s.bg(rgb(hover)))
                .active(move |s| s.bg(rgb(active)));
            if let Some(handler) = self.on_click {
                button = button.on_click(handler);
            }
        }
        button
    }
}
