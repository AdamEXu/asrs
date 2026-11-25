#![allow(clippy::disallowed_methods)]

//! The Console tab in the real hub window, headless: a first frame that does not unlock, locked
//! autoscroll, the wheel unlocking it, the hover a console leaves when its tab is hidden, and a
//! field dragged in.

#[path = "support/hub_page.rs"]
mod hub_page;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use asrs_app::views::console_layout::{ConsoleLayout, LINE_HEIGHT};
use asrs_js::ops;
use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_model::messages::MainToHub;
use asrs_model::selection::SelectionMode;
use asrs_model::tab_type::TabType;
use asrs_tabs::console::controller::{ConsoleGeometry, ConsoleRenderer};
use asrs_tabs::console::view::ConsoleRow;
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::gpui::{
    ClipboardItem, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PlatformInput, ScrollDelta, point, px,
};
use hub_page::Page;

const LINES: usize = 200;
const KEY: &str = "/Messages";

impl Page<'_> {
    fn lines(&mut self, range: std::ops::Range<usize>) {
        self.model(|model| {
            let mut log = model.page().shared_log().borrow_mut();
            for i in range {
                let text = JsString::from(format!("line {i}").as_str());
                log.put_string(&JsString::from(KEY), i as f64 / 10.0, text)
                    .unwrap();
            }
        });
    }

    fn open_console(&mut self) {
        self.open_tab(TabType::Console, &JsValue::string(KEY));
    }

    fn layout<R>(&mut self, f: impl FnOnce(&mut ConsoleLayout, &[ConsoleRow]) -> R) -> R {
        self.model(|model| {
            let renderer = model
                .tabs()
                .tabs()
                .find_map(|tab| tab.renderer().downcast_ref::<ConsoleRenderer>())
                .expect("a console");
            let layout = renderer.geometry_ref::<ConsoleLayout>();
            f(
                &mut layout.expect("a layout").clone(),
                &renderer.view().rows,
            )
        })
    }

    fn scroll_top(&mut self) -> f64 {
        self.layout(|layout, _| ConsoleLayout::scroll_top(layout))
    }

    fn max_scroll_top(&mut self) -> f64 {
        self.layout(|layout, rows| {
            let end = layout.scroll_height(rows);
            end - layout.container_client_height()
        })
    }

    fn connect(&mut self, lines: usize) {
        self.selection(|selection| {
            selection.set_live_connected(Rc::new(move || lines as f64 / 10.0))
        });
        assert_eq!(self.mode(), SelectionMode::Locked);
    }
}

fn opening_stays_locked(headless: &mut HeadlessCapture, scale: f32, from: Option<TabType>) {
    let mut page = Page::open(headless, scale);
    page.lines(0..LINES);
    page.connect(LINES);
    if let Some(kind) = from {
        page.main(MainToHub::NewTab, JsValue::Number(kind.number()));
        page.frames(3);
    }
    page.open_console();
    let console = page.model(|model| model.tabs().selected_index().expect("the console"));
    for frame in 0..6 {
        page.frames(1);
        assert_eq!(
            page.mode(),
            SelectionMode::Locked,
            "{scale}x: frame {frame} of a console opened from {from:?}"
        );
        let end = page.max_scroll_top();
        assert!(end > 2000.0, "{scale}x: {end}");
        assert_eq!(page.scroll_top(), end, "{scale}x: frame {frame}");
    }
    let end = page.max_scroll_top();

    page.select_documentation();
    page.frames(3);
    page.lines(LINES..LINES + 20);
    page.select(console);
    page.frames(6);
    assert_eq!(page.mode(), SelectionMode::Locked, "{scale}x: shown again");
    let grown = page.max_scroll_top();
    assert_eq!(grown, end + 20.0 * LINE_HEIGHT, "{scale}x: 20 more lines");
    assert_eq!(page.scroll_top(), grown, "{scale}x: new lines autoscroll");

    page.wheel(ScrollDelta::Pixels(point(px(0.0), px(60.0))));
    page.frames(3);
    assert_ne!(
        page.mode(),
        SelectionMode::Locked,
        "{scale}x: wheel unlocks"
    );
    assert_eq!(page.scroll_top(), grown - 60.0, "{scale}x: scrolled up");
    page.close();
}

fn hiding_leaves_the_hovered_row(headless: &mut HeadlessCapture, scale: f32) {
    let mut page = Page::open(headless, scale);
    page.lines(0..LINES);
    page.open_console();
    page.frames(4);
    page.dispatch(PlatformInput::MouseMove(MouseMoveEvent {
        position: point(px(700.0), px(400.0)),
        ..MouseMoveEvent::default()
    }));
    page.frames(2);
    let hovered = page.selection(|selection| selection.get_hovered_time());
    assert!(hovered.is_some(), "{scale}x: no row is hovered");
    page.select_documentation();
    page.frames(3);
    let hovered = page.selection(|selection| selection.get_hovered_time());
    assert_eq!(hovered, None, "{scale}x: a hidden console stays hovered");
    page.close();
}

