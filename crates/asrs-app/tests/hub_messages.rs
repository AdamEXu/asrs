#![allow(clippy::disallowed_methods)]

//! The messages between main and its windows in the app, headless: `main_process::start` in a
//! fresh home directory, with the real hub page, its host and `MainProcess`. The dialogs, links,
//! focus and battery sends are `files/hub-messages`' (oracle/src/files/hubMessages.ts, upstream's
//! main.ts), and what the page itself sends must be one of the messages recorded there.

#[path = "support/child.rs"]
mod child;

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

#[path = "support/settle.rs"]
mod settle;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::rc::Rc;
use std::sync::Arc;

use asrs_app::hub::{Hub, HubEvent};
use asrs_app::main_process::{self, BetaTime, MainProcess};
use asrs_app::views::field3d::Field3dTab;
use asrs_app::windows::{self, MessageBoxOptions, MessageBoxType, WindowId, Windows};
use asrs_hub::hub::HubModel;
use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_model::json_file;
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage};
use asrs_model::preferences::{Field3dMode, Preferences};
use asrs_model::tab_type::TabType;
use asrs_parity::cve1::Value;
use asrs_parity::json::Json;
use asrs_parity::vectors::{self, Record};
use asrs_services::lifecycle;
use asrs_services::window_manager::Platform;
use asrs_ui::gpui::{
    AnyWindowHandle, Entity, HeadlessAppContext, Modifiers, MouseButton, MouseDownEvent,
    MouseUpEvent, NoopTextSystem, PlatformInput, TouchEvent, TouchId, TouchPhase, point, px,
};
use asrs_ui::parity::Recorder;
use asrs_ui::platform::dialog::{Reply, Script, Shown};
use asrs_ui::platform::power::ScriptedPower;

const HOME: &str = "ASRS_HUB_MESSAGES_HOME";
const SOURCE_URL: &str = "https://example.com/field?a=b#c";

