#![allow(clippy::disallowed_methods)]

//! Replays `scroll/*`: what trusted keys, wheel events and scrollbar presses do to a scroller in
//! Blink, on a box with the hub's scrollbars, in a headless window. Every offset, every recorded
//! strip of pixels and the fade times must equal the recording.

use std::sync::Arc;
use std::time::Duration;

use asrs_model::scroll_sensor::{Wheel, WheelScroll, assign_scroll_offset};
use asrs_parity::json::{self, Json};
use asrs_parity::vectors;
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::gpui::{
    Context, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, PlatformInput, Render, ScrollDelta, ScrollEvent, Styled, Window,
    WindowHandle, div, point, px, rgb, size,
};
use asrs_ui::scroll::{self, Extent, Request, Scrollbars, Style};

const FRAME: Duration = Duration::from_millis(10);
/// How long the recorder waits for offsets to stay the same.
const SETTLE: u32 = 27;
/// A held press goes on by a frame's worth more or less.
const HELD_TOLERANCE: f64 = 46.0;
const LEVELS: i32 = 12;

struct Scroller {
    rect: [f64; 4],
    content: [f64; 2],
    background: Option<u32>,
    offset: [f64; 2],
    wheel: WheelScroll,
    bars: Scrollbars,
    pressed: bool,
    now: f64,
    scale: f64,
}

impl Scroller {
    fn client(&self, style: Style) -> [f64; 2] {
        let size = [self.rect[2], self.rect[3]];
        let taken = style.reserved();
        let mut client = size;
        for _ in 0..2 {
            client = [0, 1].map(|a| {
                let bar = self.content[1 - a] > client[1 - a];
                size[a] - if bar { taken } else { 0.0 }
            });
        }
        client
    }

    fn max(&self, style: Style) -> [f64; 2] {
        let client = self.client(style);
        [0, 1].map(|a| (self.content[a] - client[a]).max(0.0))
    }

    fn top(&self, style: Style) -> [f64; 2] {
        let max = self.max(style);
        [0, 1].map(|a| assign_scroll_offset(self.offset[a], max[a], self.scale))
    }

    fn scroll(&mut self, wheel: Wheel, client: [f64; 2], style: Style) {
        let max = self.max(style);
        if let Some(to) = self.wheel.wheel(wheel, self.now, self.offset, max, client) {
            self.offset = [0, 1].map(|a| to[a].clamp(0.0, max[a]));
        }
    }

    fn smooth(&mut self, by: [f64; 2], style: Style) {
        let wheel = Wheel {
            delta: by.map(|v| -v),
            precise: false,
        };
        self.scroll(wheel, [f64::INFINITY; 2], style);
    }
}

impl Render for Scroller {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        if let Some(to) = self.wheel.animate(self.now) {
            self.offset = to;
        }
        self.scale = f64::from(window.scale_factor());
        let style = scroll::style(cx);
        let extent = Extent {
            content: self.content,
            offset: self.top(style),
            axes: [true; 2],
            dark: self.background.is_some_and(|c| c < 0x808080),
        };
        let view = cx.entity().downgrade();
        let bars = scroll::scrollbars(&self.bars, extent, move |request, _, cx| {
            let _ = view.update(cx, |view, cx| {
                let style = scroll::style(cx);
                match request {
                    Request::Press => view.pressed = true,
                    Request::To(to) => {
                        view.wheel.stop();
                        view.offset = to;
                    }
                    Request::By(by) => view.smooth(by, style),
                }
                cx.notify();
            });
        });
        let [x, y, w, h] = self.rect.map(|v| px(v as f32));
        let background = rgb(self.background.unwrap_or(0xffffff));
        div()
            .size_full()
            .bg(rgb(0xffffff))
            .capture_any_mouse_down(cx.listener(|view, _, _, _| view.pressed = false))
            .child(
                div()
                    .id("scroller")
                    .absolute()
                    .left(x)
                    .top(y)
                    .w(w)
                    .h(h)
                    .bg(background)
                    .capture_any_mouse_down(cx.listener(|view, _, _, _| view.pressed = true))
                    .on_scroll(cx.listener(|view, event: &ScrollEvent, _, cx| {
                        let ScrollDelta::Pixels(delta) = event.delta else {
                            panic!("wheel events are precise");
                        };
                        let wheel = Wheel {
                            delta: [delta.x, delta.y].map(|v| f64::from(f32::from(v))),
                            precise: true,
                        };
                        let style = scroll::style(cx);
                        let client = view.client(style);
                        view.scroll(wheel, client, style);
                        cx.notify();
                    }))
                    .child(bars),
            )
    }
}

fn items(value: &Json) -> &[Json] {
    value.as_array().expect("an array")
}