fn dragging_a_field_in_fills_the_console(headless: &mut HeadlessCapture, scale: f32) {
    let mut page = Page::open(headless, scale);
    page.lines(0..LINES);
    page.open_tab(TabType::Console, &JsValue::Null);
    page.frames(1);
    for (x, end, rows) in [(100.0, true, 0), (700.0, false, 0), (700.0, true, LINES)] {
        let data = ops::object([("fields", JsValue::Array(vec![JsValue::string(KEY)]))]);
        let detail = ops::object([
            ("end", JsValue::Bool(end)),
            ("x", JsValue::Number(x)),
            ("y", JsValue::Number(400.0)),
            ("data", data),
        ]);
        page.model(|model| model.tab_drag_update(&detail).expect("drag-update"));
        page.frames(1);
        let count = page.layout(|_, rows| rows.len());
        assert_eq!(count, rows, "{scale}x: dragged to {x}, end {end}");
    }
    page.close();
}

impl Page<'_> {
    fn press(&mut self, x: f64, y: f64) {
        self.dispatch(PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: point(px(x as f32), px(y as f32)),
            click_count: 1,
            ..MouseDownEvent::default()
        }));
    }

    fn drag_to(&mut self, x: f64, y: f64) {
        self.dispatch(PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(x as f32), px(y as f32)),
            pressed_button: Some(MouseButton::Left),
            ..MouseMoveEvent::default()
        }));
    }

    fn release(&mut self, x: f64, y: f64) {
        self.dispatch(PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: point(px(x as f32), px(y as f32)),
            click_count: 1,
            ..MouseUpEvent::default()
        }));
    }

    fn key(&mut self, key: &str) {
        let keystroke = Keystroke {
            modifiers: Modifiers {
                platform: cfg!(target_os = "macos"),
                control: !cfg!(target_os = "macos"),
                ..Modifiers::default()
            },
            key: key.to_owned(),
            key_char: None,
        };
        self.headless
            .context()
            .update_window(self.window.into(), |_, window, cx| {
                window.dispatch_keystroke(keystroke, cx);
            })
            .expect("window");
        self.frames(1);
    }

    fn plain_key(&mut self, key: &str) {
        let keystroke = Keystroke {
            modifiers: Modifiers::default(),
            key: key.to_owned(),
            key_char: (key.len() == 1).then(|| key.to_owned()),
        };
        let window = self.window.into();
        (self.headless.context())
            .update_window(window, |_, window, cx| {
                window.dispatch_keystroke(keystroke, cx);
            })
            .expect("window");
        self.frames(1);
    }

    /// The window as it is drawn now.
    fn shot(&mut self) -> Vec<[u8; 4]> {
        let window = self.window.into();
        let image = self.headless.context().capture_screenshot(window);
        let pixels = image.expect("a screenshot").into_raw();
        pixels.as_chunks::<4>().0.to_vec()
    }

    fn one_frame(&mut self) {
        let cx = self.headless.context();
        cx.advance_clock(std::time::Duration::from_millis(16));
        cx.update_window(self.window.into(), |_, window, cx| {
            window.simulate_next_frame(cx);
        })
        .expect("window");
        cx.run_until_parked();
    }

    fn set_metadata(&mut self, time: f64, value: &str) {
        self.model(|model| {
            let key = JsString::from("/RealMetadata/Key");
            let mut log = model.page().shared_log().borrow_mut();
            log.put_string(&key, time, JsString::from(value)).unwrap();
        });
        self.model(|model| {
            let (tabs, page) = model.tabs_mut();
            let index = tabs.selected_index().expect("a selected tab");
            let tab = tabs.tab_mut(index).expect("the tab");
            tab.controller.refresh(page).expect("refresh");
        });
    }

    fn copied(&mut self) -> String {
        self.headless.context().update(|cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
        });
        self.key("c");
        let copied = self.headless.context().update(|cx| {
            let item = cx.read_from_clipboard();
            item.and_then(|item| item.text()).unwrap_or_default()
        });
        copied.replace("\r\n", "\n")
    }
}

