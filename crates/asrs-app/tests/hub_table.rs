#![allow(clippy::disallowed_methods)]

//! The Table tab in the real hub window, headless: autoscroll while playing and locked, the wheel
//! unlocking it, a first frame that does not, wheel input past an edge, and the hover a table
//! leaves when its tab is hidden. `dom/tabs/table-*` check the rules themselves against Blink.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use asrs_app::hub::Hub;
use asrs_app::views::frame::HubFrame;
use asrs_app::views::hub_host::HubEntityHost;
use asrs_hub::hub::HubModel;
use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_model::messages::{MainToHub, NamedMessage};
use asrs_model::selection::{Selection, SelectionMode};
use asrs_model::tab_type::TabType;
use asrs_tabs::table::TableRenderer;
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::gpui::{
    Entity, MouseMoveEvent, PlatformInput, ScrollDelta, ScrollEvent, WindowHandle, point, px, size,
};

const WIDTH: f32 = 1100.0;
const HEIGHT: f32 = 650.0;
const FRAME: Duration = Duration::from_millis(16);
const SETTLE_FRAMES: usize = 40;
const ROWS: usize = 200;
const KEY: &str = "/Value";

type Window = WindowHandle<HubFrame<HubEntityHost>>;

struct Page<'a> {
    headless: &'a mut HeadlessCapture,
    window: Window,
    hub: Entity<Hub>,
}

impl Page<'_> {
    fn open(headless: &mut HeadlessCapture, scale: f32) -> Page<'_> {
        let window = headless
            .open_window(size(px(WIDTH), px(HEIGHT)), scale, |window, cx| {
                HubEntityHost::open(window, false, false, Vec::new(), cx)
            })
            .expect("hub window");
        let hub = headless
            .context()
            .update_window(window.into(), |root, _, cx| {
                let frame = root.downcast::<HubFrame<HubEntityHost>>().expect("frame");
                frame.read(cx).host().hub().clone()
            })
            .expect("window");
        let mut page = Page {
            headless,
            window,
            hub,
        };
        page.frames(2);
        page
    }

    fn main(&mut self, name: MainToHub, data: JsValue) {
        let hub = self.hub.clone();
        self.headless.context().update(|cx| {
            hub.update(cx, |hub, cx| {
                hub.handle_main_message(NamedMessage::new(name, data), cx)
            })
        });
        self.headless.context().run_until_parked();
    }

    fn frames(&mut self, count: usize) {
        for _ in 0..count {
            let cx = self.headless.context();
            cx.advance_clock(FRAME);
            cx.update_window(self.window.into(), |_, window, cx| {
                window.simulate_next_frame(cx);
            })
            .expect("window");
            cx.run_until_parked();
        }
        self.headless.capture(self.window.into()).expect("render");
    }

    fn model<R>(&mut self, f: impl FnOnce(&mut HubModel) -> R) -> R {
        let hub = self.hub.clone();
        self.headless
            .context()
            .update(|cx| hub.update(cx, |hub, cx| hub.update(cx, |model, _| f(model))))
    }

    fn log(&mut self) {
        self.model(|model| {
            let mut log = model.page().shared_log().borrow_mut();
            for i in 0..ROWS {
                log.put_number(&JsString::from(KEY), i as f64 / 10.0, i as f64);
            }
        });
    }

    /// Opens a table with one column, which the next frame renders for the first time.
    fn open_table(&mut self) {
        self.main(MainToHub::NewTab, JsValue::Number(TabType::Table.number()));
        self.model(|model| {
            let (tabs, page) = model.tabs_mut();
            let index = tabs.selected_index().expect("a selected tab");
            let tab = tabs.tab_mut(index).expect("the table");
            let fields = JsValue::Array(vec![JsValue::string(KEY)]);
            tab.controller.restore_state(page, &fields).expect("fields");
        });
    }

    fn table<R>(&mut self, f: impl FnOnce(&TableRenderer) -> R) -> R {
        self.model(|model| {
            let renderer = model
                .tabs()
                .tabs()
                .find_map(|tab| tab.renderer().downcast_ref::<TableRenderer>())
                .expect("a table");
            f(renderer)
        })
    }

    fn scroll_top(&mut self) -> f64 {
        self.table(|table| table.dom().borrow_mut().layout.scroll()[1])
    }

    fn max_scroll_top(&mut self) -> f64 {
        self.table(|table| table.dom().borrow().layout.scroll_range()[1])
    }

    fn selection<R>(&mut self, f: impl FnOnce(&mut dyn Selection) -> R) -> R {
        self.model(|model| f(model.page_mut().selection_mut()))
    }

    fn mode(&mut self) -> SelectionMode {
        self.selection(|selection| selection.get_mode())
    }

    fn dispatch(&mut self, event: PlatformInput) {
        self.headless
            .context()
            .update_window(self.window.into(), |_, window, cx| {
                window.dispatch_event(event, cx);
            })
            .expect("window");
        self.headless.context().run_until_parked();
    }

    fn wheel(&mut self, delta: ScrollDelta) {
        self.dispatch(PlatformInput::Scroll(ScrollEvent {
            position: point(px(700.0), px(400.0)),
            delta,
            ..ScrollEvent::default()
        }));
    }

    fn close(self) {
        self.headless
            .context()
            .update_window(self.window.into(), |_, window, _| window.remove_window())
            .expect("window");
        self.headless.context().run_until_parked();
    }
}