fn number(value: &Json) -> f64 {
    value.as_f64().expect("a number")
}

fn text(value: &Json) -> String {
    value.as_str().expect("a string").to_string_lossy()
}

/// The number after `name` in inline styles.
fn css(markup: &str, name: &str) -> f64 {
    let at = markup
        .find(name)
        .unwrap_or_else(|| panic!("{name} in {markup}"))
        + name.len();
    let rest = &markup[at..];
    let end = rest.find("px").expect("px");
    rest[..end].parse().expect("a length")
}

struct Page<'a> {
    headless: &'a mut HeadlessCapture,
    window: WindowHandle<Scroller>,
    button: Option<MouseButton>,
}

impl Page<'_> {
    fn open<'a>(headless: &'a mut HeadlessCapture, markup: &str, scale: f32) -> Page<'a> {
        let (outer, inner) = markup.split_once("'>").expect("a child");
        let background = outer.split("background:").nth(1).expect("a background");
        let background = match background.trim_start_matches('#') {
            "fff" => Some(0xffffff),
            "222" => Some(0x222222),
            "transparent" => None,
            other => panic!("background {other}"),
        };
        let rect = ["left:", "top:", "width:", "height:"].map(|name| css(outer, name));
        let content = ["width:", "height:"].map(|name| css(inner, name));
        let window = headless
            .open_window(size(px(500.0), px(500.0)), scale, |_, cx| {
                use asrs_ui::gpui::AppContext;
                cx.new(|_| Scroller {
                    rect,
                    content: [content[0], content[1]],
                    background,
                    offset: [0.0; 2],
                    wheel: WheelScroll::default(),
                    bars: Scrollbars::default(),
                    pressed: false,
                    now: 0.0,
                    scale: f64::from(scale),
                })
            })
            .expect("window");
        let mut page = Page {
            headless,
            window,
            button: None,
        };
        page.frames(2);
        page
    }

    fn frames(&mut self, count: u32) {
        for _ in 0..count {
            let cx = self.headless.context();
            cx.advance_clock(FRAME);
            self.window
                .update(cx, |view, window, cx| {
                    view.now += FRAME.as_secs_f64() * 1000.0;
                    cx.notify();
                    window.simulate_next_frame(cx);
                })
                .expect("window");
            cx.run_until_parked();
        }
    }

    fn settle(&mut self) {
        let mut last = self.update(|view, style| view.top(style));
        let mut same = 0;
        while same < SETTLE {
            self.frames(1);
            let top = self.update(|view, style| view.top(style));
            same = if top == last { same + 1 } else { 0 };
            last = top;
        }
    }

    fn input(&mut self, input: PlatformInput) {
        let cx = self.headless.context();
        cx.update_window(self.window.into(), |_, window, cx| {
            window.dispatch_event(input, cx);
        })
        .expect("window");
        cx.run_until_parked();
    }

    fn mouse(&mut self, kind: &str, x: f64, y: f64) {
        let position = point(px(x as f32), px(y as f32));
        self.input(PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: self.button,
            ..Default::default()
        }));
        let button = MouseButton::Left;
        if kind == "press" || kind == "click" {
            self.button = Some(button);
            self.input(PlatformInput::MouseDown(MouseDownEvent {
                button,
                position,
                click_count: 1,
                ..Default::default()
            }));
        }
        if kind == "release" || kind == "click" {
            self.button = None;
            self.input(PlatformInput::MouseUp(MouseUpEvent {
                button,
                position,
                click_count: 1,
                ..Default::default()
            }));
        }
    }

    fn update<R>(&mut self, f: impl FnOnce(&mut Scroller, Style) -> R) -> R {
        let cx = self.headless.context();
        self.window
            .update(cx, |view, _, cx| {
                let style = scroll::style(cx);
                cx.notify();
                f(view, style)
            })
            .expect("window")
    }

    fn key(&mut self, name: &str, modifiers: &[String]) {
        let mut keystroke = asrs_ui::gpui::Keystroke {
            key: match name.strip_prefix("Arrow") {
                Some(arrow) => arrow.to_lowercase(),
                None => name.to_lowercase(),
            },
            ..Default::default()
        };
        for modifier in modifiers {
            match modifier.as_str() {
                "alt" => keystroke.modifiers.alt = true,
                "ctrl" => keystroke.modifiers.control = true,
                "meta" => keystroke.modifiers.platform = true,
                "shift" => keystroke.modifiers.shift = true,
                other => panic!("modifier {other}"),
            }
        }
        self.update(|view, style| {
            if let Some(step) = scroll::step(&keystroke).filter(|_| view.pressed) {
                let by = step.delta(view.client(style), view.max(style), view.scale);
                view.smooth(by, style);
            }
        });
    }

    /// `a.scrollTop = 5;a.scrollLeft = 50`.
    fn script(&mut self, script: &str) {
        for part in script.split(';') {
            let (name, value) = part.split_once('=').expect("an assignment");
            self.update(|view, style| {
                let (max, client) = (view.max(style), view.client(style));
                let value = match value {
                    "(a.scrollHeight-a.clientHeight)/2" => {
                        (view.content[1].max(client[1]) - client[1]) / 2.0
                    }
                    number => number.parse().expect("a number"),
                };
                let axis = usize::from(name == "a.scrollTop");
                view.wheel.stop();
                view.offset[axis] = assign_scroll_offset(value, max[axis], view.scale);
            });
        }
    }

    /// Runs an op without its settling; whether it has a record.
    fn op(&mut self, op: &Json) {
        let parts = items(op);
        let at = |i: usize| number(&parts[i]);
        match text(&parts[0]).as_str() {
            "key" => {
                let modifiers = parts
                    .get(2)
                    .map_or(Vec::new(), |m| items(m).iter().map(text).collect());
                self.key(&text(&parts[1]), &modifiers);
            }
            "wheel" => {
                let position = point(px(at(1) as f32), px(at(2) as f32));
                let delta = point(px(-at(3) as f32), px(-at(4) as f32));
                self.input(PlatformInput::MouseMove(MouseMoveEvent {
                    position,
                    ..Default::default()
                }));
                self.input(PlatformInput::Scroll(ScrollEvent {
                    position,
                    delta: ScrollDelta::Pixels(delta),
                    ..Default::default()
                }));
            }
            kind @ ("move" | "press" | "release" | "click") => self.mouse(kind, at(1), at(2)),
            "eval" => self.script(&text(&parts[1])),
            "wait" => self.frames((at(1) / 10.0) as u32),
            other => panic!("op {other}"),
        }
        self.frames(1);
    }

    fn strip(&mut self, [x, y, w, h]: [f64; 4], scale: u32) -> Vec<Vec<[u8; 3]>> {
        let shot = self.headless.capture(self.window.into()).expect("capture");
        let device = |v: f64| v as u32 * scale;
        (device(y)..device(y + h))
            .map(|row| {
                (device(x)..device(x + w))
                    .map(|column| {
                        let [r, g, b, _] = shot.pixel(column, row);
                        [r, g, b]
                    })
                    .collect()
            })
            .collect()
    }
}

