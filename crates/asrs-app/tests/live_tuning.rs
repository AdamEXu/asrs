#![allow(clippy::disallowed_methods)]

//! The real hub window, headless, connected to mock servers on real loopback sockets: typing a
//! value in the sidebar's tuning inputs and clicking its booleans publishes them, and main's half
//! of the RLOG source answers the window.

#[path = "support/hub_page.rs"]
#[allow(dead_code)]
mod hub_page;
#[path = "support/live_server.rs"]
#[allow(dead_code)]
mod live_server;

use std::sync::Arc;
use std::time::{Duration, Instant};

use asrs_app::live;
use asrs_hub::sidebar::{RowRect, RowValue, SidebarRow};
use asrs_js::string::JsString;
use asrs_js::value::{JsValue, ToJs};
use asrs_log::log::log_util::get_or_default;
use asrs_model::messages::MainToHub;
use asrs_model::preferences::{LiveMode, LiveSubscribeMode, Preferences};
use asrs_parity::json::{self, Json};
use asrs_parity::live::msgpack::{self, Unpacked};
use asrs_parity::live::{ClientEvent, Payload};
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::gpui::{
    App, AppContext, Keystroke, MouseButton, MouseDownEvent, MouseUpEvent, PlatformInput, point, px,
};
use hub_page::Page;
use live_server::LiveServer;

/// From the search input's right edge to the tuning button's center.
const TUNING_BUTTON: f64 = 20.0;
/// From the search input's top to the first row's center.
const FIRST_ROW: f64 = 49.0;
const ROW: f64 = 30.0;
/// From a boolean's center to the sidebar's right edge.
const BOOLEAN: f64 = 16.0;
const SIDEBAR: f64 = 300.0;

const TOPICS: &str = r#"{"version":"4.1","topics":[
  {"name":"/Tune/kP","type":"double","value":0.25},
  {"name":"/Tune/on","type":"boolean","value":false}]}"#;
const RLOG_FRAME: &str = r#"["send","02003ff0000000000000010000000c2f44726976652f53706565640006646f75626c6502000000083ff8000000000000"]"#;

fn wait(page: &mut Page<'_>, what: &str, mut done: impl FnMut(&mut Page<'_>) -> bool) {
    let start = Instant::now();
    while !done(page) {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "timed out waiting for {what}: {:?} {:?}",
            rows(page)
                .iter()
                .map(|r| r.title.to_string_lossy())
                .collect::<Vec<_>>(),
            page.model(|m| m.title().to_string_lossy())
        );
        page.frames(1);
        std::thread::sleep(Duration::from_millis(4));
    }
}

fn preferences(page: &mut Page<'_>, mode: LiveMode, subscribe: LiveSubscribeMode) {
    let preferences = Preferences {
        robot_address: JsString::from("10.63.28.2"),
        live_mode: mode,
        live_subscribe_mode: subscribe,
        ..Preferences::default()
    };
    page.main(MainToHub::SetPreferences, preferences.to_js());
}

fn rows(page: &mut Page<'_>) -> Vec<SidebarRow> {
    page.model(|model| model.sidebar().displayed_rows())
}

fn row(page: &mut Page<'_>, title: &str) -> Option<(usize, SidebarRow)> {
    let title = JsString::from(title);
    let mut rows = rows(page).into_iter().enumerate();
    rows.find(|(_, row)| row.title == title)
}

fn expand(page: &mut Page<'_>, title: &str) {
    wait(page, title, |page| row(page, title).is_some());
    let (_, found) = row(page, title).expect("the row");
    if !found.expanded {
        page.model(|model| {
            model.with_sidebar(|s, env| s.icon_click(env, found.index, 0, 0.0));
        });
    }
}

fn search_input(page: &mut Page<'_>) -> RowRect {
    page.model(|model| model.with_sidebar(|_, env| env.geometry.search_input_rect()))
}

