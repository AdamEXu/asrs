#![allow(clippy::disallowed_methods)]

//! Replays `menus/clicks-*`: main.ts's setupMenu click handlers, every item clicked with each kind
//! of window focused. Each click runs `menus::click` against the same windows and files, with the
//! recorded dialog replies, and everything it did is compared with what upstream's handler did.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use asrs_app::menus::click::{self, ClickEnv, Effect, Failure, Reply, Then};
use asrs_app::menus::model::{Command, MenuItem};
use asrs_app::menus::{app_menu, context};
use asrs_js::ops;
use asrs_js::time::{FixedZone, NamedZone};
use asrs_js::value::JsValue;
use asrs_model::app_identity::AppIdentity;
use asrs_model::build_constants;
use asrs_model::messages::{HubToMain, MainToHub, MainToSatellite, NamedMessage};
use asrs_model::preferences::Preferences;
use asrs_parity::cve1::Value;
use asrs_parity::vectors::{self, Record};
use asrs_services::constants::layout_default_path;
use asrs_services::dialog::{FileFilter, MessageBox};
use asrs_services::fs::{Failure as FsFailure, Fs, MemFs};
use asrs_services::historical_start::HOOT_UNSUPPORTED;
use asrs_services::lifecycle::{AppPaths, MainState};
use asrs_services::path::PathStyle;
use asrs_services::update::{FetchReply, ReleaseProfile, UpdateChecker, UpdateEnv};
use asrs_services::window_manager::{
    Opened, Platform, SatellitePlacement, WindowId, WindowKind, WindowManager, WindowOptions,
};
use asrs_services::windows::{Rect, WindowSystem};

fn text(value: &Value) -> String {
    value
        .as_str()
        .unwrap_or_else(|| panic!("not a string: {value:?}"))
        .to_string_lossy()
}

fn array(value: &Value) -> &[Value] {
    value
        .as_array()
        .unwrap_or_else(|| panic!("not an array: {value:?}"))
}

fn number(value: &Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("not a number: {value:?}"))
}

fn id(value: &Value) -> WindowId {
    number(value) as WindowId
}

fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    match value {
        Value::Object(pairs) => pairs
            .iter()
            .find(|(k, _)| k.to_string_lossy() == key)
            .map(|(_, v)| v)
            .unwrap_or_else(|| panic!("no {key} in {value:?}")),
        other => panic!("not an object: {other:?}"),
    }
}

fn js(value: &Value) -> JsValue {
    js_json::to_js(value).unwrap_or_else(|e| panic!("{e}: {value:?}"))
}

fn cve(value: &JsValue) -> Value {
    js_json::from_js(value)
}

/// Option keys in name order: their order means nothing to Electron.
fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(pairs) => {
            let mut pairs: Vec<_> = pairs.iter().map(|(k, v)| (k.clone(), sorted(v))).collect();
            pairs.sort_by_key(|(k, _)| k.to_string_lossy());
            Value::Object(pairs)
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

fn rect(value: &Value) -> Rect {
    let n = |key| number(field(value, key));
    Rect {
        x: n("x"),
        y: n("y"),
        width: n("width"),
        height: n("height"),
    }
}

fn page(kind: WindowKind) -> &'static str {
    match kind {
        WindowKind::Hub => "hub.html",
        WindowKind::Satellite => "satellite.html",
        WindowKind::Preferences => "preferences.html",
        WindowKind::Download => "download.html",
        WindowKind::Licenses => "licenses.html",
        other => panic!("{other:?} does not open here"),
    }
}

fn strings(items: &[&str]) -> JsValue {
    JsValue::Array(items.iter().map(|s| JsValue::string(s)).collect())
}

fn filters(filter: &FileFilter) -> JsValue {
    JsValue::Array(vec![ops::object([
        ("name", JsValue::string(filter.name)),
        ("extensions", strings(filter.extensions)),
    ])])
}

