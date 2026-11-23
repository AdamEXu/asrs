#![allow(clippy::disallowed_methods)]

//! The Console tab in the real hub window, headless: a first frame that does not unlock, locked
//! autoscroll, the wheel unlocking it, the hover a console leaves when its tab is hidden, and a
//! field dragged in.

#[path = "support/hub_page.rs"]
mod hub_page;

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
use asrs_ui::gpui::{MouseMoveEvent, PlatformInput, ScrollDelta, point, px};
use hub_page::Page;

const LINES: usize = 200;
const KEY: &str = "/Messages";

impl Page<'_> {
    fn lines(&mut self, range: std::ops::Range<usize>) {
        self.model(|model| {
            let mut log = model.page().shared_log().borrow_mut();
            for i in range {
                let text = JsString::from(format!("line {i}").as_str());
                log.put_string(&JsString::from(KEY), i as f64 / 10.0, text);
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
    }
    println!("hub_console: ok");
}