/// What Blink's `(td, 0)` boundary points and text iterator make of rows that come and change:
/// see `selection/gestures` and the `select-console` scenario.
fn selected_text_survives_new_lines(headless: &mut HeadlessCapture, scale: f32) {
    let mut page = Page::open(headless, scale);
    page.lines(0..20);
    page.open_console();
    page.frames(4);
    let container = page.layout(|layout, _| layout.container_rect());
    let left = container.x + 98.0 + 6.0 + 1.0;
    let right = container.x + container.width - 20.0;
    let row = |index: f64| container.y + 31.0 + (index + 0.5) * LINE_HEIGHT;
    let line = |range: std::ops::Range<usize>| {
        let lines: Vec<String> = range.map(|i| format!("line {i}")).collect();
        lines.join("\n")
    };
    page.press(left, row(2.0));
    page.drag_to(right, row(3.0));
    page.drag_to(right, row(4.0));
    page.release(right, row(4.0));
    page.frames(1);
    assert_eq!(page.copied(), line(2..5), "{scale}x: dragged");
    page.lines(20..LINES);
    page.frames(3);
    assert_eq!(page.copied(), line(2..5), "{scale}x: with more lines");
    page.key("a");
    assert_eq!(page.copied(), line(0..LINES), "{scale}x: all");
    page.lines(LINES..LINES + 3);
    page.frames(3);
    let all = line(0..LINES + 3);
    assert_eq!(page.copied(), all, "{scale}x: all of more lines");
    let time = container.x + 50.0;
    page.press(time, row(2.0));
    page.release(time, row(2.0));
    assert_eq!(page.copied(), all, "{scale}x: a press on a timestamp");
    let console = page.model(|model| model.tabs().selected_index().expect("the console"));
    page.select_documentation();
    page.frames(3);
    assert_eq!(page.copied(), "", "{scale}x: a hidden console");
    page.select(console);
    page.frames(3);
    page.press(left, row(2.0));
    page.release(left, row(2.0));
    assert_eq!(page.copied(), "", "{scale}x: a click on the selection");
    page.close();
}

/// `selectAllChildren` of the body: no text node of the table is an end of it.
fn selecting_all_survives_a_rebuilt_table(headless: &mut HeadlessCapture, scale: f32) {
    let mut page = Page::open(headless, scale);
    page.set_metadata(0.0, "first");
    page.open_tab(TabType::Metadata, &JsValue::Null);
    page.frames(3);
    let plain = page.shot();
    page.key("a");
    page.frames(2);
    let selected = page.shot();
    let mut counts = HashMap::new();
    for (before, after) in plain.iter().zip(&selected) {
        if before != after {
            *counts.entry(*after).or_insert(0) += 1;
        }
    }
    let (color, area) = counts.into_iter().max_by_key(|e| e.1).expect("selected");
    page.set_metadata(1.0, "other");
    page.one_frame();
    let left = page.shot().iter().filter(|p| **p == color).count();
    assert!(
        left > area / 2,
        "{scale}x: {left} of {area} pixels selected"
    );
    assert_eq!(page.copied(), "other", "{scale}x");
    page.close();
}

fn keys_with_text_selected(headless: &mut HeadlessCapture, scale: f32) {
    let mut page = Page::open(headless, scale);
    page.lines(0..20);
    page.open_console();
    page.frames(4);
    let container = page.layout(|layout, _| layout.container_rect());
    page.press(container.x + 105.0, container.y + 40.0);
    page.drag_to(container.x + container.width - 20.0, container.y + 40.0);
    page.release(container.x + container.width - 20.0, container.y + 40.0);
    page.frames(1);
    assert_eq!(page.copied(), "line 0", "{scale}x");
    assert_eq!(page.mode(), SelectionMode::Static);
    page.plain_key("space");
    assert_eq!(page.mode(), SelectionMode::Playback, "{scale}x: space");
    page.plain_key("space");
    let time = page.selection(|selection| selection.get_selected_time());
    page.plain_key("right");
    let stepped = page.selection(|selection| selection.get_selected_time());
    assert_ne!(stepped, time, "{scale}x: right");
    assert_eq!(page.copied(), "line 0", "{scale}x: after the keys");
    let input = [container.x + container.width - 60.0, container.y + 15.0];
    page.press(input[0], input[1]);
    page.release(input[0], input[1]);
    page.frames(2);
    assert_eq!(page.copied(), "", "{scale}x: the input's caret");
    page.plain_key("1");
    page.key("a");
    assert_eq!(page.copied(), "1", "{scale}x: the input's text");
    page.close();
}

fn main() {
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        println!("hub console: skipped, no headless renderer on this platform");
        return;
    }
    let mut headless = HeadlessCapture::new(Arc::new(())).expect("headless app");
    for scale in [1.0, 2.0] {
        for from in [None, Some(TabType::LineGraph), Some(TabType::Table)] {
            opening_stays_locked(&mut headless, scale, from);
        }
        hiding_leaves_the_hovered_row(&mut headless, scale);
        dragging_a_field_in_fills_the_console(&mut headless, scale);
        selected_text_survives_new_lines(&mut headless, scale);
        selecting_all_survives_a_rebuilt_table(&mut headless, scale);
        keys_with_text_selected(&mut headless, scale);
    }
    println!("hub_console: ok");
}