fn box_options(options: &MessageBox) -> JsValue {
    let mut pairs = vec![
        ("type", JsValue::string(options.kind)),
        ("title", JsValue::string(options.title)),
        ("message", JsValue::String(options.message.clone())),
        ("detail", JsValue::String(options.detail.clone())),
    ];
    if let Some(buttons) = &options.buttons {
        pairs.push(("buttons", strings(buttons)));
    }
    if let Some(v) = options.default_id {
        pairs.push(("defaultId", JsValue::Number(v)));
    }
    if let Some(v) = options.cancel_id {
        pairs.push(("cancelId", JsValue::Number(v)));
    }
    if options.icon {
        pairs.push(("icon", JsValue::string("WINDOW_ICON")));
    }
    ops::object(pairs)
}

#[derive(Debug, Default, PartialEq)]
struct Observed {
    dialogs: Vec<(String, Option<WindowId>, Value)>,
    sent: Vec<(WindowId, Value)>,
    popups: Vec<(WindowId, f64, f64, Vec<String>)>,
    created: Vec<(WindowId, String, Rect)>,
    focused: Vec<WindowId>,
    closed: Vec<WindowId>,
    destroyed: Vec<WindowId>,
    moved: Vec<WindowId>,
    external: Vec<String>,
    paths: Vec<Value>,
    clipboard: Vec<String>,
    files: Vec<(String, Option<String>)>,
    throw: Option<String>,
    rejections: Vec<String>,
}

struct Bounds<'a>(&'a WindowManager);

