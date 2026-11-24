#![allow(clippy::disallowed_methods)]

//! Live sources and the NT4 publisher through `asrs_app::hub::Hub` and the app's own sockets,
//! against mock servers on real loopback sockets: connecting, data in the `Log`, the server
//! dying, reconnecting and stopping. The executor's clock is simulated and only moves while no
//! connection has anything to say, so a slow machine does not time out a server that answers.

#[path = "support/live_server.rs"]
#[allow(dead_code)]
mod live_server;

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};

use asrs_app::hub::{FrameClock, Hub, HubConfig, HubEvent};
use asrs_app::live;
use asrs_hub::hub::HubView;
use asrs_hub::sidebar::{NoLayout, SidebarGeometry};
use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::log::Log;
use asrs_log::log::log_util::get_or_default;
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage};
use asrs_model::preferences::{LiveMode, LiveSubscribeMode, Preferences};
use asrs_model::selection::{Selection, SelectionMode};
use asrs_model::tab_type::TabType;
use asrs_parity::json::{self, Json};
use asrs_parity::live::msgpack::{self, Unpacked};
use asrs_parity::live::{ClientEvent, Payload};
use asrs_ui::gpui::{
    AnyWindowHandle, AppContext, Context, Entity, HeadlessAppContext, IntoElement, NoopTextSystem,
    Render, Window, div, px, size,
};
use live_server::LiveServer;

const TIMEOUT: Duration = Duration::from_secs(60);
const STEP: Duration = Duration::from_millis(5);
const ADDRESS: &str = "10.63.28.2";
const CSV: &str = "Timestamp,Key,Value\n0,/Log,1\n";
const NT_CSV: &str = "Timestamp,Key,Value\n0,NT:/Data/x,2.5\n1,NT:/Data/x,-1\n0,NT:/Data/flag,true\n0,NT:/Skip/y,4\n";

fn js(s: &str) -> JsString {
    JsString::from(s)
}

struct View {
    sidebar: NoLayout,
}

impl HubView for View {
    fn inner_height(&self) -> f64 {
        650.0
    }

    fn renderer_top(&self, _show_timeline: bool) -> f64 {
        92.0
    }

    fn dark(&self) -> bool {
        false
    }

    fn sidebar_geometry(&mut self) -> &mut dyn SidebarGeometry {
        &mut self.sidebar
    }

    fn source_list_dom(&mut self) -> &mut dyn asrs_model::source_list::SourceListDom {
        asrs_model::source_list::Undisplayed::dom()
    }
}

struct Shell(#[allow(dead_code)] Entity<Hub>);

impl Render for Shell {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div()
    }
}

struct Page {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    hub: Option<Entity<Hub>>,
    titles: Rc<RefCell<Vec<String>>>,
    main: Rc<RefCell<Vec<NamedMessage<HubToMain>>>>,
    exceptions: Rc<RefCell<Vec<String>>>,
    files: Vec<std::path::PathBuf>,
}

impl Page {
    fn new(server: &LiveServer) -> Page {
        let mut cx = HeadlessAppContext::new(Arc::new(NoopTextSystem::new()));
        let slot = Rc::new(RefCell::new(None));
        let built = slot.clone();
        let dial = server.dial();
        let window = cx
            .open_window(size(px(1100.0), px(650.0)), move |window, cx| {
                let mut config = HubConfig::new(Box::new(View { sidebar: NoLayout }));
                config.clock = Rc::new(FrameClock::new(cx));
                config.dial = Some(dial);
                let hub = cx.new(|cx| Hub::new(Some(window), config, cx));
                *built.borrow_mut() = Some(hub.clone());
                cx.new(|_| Shell(hub))
            })
            .expect("a headless window");
        let hub: Entity<Hub> = slot.borrow_mut().take().expect("the window built the hub");
        let titles = Rc::new(RefCell::new(Vec::new()));
        let main = Rc::new(RefCell::new(Vec::new()));
        let exceptions = Rc::new(RefCell::new(Vec::new()));
        let (title_sink, main_sink, exception_sink) =
            (titles.clone(), main.clone(), exceptions.clone());
        cx.update(|cx| {
            live::attach(&hub, cx);
            cx.subscribe(&hub, move |hub, event, cx| match event {
                HubEvent::Title => {
                    let title = hub.read(cx).model().title().to_string_lossy();
                    title_sink.borrow_mut().push(title);
                }
                HubEvent::Main(message) => main_sink.borrow_mut().push(message.clone()),
                HubEvent::Exception { task, error } => {
                    exception_sink.borrow_mut().push(format!("{task}: {error}"));
                }
                _ => {}
            })
            .detach();
        });
        Page {
            cx,
            window: window.into(),
            hub: Some(hub),
            titles,
            main,
            exceptions,
            files: Vec::new(),
        }
    }