/// The rows a record's runs spell out.
fn expand(rows: &Json) -> Vec<Vec<[u8; 3]>> {
    let mut out = Vec::new();
    for row in items(rows) {
        let [count, runs] = items(row) else {
            panic!("a row is a count and its runs");
        };
        let mut line = Vec::new();
        for run in text(runs).split(' ') {
            let (n, color) = run.split_once(':').expect("a run");
            let channel = |i: usize| u8::from_str_radix(&color[i..i + 2], 16).expect("hex");
            let color = [channel(0), channel(2), channel(4)];
            line.extend(std::iter::repeat_n(color, n.parse().expect("a length")));
        }
        out.extend(std::iter::repeat_n(line, number(count) as usize));
    }
    out
}

/// The first pixel that differs by more than antialiasing does between rasterizers.
fn differs(want: &[Vec<[u8; 3]>], got: &[Vec<[u8; 3]>]) -> Option<String> {
    assert_eq!(want.len(), got.len(), "rows");
    for (y, (want, got)) in want.iter().zip(got).enumerate() {
        assert_eq!(want.len(), got.len(), "columns");
        for (x, (want, got)) in want.iter().zip(got).enumerate() {
            let off = (0..3).map(|c| (i32::from(want[c]) - i32::from(got[c])).abs());
            if off.max() > Some(LEVELS) {
                return Some(format!(
                    "({x}, {y}): {want:02x?} recorded, {got:02x?} drawn"
                ));
            }
        }
    }
    None
}

