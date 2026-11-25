#![allow(clippy::disallowed_methods)]

//! The scrolling keys and the scrollbars of the real hub window, headless. How far a key goes is
//! `scroll/keys`' for a scroller of the view's height, and what shows where it goes is read off
//! the boxes of the rendered text.

#[allow(dead_code)]
#[path = "support/hub_page.rs"]
mod hub_page;

use std::sync::Arc;

use asrs_app::views::console_layout::ConsoleLayout;
use asrs_js::ops;
use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_model::tab_type::TabType;
use asrs_parity::json::Json;
use asrs_parity::vectors;
use asrs_tabs::console::controller::ConsoleRenderer;
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::gpui::{
    KeyDownEvent, KeyUpEvent, Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PlatformInput, point, px,
};
use asrs_ui::parity::Recorder;
use asrs_ui::scroll::{Style, set_style};
use hub_page::Page;

/// The host whose recording stands for this one: Blink's offsets are the same on both of a system's
/// architectures.
const HOST: &str = if cfg!(target_os = "macos") {
    "macos-aarch64"
} else {
    "linux-x86_64"
};
const SETTLE_FRAMES: usize = 40;
const ROWS: usize = 400;
/// The `scrollTop` the recorded keys scroll from.
const RECORDED_FROM: f64 = 700.0;

fn items(value: Option<&Json>) -> &[Json] {
    value.and_then(Json::as_array).expect("an array")
}

/// How far `key` goes in a scroller `height` px high.
fn recorded(key: &str, height: f64, scale: f32) -> f64 {
    let records = vectors::load(&format!("scroll/keys/{HOST}")).expect("vectors");
    let step = format!("page{height}");
    let text = |value: &Json| value.as_str().map(|s| s.to_string_lossy());
    let found = records.iter().find(|record| {
        record.string("step").as_deref() == Some(&step)
            && record.field("scale").and_then(Json::as_f64) == Some(f64::from(scale))
            && (record.field("op"))
                .is_some_and(|op| text(&items(Some(op))[1]).as_deref() == Some(key))
    });
    let found = found.unwrap_or_else(|| panic!("no {key} in a scroller {height} px high"));
    let top = items(Some(&items(found.field("scroll"))[1]))[1].as_f64();
    top.expect("a number") - RECORDED_FROM
}

impl Page<'_> {
    fn log(&mut self) {
        self.model(|model| {
            let mut log = model.page().shared_log().borrow_mut();
            for i in 0..ROWS {
                let time = i as f64 / 10.0;
                log.put_number(&JsString::from("/Value"), time, i as f64)
                    .unwrap();
                let line = JsString::from(format!("line {i}").as_str());
                log.put_string(&JsString::from("/Messages"), time, line)
                    .unwrap();
            }
            for i in 0..80 {
                let key = JsString::from(format!("/Field{i:02}").as_str());
                log.put_number(&key, 0.0, 0.0).unwrap();
            }
            drop(log);
            model
                .with_sidebar(|sidebar, env| sidebar.refresh(env, false))
                .expect("refresh");
        });
    }

    /// Whether a text field has the focus.
    fn typing(&mut self) -> bool {
        self.headless
            .context()
            .update_window(self.window.into(), |_, window, _| {
                asrs_ui::input::focused(window)
            })
            .expect("window")
    }

    fn press(&mut self, key: &'static str) {
        let keystroke = Keystroke::parse(key).expect("a keystroke");
        self.dispatch(PlatformInput::KeyDown(KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        }));
        self.dispatch(PlatformInput::KeyUp(KeyUpEvent { keystroke }));
        self.frames(SETTLE_FRAMES);
    }

    fn move_to(&mut self, [x, y]: [f64; 2], pressed_button: Option<MouseButton>) {
        self.dispatch(PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(x as f32), px(y as f32)),
            pressed_button,
            ..MouseMoveEvent::default()
        }));
    }

    fn down(&mut self, [x, y]: [f64; 2]) {
        self.dispatch(PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: point(px(x as f32), px(y as f32)),
            click_count: 1,
            ..MouseDownEvent::default()
        }));
    }

    fn up(&mut self, [x, y]: [f64; 2]) {
        self.dispatch(PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: point(px(x as f32), px(y as f32)),
            click_count: 1,
            ..MouseUpEvent::default()
        }));
    }

    fn click(&mut self, at: [f64; 2]) {
        self.move_to(at, None);
        self.frames(2);
        self.down(at);
        self.up(at);
        self.frames(2);
    }

    fn record(&mut self) -> Recorder {
        self.headless
            .context()
            .update(|cx| cx.set_global(Recorder::default()));
        let window = self.window.into();
        self.headless.capture(window).expect("render");
        self.headless
            .context()
            .update(|cx| std::mem::take(cx.global_mut::<Recorder>()))
    }

    /// The top of the box of every text.
    fn tops(&mut self) -> Vec<(String, f64)> {
        let texts = self.record().texts;
        let runs = texts.into_iter().filter(|run| !run.rects.is_empty());
        runs.map(|run| (run.text, run.rects[0][1])).collect()
    }

    fn rect(&mut self, selector: &str) -> [f64; 4] {
        let boxes = self.record().boxes;
        let found = boxes.iter().find(|(id, _)| id.as_ref() == selector);
        found.unwrap_or_else(|| panic!("no {selector}")).1
    }
}

