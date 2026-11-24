//! The beta welcome prompt's page (`www/betaWelcome.html`).

use std::time::{Duration, Instant};

use asrs_canvas::text::dom::Lu;
use asrs_js::value::JsValue;
use asrs_tabs::info::BetaWelcome;
use asrs_tabs::info::beta_welcome::CHECKBOXES;
use asrs_ui::platform;
use asrs_ui::focus;
use asrs_ui::gpui::{
    AnyElement, App, Context, Div, EventEmitter, FocusHandle, Focusable, HoverListenerMode,
    InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent,
    ParentElement, Render, StatefulInteractiveElement, Styled, Subscription, Window, div, px, rgb,
};
use asrs_ui::input::{Checkbox, PointerState};
use asrs_ui::parity::probed;
use asrs_ui::theme::{Appearance, FOCUS_RING_RADIUS};
use asrs_ui::tooltip::{titled, untitled};

use super::layout::{BetaLayout, BetaRow, LuRect, TextBlock, Typesetter};
use super::page::{self, PageScroll, Placer, ScrolledPage, mix};
use super::text::{DocumentText, PageText};

/// `h2` and `button`: a colour per theme, light then dark.
const ACCENT: [u32; 2] = [0x012be5, 0x446ce3];
const BUTTON_HOVER: [u32; 2] = [0x274cf2, 0x2751cf];
const BUTTON_ACTIVE: [u32; 2] = [0x5f7bf5, 0x0e37b3];
const PURPLE: [u32; 2] = [0xc566e2, 0xb560cd];
const BUTTON_RADIUS: f32 = 6.0;
const DISABLED_OPACITY: f64 = 0.35;
const OPACITY_TRANSITION: Duration = Duration::from_millis(200);

/// The button posted `null` to main.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Acknowledged;

/// `document.activeElement`, unless it is the body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Checkbox(usize),
    Button,
}

/// The button's `transition: opacity 0.2s` from `from` since `start`.
#[derive(Clone, Copy)]
struct Fade {
    from: f64,
    start: Instant,
}

pub struct BetaWelcomePage {
    page: BetaWelcome,
    app_name: &'static str,
    typesetter: Typesetter,
    scroll: PageScroll,
    text: PageText,
    body: FocusHandle,
    checkboxes: [FocusHandle; CHECKBOXES],
    button: FocusHandle,
    /// GPUI's own hover styles end at a key press.
    hovered: Option<Control>,
    pressed: Option<Control>,
    /// The control a held Space is pressing.
    space: Option<Control>,
    fade: Option<Fade>,
    _keys: Subscription,
}

impl EventEmitter<Acknowledged> for BetaWelcomePage {}

impl BetaWelcomePage {
    pub fn new(app_name: &'static str, window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let body = cx.focus_handle();
        window.focus(&body, cx);
        BetaWelcomePage {
            page: BetaWelcome::default(),
            app_name,
            typesetter: Typesetter::default(),
            scroll: PageScroll::new(cx),
            text: PageText::default(),
            body,
            checkboxes: std::array::from_fn(|_| cx.focus_handle()),
            button: cx.focus_handle(),
            hovered: None,
            pressed: None,
            space: None,
            fade: None,
            _keys: Self::keys(window, cx),
        }
    }