fn click(page: &mut Page<'_>, (x, y): (f64, f64)) {
    let position = point(px(x as f32), px(y as f32));
    page.dispatch(PlatformInput::MouseDown(MouseDownEvent {
        button: MouseButton::Left,
        position,
        click_count: 1,
        ..MouseDownEvent::default()
    }));
    page.dispatch(PlatformInput::MouseUp(MouseUpEvent {
        button: MouseButton::Left,
        position,
        click_count: 1,
        ..MouseUpEvent::default()
    }));
    page.frames(1);
}

fn click_value(page: &mut Page<'_>, title: &str) {
    let (index, row) = row(page, title).expect("the row");
    let x = match row.value {
        RowValue::Bool { .. } => SIDEBAR - BOOLEAN,
        _ => SIDEBAR - row.value_width / 2.0,
    };
    let y = search_input(page).y + FIRST_ROW + index as f64 * ROW;
    click(page, (x, y));
}

fn keys(page: &mut Page<'_>, keys: &[&'static str]) {
    for key in keys {
        let keystroke = Keystroke::parse(key).expect("a keystroke");
        let window = page.window;
        page.headless
            .context()
            .update_window(window.into(), |_, window, cx| {
                window.dispatch_keystroke(keystroke, cx);
            })
            .expect("window");
        page.headless.context().run_until_parked();
        page.frames(1);
    }
}

#[derive(Default)]
struct Wire {
    /// `(topic, uid)` of each `publish`, and uids of each `unpublish`.
    published: Vec<(String, f64)>,
    unpublished: Vec<f64>,
    /// Topics of each `subscribe` that asks for values.
    subscribed: Vec<String>,
    values: Vec<(f64, Unpacked)>,
}