fn replay(headless: &mut HeadlessCapture, id: &str) -> [usize; 3] {
    let spec = vectors::workspace_root().join(format!("parity/specs/{id}.entry.json"));
    let spec = json::parse(&std::fs::read_to_string(spec).expect("the spec")).expect("JSON");
    let fixture = spec.get("fixture").expect("a fixture");
    let jump = matches!(fixture.get("jump"), Some(Json::Bool(true)));
    let list = |key: &str| items(fixture.get(key).expect("a list")).to_vec();
    let host = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let records = vectors::load(&format!("{id}/{host}")).expect("vectors");
    let mut records = records.iter();
    let mut counts = [0; 3];
    let styles: Vec<String> = if cfg!(target_os = "macos") {
        list("styles").iter().map(text).collect()
    } else {
        vec!["fluent".to_owned()]
    };
    for style in &styles {
        let kind = match style.as_str() {
            "overlay" => Style::Overlay,
            "classic" => Style::Classic,
            _ => Style::Fluent,
        };
        headless
            .context()
            .update(|cx| scroll::set_style(kind, jump, cx));
        for scale in list("scales").iter().map(number) {
            for step in list("steps") {
                let name = format!(
                    "{id} {style} {scale}x {}",
                    text(step.get("name").expect("name"))
                );
                let mut page = Page::open(
                    headless,
                    &text(step.get("markup").expect("markup")),
                    scale as f32,
                );
                let boxes = records.next().expect("a record");
                let want = items(&items(boxes.field("boxes").expect("boxes"))[1]);
                let client = page.update(|view, style| view.client(style));
                let want = [number(&want[0]), number(&want[1])];
                assert_eq!(client, want, "{name}: the client box");
                let mut held = false;
                for op in items(step.get("ops").expect("ops")) {
                    let parts = items(op);
                    let kind = text(&parts[0]);
                    let at = format!("{name} {}", op.to_compact());
                    if kind == "pixels" {
                        page.frames(1);
                        let record = records.next().expect("a record");
                        let rect: Vec<f64> = items(&parts[2]).iter().map(number).collect();
                        let rect = [rect[0], rect[1], rect[2], rect[3]];
                        let got = page.strip(rect, scale as u32);
                        let want = expand(record.field("rows").expect("rows"));
                        if let Some(at_pixel) = differs(&want, &got) {
                            panic!("{at}: {at_pixel}");
                        }
                        counts[1] += 1;
                        continue;
                    }
                    if kind == "fade" {
                        let record = records.next().expect("a record");
                        let rect: Vec<f64> = items(&parts[1]).iter().map(number).collect();
                        let rect = [rect[0], rect[1], rect[2], 1.0];
                        let first = page.strip(rect, 1);
                        let (mut begins, mut ends) = (None, None);
                        for frame in 1..150 {
                            page.frames(1);
                            let strip = page.strip(rect, 1);
                            let now = f64::from(frame) * 10.0;
                            if begins.is_none() && strip != first {
                                begins = Some(now);
                            }
                            if strip[0].iter().all(|c| *c == [255; 3]) {
                                ends.get_or_insert(now);
                            }
                        }
                        let want = items(record.field("fade").expect("fade"));
                        for (got, want) in [begins, ends].into_iter().zip(want) {
                            let (got, want) = (got.expect("it fades"), number(want));
                            assert!(
                                (got - want).abs() <= 100.0,
                                "{at}: {got} ms, recorded {want}"
                            );
                        }
                        counts[2] += 1;
                        continue;
                    }
                    page.op(op);
                    let quick = matches!(parts.last(), Some(Json::String(s)) if s.to_string_lossy() == "quick");
                    if quick || kind == "wait" {
                        continue;
                    }
                    page.settle();
                    let record = records.next().expect("a record");
                    assert_eq!(record.field("op").expect("op"), op, "{at}");
                    let want = &items(record.field("scroll").expect("scroll"))[1];
                    let want = [number(&items(want)[0]), number(&items(want)[1])];
                    let got = page.update(|view, style| view.top(style));
                    held = match kind.as_str() {
                        "press" => true,
                        "release" => held,
                        _ => false,
                    };
                    let tolerance = if held { HELD_TOLERANCE } else { 1.0 };
                    for axis in 0..2 {
                        let off = (got[axis] - want[axis]).abs();
                        assert!(off <= tolerance, "{at}: {got:?}, recorded {want:?}");
                    }
                    if held {
                        page.update(|view, _| view.offset = want);
                    }
                    counts[0] += 1;
                }
                let cx = page.headless.context();
                let window = page.window;
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .expect("window");
                cx.run_until_parked();
            }
        }
    }
    assert!(records.next().is_none(), "{id}: records left over");
    counts
}

fn main() {
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        println!("scroll vectors: skipped, no headless renderer on this platform");
        return;
    }
    let mut headless = HeadlessCapture::new(Arc::new(())).expect("headless");
    let mac = cfg!(target_os = "macos");
    for id in ["scroll/keys", "scroll/bars", "scroll/jump", "scroll/fade"] {
        if !mac && matches!(id, "scroll/jump" | "scroll/fade") {
            continue;
        }
        let [offsets, strips, fades] = replay(&mut headless, id);
        assert!(offsets + fades > 0, "{id}: nothing compared");
        println!("{id}: {offsets} offsets, {strips} strips, {fades} fades");
    }
    println!("scroll_vectors: ok");
}