    fn hub(&self) -> Entity<Hub> {
        self.hub.clone().expect("the window is open")
    }

    fn send(&mut self, name: MainToHub, data: JsValue) {
        let hub = self.hub();
        self.cx.update(|cx| {
            hub.update(cx, |hub, cx| {
                hub.handle_main_message(NamedMessage::new(name, data), cx)
            })
        });
    }

    fn preferences(&mut self, edit: impl FnOnce(&mut Preferences)) {
        let mut preferences = Preferences {
            robot_address: js(ADDRESS),
            ..Preferences::default()
        };
        edit(&mut preferences);
        self.send(MainToHub::SetPreferences, preferences.to_js());
    }

    fn start_live(&mut self, from: &str) {
        self.send(MainToHub::StartLive, JsValue::string(from));
    }

    /// Restores one Table tab of `fields`, which makes them active.
    fn table(&mut self, fields: &[&str]) {
        let object =
            |pairs: Vec<(&str, JsValue)>| JsValue::Object(pairs.into_iter().map(entry).collect());
        fn entry((key, value): (&str, JsValue)) -> (JsString, JsValue) {
            (js(key), value)
        }
        let fields = fields.iter().map(|f| JsValue::string(f)).collect();
        let tab = |kind: TabType, controller: JsValue| {
            object(vec![
                ("type", JsValue::Number(kind.number())),
                ("title", JsValue::string("")),
                ("controller", controller),
                ("controllerUUID", JsValue::string("uuid")),
                ("renderer", JsValue::Null),
                ("controlsHeight", JsValue::Number(0.0)),
            ])
        };
        let tabs = vec![
            tab(TabType::Documentation, JsValue::Null),
            tab(TabType::Table, JsValue::Array(fields)),
        ];
        let state = object(vec![
            (
                "sidebar",
                object(vec![
                    ("width", JsValue::Number(300.0)),
                    ("expanded", JsValue::Array(Vec::new())),
                ]),
            ),
            (
                "tabs",
                object(vec![
                    ("selected", JsValue::Number(1.0)),
                    ("tabs", JsValue::Array(tabs)),
                ]),
            ),
        ]);
        self.send(MainToHub::RestoreState, state);
    }