impl WindowSystem for Bounds<'_> {
    fn window_bounds(&self, id: f64) -> Option<Rect> {
        self.0.bounds(id as WindowId)
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

/// The window ids are upstream's: both sides count from 1 in creation order.
struct World {
    platform: Platform,
    arch: String,
    now: f64,
    asset_status: String,
    owlet_status: String,
    fs: MemFs,
    main: MainState,
    manager: WindowManager,
    template: Vec<MenuItem>,
    checker: UpdateChecker,
    ports: BTreeSet<WindowId>,
    replies: VecDeque<Value>,
    closing: Vec<WindowId>,
    waiting: VecDeque<(Then, Reply)>,
    seen: Observed,
}

fn named(name: &str, data: &JsValue) -> Value {
    cve(&ops::object([
        ("name", JsValue::string(name)),
        ("data", data.clone()),
    ]))
}

impl World {
    fn new(record: &Record) -> World {
        let value = |key: &str| record.value(key).unwrap_or_else(|_| panic!("{key}"));
        let fs = MemFs::new();
        let mut seeded = Vec::new();
        for file in array(&value("files")) {
            let name = text(field(file, "name"));
            let bytes = text(field(file, "text")).into_bytes();
            fs.put(&name, &bytes);
            seeded.push((name, bytes));
        }
        for dir in array(&value("dirs")) {
            fs.put_dir(&text(dir));
        }
        let platform = match text(&value("platform")).as_str() {
            "darwin" => Platform::Mac,
            "win32" => Platform::Windows,
            "linux" => Platform::Linux,
            other => panic!("platform {other}"),
        };
        let paths = AppPaths {
            style: PathStyle::Posix,
            user_data: "/fake/userData".to_owned(),
            temp: "/fake/temp".to_owned(),
            bundled_assets: text(&value("bundledAssets")),
            packaged: value("packaged").as_bool().expect("packaged"),
        };
        let mut manager = WindowManager::for_platform(platform);
        let (main, prefs) =
            MainState::start(&fs, &Bounds(&manager), paths, &text(&value("platform")))
                .expect("startup");
        assert_eq!(fs.dump(), seeded, "files after startup");
        fs.take_writes();
        let template = app_menu::template(platform, &prefs, AppIdentity::UPSTREAM);
        let created = value("created");
        let [hub] = array(&created) else {
            panic!("startup opened {created:?}");
        };
        let (window, options) = manager.create_hub(None);
        let mut world = World {
            platform,
            arch: text(&value("arch")),
            now: number(&value("now")),
            asset_status: text(&value("assetStatus")),
            owlet_status: text(&value("owletStatus")),
            fs,
            main,
            manager,
            template,
            checker: UpdateChecker::new(),
            ports: BTreeSet::new(),
            replies: VecDeque::new(),
            closing: Vec::new(),
            waiting: VecDeque::new(),
            seen: Observed::default(),
        };
        world.opened(window, &options);
        assert_eq!(world.seen.created, vec![created_record(hub)]);
        world
    }

    fn opened(&mut self, window: WindowId, options: &WindowOptions) {
        let bounds = Rect {
            x: options.x.unwrap_or(0.0),
            y: options.y.unwrap_or(0.0),
            width: options.width,
            height: options.height,
        };
        self.manager.set_bounds(window, bounds);
        if options.kind == WindowKind::Hub {
            self.ports.insert(window);
        }
        self.seen
            .created
            .push((window, page(options.kind).to_owned(), bounds));
    }

    fn send(&mut self, window: WindowId, name: &str, data: &JsValue) {
        if self.ports.contains(&window) {
            self.seen.sent.push((window, named(name, data)));
        }
    }

    fn send_hubs(&mut self, name: MainToHub, data: &JsValue) {
        for hub in self.manager.hubs().to_vec() {
            self.send(hub, name.name(), data);
        }
    }

    fn send_active_satellites(&mut self) {
        let active = self.manager.active_satellites();
        let active = JsValue::Array(active.iter().map(|uuid| JsValue::string(uuid)).collect());
        self.send_hubs(MainToHub::SetActiveSatellites, &active);
    }

    fn save_satellite_ids(&mut self) {
        let mut ids = JsValue::Object(Default::default());
        for (uuid, windows) in self.manager.satellites() {
            let windows = windows
                .iter()
                .map(|id| ops::object([("id", JsValue::Number(*id as f64))]))
                .collect();
            ops::set_key(&mut ids, &uuid.as_str().into(), JsValue::Array(windows))
                .expect("a plain object");
        }
        self.main
            .tracker
            .save_satellite_ids(&ids)
            .expect("saveSatelliteIds");
    }

    fn satellite(&mut self, uuid: &str, placement: SatellitePlacement) {
        let (window, options) = self.manager.create_satellite(uuid, placement);
        self.opened(window, &options);
        self.save_satellite_ids();
        self.send_active_satellites();
    }

    fn closed(&mut self, window: WindowId) {
        let satellite = self.manager.kind(window) == Some(WindowKind::Satellite);
        self.manager.closed(window);
        if satellite {
            self.save_satellite_ids();
            self.send_active_satellites();
        }
    }

    fn preferences(&mut self) {
        let data = self.main.preferences(&self.fs).expect("prefs.json");
        for window in self.manager.preference_targets() {
            self.send(window, MainToHub::SetPreferences.name(), &data);
        }
        app_menu::apply_preferences(&mut self.template, &Preferences::from_js(&data));
    }

    fn dialog(&mut self, kind: &str, window: Option<WindowId>, options: &JsValue) -> Value {
        self.seen
            .dialogs
            .push((kind.to_owned(), window, sorted(&cve(options))));
        self.replies
            .pop_front()
            .unwrap_or_else(|| panic!("upstream showed no {kind} dialog: {options:?}"))
    }

    fn message_box(
        &mut self,
        window: Option<WindowId>,
        sync: bool,
        options: &MessageBox,
        then: Then,
    ) {
        assert_eq!(options.modal, window.is_some());
        let kind = if sync { "boxSync" } else { "box" };
        let reply = self.dialog(kind, window, &box_options(options));
        let response = if sync {
            number(&reply)
        } else {
            number(field(&reply, "response"))
        };
        let reply = Reply::Box(response as usize);
        if sync {
            self.resume(then, &reply);
        } else {
            self.waiting.push_back((then, reply));
        }
    }

    fn run(
        &mut self,
        handler: impl FnOnce(&ClickEnv<'_>, &mut Vec<Effect>) -> Result<(), Failure>,
    ) {
        let mut effects = Vec::new();
        let result = {
            let tracker = &self.main.tracker;
            let manager = &self.manager;
            let application_state = || tracker.current_application_state(&Bounds(manager));
            handler(
                &ClickEnv {
                    platform: self.platform,
                    arch: &self.arch,
                    identity: AppIdentity::UPSTREAM,
                    profile: ReleaseProfile::PARITY,
                    fs: &self.fs,
                    paths: &self.main.paths,
                    windows: manager,
                    asset_status: &self.asset_status,
                    layout_default_path: layout_default_path(self.now, &FixedZone(0.0)),
                    application_state: &application_state,
                },
                &mut effects,
            )
        };
        match result {
            Ok(()) => {}
            Err(Failure::Throw(e)) => {
                assert_eq!(self.seen.throw, None);
                self.seen.throw = Some(e.name.to_string());
            }
            Err(Failure::Rejection(e)) => self.seen.rejections.push(e.name.to_string()),
        }
        for effect in effects {
            self.perform(effect);
        }
    }

    fn resume(&mut self, then: Then, reply: &Reply) {
        self.run(|env, out| click::resume(then, reply, env, out));
    }

    fn settle(&mut self) {
        while let Some((then, reply)) = self.waiting.pop_front() {
            self.resume(then, &reply);
        }
        for window in std::mem::take(&mut self.closing) {
            self.seen.closed.push(window);
            self.closed(window);
        }
        assert!(
            self.replies.is_empty(),
            "the port did not show {:?}",
            self.replies
        );
        for name in self.fs.take_writes() {
            let text = self.fs.read(&name).ok();
            let file = (name, text.map(|t| String::from_utf8_lossy(&t).into_owned()));
            if !self.seen.files.contains(&file) {
                self.seen.files.push(file);
            }
        }
    }

    fn perform(&mut self, effect: Effect) {
        match effect {
            Effect::Send { window, name, data } => self.send(window, name.name(), &data),
            Effect::SendAllPreferences => self.preferences(),
            Effect::ReloadAssets => {
                self.main.assets.reload(&self.fs).expect("loadAssets");
                self.preferences();
                let assets = self.main.assets.set_assets_data();
                let satellites: Vec<WindowId> = self
                    .manager
                    .satellites()
                    .iter()
                    .flat_map(|(_, windows)| windows.to_vec())
                    .collect();
                for satellite in satellites {
                    self.send(satellite, MainToSatellite::SetAssets.name(), &assets);
                }
                self.send_hubs(MainToHub::SetAssets, &assets);
            }
            Effect::MessageBox {
                window,
                sync,
                options,
                then,
            } => self.message_box(window, sync, &options, then),
            Effect::OpenDialog {
                window,
                options,
                then,
            } => {
                let mut pairs = vec![("title", JsValue::string(&options.title))];
                if let Some(message) = options.message {
                    pairs.push(("message", JsValue::string(message)));
                }
                if let Some(label) = options.button_label {
                    pairs.push(("buttonLabel", JsValue::string(label)));
                }
                pairs.push(("properties", strings(options.properties)));
                if let Some(filter) = &options.filters {
                    pairs.push(("filters", filters(filter)));
                }
                let reply = self.dialog("open", window, &ops::object(pairs));
                let files = array(field(&reply, "filePaths")).iter().map(text).collect();
                self.waiting.push_back((then, Reply::Open(files)));
            }
            Effect::SaveDialog {
                window,
                options,
                then,
            } => {
                let described = ops::object([
                    ("title", JsValue::string(options.title)),
                    ("defaultPath", JsValue::string(&options.default_path)),
                    ("properties", strings(options.properties)),
                    ("filters", filters(&options.filters)),
                ]);
                let reply = self.dialog("save", window, &described);
                let canceled = field(&reply, "canceled").as_bool().expect("canceled");
                let path = (!canceled).then(|| text(field(&reply, "filePath")));
                self.waiting.push_back((then, Reply::Save(path)));
            }
            Effect::OpenFilesSmart { files, window } => {
                let files = files.iter().map(|f| JsValue::string(f)).collect();
                let data = ops::object([
                    ("files", JsValue::Array(files)),
                    ("merge", JsValue::Bool(false)),
                ]);
                self.send(window, MainToHub::OpenFiles.name(), &data);
            }
            Effect::CheckForUpdate => {
                let env = UpdateEnv {
                    platform: match self.platform {
                        Platform::Mac => "darwin",
                        Platform::Windows => "win32",
                        Platform::Linux => "linux",
                    },
                    arch: &self.arch,
                    running_under_arm64_translation: false,
                    packaged: self.main.paths.packaged,
                    version: build_constants::VERSION,
                    is_beta: true,
                    repository: ReleaseProfile::PARITY.repository,
                    app_name: AppIdentity::UPSTREAM.name,
                };
                let zone = NamedZone::get("UTC").expect("UTC");
                let checked = self
                    .checker
                    .check_for_update(&env, &zone, true, |_| FetchReply::Rejected);
                let button = checked.button.expect("the check resolves");
                self.send_hubs(MainToHub::ShowUpdateButton, &JsValue::Bool(button));
                let prompt = checked.prompt.expect("alwaysPrompt");
                self.message_box(None, false, &prompt, Then::Nothing);
            }
            Effect::OpenHub(state) => {
                let bounds = state.as_ref().map(|state| rect(&cve(state)));
                let (window, options) = self.manager.create_hub(bounds);
                self.opened(window, &options);
            }
            Effect::OpenSatellite(state) => {
                let state = cve(&state);
                let uuid = text(field(&state, "uuid"));
                self.satellite(&uuid, SatellitePlacement::Saved(rect(&state)));
            }
            Effect::OpenWindow { kind, parent } => {
                let opened = match kind {
                    WindowKind::Preferences => self.manager.open_preferences(parent),
                    WindowKind::Download => self.manager.open_download(parent),
                    WindowKind::Licenses => self.manager.open_licenses(parent),
                    other => panic!("{other:?} does not open from the menu"),
                };
                match opened {
                    Opened::New(window, options) => self.opened(window, &options),
                    Opened::Focus(window) => {
                        self.seen.focused.push(window);
                        if kind == WindowKind::Download {
                            self.send(window, "set-focused", &JsValue::Bool(true));
                        }
                    }
                }
            }
            Effect::NewTabPopup { window, rect } => {
                let menu = context::new_tab(&rect).expect("newTabPopup");
                let (x, y) = menu.position.expect("a position");
                let labels = menu
                    .items
                    .iter()
                    .map(|item| item.label.clone().expect("a label"))
                    .collect();
                self.seen.popups.push((window, x, y, labels));
            }
            Effect::Close(window) => self.closing.push(window),
            Effect::Destroy(window) => {
                self.seen.destroyed.push(window);
                self.closed(window);
            }
            Effect::MoveTop(window) => self.seen.moved.push(window),
            Effect::OpenPath(path) => self.seen.paths.push(cve(&path)),
            Effect::OpenExternal(url) => self.seen.external.push(url),
            Effect::WriteClipboard(text) => self.seen.clipboard.push(text),
        }
    }

    fn check_menu(&self, record: &Record, at: &str) {
        let item = |path: &[usize]| MenuItem::at(&self.template, path).expect("a template item");
        let label = |path: &[usize]| JsValue::string(item(path).label.as_deref().expect("a label"));
        let checked = |path: &[usize]| match item(path).checked {
            Some(checked) => JsValue::Bool(checked),
            None => JsValue::Undefined,
        };
        let got = JsValue::Array(vec![
            label(&[1, 2, 0]),
            label(&[1, 3, 0]),
            checked(&[0, 7]),
            checked(&[1, 7]),
            checked(&[1, 8]),
        ]);
        assert_eq!(record.value("menu").expect("menu"), cve(&got), "{at}: menu");
    }
}

fn created_record(value: &Value) -> (WindowId, String, Rect) {
    (
        id(field(value, "id")),
        text(field(value, "file")),
        rect(field(value, "bounds")),
    )
}

fn recorded(record: &Record, command: Option<Command>, world: &World) -> (Observed, Vec<Value>) {
    let value = |key: &str| {
        record
            .value(key)
            .unwrap_or_else(|_| panic!("line {}: {key}", record.line))
    };
    let ids = |key: &str| array(&value(key)).iter().map(id).collect();
    let texts = |key: &str| -> Vec<String> { array(&value(key)).iter().map(text).collect() };
    let mut replies = Vec::new();
    let dialogs = array(&value("dialogs"))
        .iter()
        .map(|dialog| {
            replies.push(field(dialog, "reply").clone());
            let window = match field(dialog, "window") {
                Value::Null => None,
                v => Some(id(v)),
            };
            let mut options = sorted(field(dialog, "options"));
            if command == Some(Command::OwletDownloadStatus) {
                replace_owlet_status(&mut options, &world.owlet_status);
            }
            (text(field(dialog, "kind")), window, options)
        })
        .collect();
    let seen = Observed {
        dialogs,
        sent: array(&value("sent"))
            .iter()
            .map(|sent| (id(field(sent, "to")), field(sent, "message").clone()))
            .collect(),
        popups: array(&value("popups"))
            .iter()
            .map(|popup| {
                (
                    id(field(popup, "window")),
                    number(field(popup, "x")),
                    number(field(popup, "y")),
                    array(field(popup, "labels")).iter().map(text).collect(),
                )
            })
            .collect(),
        created: array(&value("created"))
            .iter()
            .map(created_record)
            .collect(),
        focused: ids("focused"),
        closed: ids("closed"),
        destroyed: ids("destroyed"),
        moved: ids("moved"),
        external: texts("external"),
        paths: array(&value("paths")).to_vec(),
        clipboard: texts("clipboard"),
        files: array(&value("files"))
            .iter()
            .map(|file| {
                let contents = match field(file, "text") {
                    Value::Null => None,
                    v => Some(text(v)),
                };
                (text(field(file, "name")), contents)
            })
            .collect(),
        throw: match value("throw") {
            Value::Null => None,
            v => Some(text(&v)),
        },
        rejections: texts("rejections"),
    };
    (seen, replies)
}

/// Owlet is out of scope, so its box says what opening a `.hoot` file says in place of a status.
fn replace_owlet_status(options: &mut Value, status: &str) {
    let Value::Object(pairs) = options else {
        panic!("{options:?}");
    };
    let detail = pairs
        .iter_mut()
        .find(|(k, _)| k.to_string_lossy() == "detail")
        .expect("detail");
    assert_eq!(text(&detail.1), status);
    detail.1 = cve(&JsValue::string(HOOT_UNSUPPORTED));
}

fn click_paths(items: &[MenuItem], prefix: &[usize], out: &mut BTreeSet<Vec<usize>>) {
    for (index, item) in items.iter().enumerate() {
        let path = [prefix, &[index]].concat();
        if let Some(submenu) = &item.submenu {
            click_paths(submenu, &path, out);
        }
        if item.command.is_some() {
            out.insert(path);
        }
    }
}

fn replay(vector: &str) -> BTreeMap<&'static str, usize> {
    let records = vectors::load(vector).unwrap_or_else(|e| panic!("{e}"));
    let (config, records) = records.split_first().expect("records");
    assert_eq!(config.string("op").as_deref(), Some("config"));
    let mut world = World::new(config);
    world.check_menu(config, "startup");
    let mut unclicked = BTreeSet::new();
    click_paths(&world.template, &[], &mut unclicked);
    let mut tally = BTreeMap::new();
    let mut count = |what: &'static str, n: usize| *tally.entry(what).or_default() += n;
    for record in records {
        let op = record.string("op").expect("op");
        let at = format!("{vector} line {} {op}", record.line);
        let value = |key: &str| record.value(key).unwrap_or_else(|_| panic!("{at}: {key}"));
        world.seen = Observed::default();
        match op.as_str() {
            "putFile" => world
                .fs
                .put(&text(&value("name")), text(&value("text")).as_bytes()),
            "removeFile" => world.fs.remove(&text(&value("name"))),
            "setFailure" => {
                let failure = match value("kind") {
                    Value::Null => None,
                    v if text(&v) == "write" => Some(FsFailure::Write),
                    v => panic!("{at}: {v:?}"),
                };
                world.fs.set_failure(&text(&value("name")), failure);
            }
            "satellite" => {
                let parent = SatellitePlacement::Parent(id(&value("parent")));
                world.satellite(&text(&value("uuid")), parent);
                let want = created_record(&value("window"));
                assert_eq!(world.seen.created, vec![want], "{at}");
            }
            "saveState" => world
                .main
                .tracker
                .save_renderer_state(id(&value("window")) as f64, js(&value("state"))),
            "domReady" => {
                world.ports.insert(id(&value("window")));
                world.preferences();
                world.check_menu(record, &at);
            }
            "close" => world.closed(id(&value("window"))),
            "click" | "updatePreferences" => {
                let window = match value("window") {
                    Value::Null => None,
                    v => Some(id(&v)),
                };
                if let Some(focused) = world.manager.focused() {
                    world.manager.blur(focused);
                }
                if let Some(window) = window {
                    world.manager.focus(window);
                }
                let mut command = None;
                if op == "click" {
                    let path: Vec<usize> = array(&value("path"))
                        .iter()
                        .map(|v| number(v) as usize)
                        .collect();
                    let item = MenuItem::at(&world.template, &path).expect("an item");
                    assert_eq!(item.label, Some(text(&value("label"))), "{at}");
                    command = Some(item.command.expect("a command"));
                    unclicked.remove(&path);
                }
                let (want, replies) = recorded(record, command, &world);
                count("dialogs", want.dialogs.len());
                count("sent", want.sent.len());
                count("created", want.created.len());
                count("files", want.files.len());
                count("failures", want.rejections.len() + want.throw.iter().len());
                world.replies = replies.into();
                match command {
                    Some(command) => {
                        let checked = value("checked").as_bool().expect("checked");
                        world.run(|env, out| click::click(command, checked, window, env, out));
                    }
                    None => {
                        let window = window.expect("a hub");
                        let message =
                            NamedMessage::new(HubToMain::UpdatePreferences, js(&value("data")));
                        match world.main.hub_message(&world.fs, window as f64, &message) {
                            Ok(()) => world.preferences(),
                            Err(e) => world.seen.rejections.push(e.name.to_string()),
                        }
                    }
                }
                world.settle();
                assert_eq!(world.seen, want, "{at} {command:?}");
                world.check_menu(record, &at);
            }
            other => panic!("{at}: unknown op {other}"),
        }
        count(if op == "click" { "clicks" } else { "setup" }, 1);
    }
    assert!(
        unclicked.is_empty(),
        "{vector}: never clicked {unclicked:?}"
    );
    let compared: usize = tally["clicks"] + tally["setup"];
    assert_eq!(compared, records.len());
    tally
}

#[test]
fn clicks_on_macos() {
    let tally = replay("menus/clicks-darwin");
    assert!(tally["clicks"] > 300 && tally["failures"] > 10, "{tally:?}");
}

#[test]
fn clicks_on_windows() {
    let tally = replay("menus/clicks-win32");
    assert!(tally["clicks"] > 300 && tally["failures"] > 10, "{tally:?}");
}

#[test]
fn clicks_on_linux() {
    let tally = replay("menus/clicks-linux");
    assert!(tally["clicks"] > 300 && tally["failures"] > 10, "{tally:?}");
}

#[test]
fn clicks_in_a_packaged_build() {
    let tally = replay("menus/clicks-darwin-packaged");
    assert!(tally["clicks"] > 300 && tally["failures"] > 10, "{tally:?}");
}