    /// Tab moves between the controls from anywhere. Enter clicks the button at the press and Space
    /// clicks at the release, where GPUI would click for both.
    fn keys(window: &Window, cx: &mut Context<'_, Self>) -> Subscription {
        let handle = window.window_handle();
        let this = cx.entity().downgrade();
        cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle() != handle || platform::menu_takes_keys(window) {
                return;
            }
            let _ = this.update(cx, |view, cx| match event.keystroke.key.as_str() {
                "tab" => {
                    if let Some(back) = focus::tab_key(&event.keystroke) {
                        view.tab(back, window, cx);
                        cx.stop_propagation();
                    }
                }
                key @ ("space" | "enter") => {
                    let Some(control) = view.active(window) else {
                        return;
                    };
                    if key == "space" {
                        view.space = Some(control);
                        cx.notify();
                    } else if control == Control::Button {
                        view.click_continue(cx);
                    }
                    cx.stop_propagation();
                }
                _ => {}
            });
        })
    }

    fn key_up(&mut self, event: &KeyUpEvent, window: &Window, cx: &mut Context<'_, Self>) {
        if event.keystroke.key != "space" {
            return;
        }
        match self
            .space
            .take()
            .filter(|c| self.active(window) == Some(*c))
        {
            Some(Control::Checkbox(index)) => self.toggle(index, cx),
            Some(Control::Button) => self.click_continue(cx),
            None => {}
        }
        cx.notify();
    }

    pub fn page(&self) -> &BetaWelcome {
        &self.page
    }

    /// `port2.postMessage(isAlpha())` from main.
    pub fn message(&mut self, is_alpha: bool, cx: &mut Context<'_, Self>) {
        self.page.message(&JsValue::Bool(is_alpha));
        cx.notify();
    }

    pub fn active(&self, window: &Window) -> Option<Control> {
        let mut checkboxes = self.checkboxes.iter();
        if let Some(index) = checkboxes.position(|handle| handle.is_focused(window)) {
            return Some(Control::Checkbox(index));
        }
        (self.page.enabled() && self.button.is_focused(window)).then_some(Control::Button)
    }

    fn focus_on(&mut self, control: Option<Control>, window: &mut Window, cx: &mut App) {
        let handle = match control {
            Some(Control::Checkbox(index)) => &self.checkboxes[index],
            Some(Control::Button) => &self.button,
            None => &self.body,
        };
        window.focus(handle, cx);
        self.space = None;
    }

    /// Past either end the focus leaves the page, and from there enters at the other.
    fn tab(&mut self, back: bool, window: &mut Window, cx: &mut Context<'_, Self>) {
        let mut stops: Vec<Control> = (0..CHECKBOXES).map(Control::Checkbox).collect();
        if self.page.enabled() {
            stops.push(Control::Button);
        }
        let at = self
            .active(window)
            .and_then(|active| stops.iter().position(|stop| *stop == active));
        let next = match (at, back) {
            (None, false) => Some(0),
            (None, true) => Some(stops.len() - 1),
            (Some(i), false) => (i + 1 < stops.len()).then_some(i + 1),
            (Some(i), true) => i.checked_sub(1),
        };
        self.focus_on(next.map(|i| stops[i]), window, cx);
        cx.notify();
    }

    fn toggle(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let opacity = self.opacity(cx);
        let enabled = self.page.enabled();
        self.page.toggle(index);
        if self.page.enabled() != enabled {
            self.fade = Some(Fade {
                from: opacity,
                start: cx.background_executor().now(),
            });
        }
        cx.notify();
    }

    fn click_continue(&mut self, cx: &mut Context<'_, Self>) {
        if self.page.click_continue() {
            cx.emit(Acknowledged);
        }
    }

    /// The button's `opacity` in its transition.
    fn opacity(&self, cx: &App) -> f64 {
        let to = if self.page.enabled() {
            1.0
        } else {
            DISABLED_OPACITY
        };
        let Some(Fade { from, start }) = self.fade else {
            return to;
        };
        let elapsed = cx.background_executor().now() - start;
        let t = elapsed.as_secs_f64() / OPACITY_TRANSITION.as_secs_f64();
        if t >= 1.0 {
            return to;
        }
        from + (to - from) * ease(t)
    }
}

/// `cubic-bezier(0.25, 0.1, 0.25, 1)` (`ease`) at `t`.
fn ease(t: f64) -> f64 {
    let (x1, y1, x2, y2) = (0.25, 0.1, 0.25, 1.0);
    let bezier = |a: f64, b: f64, s: f64| {
        let r = 1.0 - s;
        3.0 * r * r * s * a + 3.0 * r * s * s * b + s * s * s
    };
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..48 {
        let s = (low + high) / 2.0;
        if bezier(x1, x2, s) < t {
            low = s;
        } else {
            high = s;
        }
    }
    bezier(y1, y2, (low + high) / 2.0)
}

impl Focusable for BetaWelcomePage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.body.clone()
    }
}

impl ScrolledPage for BetaWelcomePage {
    fn scroll(&mut self) -> &mut PageScroll {
        &mut self.scroll
    }

    fn text(&mut self) -> &mut PageText {
        &mut self.text
    }

    fn document_text(&self, _: &Window) -> Option<Box<dyn DocumentText>> {
        None
    }
}

impl BetaWelcomePage {
    fn pointer(&self, control: Control) -> PointerState {
        if self.space == Some(control) {
            return PointerState::Pressed;
        }
        match (self.hovered == Some(control), self.pressed == Some(control)) {
            (true, true) => PointerState::Pressed,
            (true, false) => PointerState::Hovered,
            _ => PointerState::Normal,
        }
    }

