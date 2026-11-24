//! The real hub window, headless, driven frame by frame.

use std::time::Duration;

use asrs_app::hub::Hub;
use asrs_app::views::frame::HubFrame;
use asrs_app::views::hub_host::{HubEntityHost, boot_messages};
use asrs_hub::hub::HubModel;
use asrs_js::value::{JsValue, ToJs};
use asrs_model::messages::{MainToHub, NamedMessage};
use asrs_model::preferences::Preferences;
use asrs_model::selection::{Selection, SelectionMode};
use asrs_model::tab_type::TabType;
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::gpui::{
    Entity, PlatformInput, ScrollDelta, ScrollEvent, WindowHandle, point, px, size,
};

const WIDTH: f32 = 1100.0;
const HEIGHT: f32 = 650.0;
const FRAME: Duration = Duration::from_millis(16);
const RANDOM_SEED: u64 = 1;

type Window = WindowHandle<HubFrame<HubEntityHost>>;

pub struct Page<'a> {
    headless: &'a mut HeadlessCapture,
    window: Window,
    hub: Entity<Hub>,
}

impl Page<'_> {
    pub fn open(headless: &mut HeadlessCapture, scale: f32) -> Page<'_> {
        let window = headless
            .open_window(size(px(WIDTH), px(HEIGHT)), scale, |window, cx| {
                let preferences = Preferences::default().to_js();
                let boot = boot_messages(JsValue::Null, false, preferences, Vec::new());
                HubEntityHost::open_with(window, false, boot, Some(RANDOM_SEED), cx)
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

    pub fn main(&mut self, name: MainToHub, data: JsValue) {
        let hub = self.hub.clone();
        self.headless.context().update(|cx| {
            hub.update(cx, |hub, cx| {
                hub.handle_main_message(NamedMessage::new(name, data), cx)
            })
        });
        self.headless.context().run_until_parked();
    }

    pub fn frames(&mut self, count: usize) {
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

    pub fn model<R>(&mut self, f: impl FnOnce(&mut HubModel) -> R) -> R {
        let hub = self.hub.clone();
        self.headless
            .context()
            .update(|cx| hub.update(cx, |hub, cx| hub.update(cx, |model, _| f(model))))
    }

    /// Opens a tab and restores `fields` in its controller, which the next frame renders for the
    /// first time.
    pub fn open_tab(&mut self, kind: TabType, fields: &JsValue) {
        self.main(MainToHub::NewTab, JsValue::Number(kind.number()));
        self.model(|model| {
            let (tabs, page) = model.tabs_mut();
            let index = tabs.selected_index().expect("a selected tab");
            let tab = tabs.tab_mut(index).expect("the tab");
            tab.controller.restore_state(page, fields).expect("fields");
        });
    }

    pub fn select(&mut self, index: usize) {
        self.model(|model| {
            let (tabs, page) = model.tabs_mut();
            tabs.set_selected(page, &JsValue::Number(index as f64))
                .expect("the tab");
        });
    }

    pub fn select_documentation(&mut self) {
        self.select(0);
    }

    pub fn selection<R>(&mut self, f: impl FnOnce(&mut dyn Selection) -> R) -> R {
        self.model(|model| f(model.page_mut().selection_mut()))
    }

    pub fn mode(&mut self) -> SelectionMode {
        self.selection(|selection| selection.get_mode())
    }

    pub fn dispatch(&mut self, event: PlatformInput) {
        self.headless
            .context()
            .update_window(self.window.into(), |_, window, cx| {
                window.dispatch_event(event, cx);
            })
            .expect("window");
        self.headless.context().run_until_parked();
    }

    pub fn wheel(&mut self, delta: ScrollDelta) {
        self.dispatch(PlatformInput::Scroll(ScrollEvent {
            position: point(px(700.0), px(400.0)),
            delta,
            ..ScrollEvent::default()
        }));
    }

    pub fn close(self) {
        self.headless
            .context()
            .update_window(self.window.into(), |_, window, _| window.remove_window())
            .expect("window");
        self.headless.context().run_until_parked();
    }
}