    fn open_log(&mut self, name: &str, content: &str) {
        let dir = std::env::temp_dir().join(format!(
            "asrs-live-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let path = dir.join(name);
        std::fs::write(&path, content).expect("a log");
        let mut o = JsObject::new();
        let file = JsValue::string(&path.to_string_lossy());
        o.insert("files", JsValue::Array(vec![file]));
        o.insert("merge", JsValue::Bool(false));
        self.send(MainToHub::OpenFiles, JsValue::Object(o));
        self.files.push(path);
        let title = format!("{name} \u{2014} AdvantageScope");
        self.drive_until("the log to open", |page| page.title() == title);
    }

    fn read<R>(&mut self, f: impl FnOnce(&Hub) -> R) -> R {
        let hub = self.hub();
        self.cx.update(|cx| f(hub.read(cx)))
    }

    fn title(&mut self) -> String {
        self.read(|hub| hub.model().title().to_string_lossy())
    }

    fn take_titles(&self) -> Vec<String> {
        let mut titles = self.titles.take();
        titles.dedup();
        titles
            .iter()
            .map(|t| t.trim_end_matches(" \u{2014} AdvantageScope").to_owned())
            .collect()
    }

    fn log<R>(&mut self, f: impl FnOnce(&Log) -> R) -> R {
        self.read(|hub| f(&hub.model().page().log()))
    }

    fn weak_log(&mut self) -> Weak<RefCell<Log>> {
        self.read(|hub| Rc::downgrade(hub.model().page().shared_log()))
    }

    /// The last value of a number field; NaN when it has none.
    fn number(&mut self, key: &str) -> f64 {
        self.log(|log| get_or_default(log, &js(key), f64::MAX, f64::NAN, None))
    }

    fn mode(&mut self) -> SelectionMode {
        self.read(|hub| hub.model().page().selection().get_mode())
    }

    fn runtime_tasks(&mut self) -> usize {
        self.cx.update(|cx| live::runtime_tasks(cx))
    }

    fn next_live_timer(&mut self) -> Option<f64> {
        self.read(|hub| hub.model().next_live_timer())
    }

    fn frame(&mut self) {
        self.cx
            .update_window(self.window, |_, window, cx| window.simulate_next_frame(cx))
            .expect("the window is open");
        self.cx.run_until_parked();
    }

    fn advance(&mut self, ms: u64) {
        let mut left = Duration::from_millis(ms);
        while !left.is_zero() {
            let step = left.min(STEP);
            left -= step;
            self.cx.advance_clock(step);
            self.cx.run_until_parked();
            if self.hub.is_some() {
                self.frame();
            }
        }
    }

    /// Runs the page until `done`. Time passes only in steps in which no connection posted
    /// anything, and half as fast as on the wall clock.
    fn drive_until(&mut self, what: &str, mut done: impl FnMut(&mut Page) -> bool) {
        let start = Instant::now();
        loop {
            self.cx.run_until_parked();
            if done(self) {
                return;
            }
            let titles = self.titles.borrow().clone();
            assert!(
                start.elapsed() < TIMEOUT,
                "timed out waiting for {what}; titles {titles:?}"
            );
            if !self.read(|hub| hub.wait_for_live(STEP * 2)) {
                self.advance(STEP.as_millis() as u64);
            }
        }
    }

    /// Everything a live connection held is gone.
    fn assert_free(&mut self, server: &LiveServer) {
        assert_eq!(self.next_live_timer(), None, "timers of a stopped source");
        let start = Instant::now();
        while self.runtime_tasks() > 0 || server.connections() > 0 {
            assert!(
                start.elapsed() < Duration::from_secs(4),
                "{} tasks and {} connections of a stopped source",
                self.runtime_tasks(),
                server.connections()
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let accepted = server.accepted();
        self.advance(4000);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(server.accepted(), accepted, "a stopped source reconnected");
        assert_eq!(self.runtime_tasks(), 0);
    }

    fn close(&mut self) {
        self.hub = None;
        self.cx
            .update_window(self.window, |_, window, _| window.remove_window())
            .expect("the window is open");
        self.cx.run_until_parked();
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        for file in &self.files {
            let _ = std::fs::remove_file(file);
        }
        if let Some(dir) = self.files.first().and_then(|f| f.parent()) {
            let _ = std::fs::remove_dir(dir);
        }
        let exceptions = self.exceptions.take();
        if !std::thread::panicking() {
            assert!(exceptions.is_empty(), "{exceptions:?}");
        }
    }
}

fn searching(address: &str) -> String {
    format!("{address} (Searching)")
}

struct Case<'a> {
    protocol: &'a str,
    server: &'a str,
    mode: LiveMode,
    /// The fields made active, as a subscription or a poll needs.
    table: &'a [&'a str],
    /// The step that gives `key` its first value on a connection; without one the server's own
    /// state does.
    first: Option<&'a str>,
    key: &'a str,
    first_value: f64,
    second: &'a str,
    second_value: f64,
}

impl Case<'_> {
    /// Drives the page until a connection's first value arrives.
    fn connect(&self, page: &mut Page, server: &LiveServer, opened: &mut usize, value: f64) {
        page.drive_until("a connection's first value", |page| {
            if server.opened() > *opened {
                *opened = server.opened();
                self.first.iter().for_each(|step| server.step(step));
            }
            page.number(self.key) == value
        });
    }
}

/// Connect, data, the server dies, it comes back, stop.
fn source(case: &Case<'_>) {
    let mut server = LiveServer::new(case.protocol, case.server);
    let mut page = Page::new(&server);
    page.preferences(|p| p.live_mode = case.mode);
    server.down();
    page.start_live("robot");
    page.drive_until("the page to search", |page| {
        page.title().contains("(Searching)")
    });
    page.advance(1500);
    assert_eq!(page.take_titles(), [searching(ADDRESS)]);
    assert!(page.mode() != SelectionMode::Locked);

    server.up();
    if !case.table.is_empty() {
        page.table(case.table);
    }
    let mut opened = server.opened();
    case.connect(&mut page, &server, &mut opened, case.first_value);
    assert_eq!(page.take_titles(), [ADDRESS]);
    assert_eq!(page.mode(), SelectionMode::Locked);
    server.step(case.second);
    page.drive_until("the second value", |page| {
        page.number(case.key) == case.second_value
    });
    assert_eq!(page.take_titles(), Vec::<String>::new());

    let old_log = page.weak_log();
    server.down();
    page.drive_until("the page to search again", |page| {
        page.title().contains("(Searching)")
    });
    page.drive_until("the live selection to end", |page| {
        page.mode() != SelectionMode::Locked
    });
    assert_eq!(page.number(case.key), case.second_value, "the log is kept");
    server.up();
    let again = match case.first {
        Some(_) => case.first_value,
        None => case.second_value,
    };
    page.drive_until("the log to be replaced", |page| {
        if server.opened() > opened {
            opened = server.opened();
            case.first.iter().for_each(|step| server.step(step));
        }
        !page.weak_log().ptr_eq(&old_log)
    });
    case.connect(&mut page, &server, &mut opened, again);
    page.advance(100);
    assert!(
        old_log.upgrade().is_none(),
        "the first connection's log is freed"
    );
    assert_eq!(page.take_titles(), [searching(ADDRESS), ADDRESS.to_owned()]);

    page.open_log("stop.csv", CSV);
    assert_eq!(
        page.take_titles(),
        ["stop.csv (Loading)".to_owned(), "stop.csv".to_owned()]
    );
    assert_eq!(page.number("/Log"), 1.0);
    page.assert_free(&server);
    assert_eq!(page.number("/Log"), 1.0);
    assert_eq!(page.take_titles(), Vec::<String>::new());

    page.start_live("sim");
    case.connect(&mut page, &server, &mut opened, again);
    assert_eq!(page.title(), "127.0.0.1 \u{2014} AdvantageScope");
    page.close();
    let start = Instant::now();
    while page.runtime_tasks() > 0 || server.connections() > 0 {
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "a closed window's connections stay open"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

const TOPICS: &str = r#"{"version":"4.1","topics":[
  {"name":"/Drive/x","type":"double","value":1.5},
  {"name":"/AdvantageKit/Drive/x","type":"double","value":1.5},
  {"name":"/Tuning/kP","type":"double","value":0.25}]}"#;

fn nt4(mode: LiveMode, key: &str, topic: &str, ports: &str) {
    let server = TOPICS.replace(
        "{\"version\"",
        &format!("{{\"ports\":[{ports}],\"version\""),
    );
    let second = format!(r#"["set","{topic}",2.5]"#);
    source(&Case {
        protocol: "nt4",
        server: &server,
        mode,
        table: &[key],
        first: None,
        key,
        first_value: 1.5,
        second: &second,
        second_value: 2.5,
    });
}

#[test]
fn nt4_default() {
    nt4(LiveMode::Nt4, "NT:/Drive/x", "/Drive/x", "5810");
}

#[test]
fn nt4_advantagekit() {
    nt4(
        LiveMode::Nt4AKit,
        "/Drive/x",
        "/AdvantageKit/Drive/x",
        "5810",
    );
}

#[test]
fn nt4_systemcore() {
    nt4(LiveMode::Nt4Systemcore, "NT:/Drive/x", "/Drive/x", "6810");
}

#[test]
fn nt4_driver_station() {
    let server = TOPICS.replace("{\"version\"", "{\"ports\":[6767],\"version\"");
    let server = LiveServer::new("nt4", &server);
    let mut page = Page::new(&server);
    page.preferences(|p| p.live_subscribe_mode = LiveSubscribeMode::Logging);
    page.start_live("ds");
    page.drive_until("the driver station's data", |page| {
        page.number("DS:/Drive/x") == 1.5
    });
    assert_eq!(
        page.take_titles(),
        [searching("127.0.0.1"), "127.0.0.1".to_owned()]
    );
    page.open_log("ds.csv", CSV);
    page.assert_free(&server);
}

#[test]
fn rlog() {
    const FIRST: &str = r#"["send","02003ff0000000000000010000000c2f44726976652f53706565640006646f75626c6502000000083ff8000000000000"]"#;
    const SECOND: &str = r#"["send","003ff800000000000002000000084008000000000000"]"#;
    source(&Case {
        protocol: "rlog",
        server: "{}",
        mode: LiveMode::Rlog,
        table: &[],
        first: Some(FIRST),
        key: "/Drive/Speed",
        first_value: 1.5,
        second: SECOND,
        second_value: 3.0,
    });
}

const PHOENIX: &str = r#"{"busUtil":12.5,"startUs":5000000,"points":1,"stepUs":20000,"devices":[
  {"device":{"CANbus":"rio","CANivoreDevName":"","ID":1,"Model":"Talon FX","Name":"Talon FX (Device ID 1)","IsPROApplication":true},
   "signals":[{"Id":12,"Name":"Position","Summary":"Rotor position","Units":"rotations"}],
   "values":{"12":[VALUE]}}]}"#;

