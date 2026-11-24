#![allow(clippy::disallowed_methods)]

//! The page's keyboard shortcuts in the real hub window, headless: what has the focus decides
//! whether `SelectionImpl`'s `keydown` listener acts, as `event.target` does upstream.

#[path = "support/hub_page.rs"]
#[allow(dead_code)]
mod hub_page;

use std::sync::Arc;

use asrs_js::ops;
use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_model::messages::MainToHub;
use asrs_model::selection::SelectionMode;
use asrs_model::tab_type::TabType;
use asrs_tabs::table::TableRenderer;
use asrs_tabs::video::VideoController;
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::gpui::{
    KeyDownEvent, KeyUpEvent, Keystroke, MouseButton, MouseDownEvent, MouseUpEvent, PlatformInput,
    point, px,
};
use asrs_ui::platform::{PopupEntry, PopupMenu};
use asrs_ui::{input, platform};
use hub_page::Page;

const KEY: &str = "/Value";
/// In the table's container.
const JUMP_INPUT: (f32, f32) = (30.0, 15.0);
const JUMP_BUTTON: (f32, f32) = (82.0, 15.0);
const BODY: (f32, f32) = (400.0, 412.0);
/// In the window.
const SLIDER: (f32, f32) = (800.0, 582.0);
const SEARCH: (f32, f32) = (150.0, 87.0);
const TITLE_BAR: (f32, f32) = (600.0, 20.0);
const CANVAS: (f32, f32) = (700.0, 412.0);

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
        self.click_at((container.x as f32 + x, container.y as f32 + y));
    }

    fn press_at(&mut self, button: MouseButton, (x, y): (f32, f32)) {
        self.dispatch(PlatformInput::MouseDown(MouseDownEvent {
            button,
            position: point(px(x), px(y)),
            click_count: 1,
            ..MouseDownEvent::default()
        }));
        self.frames(1);
    }

    fn click_at(&mut self, (x, y): (f32, f32)) {
        let position = point(px(x), px(y));
        self.press_at(MouseButton::Left, (x, y));
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

    /// Where a click on the jump button would now go.
    fn jump_to(&mut self, value: &str) {
        let value = JsString::from(value);
        self.table(|table| table.dom().borrow_mut().jump_input.set_value(value));
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

fn open_video(page: &mut Page<'_>) {
    page.open_tab(TabType::Video, &JsValue::Null);
    let uuid = page.model(|model| {
        let tabs = model.tabs();
        let index = tabs.selected_index().expect("a selected tab");
        tabs.tab(index).expect("the tab").uuid().clone()
    });
    let data = ops::object([
        ("uuid", uuid),
        ("imgFolder", JsValue::string("/none/")),
        ("fps", JsValue::Number(30.0)),
        ("totalFrames", JsValue::Number(100.0)),
        ("completedFrames", JsValue::Number(100.0)),
        ("autoEndFrame", JsValue::Number(-1.0)),
    ]);
    page.main(MainToHub::VideoData, data);
    page.frames(2);
}

fn slider_value(page: &mut Page<'_>) -> f64 {
    page.model(|model| {
        let mut tabs = model.tabs().tabs();
        let video = tabs.find_map(|tab| tab.controller().downcast_ref::<VideoController>());
        video.expect("a video").slider().number()
    })
}

fn in_input(page: &mut Page<'_>) -> bool {
    let window = page.window.into();
    page.headless
        .context()
        .update_window(window, |_, window, _| input::focused(window))
        .expect("window")
}

/// Electron gives the page no events over `-webkit-app-region: drag`, which only macOS's hub has
/// here.
fn title_bar_keeps_the_focus(page: &mut Page<'_>) {
    page.click_at(SEARCH);
    assert!(in_input(page), "a press focuses the search field");
    // GPUI's test window panics when it is moved, which the left button does.
    page.press_at(MouseButton::Right, TITLE_BAR);
    assert!(in_input(page), "the title bar is not the page's");
    page.click_at(CANVAS);
    assert!(
        !in_input(page),
        "a press on the page blurs the search field"
    );
}

/// Chromium sends the keys of a window to the menu that popped up for it.
fn tab_goes_to_an_open_menu(page: &mut Page<'_>) {
    page.click_at(SEARCH);
    page.press("tab");
    assert!(!in_input(page), "Tab leaves the search field");
    page.click_at(SEARCH);
    let window = page.window.into();
    page.headless
        .context()
        .update_window(window, |_, window, cx| {
            let item = PopupEntry::new("Item");
            platform::popup_menu(
                window,
                cx,
                PopupMenu::new().entry(item),
                100,
                120,
                |_, _| {},
            );
        })
        .expect("window");
    page.frames(2);
    page.press("tab");
    assert!(in_input(page), "Tab with a menu open is the menu's");
    page.headless
        .context()
        .update_window(window, |_, window, _| platform::close_popup_menu(window))
        .expect("window");
    page.frames(2);
    page.press("tab");
    assert!(!in_input(page), "Tab once the menu closed");
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

    if cfg!(target_os = "macos") {
        title_bar_keeps_the_focus(&mut page);
    }
    if cfg!(target_os = "linux") {
        tab_goes_to_an_open_menu(&mut page);
    }
    page.click(JUMP_BUTTON);
    page.jump_to("15");
    let top = page.scroll_top();
    space_toggles_playback(&mut page, "a button", true);
    assert_eq!(page.scroll_top(), top, "Space also clicked the button");
    space_toggles_once(&mut page);

    open_video(&mut page);
    page.click_at(SLIDER);
    assert!(slider_value(&mut page) > 1.0, "the slider is clicked");
    space_toggles_playback(&mut page, "the video's slider", false);
    page.select_documentation();
    page.frames(2);
    space_toggles_playback(&mut page, "a slider its tab hid", true);
    page.close();
    println!("hub_keys: ok");
}