/// How far the texts that `inside` picks went up between two captures.
fn moved(before: &[(String, f64)], after: &[(String, f64)], inside: &str) -> f64 {
    let in_both = before.iter().filter(|(text, _)| text.starts_with(inside));
    let mut by = in_both.filter_map(|(text, top)| {
        let after = after.iter().find(|(other, _)| other == text)?;
        Some(top - after.1)
    });
    by.next()
        .unwrap_or_else(|| panic!("no {inside} text in both of {before:?}"))
}

/// The keys in a view whose texts start with `inside`, after a click at `at`.
fn keys(page: &mut Page<'_>, name: &str, inside: &str, at: [f64; 2], height: f64, scale: f32) {
    let untouched = page.tops();
    page.press("down");
    assert_eq!(
        moved(&untouched, &page.tops(), inside),
        0.0,
        "{name}: nothing was pressed"
    );
    page.click(at);
    page.press("home");
    let start = page.tops();
    let mut before = start.clone();
    for (key, recording) in [
        ("down", "ArrowDown"),
        ("pagedown", "PageDown"),
        ("pageup", "PageUp"),
    ] {
        page.press(key);
        let after = page.tops();
        let want = recorded(recording, height, scale);
        let got = moved(&before, &after, inside);
        assert!(
            (got - want).abs() < 0.01,
            "{name} {scale}x {key}: {got}, recorded {want}"
        );
        before = after;
    }
    page.press("home");
    assert_eq!(moved(&start, &page.tops(), inside), 0.0, "{name}: home");
    let [x, y, w, h] = page.rect("div.side-bar-handle");
    page.click([x + w / 2.0, y + h / 2.0]);
    page.press("down");
    assert_eq!(
        moved(&start, &page.tops(), inside),
        0.0,
        "{name}: pressed elsewhere"
    );
}

fn open(headless: &mut HeadlessCapture, scale: f32) -> Page<'_> {
    let mut page = Page::open(headless, scale);
    page.log();
    page.frames(4);
    page
}

fn table(headless: &mut HeadlessCapture, scale: f32) {
    let mut page = open(headless, scale);
    let fields = JsValue::Array(vec![JsValue::string("/Value")]);
    page.open_tab(TabType::Table, &fields);
    page.frames(4);
    let [x, y, w, h] = page.rect("div.renderer-content");
    keys(&mut page, "table", "+", [x + w / 2.0, y + 100.0], h, scale);
    page.close();
}

fn console(headless: &mut HeadlessCapture, scale: f32) {
    let mut page = open(headless, scale);
    page.open_tab(TabType::Console, &JsValue::string("/Messages"));
    page.frames(4);
    let [x, y, w, h] = page.rect(".console-table-container");
    keys(
        &mut page,
        "console",
        "line ",
        [x + w / 2.0, y + 100.0],
        h,
        scale,
    );
    page.close();
}

fn side_bar(headless: &mut HeadlessCapture, scale: f32) {
    let mut page = open(headless, scale);
    let [x, y, w, h] = page.rect("div.side-bar");
    keys(
        &mut page,
        "side bar",
        "Field",
        [x + w - 20.0, y + 200.0],
        h,
        scale,
    );
    page.close();
}

