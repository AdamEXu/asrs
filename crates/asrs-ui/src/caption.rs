//! The caption buttons Electron draws for `titleBarOverlay` on Windows
//! (`WinCaptionButtonContainer`, over the top right corner of the page), measured from Electron
//! 39 on Windows 11.

use asrs_canvas::Pixmap;
use gpui_kit::{
    AnyElement, App, Bounds, Entity, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Styled, Window, WindowControlArea,
    canvas, div, fill, point, px, rgba, size,
};

use crate::input::skia;

/// From the window's top right corner, inside the border DWM draws there.
const BORDER: f32 = 1.0;
const HEIGHT: f32 = 37.0;
const SYMBOL: f32 = 10.0;
/// The gap a button's background leaves on its left.
const SPACING: f32 = 1.0;

const CLOSE: u32 = 0xe81123;
const HOVERED_ALPHA: u32 = 0x1a;
const PRESSED_ALPHA: u32 = 0x33;
const CLOSE_PRESSED_ALPHA: u32 = 0x98;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Button {
    Minimize,
    Maximize,
    Close,
}

const BUTTONS: [(Button, f32); 3] = [
    (Button::Minimize, 45.0),
    (Button::Maximize, 46.0),
    (Button::Close, 46.0),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Normal,
    Hovered,
    Pressed,
}

#[derive(Default)]
struct Pointer {
    over: Option<usize>,
    pressed: Option<usize>,
    in_window: bool,
}

impl Pointer {
    fn state(&self, index: usize) -> State {
        match (self.over == Some(index), self.pressed) {
            (true, Some(pressed)) if pressed == index => State::Pressed,
            (true, None) => State::Hovered,
            _ => State::Normal,
        }
    }
}

/// `0xRRGGBBAA` of a button's background.
fn background(button: Button, state: State, dark: bool) -> Option<u32> {
    let feature = if dark { 0xffffff } else { 0x000000 };
    let (color, alpha) = match (state, button) {
        (State::Normal, _) => return None,
        (State::Hovered, Button::Close) => (CLOSE, 0xff),
        (State::Pressed, Button::Close) => (CLOSE, CLOSE_PRESSED_ALPHA),
        (State::Hovered, _) => (feature, HOVERED_ALPHA),
        (State::Pressed, _) => (feature, PRESSED_ALPHA),
    };
    Some((color << 8) | alpha)
}

/// `titleBarOverlay`'s `symbolColor` as `main.ts` sets it.
fn symbol_color(button: Button, state: State, dark: bool) -> u32 {
    if dark || (button == Button::Close && state != State::Normal) {
        0xffffff
    } else {
        0x000000
    }
}

/// `Win11IconPainter`'s icon in a square of `side` device px.
fn symbol(button: Button, maximized: bool, side: u32, scale: f32, color: u32) -> Option<Pixmap> {
    let stroke = scale.floor().max(1.0);
    let n = side as f32;
    let paint = skia::paint_attrs("stroke", (color << 8) | 0xff);
    let box_at = |x: f32, y: f32, side: f32| {
        let radius = (1.5 * scale).floor();
        let inset = stroke / 2.0;
        skia::rect(
            f64::from(x + inset),
            f64::from(y + inset),
            f64::from(side - stroke),
            f64::from(side - stroke),
            f64::from(radius),
            &format!("fill=\"none\" {paint} stroke-width=\"{stroke}\""),
        )
    };
    match (button, maximized) {
        (Button::Maximize, false) => skia::svg(side, side, 1.0, &box_at(0.0, 0.0, n)),
        (Button::Maximize, true) => {
            let gap = (2.0 * scale).floor();
            skia::svg(side, side, 1.0, &box_at(0.0, gap, n - gap))
        }
        (Button::Minimize, _) => {
            let top = (n / 2.0 - stroke / 2.0 + 0.5).floor();
            let line = format!(
                "<path d=\"M0 {top} H{n} V{} H0 Z\" {}/>",
                top + stroke,
                skia::paint_attrs("fill", (color << 8) | 0xff)
            );
            skia::svg(side, side, 1.0, &line)
        }
        (Button::Close, _) => {
            let mut pixmap = Pixmap::new(side, side)?;
            let reach = (stroke * std::f32::consts::FRAC_1_SQRT_2).floor() as i32;
            let [_, r, g, b] = color.to_be_bytes();
            let side = side as i32;
            let (pixels, _) = pixmap.data_mut().as_chunks_mut::<4>();
            for (i, pixel) in pixels.iter_mut().enumerate() {
                let (x, y) = (i as i32 % side, i as i32 / side);
                if (x - y).abs() <= reach || (x + y - (side - 1)).abs() <= reach {
                    *pixel = [r, g, b, 255];
                }
            }
            Some(pixmap)
        }
    }
}

