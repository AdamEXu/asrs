#![allow(clippy::disallowed_methods)]

//! Replays `windows/*` vectors against `asrs_services::window_manager`. Every window main.ts
//! constructs is compared on its id and its `BrowserWindow` options, the focused window after
//! every step, and the preference broadcast order (hubs most recently focused first, then
//! satellites, then the download window) at every `order` step.

use asrs_js::ops;
use asrs_js::time::{FixedZone, SetClock};
use asrs_js::value::JsValue;
use asrs_parity::cve1::Value;
use asrs_parity::vectors::{self, Record};
use asrs_services::beta;
use asrs_services::constants;
use asrs_services::fs::MemFs;
use asrs_services::path::PathStyle;
use asrs_services::state_tracker;
use asrs_services::window_manager::{
    OptionValue, Platform, SatellitePlacement, WindowId, WindowKind, WindowManager, WindowOptions,
};
use asrs_services::windows::{Rect, WindowSystem};

const USER_DATA: &str = "/fake/userData";

struct OneDisplay;

impl WindowSystem for OneDisplay {
    fn window_bounds(&self, _: f64) -> Option<Rect> {
        None
    }

    fn displays(&self) -> Vec<Rect> {
        vec![self.primary_display()]
    }

    fn primary_display(&self) -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        }
    }
}

struct Replay {
    windows: WindowManager,
    fs: MemFs,
    state_file: String,
    created: Vec<(WindowId, WindowOptions)>,
    compared: [usize; 3],
}

fn number(value: &Value) -> f64 {
    value.as_f64().expect("number")
}

fn string(value: &Value) -> String {
    value.as_str().expect("string").to_string_lossy()
}

fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    value
        .get(key)
        .unwrap_or_else(|| panic!("no {key:?} in {value:?}"))
}

fn rect_of(value: &JsValue) -> Rect {
    let get = |key: &str| {
        ops::get(value, key)
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or_else(|| panic!("saved window has no numeric {key}"))
    };
    Rect {
        x: get("x"),
        y: get("y"),
        width: get("width"),
        height: get("height"),
    }
}

fn option_value(value: &OptionValue) -> Value {
    match value {
        OptionValue::Undefined => Value::Undefined,
        OptionValue::Bool(b) => Value::Bool(*b),
        OptionValue::Number(n) => Value::number(*n),
        OptionValue::String(s) => Value::string(s),
        OptionValue::Window(id) => Value::number(*id as f64),
    }
}

