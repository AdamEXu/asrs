//! What the pages share: the document's scrolling and boxes on the device pixel grid.

use std::time::Instant;

use asrs_canvas::text::dom::{LineHeight, Lu, WhiteSpace};
use asrs_model::scroll_sensor::{Wheel, WheelScroll, assign_scroll_offset};
use asrs_ui::canvas_view::geometry::mouse_client;
use asrs_ui::gpui::{
    App, Context, CursorStyle, DispatchPhase, Div, InteractiveElement, KeyDownEvent, Keystroke,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Render, Rgba, ScrollEvent,
    Stateful, Styled, Window, canvas, div, px,
};
use asrs_ui::parity::{Recorder, TextRun, record_text};
use asrs_ui::platform::EditCommand;
use asrs_ui::selection;
use asrs_ui::text::{Align, Text, TextStyle, snap_remainder};

use super::layout::{LuRect, TextBlock, snapped};
use super::text::{self, DocumentText, PageText};
use crate::views::cell_text::css;
use crate::views::frame::wheel_of;

/// An arrow key's step in px.
const LINE_STEP: f64 = 40.0;
/// How much of the window a Page Up or Page Down goes by at least, and the most device px it leaves.
const PAGE_FRACTION: f64 = 0.875;
const PAGE_OVERLAP: f64 = 40.0;

/// The scroll position of the document.
pub struct PageScroll {
    offset: f64,
    max: f64,
    client: f64,
    scale: f32,
    wheel: WheelScroll,
    started: Instant,
}

impl PageScroll {
    pub fn new(cx: &App) -> PageScroll {
        PageScroll {
            offset: 0.0,
            max: 0.0,
            client: 0.0,
            scale: 1.0,
            wheel: WheelScroll::default(),
            started: cx.background_executor().now(),
        }
    }

    pub fn now(&self, cx: &App) -> f64 {
        let elapsed = cx.background_executor().now() - self.started;
        elapsed.as_secs_f64() * 1000.0
    }

    /// Takes the sizes of the document and of the window, and advances a running animation.
    pub fn lay_out<V: Render>(&mut self, height: Lu, window: &mut Window, cx: &mut Context<'_, V>) {
        self.scale = window.scale_factor();
        self.client = f64::from(f32::from(window.viewport_size().height));
        self.max = (css(snapped(height), self.scale) - self.client).max(0.0);
        if let Some([_, to]) = self.wheel.animate(self.now(cx)) {
            self.offset = to;
        }
        self.offset = self.offset.clamp(0.0, self.max);
        if self.wheel.animating() {
            window.request_animation_frame();
        }
    }

    /// `document.documentElement.scrollTop`.
    pub fn top(&self) -> f64 {
        assign_scroll_offset(self.offset, self.max, f64::from(self.scale))
    }

    pub fn scroll_by(&mut self, by: f64) {
        self.wheel.stop();
        self.offset = (self.top() + by).clamp(0.0, self.max);
    }

    fn scroll(&mut self, wheel: Wheel, client: f64, cx: &App) {
        let now = self.now(cx);
        let (offset, max) = ([0.0, self.offset], [0.0, self.max]);
        let client = [f64::INFINITY, client];
        if let Some([_, to]) = self.wheel.wheel(wheel, now, offset, max, client) {
            self.offset = to.clamp(0.0, self.max);
        }
    }

    pub fn wheel(&mut self, event: &ScrollEvent, cx: &App) {
        self.scroll(wheel_of(event), self.client, cx);
    }

    /// The document's scrolling keys; whether this is one.
    pub fn key(&mut self, keystroke: &Keystroke, cx: &App) -> bool {
        let modifiers = keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.function {
            return false;
        }
        let page = (self.client * PAGE_FRACTION)
            .max(self.client - PAGE_OVERLAP / f64::from(self.scale))
            .max(1.0);
        let end = self.max + self.client;
        let by = match keystroke.key.as_str() {
            "up" if modifiers.platform => -end,
            "down" if modifiers.platform => end,
            _ if modifiers.platform => return false,
            "up" => -LINE_STEP,
            "down" => LINE_STEP,
            "pageup" => -page,
            "pagedown" => page,
            "space" if modifiers.shift => -page,
            "space" => page,
            "home" => -end,
            "end" => end,
            _ => return false,
        };
        let wheel = Wheel {
            delta: [0.0, -by],
            precise: false,
        };
        self.scroll(wheel, f64::INFINITY, cx);
        true
    }
}

