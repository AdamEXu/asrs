#![allow(clippy::disallowed_methods)]

//! Wheel input through the real hub window, headless: a notched tick over the timeline zooms it
//! and one over a crowded tab bar scrolls it, each animating over the frames the test delivers.
//! The animations run on the executor's clock, simulated here, so the same frames land on the
//! same result however much real time passes between them.

use std::sync::Arc;
use std::time::Duration;

use asrs_app::hub::Hub;
use asrs_app::views::frame::{FrameHost, HubFrame};
use asrs_app::views::hub_host::HubEntityHost;
use asrs_hub::frame::FrameElement;
use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_model::messages::{MainToHub, NamedMessage};
use asrs_model::selection::Selection;
use asrs_model::tab_type::TabType;
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::gpui::{
    Entity, PlatformInput, ScrollDelta, ScrollEvent, WindowHandle, point, px, size,
};

const WIDTH: f32 = 1100.0;
const HEIGHT: f32 = 650.0;
const FRAME: Duration = Duration::from_millis(16);
const SETTLE_FRAMES: usize = 40;

type Window = WindowHandle<HubFrame<HubEntityHost>>;

struct Page<'a> {
    headless: &'a mut HeadlessCapture,
    window: Window,
    hub: Entity<Hub>,
    scale: f32,
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
            scale,
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

    fn rect(&mut self, element: FrameElement) -> [f64; 4] {
        let (w, h, scale) = (f64::from(WIDTH), f64::from(HEIGHT), f64::from(self.scale));
        self.headless
            .context()
            .update_window(self.window.into(), |root, _, cx| {
                let frame = root.downcast::<HubFrame<HubEntityHost>>().expect("frame");
                frame.read(cx).layout(cx, w, h, scale).rect(element)
            })
            .expect("window")
            .unwrap_or_else(|| panic!("no {element:?}"))
    }

    fn wheel(&mut self, element: FrameElement, lines: f32) {
        let r = self.rect(element);
        let position = point(
            px((r[0] + r[2] / 2.0) as f32),
            px((r[1] + r[3] / 2.0) as f32),
        );
        let event = PlatformInput::Scroll(ScrollEvent {
            position,
            delta: ScrollDelta::Lines(point(0.0, lines)),
            ..ScrollEvent::default()
        });
        self.headless
            .context()
            .update_window(self.window.into(), |_, window, cx| {
                window.dispatch_event(event, cx);
            })
            .expect("window");
        self.headless.context().run_until_parked();
    }

    fn timeline_range(&mut self) -> [f64; 2] {
        let hub = self.hub.clone();
        self.headless.context().update(|cx| {
            hub.update(cx, |hub, cx| {
                hub.update(cx, |model, _| {
                    model.page_mut().selection_mut().get_timeline_range()
                })
            })
        })
    }

    fn tab_bar_scroll_left(&mut self) -> f64 {
        self.headless
            .context()
            .update_window(self.window.into(), |root, _, cx| {
                let frame = root.downcast::<HubFrame<HubEntityHost>>().expect("frame");
                frame
                    .read(cx)
                    .host()
                    .geometry()
                    .tab_bar()
                    .borrow()
                    .scroll_left
            })
            .expect("window")
    }

    fn close(self) {
        self.headless
            .context()
            .update_window(self.window.into(), |_, window, _| window.remove_window())
            .expect("window");
        self.headless.context().run_until_parked();
    }
}

/// Zooms the timeline of a 100 s log with one notched tick, sleeping `stall` of real time
/// before each frame, and returns the range before and after.
fn zoom(headless: &mut HeadlessCapture, scale: f32, stall: Duration) -> ([f64; 2], [f64; 2]) {
    let mut page = Page::open(headless, scale);
    page.main(
        MainToHub::NewTab,
        JsValue::Number(TabType::Mechanism.number()),
    );
    let hub = page.hub.clone();
    page.headless.context().update(|cx| {
        hub.update(cx, |hub, cx| {
            hub.update(cx, |model, _| {
                let mut log = model.page().shared_log().borrow_mut();
                let key = JsString::from("/Value");
                for t in 0..=100 {
                    log.put_number(&key, f64::from(t), f64::from(t));
                }
            })
        })
    });
    page.frames(4);
    let before = page.timeline_range();
    page.wheel(FrameElement::Timeline, 1.0);
    for _ in 0..SETTLE_FRAMES {
        std::thread::sleep(stall);
        page.frames(1);
    }
    let after = page.timeline_range();
    page.close();
    (before, after)
}

fn tab_bar(headless: &mut HeadlessCapture, scale: f32) -> (f64, f64) {
    let mut page = Page::open(headless, scale);
    for _ in 0..12 {
        page.main(
            MainToHub::NewTab,
            JsValue::Number(TabType::Mechanism.number()),
        );
    }
    page.frames(4);
    let before = page.tab_bar_scroll_left();
    page.wheel(FrameElement::TabBar, -1.0);
    page.frames(SETTLE_FRAMES);
    let after = page.tab_bar_scroll_left();
    page.close();
    (before, after)
}

fn main() {
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        println!("hub wheel: skipped, no headless renderer on this platform");
        return;
    }
    let mut headless = HeadlessCapture::new(Arc::new(())).expect("headless app");
    for scale in [1.0, 2.0] {
        let (before, after) = zoom(&mut headless, scale, Duration::ZERO);
        assert!(
            after[1] - after[0] < before[1] - before[0],
            "{scale}x: the timeline did not zoom in: {before:?} -> {after:?}"
        );
        let (_, stalled) = zoom(&mut headless, scale, Duration::from_millis(40));
        assert_eq!(
            stalled, after,
            "{scale}x: slow frames changed where the zoom settled"
        );
        println!("{scale}x timeline: {before:?} -> {after:?}");

        let (before, after) = tab_bar(&mut headless, scale);
        assert!(
            after > before,
            "{scale}x: the tab bar did not scroll: {before} -> {after}"
        );
        println!("{scale}x tab bar: scrollLeft {before} -> {after}");
    }
    println!("hub_wheel: ok");
}