fn connected(page: &mut Page<'_>) {
    page.selection(|selection| selection.set_live_connected(Rc::new(|| ROWS as f64 / 10.0)));
    assert_eq!(page.mode(), SelectionMode::Locked);
}

fn locked_autoscrolls_and_wheel_unlocks(headless: &mut HeadlessCapture, scale: f32) {
    let mut page = Page::open(headless, scale);
    page.log();
    connected(&mut page);
    page.open_table();
    page.frames(6);
    assert_eq!(
        page.mode(),
        SelectionMode::Locked,
        "{scale}x: the first frames unlocked"
    );
    let end = page.max_scroll_top();
    assert!(end > 4000.0, "{scale}x: {end}");
    assert_eq!(page.scroll_top(), end, "{scale}x: locked autoscroll");

    page.model(|model| {
        let mut log = model.page().shared_log().borrow_mut();
        for i in ROWS..ROWS + 20 {
            log.put_number(&JsString::from(KEY), i as f64 / 10.0, i as f64);
        }
    });
    page.selection(|selection| {
        selection.set_live_connected(Rc::new(|| (ROWS + 20) as f64 / 10.0));
        selection.lock();
    });
    page.frames(4);
    let grown = page.max_scroll_top();
    assert_eq!(grown, end + 20.0 * 25.0, "{scale}x: 20 more rows");
    assert_eq!(page.scroll_top(), grown, "{scale}x: new rows autoscroll");
    assert_eq!(page.mode(), SelectionMode::Locked);

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

fn playback_autoscrolls(headless: &mut HeadlessCapture, scale: f32) {
    let mut page = Page::open(headless, scale);
    page.log();
    page.open_table();
    page.frames(4);
    page.selection(|selection| selection.set_selected_time(15.0));
    page.frames(4);
    assert_eq!(page.scroll_top(), 0.0, "{scale}x: a static time scrolls");
    page.selection(|selection| {
        selection.set_playback_speed(0.001);
        selection.play();
    });
    page.frames(4);
    assert_eq!(page.mode(), SelectionMode::Playback);
    let height = page.table(|table| table.dom().borrow().layout.container.height);
    assert_eq!(
        page.scroll_top(),
        151.0 * 25.0 - (height - 31.0),
        "{scale}x: playback autoscroll puts row 150 at the bottom"
    );
    page.close();
}

fn wheel_past_an_edge(headless: &mut HeadlessCapture, scale: f32) {
    let mut settled = Vec::new();
    for ticks in [&[-1.0][..], &[1.0, -1.0], &[1.0, 1.0, 1.0, -1.0]] {
        let mut page = Page::open(headless, scale);
        page.log();
        page.open_table();
        page.frames(4);
        for tick in ticks {
            page.wheel(ScrollDelta::Lines(point(0.0, *tick)));
            page.frames(1);
        }
        page.frames(SETTLE_FRAMES);
        settled.push(page.scroll_top());
        page.close();
    }
    assert!(settled[0] > 0.0, "{scale}x: the wheel scrolls");
    assert!(
        settled.iter().all(|top| *top == settled[0]),
        "{scale}x: ticks past the top count against later ones: {settled:?}"
    );
}

fn hiding_leaves_the_body(headless: &mut HeadlessCapture, scale: f32) {
    let mut page = Page::open(headless, scale);
    page.log();
    page.open_table();
    page.frames(4);
    page.dispatch(PlatformInput::MouseMove(MouseMoveEvent {
        position: point(px(400.0), px(400.0)),
        ..MouseMoveEvent::default()
    }));
    page.frames(2);
    let hovered = page.selection(|selection| selection.get_hovered_time());
    assert!(hovered.is_some(), "{scale}x: no row is hovered");
    page.model(|model| {
        let (tabs, page) = model.tabs_mut();
        tabs.set_selected(page, &JsValue::Number(0.0))
            .expect("the documentation");
    });
    page.frames(3);
    let cursor = page.table(|table| table.dom().borrow().hover_cursor_y);
    assert_eq!(cursor, None, "{scale}x: a hidden table stays hovered");
    page.close();
}

fn main() {
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        println!("hub table: skipped, no headless renderer on this platform");
        return;
    }
    let mut headless = HeadlessCapture::new(Arc::new(())).expect("headless app");
    for scale in [1.0, 2.0] {
        locked_autoscrolls_and_wheel_unlocks(&mut headless, scale);
        playback_autoscrolls(&mut headless, scale);
        wheel_past_an_edge(&mut headless, scale);
        hiding_leaves_the_body(&mut headless, scale);
    }
    println!("hub_table: ok");
}