impl Wire {
    fn read(&mut self, server: &LiveServer) {
        for event in server.take_seen() {
            match event {
                ClientEvent::WsSend {
                    data: Payload::Text(text),
                    ..
                } => {
                    let list = json::parse(&text).expect("JSON");
                    for message in list.as_array().expect("a list") {
                        self.message(message);
                    }
                }
                ClientEvent::WsSend {
                    data: Payload::Binary(bytes),
                    ..
                } => {
                    for item in msgpack::decode_all(&bytes) {
                        if let Unpacked::Arr(items) = item
                            && let [Unpacked::Number(id), _, _, value] = items.as_slice()
                            && *id >= 0.0
                        {
                            self.values.push((*id, value.clone()));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn message(&mut self, message: &Json) {
        let method = message
            .get("method")
            .and_then(Json::as_str)
            .expect("a method");
        let params = message.get("params").expect("params");
        let uid = params.get("pubuid").and_then(Json::as_f64);
        if method.eq_str("publish") {
            let name = params.get("name").and_then(Json::as_str).expect("a name");
            let uid = uid.expect("a uid");
            self.published.push((name.to_string_lossy(), uid));
        } else if method.eq_str("unpublish") {
            self.unpublished.push(uid.expect("a uid"));
        } else if method.eq_str("subscribe") {
            let options = params.get("options").expect("options");
            let topics = params.get("topics").and_then(Json::as_array);
            let topics = topics.expect("topics").iter();
            let topics = topics.map(|t| t.as_str().expect("a topic").to_string_lossy());
            if options.get("topicsonly") == Some(&Json::Bool(false)) {
                self.subscribed.extend(topics);
            }
        }
    }

    fn uid(&self, topic: &str) -> Option<f64> {
        let found = self.published.iter().rfind(|(name, _)| name == topic);
        found.map(|(_, uid)| *uid)
    }

    fn sent(&self, topic: &str, value: &Unpacked) -> bool {
        self.uid(topic)
            .is_some_and(|uid| self.values.iter().any(|(id, v)| *id == uid && v == value))
    }
}

fn set_dial(headless: &mut HeadlessCapture, server: &LiveServer) {
    let dial = server.dial();
    headless.context().update(|cx| live::set_dial(cx, dial));
}

fn tuning(page: &mut Page<'_>, server: &LiveServer) {
    let mut wire = Wire::default();
    preferences(page, LiveMode::Nt4, LiveSubscribeMode::Logging);
    page.main(MainToHub::StartLive, JsValue::string("robot"));
    expand(page, "Tune");
    wait(page, "the tuning button", |page| {
        page.model(|model| !model.sidebar().tuning_button_hidden())
    });
    page.frames(2);
    let search = search_input(page);
    let button = (
        search.x + search.width + TUNING_BUTTON,
        search.y + search.height / 2.0,
    );
    click(page, button);
    assert!(page.model(|model| model.sidebar().tuning_button_active()));

    page.frames(2);
    click_value(page, "kP");
    keys(page, &["3", ".", "5", "enter"]);
    wait(page, "the typed value", |_| {
        wire.read(server);
        wire.sent("/Tune/kP", &Unpacked::Number(3.5))
    });

    click_value(page, "on");
    wait(page, "the clicked boolean", |_| {
        wire.read(server);
        wire.sent("/Tune/on", &Unpacked::Bool(true))
    });

    server.step(r#"["announce","/Tune/aa","double"]"#);
    server.step(r#"["set","/Tune/aa",1]"#);
    wait(page, "the new field's row", |page| {
        let above = row(page, "aa").map(|(i, _)| i);
        above.is_some() && above < row(page, "kP").map(|(i, _)| i)
    });
    page.frames(2);
    click_value(page, "kP");
    keys(page, &["backspace", "backspace", "backspace", "backspace"]);
    keys(page, &["7", "enter"]);
    wait(page, "the value typed in the moved row", |_| {
        wire.read(server);
        assert_eq!(wire.uid("/Tune/aa"), None, "another row's field was tuned");
        wire.sent("/Tune/kP", &Unpacked::Number(7.0))
    });
    click_value(page, "kP");
    keys(page, &["backspace", "backspace", "enter"]);
    wait(page, "the cleared field", |_| {
        wire.read(server);
        let uid = wire.uid("/Tune/kP").expect("a published topic");
        wire.unpublished.contains(&uid)
    });
}

fn sidebar_fields(page: &mut Page<'_>, server: &LiveServer) {
    let mut wire = Wire::default();
    preferences(page, LiveMode::Nt4, LiveSubscribeMode::LowBandwidth);
    page.main(MainToHub::StartLive, JsValue::string("robot"));
    expand(page, "Tune");
    wait(page, "the row", |page| row(page, "kP").is_some());
    wait(page, "a subscription to the row's field", |_| {
        wire.read(server);
        wire.subscribed.iter().any(|topic| topic == "/Tune/kP")
    });
}

fn rlog(page: &mut Page<'_>, server: &LiveServer) {
    preferences(page, LiveMode::Rlog, LiveSubscribeMode::Logging);
    page.main(MainToHub::StartLive, JsValue::string("robot"));
    wait(page, "the RLOG connection", |_| server.opened() > 0);
    server.step(RLOG_FRAME);
    wait(page, "the RLOG data", |page| {
        page.model(|model| {
            let key = JsString::from("/Drive/Speed");
            get_or_default(&model.page().log(), &key, f64::MAX, f64::NAN, None) == 1.5
        })
    });
    let title = page.model(|model| model.title().to_string_lossy());
    assert_eq!(title, "10.63.28.2 \u{2014} AdvantageScope");
}

fn main() {
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        println!("live tuning: skipped, no headless renderer on this platform");
        return;
    }
    let nt4 = LiveServer::new("nt4", TOPICS);
    let rlog_server = LiveServer::new("rlog", "{}");
    let mut headless = HeadlessCapture::new(Arc::new(())).expect("headless app");
    set_dial(&mut headless, &nt4);
    let mut page = Page::open(&mut headless, 1.0);
    tuning(&mut page, &nt4);
    page.close();
    nt4.take_seen();
    let mut page = Page::open(&mut headless, 1.0);
    sidebar_fields(&mut page, &nt4);
    page.close();
    set_dial(&mut headless, &rlog_server);
    let mut page = Page::open(&mut headless, 1.0);
    rlog(&mut page, &rlog_server);
    page.close();
    let start = Instant::now();
    let tasks = |cx: &mut App| {
        // Ends an update, which is when GPUI releases the closed window's entities.
        cx.new(|_| ());
        live::runtime_tasks(cx)
    };
    while headless.context().update(tasks) > 0 {
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "a closed window's connections stay open"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    println!("live tuning: ok");
}