pub trait ScrolledPage: Render {
    fn scroll(&mut self) -> &mut PageScroll;

    fn text(&mut self) -> &mut PageText;

    /// The document's text that can be selected, once it is laid out.
    fn document_text(&self, window: &Window) -> Option<Box<dyn DocumentText>>;
}

/// The page's root element, which scrolls the document.
pub fn root<V: ScrolledPage>(
    id: &'static str,
    view: &mut V,
    cx: &mut Context<'_, V>,
) -> Stateful<Div> {
    let entity = cx.entity().downgrade();
    // Window listeners: a press that leaves the window keeps delivering moves and its release.
    let listeners = canvas(
        |_, _, _| {},
        move |_, _, window, _| {
            let view = entity.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble {
                    let _ = view.update(cx, |view, cx| text::mouse_down(view, event, window, cx));
                }
            });
            let view = entity.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble {
                    let at = [event.position.x, event.position.y];
                    let at = at.map(|v| mouse_client(f32::from(v)));
                    let pressed = event.pressed_button.is_some();
                    let _ = view.update(cx, |view, cx| {
                        text::mouse_move(view, at, pressed, window, cx);
                    });
                }
            });
            let view = entity.clone();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble {
                    let at = [event.position.x, event.position.y];
                    let at = at.map(|v| mouse_client(f32::from(v)));
                    let _ = view.update(cx, |view, cx| text::mouse_up(view, at, window, cx));
                }
            });
        },
    );
    let mut root = div().id(id);
    if view.text().cursor() {
        root = root.cursor(CursorStyle::IBeam);
    }
    root.relative()
        .size_full()
        .overflow_hidden()
        .child(listeners.absolute().size_full())
        .on_action(cx.listener(|view, _: &selection::Copy, window, cx| {
            text::run(view, EditCommand::Copy, window, cx)
        }))
        .on_action(cx.listener(|view, _: &selection::SelectAll, window, cx| {
            text::run(view, EditCommand::SelectAll, window, cx)
        }))
        .on_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
            text::key_down(view, event, window, cx)
        }))
        .on_scroll(cx.listener(|view, event: &ScrollEvent, _, cx| {
            view.scroll().wheel(event, cx);
            cx.notify();
        }))
}

/// Places the document's boxes in the window for a scroll position.
#[derive(Clone, Copy)]
pub struct Placer {
    scale: f32,
    /// Device px.
    scrolled: Lu,
}

impl Placer {
    pub fn new(scroll: &PageScroll) -> Placer {
        let top = scroll.top() * f64::from(scroll.scale);
        Placer {
            scale: scroll.scale,
            scrolled: asrs_js::math::round(top) as Lu * 64,
        }
    }

    fn device(&self, v: Lu) -> f32 {
        snapped(v) as f32 / 64.0 / self.scale
    }