fn run_in_a_fresh_home(test: &str) {
    let home = std::env::temp_dir().join(format!("asrs-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("home");
    let mut child = Process::new(std::env::current_exe().expect("the test binary"));
    child.args(["--exact", test, "--ignored", "--nocapture"]);
    for variable in [
        "HOME",
        "APPDATA",
        "XDG_CONFIG_HOME",
        "TMPDIR",
        "TEMP",
        "TMP",
        HOME,
    ] {
        child.env(variable, &home);
    }
    let status = child::run(&mut child, test);
    let _ = std::fs::remove_dir_all(&home);
    assert!(status.success(), "{test}: {status}");
}

#[test]
fn main_answers_as_upstream() {
    run_in_a_fresh_home("the_vectors");
}

#[test]
fn field_2d_warns_and_opens_its_source() {
    run_in_a_fresh_home("field_2d");
}

#[test]
fn field_3d_warns_and_follows_the_battery() {
    run_in_a_fresh_home("field_3d");
}

#[test]
fn the_type_conflict_warning_shows_its_box() {
    run_in_a_fresh_home("type_conflict");
}

#[test]
fn the_touch_bar_has_no_timer() {
    run_in_a_fresh_home("touch_bar");
}

#[test]
fn a_touch_drags_a_field() {
    run_in_a_fresh_home("touch_drag");
}

fn records() -> Vec<Record> {
    vectors::load("files/hub-messages").unwrap_or_else(|e| panic!("{e}"))
}

fn js(value: &Value) -> JsValue {
    js_json::to_js(value).unwrap_or_else(|e| panic!("{e}: {value:?}"))
}

fn value(record: &Record, key: &str) -> JsValue {
    js(&record
        .value(key)
        .unwrap_or_else(|e| panic!("line {}: {e}", record.line)))
}

fn id(record: &Record, key: &str) -> Option<WindowId> {
    record
        .field(key)
        .and_then(Json::as_f64)
        .map(|id| id as WindowId)
}

fn text(value: &JsValue, key: &str) -> String {
    let text = value.get(key).and_then(JsValue::as_str);
    text.unwrap_or_else(|| panic!("no {key} in {value:?}"))
        .to_string_lossy()
}

fn bytes(value: &JsValue) -> Vec<u8> {
    match value {
        JsValue::Bytes(bytes) => bytes.to_vec(),
        other => panic!("not bytes: {other:?}"),
    }
}

fn list(value: JsValue) -> Vec<JsValue> {
    value.as_array().expect("an array").to_vec()
}

/// A recorded `showMessageBox` call's options.
fn message_box(options: &JsValue) -> MessageBoxOptions {
    let buttons = options.get("buttons").and_then(JsValue::as_array);
    let label = options.get("checkboxLabel").and_then(JsValue::as_str);
    MessageBoxOptions {
        kind: match text(options, "type").as_str() {
            "info" => MessageBoxType::Info,
            "error" => MessageBoxType::Error,
            other => panic!("{other}"),
        },
        title: text(options, "title"),
        message: text(options, "message"),
        detail: text(options, "detail"),
        buttons: buttons
            .into_iter()
            .flatten()
            .map(|b| b.as_str().expect("a label").to_string_lossy())
            .collect(),
        checkbox_label: label.map_or(String::new(), |l| l.to_string_lossy()),
        ..MessageBoxOptions::default()
    }
}

/// The box main shows for the hub message `name`, and the data that message has.
fn recorded_box(name: &str) -> (MessageBoxOptions, JsValue) {
    let record = records()
        .into_iter()
        .find(|r| r.string("name").as_deref() == Some(name))
        .expect(name);
    let dialog = &list(value(&record, "dialogs"))[0];
    let options = dialog.get("options").expect("options");
    (message_box(options), value(&record, "data"))
}

struct AppUnderTest {
    cx: HeadlessAppContext,
    user_data: PathBuf,
}

impl AppUnderTest {
    /// `None` outside the child process. `prepare` runs on the user-data directory first.
    fn start(prepare: impl FnOnce(&Path)) -> Option<AppUnderTest> {
        std::env::var_os(HOME)?;
        let user_data =
            lifecycle::user_data_dir(Platform::current(), |name| std::env::var(name).ok());
        let user_data = PathBuf::from(user_data.expect("a user-data directory"));
        std::fs::create_dir_all(&user_data).expect("user data");
        std::fs::write(
            user_data.join("beta-2027.json"),
            r#"{"welcomeComplete":true,"surveyStatus":true}"#,
        )
        .expect("beta-2027.json");
        prepare(&user_data);
        let mut cx = HeadlessAppContext::new(Arc::new(NoopTextSystem::new()));
        let (_send, events) = async_channel::unbounded();
        cx.update(|cx| {
            asrs_ui::init(cx);
            windows::init(cx);
            cx.set_global(Script::default());
            cx.set_global(BetaTime(0.0));
            cx.set_global(ScriptedPower { on_battery: false });
            main_process::start(cx, &[], events);
        });
        let mut app = AppUnderTest { cx, user_data };
        app.settle();
        Some(app)
    }

    fn settle(&mut self) {
        settle::settle(&mut self.cx);
    }

    fn prefs_path(&self) -> PathBuf {
        self.user_data.join("prefs.json")
    }

    fn pref(&self, key: &str) -> Option<JsValue> {
        let bytes = std::fs::read(self.prefs_path()).expect("prefs.json");
        let prefs = json_file::parse(&bytes).expect("prefs.json");
        prefs.get(key).cloned()
    }

    fn hubs(&mut self) -> Vec<WindowId> {
        self.cx.update(|cx| Windows::manager(cx).hubs().to_vec())
    }

    fn only_hub(&mut self) -> WindowId {
        let hubs = self.hubs();
        let [hub] = hubs[..] else {
            panic!("{hubs:?}");
        };
        hub
    }

    fn handle(&mut self, window: WindowId) -> AnyWindowHandle {
        let handle = self.cx.update(|cx| Windows::handle(cx, window));
        handle.expect("open")
    }

    fn entity(&mut self, window: WindowId) -> Entity<Hub> {
        let hubs = self.cx.update(|cx| MainProcess::hub_windows(cx));
        let hub = hubs.iter().find(|(id, _)| *id == window).expect("a hub");
        hub.1.upgrade().expect("a live hub")
    }

    fn page<R>(&mut self, window: WindowId, act: impl FnOnce(&mut HubModel) -> R) -> R {
        let hub = self.entity(window);
        self.cx
            .update(|cx| hub.update(cx, |hub, cx| hub.update(cx, |model, _| act(model))))
    }

    /// `window.sendMainMessage` in a hub window's page.
    fn send_main_message(&mut self, window: WindowId, message: NamedMessage<HubToMain>) {
        let hub = self.entity(window);
        self.cx.update(|cx| {
            hub.update(cx, |hub, cx| {
                hub.update(cx, |_, host| host.send_main_message(message))
            });
        });
    }

    /// The messages the window's page sends main from here on.
    fn record_messages(&mut self, window: WindowId) -> Rc<RefCell<Vec<NamedMessage<HubToMain>>>> {
        let sent = Rc::new(RefCell::new(Vec::new()));
        let hub = self.entity(window);
        let log = sent.clone();
        self.cx.update(|cx| {
            cx.subscribe(&hub, move |_, event, _| {
                if let HubEvent::Main(message) = event {
                    log.borrow_mut().push(message.clone());
                }
            })
            .detach();
        });
        sent
    }

    fn restore(&mut self, window: WindowId, tab: &str) {
        let state = format!(
            r#"{{"sidebar":{{"width":300,"expanded":[]}},"tabs":{{"selected":1,"tabs":[{{"type":0,"title":"","controller":null,"controllerUUID":"documentation","renderer":"","controlsHeight":0}},{tab}]}}}}"#
        );
        let state = asrs_js::json::parse_str(&state).expect("a state");
        let hub = self.entity(window);
        self.cx.update(|cx| {
            hub.update(cx, |hub, cx| {
                hub.handle_main_message(NamedMessage::new(MainToHub::RestoreState, state), cx)
            });
        });
        self.settle();
    }

    fn frames(&mut self, window: WindowId, count: usize) {
        let handle = self.handle(window);
        for _ in 0..count {
            self.cx
                .update_window(handle, |_, window, cx| window.simulate_next_frame(cx))
                .expect("open");
            self.settle();
        }
    }

    fn focus(&mut self, window: WindowId) {
        let handle = self.handle(window);
        self.cx
            .update_window(handle, |_, window, _| window.activate_window())
            .expect("open");
        self.settle();
    }

    /// The computer's power changes, seen from inside an update of `window`, which tells the app.
    fn set_power(&mut self, window: WindowId, on_battery: bool) {
        let handle = self.handle(window);
        self.cx
            .update_window(handle, |_, _, cx| {
                cx.set_global(ScriptedPower { on_battery });
            })
            .expect("open");
        self.settle();
    }

    fn dispatch(&mut self, window: WindowId, inputs: Vec<PlatformInput>) {
        let handle = self.handle(window);
        self.cx
            .update_window(handle, |_, window, _| window.refresh())
            .expect("open");
        self.settle();
        self.cx
            .update_window(handle, |_, window, cx| {
                for input in inputs {
                    window.dispatch_event(input, cx);
                }
            })
            .expect("open");
        self.settle();
    }

    fn click_at(&mut self, window: WindowId, x: f64, y: f64) {
        let position = point(px(x as f32), px(y as f32));
        let (button, modifiers) = (MouseButton::Left, Modifiers::default());
        self.dispatch(
            window,
            vec![
                PlatformInput::MouseDown(MouseDownEvent {
                    button,
                    position,
                    modifiers,
                    click_count: 1,
                    first_mouse: false,
                }),
                PlatformInput::MouseUp(MouseUpEvent {
                    button,
                    position,
                    modifiers,
                    click_count: 1,
                }),
            ],
        );
    }

    fn touch(&mut self, window: WindowId, phase: TouchPhase, x: f64, y: f64) {
        self.dispatch(
            window,
            vec![PlatformInput::Touch(TouchEvent {
                id: TouchId(1),
                phase,
                position: point(px(x as f32), px(y as f32)),
                ..TouchEvent::default()
            })],
        );
    }

    /// The boxes of the text `text` in the order the window paints them.
    fn text_boxes(&mut self, window: WindowId, text: &str) -> Vec<[f64; 4]> {
        self.cx.update(|cx| cx.set_global(Recorder::default()));
        let handle = self.handle(window);
        self.cx
            .update_window(handle, |_, window, _| window.refresh())
            .expect("open");
        self.settle();
        let recorder = self.cx.update(|cx| cx.remove_global::<Recorder>());
        let runs = recorder.texts.iter().filter(|run| run.text == text);
        runs.map(|run| run.rects[0]).collect()
    }

    fn text_box(&mut self, window: WindowId, text: &str) -> [f64; 4] {
        let boxes = self.text_boxes(window, text);
        let [found] = boxes[..] else {
            panic!("{text:?}: {boxes:?}");
        };
        found
    }

    /// Where `#dragItem` shows the field `text`, over its label.
    fn drag(&mut self, window: WindowId, text: &str) -> Option<[f64; 2]> {
        let boxes = self.text_boxes(window, text);
        boxes.get(1).map(|[x, y, _, _]| [*x, *y])
    }

    fn reply(&mut self, replies: &[Reply]) {
        self.cx
            .update(|cx| cx.global_mut::<Script>().replies = replies.to_vec().into());
    }

    fn shown(&mut self) -> Vec<(Option<WindowId>, Shown)> {
        self.cx.update(|cx| {
            let script = cx.global_mut::<Script>();
            let left = std::mem::take(&mut script.replies);
            assert!(left.is_empty(), "never asked for {left:?}");
            let shown = std::mem::take(&mut script.shown);
            shown
                .into_iter()
                .map(|(window, shown)| {
                    let id = window.map(|window| Windows::id_of(cx, window).expect("a window"));
                    (id, shown)
                })
                .collect()
        })
    }

    fn opened_urls(&mut self) -> Vec<String> {
        self.cx
            .update(|cx| std::mem::take(&mut cx.global_mut::<Script>().opened_urls))
    }
}

/// What main has told a window.
#[derive(Clone, Debug, Default, PartialEq)]
struct Told {
    focused: Option<bool>,
    battery: Option<bool>,
    preferences: Option<Preferences>,
}

#[test]
#[ignore = "run by main_answers_as_upstream with its own home directory"]
fn the_vectors() {
    let Some(mut app) = AppUnderTest::start(|_| {}) else {
        return;
    };
    let mut windows: HashMap<WindowId, WindowId> = HashMap::new();
    let mut satellites: Vec<WindowId> = Vec::new();
    let mut told: HashMap<WindowId, Told> = HashMap::new();
    let first = app.only_hub();
    let records = records();
    let [config, start, records @ ..] = &records[..] else {
        panic!("{} records", records.len());
    };
    assert_eq!(config.string("op").as_deref(), Some("config"));
    assert_eq!(start.string("op").as_deref(), Some("start"));
    std::fs::write(app.prefs_path(), bytes(&value(start, "prefs"))).expect("prefs");
    app.cx.update(MainProcess::send_all_preferences);
    app.settle();
    let mut tally = [0; 6];
    for record in records {
        let op = record.string("op").expect("op");
        let at = format!("line {} {op}", record.line);
        let kinds = ["open", "message", "focus", "power", "destroy", "setPrefs"];
        let kind = kinds.iter().position(|kind| *kind == op);
        tally[kind.unwrap_or_else(|| panic!("{at}"))] += 1;
        if op == "setPrefs" {
            let text = record.string("text").expect("text");
            std::fs::write(app.prefs_path(), text).expect("prefs");
            continue;
        }
        for sent in list(value(record, "sent")) {
            let to = sent.get("to").and_then(JsValue::as_f64).expect("to") as WindowId;
            let message = sent.get("message").expect("message");
            let data = message.get("data").expect("data");
            let told = told.entry(to).or_default();
            match text(message, "name").as_str() {
                "set-focused" => told.focused = Some(data.as_bool().expect("a boolean")),
                "set-battery" => told.battery = Some(data.as_bool().expect("a boolean")),
                "set-preferences" => told.preferences = Some(Preferences::from_js(data)),
                other => panic!("{at}: {other}"),
            }
        }
        let window = id(record, "window");
        let mut replies = Vec::new();
        let mut expected = Vec::new();
        for dialog in list(value(record, "dialogs")) {
            let parent = dialog.get("window").and_then(JsValue::as_f64);
            let parent = windows[&(parent.expect("a parent") as WindowId)];
            let options = message_box(dialog.get("options").expect("options"));
            expected.push((Some(parent), Shown::Message(options)));
            let checked = dialog.get("checkboxChecked").and_then(JsValue::as_bool);
            replies.push(match checked {
                Some(true) => Reply::Checked(0),
                _ => Reply::Button(0),
            });
        }
        app.reply(&replies);
        let before = std::fs::read(app.prefs_path()).expect("prefs.json");
        match op.as_str() {
            "open" => {
                let vector = window.expect("window");
                let kind = record.string("kind").expect("kind");
                let opened = if vector == 1 {
                    first
                } else if kind == "hub" {
                    let opened = app
                        .cx
                        .update(|cx| MainProcess::open_hub(cx, None, Vec::new()));
                    opened.expect("a hub window")
                } else {
                    let data = asrs_js::json::parse_str(r#"{"uuid":"satellite-uuid","type":2}"#);
                    let data = data.expect("JSON");
                    let message = NamedMessage::new(HubToMain::CreateSatellite, data);
                    app.send_main_message(first, message);
                    app.settle();
                    let pages = app.cx.update(|cx| MainProcess::satellite_windows(cx));
                    let opened = pages.last().expect("a satellite").0;
                    satellites.push(opened);
                    opened
                };
                windows.insert(vector, opened);
            }
            "message" => {
                let name = record.string("name").expect("name");
                let name = HubToMain::from_name(&name).expect("a hub message");
                let from = windows[&window.expect("window")];
                app.send_main_message(from, NamedMessage::new(name, value(record, "data")));
                let during = value(record, "during");
                if !during.is_null() {
                    let name = HubToMain::from_name(&text(&during, "name")).expect("a message");
                    let data = during.get("data").cloned().expect("data");
                    app.send_main_message(from, NamedMessage::new(name, data));
                }
            }
            "focus" => app.focus(windows[&window.expect("window")]),
            "power" => {
                let on_battery = record.string("name").as_deref() == Some("on-battery");
                app.set_power(first, on_battery);
            }
            "destroy" => {
                let vector = window.expect("window");
                let closed = windows.remove(&vector).expect("a window");
                satellites.retain(|id| *id != closed);
                app.cx.update(|cx| windows::close(cx, closed));
            }
            other => panic!("{at}: {other}"),
        }
        app.settle();

        assert_eq!(app.shown(), expected, "{at}: dialogs");
        let external: Vec<String> = list(value(record, "external"))
            .iter()
            .map(|url| url.as_str().expect("a URL").to_string_lossy())
            .collect();
        assert_eq!(app.opened_urls(), external, "{at}: links");
        let after = std::fs::read(app.prefs_path()).expect("prefs.json");
        match value(record, "prefs") {
            JsValue::Null => assert_eq!(after, before, "{at}: prefs.json"),
            written => assert_eq!(
                String::from_utf8_lossy(&after),
                String::from_utf8_lossy(&bytes(&written)),
                "{at}: prefs.json"
            ),
        }
        for (vector, open) in &windows {
            let told = &told[vector];
            let satellite = satellites.contains(open);
            let read =
                |page: &HubModel| (page.page().is_battery(), page.page().preferences().cloned());
            let (battery, preferences) = if satellite {
                let pages = app.cx.update(|cx| MainProcess::satellite_windows(cx));
                let page = pages.into_iter().find(|(id, _)| id == open);
                let (_, page) = page.expect("a satellite");
                app.cx.update(|cx| read(page.read(cx).model().hub()))
            } else {
                app.page(*open, |hub| read(hub))
            };
            assert_eq!(
                (Some(battery), &preferences),
                (told.battery, &told.preferences),
                "{at}: window {vector}"
            );
            // Windows take the focus as they open here, which the recorded ones do not.
            if op == "focus" && !satellite {
                let focused = app.page(*open, |hub| hub.is_focused());
                assert_eq!(
                    focused,
                    told.focused.unwrap_or(true),
                    "{at}: window {vector}'s focus"
                );
            }
        }
    }
    assert_eq!(tally, [4, 17, 6, 4, 2, 6]);
}

/// A 1 x 1 image.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

const FIELDS: [(&str, &str); 2] = [("FRC", "wall-blue"), ("FTC", "center-red")];

/// A field of each kind whose coordinate system main warns about.
fn write_fields(user_data: &Path) {
    for (kind, coordinates) in FIELDS {
        let folder = user_data.join("userAssets").join(format!("Field2d_{kind}"));
        std::fs::create_dir_all(&folder).expect("an asset folder");
        let config = format!(
            r#"{{"name":"Test","isFTC":{},"coordinateSystem":"{coordinates}","sourceUrl":"{SOURCE_URL}","topLeft":[0,0],"bottomRight":[1,1],"widthInches":144,"heightInches":144}}"#,
            kind == "FTC"
        );
        std::fs::write(folder.join("config.json"), config).expect("config.json");
        std::fs::write(folder.join("image.png"), PNG).expect("image.png");
    }
}

fn warning(kind: &str) -> &'static str {
    match kind {
        "FRC" => "frc-coordinate-system-warning",
        _ => "ftc-coordinate-system-warning",
    }
}

/// The messages among `sent` that `files/hub-messages` has.
fn covered(sent: &RefCell<Vec<NamedMessage<HubToMain>>>) -> Vec<(&'static str, JsValue)> {
    let sent = std::mem::take(&mut *sent.borrow_mut());
    sent.into_iter()
        .filter(|m| {
            matches!(
                m.name,
                HubToMain::Alert
                    | HubToMain::Error
                    | HubToMain::OpenLink
                    | HubToMain::FrcCoordinateSystemWarning
                    | HubToMain::FtcCoordinateSystemWarning
            )
        })
        .map(|m| (m.name.name(), m.data))
        .collect()
}

fn field_2d_tab(field: &str) -> String {
    format!(
        r#"{{"type":2,"title":"Field","controller":{{"sources":[],"field":"{field}","orientation":0,"size":"large"}},"controllerUUID":"tab","renderer":null,"controlsHeight":200}}"#
    )
}

#[test]
#[ignore = "run by field_2d_warns_and_opens_its_source with its own home directory"]
fn field_2d() {
    let Some(mut app) = AppUnderTest::start(write_fields) else {
        return;
    };
    let hub = app.only_hub();
    let sent = app.record_messages(hub);
    for (kind, _) in FIELDS {
        let (options, data) = recorded_box(warning(kind));
        app.reply(&[Reply::Button(0)]);
        app.restore(hub, &field_2d_tab(&format!("{kind}:Test")));
        app.frames(hub, 3);
        assert_eq!(app.shown(), [(Some(hub), Shown::Message(options))]);
        assert_eq!(covered(&sent), [(warning(kind), data)]);
    }

    // "Don't Show Again" lasts.
    let (options, _) = recorded_box(warning("FRC"));
    app.reply(&[Reply::Checked(0)]);
    app.restore(hub, &field_2d_tab("FRC:Test"));
    app.frames(hub, 3);
    assert_eq!(app.shown(), [(Some(hub), Shown::Message(options))]);
    let skip = app.pref("skipFRCCoordinateSystemWarning");
    assert_eq!(skip, Some(JsValue::Bool(true)));
    for (kind, _) in FIELDS {
        let shown: &[Reply] = if kind == "FTC" {
            &[Reply::Button(0)]
        } else {
            &[]
        };
        app.reply(shown);
        app.restore(hub, &field_2d_tab(&format!("{kind}:Test")));
        app.frames(hub, 3);
        assert_eq!(app.shown().len(), shown.len(), "{kind}");
    }
    sent.borrow_mut().clear();

    let [x, y, width, height] = app.text_box(hub, "Source");
    app.click_at(hub, x + width / 2.0, y + height / 2.0);
    assert_eq!(app.opened_urls(), [SOURCE_URL]);
    let link = ("open-link", JsValue::string(SOURCE_URL));
    assert_eq!(covered(&sent), [link]);
    for url in [
        "file:///Applications/Calculator.app",
        "javascript:alert(1)",
        "vscode://file/etc/passwd",
        r"C:\Windows\System32\calc.exe",
        "example.com",
        "",
    ] {
        let link = NamedMessage::new(HubToMain::OpenLink, JsValue::string(url));
        app.send_main_message(hub, link);
        app.settle();
        assert_eq!(app.opened_urls(), [""; 0], "{url}");
    }
    let none = NamedMessage::new(HubToMain::OpenLink, JsValue::Undefined);
    app.send_main_message(hub, none);
    app.settle();
    assert_eq!(app.opened_urls(), [""; 0]);
}

#[test]
#[ignore = "run by the_touch_bar_has_no_timer with its own home directory"]
fn touch_bar() {
    let Some(mut app) = AppUnderTest::start(|_| {}) else {
        return;
    };
    let hub = app.only_hub();
    let sent = app.record_messages(hub);
    let executor = app.cx.update(|cx| cx.background_executor().clone());
    for _ in 0..4 {
        executor.advance_clock(std::time::Duration::from_millis(250));
        app.settle();
    }
    let names: Vec<HubToMain> = sent.borrow().iter().map(|m| m.name).collect();
    assert!(
        names.contains(&HubToMain::SaveState),
        "timers run: {names:?}"
    );
    assert!(!names.contains(&HubToMain::UpdateTouchBarSlider));
}

fn field_3d_mode(hub: &HubModel) -> Option<Field3dMode> {
    let mut tabs = hub.tabs().tabs();
    let tab = tabs.find(|tab| tab.tab_type() == Some(TabType::Field3d))?;
    let tab = tab.renderer().downcast_ref::<Field3dTab>()?;
    Some(tab.renderer().implementation()?.mode())
}

#[test]
#[ignore = "run by field_3d_warns_and_follows_the_battery with its own home directory"]
fn field_3d() {
    let Some(mut app) = AppUnderTest::start(|_| {}) else {
        return;
    };
    let hub = app.only_hub();
    let sent = app.record_messages(hub);
    let modes = r#"{"field3dModeAc":"standard","field3dModeBattery":"low-power"}"#;
    let modes = asrs_js::json::parse_str(modes).expect("JSON");
    app.send_main_message(hub, NamedMessage::new(HubToMain::UpdatePreferences, modes));
    app.settle();

    let (options, data) = recorded_box(warning("FTC"));
    app.reply(&[Reply::Button(0)]);
    app.restore(
        hub,
        r#"{"type":3,"title":"Field","controller":{"sources":[],"game":"FTC:Evergreen"},"controllerUUID":"tab","renderer":null,"controlsHeight":200}"#,
    );
    app.frames(hub, 3);
    assert_eq!(app.shown(), [(Some(hub), Shown::Message(options))]);
    assert_eq!(covered(&sent), [(warning("FTC"), data)]);

    for (on_battery, mode) in [
        (false, Field3dMode::Standard),
        (true, Field3dMode::LowPower),
        (false, Field3dMode::Standard),
    ] {
        app.set_power(hub, on_battery);
        app.frames(hub, 2);
        let drawn = app.page(hub, |hub| field_3d_mode(hub));
        assert_eq!(drawn, Some(mode), "on battery: {on_battery}");
    }
}

/// A field of the log, shown in the sidebar.
fn put_field(app: &mut AppUnderTest, hub: WindowId, conflict: bool) {
    app.page(hub, |hub| {
        {
            let mut log = hub.page().shared_log().borrow_mut();
            let key = JsString::from("/Field");
            log.put_number(&key, 0.0, 1.0);
            if conflict {
                log.put_string(&key, 1.0, JsString::from("text"));
            }
        }
        hub.with_sidebar(|s, env| s.refresh(env, true))
            .expect("refresh");
    });
    app.settle();
    app.frames(hub, 2);
}

#[test]
#[ignore = "run by the_type_conflict_warning_shows_its_box with its own home directory"]
fn type_conflict() {
    let Some(mut app) = AppUnderTest::start(|_| {}) else {
        return;
    };
    let hub = app.only_hub();
    let sent = app.record_messages(hub);
    put_field(&mut app, hub, true);
    let (options, data) = recorded_box("alert");
    app.reply(&[Reply::Button(0)]);
    let [x, y, width, height] = app.text_box(hub, " \u{26a0}");
    app.click_at(hub, x + width / 2.0, y + height / 2.0);
    assert_eq!(app.shown(), [(Some(hub), Shown::Message(options))]);
    assert_eq!(covered(&sent), [("alert", data)]);
}

#[test]
#[ignore = "run by a_touch_drags_a_field with its own home directory"]
fn touch_drag() {
    let Some(mut app) = AppUnderTest::start(|_| {}) else {
        return;
    };
    let hub = app.only_hub();
    app.restore(
        hub,
        r#"{"type":1,"title":"Line Graph","controller":null,"controllerUUID":"tab","renderer":null,"controlsHeight":200}"#,
    );
    put_field(&mut app, hub, false);
    let [x, y, width, height] = app.text_box(hub, "Field");
    let start = [x + width / 2.0, y + height / 2.0];
    app.touch(hub, TouchPhase::Started, start[0], start[1]);
    let drag = app.drag(hub, "Field").expect("a drag from the first touch");
    // `touch.clientX - label.getBoundingClientRect().x`, off the touch again: the label's corner.
    assert_eq!(drag, [x, y]);
    app.touch(hub, TouchPhase::Moved, start[0] + 40.0, start[1] + 30.0);
    let moved = app.drag(hub, "Field").expect("the drag");
    assert_eq!(moved, [x + 40.0, y + 30.0]);

    let items = |hub: &mut HubModel| {
        let tab = hub.tabs().tab(1).expect("the line graph");
        let lists = tab.controller().source_lists();
        lists.iter().map(|list| list.items().count()).sum::<usize>()
    };
    assert_eq!(app.page(hub, items), 0);
    let bounds = app.cx.update(|cx| Windows::manager(cx).bounds(hub));
    let bounds = bounds.expect("bounds");
    let list = [300.0 + (bounds.width - 300.0) / 6.0, bounds.height - 60.0];
    app.touch(hub, TouchPhase::Moved, list[0], list[1]);
    app.touch(hub, TouchPhase::Ended, list[0], list[1]);
    assert_eq!(app.drag(hub, "Field"), None);
    assert_eq!(app.page(hub, items), 1, "dropped into the list");
}