#[test]
fn phoenix() {
    let first = PHOENIX.replace("VALUE", "0.5");
    let devices = |value: &str| {
        let server = PHOENIX.replace("VALUE", value);
        let server = json::parse(&server).expect("a server");
        let devices = server.get("devices").expect("devices").to_compact();
        format!(r#"["devices",{devices}]"#)
    };
    let second = devices("1.25");
    source(&Case {
        protocol: "phoenix",
        server: &first,
        mode: LiveMode::Phoenix,
        table: &["Phoenix6/TalonFX-1/Position"],
        first: None,
        key: "Phoenix6/TalonFX-1/Position",
        first_value: 0.5,
        second: &second,
        second_value: 1.25,
    });
}

#[test]
fn ftc_dashboard() {
    source(&Case {
        protocol: "ftc",
        server: r#"{"status":{"activeOpMode":"Tele","activeOpModeStatus":"RUNNING"}}"#,
        mode: LiveMode::FtcDashboard,
        table: &[],
        first: Some(r#"["send",{"telemetry":[{"data":{"speed":"2.5"}}]}]"#),
        key: "speed",
        first_value: 2.5,
        second: r#"["send",{"telemetry":[{"data":{"speed":"-3"}}]}]"#,
        second_value: -3.0,
    });
}

#[test]
fn live_discard_bounds_the_log() {
    let server = LiveServer::new(
        "ftc",
        r#"{"status":{"activeOpMode":"Tele","activeOpModeStatus":"RUNNING"}}"#,
    );
    let mut page = Page::new(&server);
    page.preferences(|p| {
        p.live_mode = LiveMode::FtcDashboard;
        p.live_discard = 2.0;
    });
    page.start_live("robot");
    page.drive_until("the connection", |_| server.connections() > 0);
    for i in 0..40 {
        server.step(&format!(
            r#"["send",{{"telemetry":[{{"data":{{"n":"{i}"}}}}]}}]"#
        ));
        page.drive_until("a value", |page| page.number("n") == f64::from(i));
        page.advance(250);
    }
    let [start, end] = page.log(|log| log.get_timestamp_range());
    assert!(end - start <= 3.0, "{start} to {end} is kept");
    let kept = page.log(|log| {
        let field = log.get_number(&js("n"), f64::MIN, f64::MAX, None, None);
        field.map_or(0, |f| f.timestamps.len())
    });
    assert!((5..=14).contains(&kept), "{kept} of 40 values are kept");
}

fn sent_text(seen: &[ClientEvent]) -> Vec<Json> {
    seen.iter()
        .filter_map(|e| match e {
            ClientEvent::WsSend {
                data: Payload::Text(text),
                ..
            } => json::parse(text).ok(),
            _ => None,
        })
        .flat_map(|list| list.as_array().map(<[Json]>::to_vec).unwrap_or_default())
        .collect()
}

fn method<'a>(messages: &'a [Json], name: &str) -> Vec<&'a Json> {
    messages
        .iter()
        .filter(|m| {
            m.get("method")
                .and_then(Json::as_str)
                .is_some_and(|s| s.eq_str(name))
        })
        .filter_map(|m| m.get("params"))
        .collect()
}