    /// Blink's pixel-snapped rect of a box.
    pub fn place(&self, [x, y, w, h]: LuRect) -> Div {
        let y = y - self.scrolled;
        let (left, top) = (self.device(x), self.device(y));
        div()
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(self.device(x + w) - left))
            .h(px(self.device(y + h) - top))
    }

    /// The part of a box's position that snapping dropped, in CSS px.
    pub fn remainder(&self, [x, y, ..]: LuRect) -> [f64; 2] {
        [x, y - self.scrolled].map(|v| css(v - snapped(v), self.scale))
    }

    /// The part of `text` in each box of a selection, in the colour selected text has.
    pub fn recoloured(&self, rects: &[LuRect], text: impl Fn() -> Vec<Div>) -> Vec<Div> {
        rects
            .iter()
            .map(|&rect| {
                let [x, y, ..] = rect;
                let left = px(-self.device(x));
                let top = px(-self.device(y - self.scrolled));
                let layer = div().absolute().left(left).top(top).children(text());
                self.place(rect).overflow_hidden().child(layer)
            })
            .collect()
    }

    /// The boxes a selection paints.
    pub fn selection(&self, rects: &[LuRect], color: Rgba) -> Vec<Div> {
        let boxes = rects.iter();
        boxes.map(|&rect| self.place(rect).bg(color)).collect()
    }

    /// A line's piece of text in its box.
    pub fn text(&self, text: &str, style: &TextStyle, color: u32, bounds: LuRect) -> Div {
        let [x, y, _, height] = bounds;
        let y = y - self.scrolled;
        let remainder = |v: Lu| snap_remainder(v as f32 / 64.0 / self.scale, self.scale);
        let mut style = *style;
        style.align = Align::Start;
        style.white_space = WhiteSpace::Pre;
        style.line_height = LineHeight::Px(css(height, self.scale) as f32);
        div()
            .absolute()
            .left(px(self.device(x)))
            .top(px(self.device(y)))
            .child(
                Text::new(text.to_owned(), style)
                    .color(color)
                    .subpixel(remainder(x), remainder(y))
                    .unrecorded(),
            )
    }

    /// A block's text, each node in the colour `color` gives it.
    pub fn block(&self, block: &TextBlock, color: impl Fn(usize) -> u32, cx: &mut App) -> Vec<Div> {
        let [x, y] = block.origin;
        let height = block.flow.box_height;
        let lines = block.flow.lines.iter();
        lines
            .flat_map(|line| line.pieces.iter().map(move |piece| (line, piece)))
            .map(|(line, piece)| {
                let bounds = [x + piece.x, y + line.box_top, piece.width, height];
                self.record(cx, &piece.text, bounds);
                self.text(&piece.text, &block.style, color(piece.item), bounds)
            })
            .collect()
    }

    /// A block's text in one colour, unrecorded.
    pub fn plain_block(&self, block: &TextBlock, color: u32) -> Vec<Div> {
        let [x, y] = block.origin;
        let height = block.flow.box_height;
        let lines = block.flow.lines.iter();
        lines
            .flat_map(|line| line.pieces.iter().map(move |piece| (line, piece)))
            .map(|(line, piece)| {
                let bounds = [x + piece.x, y + line.box_top, piece.width, height];
                self.text(&piece.text, &block.style, color, bounds)
            })
            .collect()
    }

    /// Whether any of `y..y + height` is in the window.
    pub fn shows(&self, y: Lu, height: Lu, window: &Window) -> bool {
        let viewport = f32::from(window.viewport_size().height) * self.scale * 64.0;
        let top = y - self.scrolled;
        top < viewport.ceil() as Lu && top + height > 0
    }

    /// The document's part that is in the window.
    pub fn visible(&self, window: &Window) -> (Lu, Lu) {
        let viewport = f32::from(window.viewport_size().height) * self.scale * 64.0;
        (self.scrolled, self.scrolled + viewport.ceil() as Lu)
    }

    /// Reports a piece of text's box to a parity capture, as the DOM dump has it.
    pub fn record(&self, cx: &mut App, text: &str, [x, y, w, h]: LuRect) {
        if !recording(cx) {
            return;
        }
        let rect = [x, y - self.scrolled, w, h].map(|v| css(v, self.scale));
        record_text(
            cx,
            TextRun {
                text: text.to_owned(),
                rects: vec![rect],
            },
        );
    }
}

/// Whether a parity capture is reading this frame.
pub fn recording(cx: &App) -> bool {
    cfg!(feature = "parity") && cx.has_global::<Recorder>()
}

/// The window's width in whole device px.
pub fn device_width(window: &Window) -> Lu {
    let width = f64::from(f32::from(window.viewport_size().width));
    asrs_js::math::round(width * f64::from(window.scale_factor())) as Lu * 64
}

/// `over` with an opacity on the opaque `under`.
pub fn mix(over: u32, under: u32, opacity: f64) -> u32 {
    [16, 8, 0].into_iter().fold(0, |out, shift| {
        let channel = |color: u32| f64::from(color >> shift & 0xff);
        let mixed = channel(over) * opacity + channel(under) * (1.0 - opacity);
        out | (mixed.round() as u32) << shift
    })
}