/// The buttons' boxes in a window `width` wide.
fn boxes(width: Pixels) -> [Bounds<Pixels>; 3] {
    let total: f32 = BUTTONS.iter().map(|(_, w)| w).sum();
    let mut x = width - px(BORDER + total);
    BUTTONS.map(|(_, w)| {
        let bounds = Bounds::new(point(x, px(BORDER)), size(px(w), px(HEIGHT)));
        x += px(w);
        bounds
    })
}

fn over(boxes: &[Bounds<Pixels>; 3], position: Point<Pixels>) -> Option<usize> {
    boxes.iter().position(|b| b.contains(&position))
}

fn paint(
    index: usize,
    bounds: Bounds<Pixels>,
    state: State,
    dark: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let (button, _) = BUTTONS[index];
    let scale = window.scale_factor();
    if let Some(color) = background(button, state, dark) {
        let shown = Bounds::from_corners(
            bounds.origin + point(px(SPACING), px(0.0)),
            bounds.bottom_right(),
        );
        window.paint_quad(fill(shown, rgba(color)));
    }
    let device = |v: Pixels| (f32::from(v) * scale).round();
    let side = (SYMBOL * scale).round();
    // `gfx::Rect::ClampToCenteredSize`, in whole device px.
    let centred = |start: Pixels, end: Pixels| {
        let (start, end) = (device(start), device(end));
        px((start + ((end - start - side) / 2.0).floor()) / scale)
    };
    let origin = point(
        centred(bounds.left(), bounds.right()),
        centred(bounds.top(), bounds.bottom()),
    );
    let color = symbol_color(button, state, dark);
    let maximized = window.is_maximized();
    skia::paint(
        ("caption", button, maximized, color),
        Bounds::new(origin, size(px(side / scale), px(side / scale))),
        || symbol(button, maximized, side as u32, scale, color),
        window,
        cx,
    );
}

fn listen(pointer: Entity<Pointer>, boxes: [Bounds<Pixels>; 3], window: &mut Window) {
    let moved = pointer.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
        if !phase.capture() {
            return;
        }
        let over = over(&boxes, event.position);
        if moved.read(cx).over != over {
            moved.update(cx, |pointer, cx| {
                pointer.over = over;
                cx.notify();
            });
        }
        // A pressed button has the pointer, as under Chromium, and the page sees nothing of it.
        if moved.read(cx).pressed.is_some() {
            cx.stop_propagation();
        }
    });
    let pressed = pointer.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
        let over = over(&boxes, event.position);
        if phase.capture() && event.button == MouseButton::Left && over.is_some() {
            pressed.update(cx, |pointer, cx| {
                pointer.over = over;
                pointer.pressed = over;
                cx.notify();
            });
        }
    });
    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
        let left = event.button == MouseButton::Left;
        if phase.capture() && left && pointer.read(cx).pressed.is_some() {
            pointer.update(cx, |pointer, cx| {
                pointer.pressed = None;
                cx.notify();
            });
        }
    });
}

/// The buttons, for a window's root element. GPUI takes the clicks from their control areas.
pub fn caption_buttons(dark: bool, window: &mut Window, cx: &mut App) -> AnyElement {
    let pointer = window.use_keyed_state("caption-buttons", cx, |_, _| Pointer::default());
    let hovered = window.is_window_hovered();
    pointer.update(cx, |pointer, _| {
        if pointer.in_window && !hovered {
            pointer.over = None;
        }
        pointer.in_window = hovered;
    });
    let states: [State; 3] = std::array::from_fn(|i| pointer.read(cx).state(i));
    let boxes = boxes(window.viewport_size().width);
    let areas = [
        WindowControlArea::Min,
        WindowControlArea::Max,
        WindowControlArea::Close,
    ];
    let painted = canvas(
        |_, _, _| (),
        move |_, (), window, cx| {
            listen(pointer, boxes, window);
            for (index, state) in states.into_iter().enumerate() {
                paint(index, boxes[index], state, dark, window, cx);
            }
        },
    );
    div()
        .absolute()
        .size_full()
        .children(boxes.into_iter().zip(areas).map(|(bounds, area)| {
            div()
                .absolute()
                .left(bounds.left())
                .top(bounds.top())
                .w(bounds.size.width)
                .h(bounds.size.height)
                .occlude()
                .window_control_area(area)
        }))
        .child(painted.absolute().size_full())
        .into_any_element()
}