/// `(topic or publisher uid, type index, value)` of each value the client sent.
fn sent_values(seen: &[ClientEvent]) -> Vec<(f64, f64, Unpacked)> {
    let mut out = Vec::new();
    for event in seen {
        let ClientEvent::WsSend {
            data: Payload::Binary(bytes),
            ..
        } = event
        else {
            continue;
        };
        for item in msgpack::decode_all(bytes) {
            let Unpacked::Arr(items) = item else {
                continue;
            };
            if let [Unpacked::Number(id), _, Unpacked::Number(ty), value] = items.as_slice()
                && *id >= 0.0
            {
                out.push((*id, *ty, value.clone()));
            }
        }
    }
    out
}

#[test]
fn publisher() {
    let mut server = LiveServer::new("nt4", r#"{"version":"4.1"}"#);
    let mut page = Page::new(&server);
    page.preferences(|p| p.publish_filter = js("/Data"));
    page.open_log("nt.csv", NT_CSV);
    page.take_titles();
    page.send(MainToHub::StartPublish, JsValue::Bool(true));
    page.drive_until("the publisher to connect", |page| {
        page.title().contains("(Publishing)")
    });
    assert_eq!(
        page.take_titles(),
        ["nt.csv (Searching)", "nt.csv (Publishing)"]
    );

    let mut seen = Vec::new();
    page.drive_until("the first values", |_| {
        seen.extend(server.take_seen());
        sent_values(&seen).len() >= 2
    });
    page.advance(200);
    seen.extend(server.take_seen());
    let messages = sent_text(&seen);
    let published = method(&messages, "publish");
    let topics: Vec<(String, String)> = published
        .iter()
        .map(|p| {
            let text = |key| {
                p.get(key)
                    .and_then(Json::as_str)
                    .expect("text")
                    .to_string_lossy()
            };
            (text("name"), text("type"))
        })
        .collect();
    assert_eq!(
        topics,
        [
            ("/Data/x".to_owned(), "double".to_owned()),
            ("/Data/flag".to_owned(), "boolean".to_owned())
        ]
    );
    let uid = |i: usize| {
        published[i]
            .get("pubuid")
            .and_then(Json::as_f64)
            .expect("a uid")
    };
    let (x, flag) = (uid(0), uid(1));
    assert_eq!(
        sent_values(&seen),
        [
            (x, 1.0, Unpacked::Number(2.5)),
            (flag, 0.0, Unpacked::Bool(true))
        ],
        "each value is sent once"
    );

    let select = |page: &mut Page, time: f64| {
        let hub = page.hub();
        page.cx.update(|cx| {
            hub.update(cx, |hub, cx| {
                hub.update(cx, |model, _| {
                    model.page_mut().selection_mut().set_selected_time(time)
                })
            })
        });
    };
    select(&mut page, 1.5);
    let mut seen = Vec::new();
    page.drive_until("the value at the selected time", |_| {
        seen.extend(server.take_seen());
        !sent_values(&seen).is_empty()
    });
    assert_eq!(sent_values(&seen), [(x, 1.0, Unpacked::Number(-1.0))]);

    server.down();
    page.drive_until("the publisher to search", |page| {
        page.title().contains("(Searching)")
    });
    server.up();
    let mut seen = Vec::new();
    page.drive_until("the publisher to republish", |page| {
        seen.extend(server.take_seen());
        page.title().contains("(Publishing)") && method(&sent_text(&seen), "publish").len() == 2
    });
    assert_eq!(
        page.take_titles(),
        ["nt.csv (Searching)", "nt.csv (Publishing)"]
    );

    page.send(MainToHub::StopPublish, JsValue::Undefined);
    page.cx.run_until_parked();
    assert_eq!(page.take_titles(), ["nt.csv"]);
    page.assert_free(&server);

    page.send(MainToHub::StartPublish, JsValue::Bool(true));
    page.drive_until("the publisher to connect again", |page| {
        page.title().contains("(Publishing)")
    });
    page.preferences(|_| {});
    page.start_live("sim");
    page.send(MainToHub::StartPublish, JsValue::Bool(true));
    page.cx.run_until_parked();
    let refused = page.main.take().into_iter().any(|m| {
        m.name == HubToMain::Error
            && m.data.get("title").and_then(JsValue::as_str) == Some(&js("Cannot publish"))
    });
    assert!(refused, "publishing from a live source is refused");
    page.drive_until("the live source to connect", |page| {
        page.title() == "127.0.0.1 \u{2014} AdvantageScope"
    });
    page.open_log("end.csv", CSV);
    page.assert_free(&server);
}

#[test]
fn rlog_heartbeats_reach_the_server() {
    let server = LiveServer::new("rlog", "{}");
    let mut page = Page::new(&server);
    page.preferences(|p| p.live_mode = LiveMode::Rlog);
    page.start_live("robot");
    page.drive_until("the connection", |_| server.connections() > 0);
    page.advance(2000);
    let mut bytes = Vec::new();
    page.drive_until("four heartbeats", |_| {
        for event in server.take_seen() {
            if let ClientEvent::TcpSend { data, .. } = event {
                bytes.extend(data);
            }
        }
        bytes.len() >= 16
    });
    assert_eq!(&bytes[..16], [6, 3, 2, 8].repeat(4));
}

#[test]
fn a_flooding_server_leaves_the_page_responsive() {
    const MESSAGES: usize = 6000;
    let server = LiveServer::new(
        "ftc",
        r#"{"status":{"activeOpMode":"Tele","activeOpModeStatus":"RUNNING"}}"#,
    );
    let mut page = Page::new(&server);
    page.preferences(|p| p.live_mode = LiveMode::FtcDashboard);
    page.start_live("robot");
    page.drive_until("the connection", |page| {
        page.title() == format!("{ADDRESS} \u{2014} AdvantageScope")
    });
    for i in 0..MESSAGES {
        server.step(&format!(
            r#"["send",{{"telemetry":[{{"data":{{"n":"{i}","a":"{i}.5"}}}}]}}]"#
        ));
    }
    page.read(|hub| hub.wait_for_live(Duration::from_secs(10)));
    let mut slices = 0;
    let mut longest = Duration::ZERO;
    let start = Instant::now();
    let last = (MESSAGES - 1) as f64;
    while page.number("n") != last {
        assert!(start.elapsed() < TIMEOUT, "the flood never ends");
        let slice = Instant::now();
        page.cx.run_until_parked();
        longest = longest.max(slice.elapsed());
        slices += 1;
        let focused = slices % 2 == 0;
        page.send(MainToHub::SetFocused, JsValue::Bool(focused));
        assert_eq!(page.read(|hub| hub.model().is_focused()), focused);
        page.cx.advance_clock(Duration::from_millis(1));
    }
    assert!(slices > 3, "{MESSAGES} messages ran in {slices} slices");
    assert!(
        longest < Duration::from_millis(250),
        "the foreground was busy for {longest:?}"
    );
}