    fn on_hover(
        control: Control,
        cx: &mut Context<'_, Self>,
    ) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
        cx.listener(move |view, hovered: &bool, _, cx| {
            if *hovered {
                view.hovered = Some(control);
            } else if view.hovered == Some(control) {
                view.hovered = None;
            }
            cx.notify();
        })
    }

    fn on_press(
        control: Control,
        cx: &mut Context<'_, Self>,
    ) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
        cx.listener(move |view, _: &MouseDownEvent, window, cx| {
            view.pressed = Some(control);
            view.focus_on(Some(control), window, cx);
            cx.stop_propagation();
            cx.notify();
        })
    }

    fn checkbox(
        &self,
        index: usize,
        bounds: LuRect,
        placer: Placer,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let pointer = self.pointer(Control::Checkbox(index));
        let checkbox = Checkbox::new(("mark", index), self.page.checked()[index]).pointer(pointer);
        let control = div()
            .id(("checkbox", index))
            .track_focus(&self.checkboxes[index])
            .size_full()
            .on_hover(Self::on_hover(Control::Checkbox(index), cx))
            .hover_listener_mode(HoverListenerMode::InputModalityIndependent)
            .on_mouse_down(
                MouseButton::Left,
                Self::on_press(Control::Checkbox(index), cx),
            )
            .on_click(cx.listener(move |view, _, _, cx| view.toggle(index, cx)))
            .child(checkbox);
        probed("input", placer.place(bounds).child(control)).into_any_element()
    }

    fn continue_button(
        &mut self,
        bounds: LuRect,
        text: &TextBlock,
        placer: Placer,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let appearance = Appearance::of(window, cx);
        let background = appearance.theme().background;
        let opacity = self.opacity(cx);
        let pick =
            |colors: [u32; 2]| mix(appearance.pick(colors[0], colors[1]), background, opacity);
        let ring = self.page.enabled() && focus::focus_visible(&self.button, window, cx);
        let radius = if ring {
            FOCUS_RING_RADIUS
        } else {
            BUTTON_RADIUS
        };
        let color = match self.pointer(Control::Button) {
            PointerState::Hovered if self.page.enabled() => BUTTON_HOVER,
            PointerState::Pressed if self.page.enabled() => BUTTON_ACTIVE,
            _ => ACCENT,
        };
        let mut button = div()
            .id("continue")
            .track_focus(&self.button)
            .size_full()
            .rounded(px(radius))
            .bg(rgb(pick(color)))
            .on_hover(Self::on_hover(Control::Button, cx))
            .hover_listener_mode(HoverListenerMode::InputModalityIndependent);
        if self.page.enabled() {
            button = button
                .on_mouse_down(MouseButton::Left, Self::on_press(Control::Button, cx))
                .on_click(cx.listener(|view, _, _, cx| view.click_continue(cx)));
        }
        let mut boxed = placer.place(bounds).child(button);
        if ring {
            boxed = boxed.child(focus::outline(appearance));
        }
        let label = placer.block(text, |_| mix(0xffffff, background, opacity), cx);
        let label = label
            .into_iter()
            .map(|label| untitled(label).into_any_element());
        div()
            .child(probed("button", titled("Continue", boxed)))
            .children(label)
            .into_any_element()
    }

    fn row(
        &self,
        index: usize,
        row: &BetaRow,
        placer: Placer,
        color: u32,
        cx: &mut Context<'_, Self>,
    ) -> Div {
        let [first, second] = row.cells;
        let text = placer.block(&row.text, |_| color, cx);
        div()
            .child(probed("td", placer.place(first)))
            .child(probed("td", placer.place(second)))
            .child(self.checkbox(index, row.checkbox, placer, cx))
            .children(text)
    }
}

impl Render for BetaWelcomePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let (width, scale): (Lu, f32) = (page::device_width(window), window.scale_factor());
        let t = &mut self.typesetter;
        let layout = BetaLayout::new(t, &self.page, self.app_name, width, scale);
        self.scroll.lay_out(layout.height, window, cx);
        if let Some(fade) = self.fade {
            if cx.background_executor().now() - fade.start < OPACITY_TRANSITION {
                window.request_animation_frame();
            } else {
                self.fade = None;
            }
        }
        let placer = Placer::new(&self.scroll);
        let appearance = Appearance::of(window, cx);
        let theme = appearance.theme();
        let pick = |colors: [u32; 2]| appearance.pick(colors[0], colors[1]);
        let mut root = page::root("beta-welcome", self, cx)
            .track_focus(&self.body)
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, _, cx| {
                if view.scroll.key(&event.keystroke, cx) {
                    cx.notify();
                }
            }))
            .on_key_up(cx.listener(|view, event: &KeyUpEvent, window, cx| {
                view.key_up(event, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, window, cx| {
                    view.focus_on(None, window, cx);
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| {
                    view.pressed = None;
                    cx.notify();
                }),
            )
            .bg(rgb(theme.background));
        let titles = [(&layout.title, "h1", theme.text)];
        let headings = layout.headings.iter().map(|h| (h, "h2", pick(ACCENT)));
        let paragraphs = [
            (&layout.intro, "p", theme.text),
            (&layout.outro, "p", theme.text),
        ];
        for ((rect, block), id, color) in titles.into_iter().chain(headings).chain(paragraphs) {
            let purple = pick(PURPLE);
            let color = |item: usize| if item == 1 { purple } else { color };
            root = root
                .child(probed(id, placer.place(*rect)))
                .children(placer.block(block, color, cx));
        }
        root = root.child(probed("table", placer.place(layout.table)));
        for (index, row) in layout.rows.iter().enumerate() {
            root = root.child(self.row(index, row, placer, theme.text, cx));
        }
        root.child(self.continue_button(layout.button, &layout.button_text, placer, window, cx))
    }
}