fn source_list(headless: &mut HeadlessCapture, scale: f32) {
    let mut page = open(headless, scale);
    let sources: Vec<JsValue> = (0..40)
        .map(|i| {
            ops::object([
                ("type", JsValue::string("stepped")),
                ("logKey", JsValue::string(&format!("/Field{i:02}"))),
                ("logType", JsValue::string("Number")),
                ("visible", JsValue::Bool(true)),
                (
                    "options",
                    ops::object([
                        ("color", JsValue::string("#2b66a2")),
                        ("size", JsValue::string("normal")),
                    ]),
                ),
            ])
        })
        .collect();
    let conversion = || ops::object([("autoTarget", JsValue::Null), ("preset", JsValue::Null)]);
    let state = ops::object([
        ("leftSources", JsValue::Array(sources)),
        ("rightSources", JsValue::Array(Vec::new())),
        ("discreteSources", JsValue::Array(Vec::new())),
        ("leftLockedRange", JsValue::Null),
        ("rightLockedRange", JsValue::Null),
        ("leftUnitConversion", conversion()),
        ("rightUnitConversion", conversion()),
        ("leftFilter", JsValue::Number(0.0)),
        ("rightFilter", JsValue::Number(0.0)),
    ]);
    page.open_tab(TabType::LineGraph, &state);
    page.frames(4);
    let [x, y, _, h] = page.rect("div.controls-content");
    keys(
        &mut page,
        "source list",
        "/Field",
        [x + 60.0, y + 80.0],
        h - 30.0,
        scale,
    );
    page.close();
}

/// The table's overlay scrollbar shows after a scroll, scrolls by a page, and takes its presses
/// from the rows under it.
fn scrollbar(headless: &mut HeadlessCapture, scale: f32) {
    let mut page = open(headless, scale);
    let fields = JsValue::Array(vec![JsValue::string("/Value")]);
    page.open_tab(TabType::Table, &fields);
    page.frames(4);
    let [x, y, w, h] = page.rect("div.renderer-content");
    let at = [x + w - 6.0, y + h - 60.0];
    page.click([x + 200.0, y + 100.0]);
    let selected = page.selection(|s| s.get_selected_time());
    assert!(selected.is_some(), "the row was clicked");
    page.press("down");
    let before = page.tops();
    page.move_to(at, None);
    page.frames(20);
    page.down(at);
    page.up(at);
    page.frames(SETTLE_FRAMES);
    let want = recorded("PageDown", h, scale);
    let got = moved(&before, &page.tops(), "+");
    assert!(
        (got - want).abs() < 0.01,
        "scrollbar: {got}, recorded {want}"
    );
    assert_eq!(
        page.selection(|s| s.get_selected_time()),
        selected,
        "the press went to a row"
    );
    page.close();
}

/// Keys go to the scroller that holds what Tab focused, not to the one clicked last.
fn focus(headless: &mut HeadlessCapture, scale: f32) {
    let mut page = open(headless, scale);
    let fields = JsValue::Array(vec![JsValue::string("/Value")]);
    page.open_tab(TabType::Table, &fields);
    page.frames(4);
    let [x, y, w, _] = page.rect("div.side-bar");
    page.click([x + w - 20.0, y + 200.0]);
    for _ in 0..40 {
        page.press("tab");
        let before = page.tops();
        if page.typing() {
            continue;
        }
        page.press("down");
        let after = page.tops();
        if moved(&before, &after, "+") == 0.0 {
            continue;
        }
        assert_eq!(moved(&before, &after, "Field"), 0.0, "the side bar");
        page.close();
        return;
    }
    panic!("no focused element scrolls the table");
}

/// Classic scrollbars take their width off what is laid out next to them.
fn classic(headless: &mut HeadlessCapture, scale: f32) {
    let width = |headless: &mut HeadlessCapture, style: Style| {
        headless.context().update(|cx| set_style(style, false, cx));
        let mut page = open(headless, scale);
        page.open_tab(TabType::Console, &JsValue::string("/Messages"));
        page.frames(4);
        let w = page.model(|model| {
            let console = model
                .tabs()
                .tabs()
                .find_map(|tab| tab.renderer().downcast_ref::<ConsoleRenderer>())
                .expect("a console");
            let layout = console.geometry_ref::<ConsoleLayout>();
            layout.expect("a layout").client()[0]
        });
        page.close();
        w
    };
    let overlay = width(headless, Style::Overlay);
    let next_to = width(headless, Style::Classic);
    assert_eq!(overlay - next_to, 15.0, "the console's table");
    headless
        .context()
        .update(|cx| set_style(Style::Overlay, false, cx));
}

fn main() {
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        println!("hub scroll: skipped, no headless renderer on this platform");
        return;
    }
    let mut headless = HeadlessCapture::new(Arc::new(())).expect("headless app");
    for scale in [1.0, 2.0] {
        table(&mut headless, scale);
        console(&mut headless, scale);
        side_bar(&mut headless, scale);
        source_list(&mut headless, scale);
        scrollbar(&mut headless, scale);
        classic(&mut headless, scale);
        focus(&mut headless, scale);
    }
    println!("hub_scroll: ok");
}
