#![allow(clippy::disallowed_methods)]

//! The page's keyboard shortcuts in the real hub window, headless: what has the focus decides
//! whether `SelectionImpl`'s `keydown` listener acts, as `event.target` does upstream.

#[path = "support/hub_page.rs"]
#[allow(dead_code)]
mod hub_page;

use std::sync::Arc;

use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_model::selection::SelectionMode;
use asrs_model::tab_type::TabType;
use asrs_tabs::table::TableRenderer;
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::gpui::{
    KeyDownEvent, KeyUpEvent, Keystroke, MouseButton, MouseDownEvent, MouseUpEvent, PlatformInput,
    point, px,
};
use hub_page::Page;

const KEY: &str = "/Value";
/// In the table's container.
const JUMP_INPUT: (f32, f32) = (30.0, 15.0);
const JUMP_BUTTON: (f32, f32) = (82.0, 15.0);
const BODY: (f32, f32) = (400.0, 412.0);

impl Page<'_> {
    fn press(&mut self, key: &'static str) {
        let keystroke = Keystroke::parse(key).expect("a keystroke");
        self.dispatch(PlatformInput::KeyDown(KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        }));
        self.dispatch(PlatformInput::KeyUp(KeyUpEvent { keystroke }));
        self.frames(1);
    }

    fn click(&mut self, (x, y): (f32, f32)) {
        let container = self.table(|table| table.dom().borrow().layout.container);
        let position = point(px(container.x as f32 + x), px(container.y as f32 + y));
        self.dispatch(PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position,
            click_count: 1,
            ..MouseDownEvent::default()
        }));
        self.dispatch(PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position,
            click_count: 1,
            ..MouseUpEvent::default()
        }));
        self.frames(1);
    }

    fn static_at(&mut self, time: f64) {
        self.selection(|selection| {
            selection.unlock();
            selection.pause();
            selection.set_selected_time(time);
        });
        assert_eq!(self.mode(), SelectionMode::Static);
    }

    fn table<R>(&mut self, f: impl FnOnce(&TableRenderer) -> R) -> R {
        self.model(|model| {
            let mut tabs = model.tabs().tabs();
            let table = tabs.find_map(|tab| tab.renderer().downcast_ref::<TableRenderer>());
            f(table.expect("a table"))
        })
    }

    fn scroll_top(&mut self) -> f64 {
        self.table(|table| table.dom().borrow_mut().layout.scroll()[1])
    }
}

fn space_toggles_playback(page: &mut Page<'_>, focused: &str, toggles: bool) {
    page.static_at(5.0);
    page.press("space");
    let expected = match toggles {
        true => SelectionMode::Playback,
        false => SelectionMode::Static,
    };
    assert_eq!(page.mode(), expected, "Space with the focus on {focused}");
    page.static_at(5.0);
    page.press("right");
    let time = page.selection(|selection| selection.get_selected_time());
    assert_eq!(
        time != Some(5.0),
        toggles,
        "ArrowRight with the focus on {focused}: {time:?}"
    );
}

fn space_toggles_once(page: &mut Page<'_>) {
    page.static_at(5.0);
    for expected in [
        SelectionMode::Playback,
        SelectionMode::Static,
        SelectionMode::Playback,
    ] {
        page.press("space");
        assert_eq!(page.mode(), expected, "one Space, one toggle");
    }
}

fn main() {
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        println!("hub keys: skipped, no headless renderer on this platform");
        return;
    }
    let mut headless = HeadlessCapture::new(Arc::new(())).expect("headless app");
    let mut page = Page::open(&mut headless, 1.0);
    page.model(|model| {
        let mut log = model.page().shared_log().borrow_mut();
        for i in 0..200 {
            log.put_number(&JsString::from(KEY), f64::from(i) / 10.0, f64::from(i));
        }
    });
    let fields = JsValue::Array(vec![JsValue::string(KEY)]);
    page.open_tab(TabType::Table, &fields);
    page.frames(4);
    space_toggles_playback(&mut page, "nothing", true);
    page.click(JUMP_INPUT);
    space_toggles_playback(&mut page, "the jump input", false);
    page.click(BODY);
    space_toggles_playback(&mut page, "the body", true);

    page.click(JUMP_BUTTON);
    let top = page.scroll_top();
    space_toggles_playback(&mut page, "a button", true);
    assert_eq!(page.scroll_top(), top, "Space also clicked the button");
    space_toggles_once(&mut page);

    page.click(JUMP_INPUT);
    space_toggles_playback(&mut page, "the jump input again", false);
    page.select_documentation();
    page.frames(2);
    space_toggles_playback(&mut page, "an input its tab hid", true);
    page.close();
    println!("hub_keys: ok");
}