fn sorted_pairs(value: &Value) -> Vec<(String, Value)> {
    let Value::Object(pairs) = value else {
        panic!("options are not an object: {value:?}");
    };
    let mut out: Vec<(String, Value)> = pairs
        .iter()
        .map(|(k, v)| (k.to_string_lossy(), v.clone()))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn port_pairs(options: &WindowOptions) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = options
        .electron_entries()
        .iter()
        .map(|(k, v)| ((*k).to_owned(), option_value(v)))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

impl Replay {
    fn new() -> Self {
        Self {
            windows: WindowManager::for_platform(Platform::Mac),
            fs: MemFs::new(),
            state_file: String::new(),
            created: Vec::new(),
            compared: [0; 3],
        }
    }

    /// `FakeBrowserWindow.getBounds()` right after construction: the options' rectangle with
    /// missing coordinates at 0.
    fn opened(&mut self, id: WindowId, options: WindowOptions) {
        self.windows.set_bounds(
            id,
            Rect {
                x: options.x.unwrap_or(0.0),
                y: options.y.unwrap_or(0.0),
                width: options.width,
                height: options.height,
            },
        );
        self.created.push((id, options));
    }

    fn ready(&mut self) {
        let state =
            state_tracker::saved_application_state(&self.fs, &OneDisplay, &self.state_file, true)
                .expect("saved state");
        let list = |key: &str| -> Vec<JsValue> {
            ops::get(&state, key)
                .ok()
                .and_then(|v| v.as_array().map(<[JsValue]>::to_vec))
                .unwrap_or_default()
        };
        let hubs = if state == JsValue::Null {
            Vec::new()
        } else {
            list("hubs")
        };
        if hubs.is_empty() {
            let (id, options) = self.windows.create_hub(None);
            self.opened(id, options);
            return;
        }
        for hub in &hubs {
            let (id, options) = self.windows.create_hub(Some(rect_of(hub)));
            self.opened(id, options);
        }
        for satellite in list("satellites") {
            let uuid = ops::get(&satellite, "uuid")
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .expect("satellite uuid");
            let placement = SatellitePlacement::Saved(rect_of(&satellite));
            let (id, options) = self.windows.create_satellite(&uuid, placement);
            self.opened(id, options);
        }
    }

    fn dom_ready(&mut self, window: WindowId) {
        if self.windows.kind(window) != Some(WindowKind::Hub) {
            return;
        }
        let config = beta::beta_config(true, &FixedZone(0.0));
        let beta_file = constants::beta_filename(
            PathStyle::Posix,
            USER_DATA,
            config.as_ref().expect("beta").year,
        );
        let complete = beta::is_beta_welcome_complete(&self.fs, Some(&beta_file)).expect("beta");
        if !beta::is_beta_expired(config.as_ref(), &SetClock(0.0)) && !ops::to_boolean(&complete) {
            let (id, options) = self.windows.create_modal(WindowKind::BetaWelcome, window);
            self.opened(id, options);
        }
    }

    fn singleton(&mut self, opened: asrs_services::window_manager::Opened) {
        if let asrs_services::window_manager::Opened::New(id, options) = opened {
            self.opened(id, options);
        }
    }

    fn step(&mut self, record: &Record) {
        let op = record.string("op").expect("op");
        let args = record.value("args").unwrap_or(Value::Undefined);
        let window = || number(field(&args, "window")) as WindowId;
        match op.as_str() {
            "config" => {
                assert_eq!(record.string("platform").as_deref(), Some("darwin"));
                self.state_file = record.string("stateFile").expect("stateFile");
                return;
            }
            "putState" => {
                assert_eq!(record.string("file"), Some(self.state_file.clone()));
                let text = record.string("text").expect("text");
                self.fs.put(&self.state_file, text.as_bytes());
                return;
            }
            "ready" => self.ready(),
            "domReady" => self.dom_ready(window()),
            "setBounds" => {
                let b = field(&args, "bounds");
                let rect = Rect {
                    x: number(field(b, "x")),
                    y: number(field(b, "y")),
                    width: number(field(b, "width")),
                    height: number(field(b, "height")),
                };
                self.windows.set_bounds(window(), rect);
            }
            "focus" => self.windows.focus(window()),
            "close" => self.windows.closed(window()),
            "menu" => match string(field(&args, "label")).as_str() {
                "New Window" => {
                    let (id, options) = self.windows.create_hub(None);
                    self.opened(id, options);
                }
                "Settings..." => {
                    let opened = self.windows.open_preferences(window());
                    self.singleton(opened);
                }
                "Show Licenses..." => {
                    let opened = self.windows.open_licenses(window());
                    self.singleton(opened);
                }
                "Download Logs..." => {
                    let opened = self.windows.open_download(window());
                    self.singleton(opened);
                }
                other => panic!("unknown menu item {other:?}"),
            },
            "message" => {
                let data = field(&args, "data");
                match string(field(&args, "name")).as_str() {
                    "create-satellite" => {
                        let uuid = string(field(data, "uuid"));
                        let (id, options) = self
                            .windows
                            .create_satellite(&uuid, SatellitePlacement::Parent(window()));
                        self.opened(id, options);
                    }
                    "source-list-help" => {
                        let (id, options) = self.windows.open_source_list_help(window());
                        self.opened(id, options);
                    }
                    "prompt-export" => {
                        assert_eq!(field(data, "incompleteWarning"), &Value::Bool(false));
                        let (id, options) = self.windows.create_modal(WindowKind::Export, window());
                        self.opened(id, options);
                    }
                    other => panic!("unknown message {other:?}"),
                }
            }
            "popupClick" => {
                let kind = match string(field(&args, "label")).as_str() {
                    "Rename..." => WindowKind::RenameTab,
                    "Edit Range..." => WindowKind::EditRange,
                    "Edit Conversion..." => WindowKind::UnitConversion,
                    "Set FOV..." => WindowKind::EditFov,
                    other => panic!("unknown popup item {other:?}"),
                };
                let (id, options) = self.windows.create_modal(kind, window());
                self.opened(id, options);
            }
            "order" => {
                let expected: Vec<WindowId> = record
                    .value("result")
                    .expect("result")
                    .as_array()
                    .expect("order array")
                    .iter()
                    .map(|v| number(v) as WindowId)
                    .collect();
                assert_eq!(
                    self.windows.preference_targets(),
                    expected,
                    "line {}: preference order",
                    record.line
                );
                self.compared[2] += 1;
            }
            other => panic!("line {}: unknown op {other:?}", record.line),
        }
        self.compare_created(record);
        let focused = record.field("focused").and_then(|f| f.as_f64());
        assert_eq!(
            self.windows.focused(),
            focused.map(|f| f as WindowId),
            "line {}: focused window",
            record.line
        );
        self.compared[1] += 1;
    }

    fn compare_created(&mut self, record: &Record) {
        let expected = record
            .field("created")
            .and_then(|c| c.as_array())
            .expect("created");
        let created = std::mem::take(&mut self.created);
        assert_eq!(
            created.len(),
            expected.len(),
            "line {}: created windows {:?}",
            record.line,
            created
                .iter()
                .map(|(id, o)| (id, o.kind))
                .collect::<Vec<_>>()
        );
        for ((id, options), want) in created.iter().zip(expected) {
            let want_id = want.get("id").and_then(|v| v.as_f64()).expect("id");
            assert_eq!(*id, want_id as WindowId, "line {}: window id", record.line);
            let want_options =
                Value::from_json(want.get("options").expect("options")).expect("CVE-1 options");
            assert_eq!(
                port_pairs(options),
                sorted_pairs(&want_options),
                "line {}: options of window {id} ({:?})",
                record.line,
                options.kind
            );
            self.compared[0] += 1;
        }
    }
}

fn replay(id: &str) -> [usize; 3] {
    let records = vectors::load(id).unwrap_or_else(|e| panic!("{e}"));
    let mut replay = Replay::new();
    for record in &records {
        replay.step(record);
    }
    let expected_windows: usize = records
        .iter()
        .filter_map(|r| r.field("created").and_then(|c| c.as_array()))
        .map(|c| c.len())
        .sum();
    let expected_steps = records.iter().filter(|r| r.field("step").is_some()).count();
    let expected_orders = records
        .iter()
        .filter(|r| r.string("op").as_deref() == Some("order"))
        .count();
    assert_eq!(
        replay.compared,
        [expected_windows, expected_steps, expected_orders]
    );
    replay.compared
}

#[test]
fn fresh_start() {
    let [windows, steps, orders] = replay("windows/fresh-darwin");
    assert!(
        windows >= 17 && orders >= 9,
        "{windows} windows, {steps} steps"
    );
}

#[test]
fn saved_state_start() {
    let [windows, _, orders] = replay("windows/saved-darwin");
    assert!(windows >= 8 && orders == 2);
}
