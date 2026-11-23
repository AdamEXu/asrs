#![allow(clippy::disallowed_methods)]

//! Replays `files/routing-*` (oracle/src/files/routing.ts): main.ts's `openFilesSmart` and its
//! callers, the windows they open with their `dom-ready` sends, and the export flow. Each event
//! runs the port against the same files, windows and renderer states, with the recorded dialog
//! replies, and the dialogs, messages, windows, files and errors must be the recorded ones.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::collections::{BTreeMap, VecDeque};

use asrs_app::menus::click::{self, ClickEnv, Effect as Click, Reply};
use asrs_app::menus::model::Command;
use asrs_js::ops;
use asrs_js::string::JsError;
use asrs_js::value::{JsValue, ToJs};
use asrs_model::app_identity::AppIdentity;
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage};
use asrs_parity::cve1::Value;
use asrs_parity::json::Json;
use asrs_parity::vectors::{self, Record};
use asrs_services::dialog::{FileFilter, MessageBox, SaveDialog};
use asrs_services::ds_layout::default_ds_layout;
use asrs_services::export_flow::{self, ExportPrompt};
use asrs_services::fs::{Failure, Fs, MemFs};
use asrs_services::lifecycle::{self, AppPaths, HubRestore, MainState, StartupWindow};
use asrs_services::path::PathStyle;
use asrs_services::routing::{Action, Hubs, Route};
use asrs_services::update::ReleaseProfile;
use asrs_services::window_manager::{Platform, WindowId, WindowKind, WindowManager};
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

fn bytes(value: &Value) -> Vec<u8> {
    match value {
        Value::Bytes(bytes) => bytes.clone(),
        other => panic!("not bytes: {other:?}"),
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

fn save_options(options: &SaveDialog) -> JsValue {
    ops::object([
        ("title", JsValue::string(options.title)),
        (
            "defaultPath",
            match &options.default_path {
                Some(path) => JsValue::string(path),
                None => JsValue::Undefined,
            },
        ),
        ("properties", strings(options.properties)),
        ("filters", filters(&options.filters)),
    ])
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

#[derive(Debug, Default, PartialEq)]
struct Observed {
    dialogs: Vec<(String, Option<WindowId>, Value)>,
    sent: Vec<(WindowId, Value)>,
    created: Vec<(WindowId, String, Option<WindowId>)>,
    focused: Vec<WindowId>,
    destroyed: Vec<WindowId>,
    closed: Vec<WindowId>,
    external: Vec<String>,
    files: Vec<(String, Option<Vec<u8>>)>,
    throw: Option<String>,
    uncaught: Vec<String>,
    rejections: Vec<String>,
}

/// A hub window whose `dom-ready` has not run.
#[derive(Default)]
struct NewHub {
    saved: Option<JsValue>,
    layout: Option<JsValue>,
    files: Option<Vec<String>>,
    /// `firstOpenPath`, for the window `whenReady` opens it in.
    first_open: Option<String>,
}

struct Prompt {
    hub: WindowId,
    supports_akit: JsValue,
    flow: ExportPrompt,
}

/// The window ids are upstream's: both sides count from 1 in creation order.
struct World {
    fs: MemFs,
    main: MainState,
    manager: WindowManager,
    new_hubs: BTreeMap<WindowId, NewHub>,
    prompts: BTreeMap<WindowId, Prompt>,
    replies: VecDeque<Value>,
    early: Vec<String>,
    seen: Observed,
}

fn paths(config: &Value) -> AppPaths {
    AppPaths {
        style: PathStyle::Posix,
        user_data: text(field(config, "userData")),
        temp: text(field(config, "temp")),
        bundled_assets: "/fake/bundledAssets".to_owned(),
        packaged: field(config, "packaged").as_bool().expect("packaged"),
    }
}

impl World {
    fn start(config: &Value, record: &Record, early: Vec<String>) -> World {
        let value = |key: &str| record.value(key).unwrap_or_else(|_| panic!("{key}"));
        let paths = paths(config);
        let fs = MemFs::new();
        fs.put_dir(&paths.user_data);
        let mut recorded = Vec::new();
        for file in array(&value("files")) {
            let (name, data) = (text(field(file, "name")), bytes(field(file, "bytes")));
            if name != paths.prefs() {
                fs.put(&name, &data);
            }
            recorded.push((name, data));
        }
        let mut manager = WindowManager::for_platform(Platform::Mac);
        let (main, _) = MainState::start(&fs, &Bounds(&manager), paths, "darwin").expect("startup");
        // The oracle's main.ts has no asset loader.
        let assets = format!("{}/userAssets/", main.paths.user_data);
        let mut files = fs.dump();
        files.retain(|(name, _)| !name.starts_with(&assets));
        assert!(files == recorded, "files after startup");
        fs.take_writes();
        let saved = main
            .tracker
            .get_saved_application_state(&fs, &Bounds(&manager), true)
            .expect("saved state");
        let mut plan = Vec::new();
        lifecycle::startup_windows(&saved, |window| plan.push(window)).expect("a plan");
        let argv: Vec<String> = array(&value("argv")).iter().map(text).collect();
        let mut first_open = lifecycle::first_open_path(early.iter().cloned(), &argv);
        let mut new_hubs = BTreeMap::new();
        for window in plan {
            let saved = match window {
                StartupWindow::DefaultHub => None,
                StartupWindow::Hub(state) => Some(state),
                StartupWindow::Satellite(_) => panic!("no scenario saves satellites"),
            };
            let (id, _) = manager.create_hub(None);
            let hub = NewHub {
                saved,
                first_open: first_open.take(),
                ..NewHub::default()
            };
            new_hubs.insert(id, hub);
        }
        let created: Vec<WindowId> = array(&value("created"))
            .iter()
            .map(|id| number(id) as WindowId)
            .collect();
        assert_eq!(manager.hubs(), created, "windows opened at startup");
        World {
            fs,
            main,
            manager,
            new_hubs,
            prompts: BTreeMap::new(),
            replies: VecDeque::new(),
            early,
            seen: Observed::default(),
        }
    }

    fn send(&mut self, window: WindowId, name: MainToHub, data: JsValue) {
        let message = ops::object([("name", JsValue::string(name.name())), ("data", data)]);
        self.seen.sent.push((window, cve(&message)));
    }

    fn dialog(&mut self, kind: &str, window: Option<WindowId>, options: &JsValue) -> Value {
        self.seen
            .dialogs
            .push((kind.to_owned(), window, sorted(&cve(options))));
        self.replies
            .pop_front()
            .unwrap_or_else(|| panic!("upstream showed no {kind} dialog: {options:?}"))
    }

    fn message_box(&mut self, sync: bool, window: Option<WindowId>, options: &MessageBox) -> usize {
        assert_eq!(options.modal, window.is_some());
        let kind = if sync { "boxSync" } else { "box" };
        let reply = self.dialog(kind, window, &box_options(options));
        let response = if sync {
            number(&reply)
        } else {
            number(field(&reply, "response"))
        };
        response as usize
    }

    fn new_hub(&mut self, hub: NewHub) {
        let (id, _) = self.manager.create_hub(None);
        self.seen.created.push((id, "hub.html".to_owned(), None));
        self.new_hubs.insert(id, hub);
    }

    fn send_files(&mut self, window: WindowId, files: &[String]) {
        let files = files.iter().map(|f| JsValue::string(f)).collect();
        let data = ops::object([
            ("files", JsValue::Array(files)),
            ("merge", JsValue::Bool(false)),
        ]);
        self.send(window, MainToHub::OpenFiles, data);
    }

    fn act(&mut self, action: Action, files: &[String]) {
        match action {
            Action::Send(window) => self.send_files(window, files),
            Action::FocusAndSend(window) => {
                self.seen.focused.push(window);
                self.manager.focus(window);
                self.send_files(window, files);
            }
            Action::NewHub => self.new_hub(NewHub {
                files: Some(files.to_vec()),
                ..NewHub::default()
            }),
            Action::NewDsHub => self.new_hub(NewHub {
                layout: Some(default_ds_layout(true).to_js()),
                files: Some(files.to_vec()),
                ..NewHub::default()
            }),
        }
    }

    fn route(
        &mut self,
        files: &[String],
        decide: impl FnOnce(&Hubs<'_>) -> Result<Route, JsError>,
    ) -> Result<(), JsError> {
        let tracker = &self.main.tracker;
        let route = decide(&Hubs {
            fs: &self.fs,
            windows: self.manager.hubs(),
            state: &|window| tracker.get_renderer_state(window as f64),
        })?;
        match route {
            Route::Open(action) => self.act(action, files),
            Route::Ask {
                window,
                options,
                actions,
            } => {
                let response = self.message_box(true, window, &options);
                if let Some(action) = actions.get(response) {
                    self.act(*action, files);
                }
            }
        }
        Ok(())
    }

    fn open_files(&mut self, files: &[String], source: Option<WindowId>) -> Result<(), JsError> {
        self.route(files, |hubs| hubs.open_files_smart(files, source))
    }

    fn export_effect(
        &mut self,
        hub: WindowId,
        prompt: Option<WindowId>,
        request: &JsValue,
        effect: export_flow::Effect,
    ) -> Result<(), JsError> {
        use export_flow::Effect;
        match effect {
            Effect::Warn(options) => {
                let response = self.message_box(false, Some(hub), &options);
                let next = export_flow::warned(request, response)?;
                self.export_effect(hub, prompt, request, next)?;
            }
            Effect::OpenPrompt {
                supports_akit,
                path,
            } => {
                let (id, _) = self.manager.create_modal(WindowKind::Export, hub);
                self.seen
                    .created
                    .push((id, "export.html".to_owned(), Some(hub)));
                self.prompts.insert(
                    id,
                    Prompt {
                        hub,
                        supports_akit,
                        flow: ExportPrompt::new(path),
                    },
                );
            }
            Effect::Send { name, data } => self.send(hub, name, data),
            Effect::DestroyPrompt => {
                let prompt = prompt.expect("a prompt");
                self.seen.destroyed.push(prompt);
                self.manager.closed(prompt);
            }
            Effect::OpenExternal(url) => self.seen.external.push(url.to_string_lossy()),
            Effect::Save { options, export } => {
                let prompt = prompt.expect("a prompt");
                let reply = self.dialog("save", Some(prompt), &save_options(&options));
                let canceled = field(&reply, "canceled").as_bool().expect("canceled");
                let path = (!canceled).then(|| text(field(&reply, "filePath")));
                let flow = &mut self.prompts.get_mut(&prompt).expect("a prompt").flow;
                let effects = flow.saved(export, path.as_deref());
                self.export_effects(hub, Some(prompt), request, effects)?;
            }
        }
        Ok(())
    }

    fn export_effects(
        &mut self,
        hub: WindowId,
        prompt: Option<WindowId>,
        request: &JsValue,
        effects: impl IntoIterator<Item = export_flow::Effect>,
    ) -> Result<(), JsError> {
        for effect in effects {
            self.export_effect(hub, prompt, request, effect)?;
        }
        Ok(())
    }

    fn message(&mut self, window: WindowId, name: &str, data: JsValue) {
        let name = HubToMain::from_name(name).unwrap_or_else(|| panic!("message {name}"));
        let message = NamedMessage::new(name, data);
        self.main
            .hub_message(&self.fs, window as f64, &message)
            .expect("hub_message");
        let handled = match name {
            HubToMain::OpenFilesRequest => {
                let files: Vec<String> = message
                    .data
                    .get("files")
                    .and_then(JsValue::as_array)
                    .expect("files")
                    .iter()
                    .map(|f| f.as_str().expect("a path").to_string_lossy())
                    .collect();
                self.open_files(&files, Some(window))
            }
            HubToMain::PromptExport => {
                export_flow::prompt_export(&message.data, AppIdentity::UPSTREAM)
                    .and_then(|effect| self.export_effects(window, None, &message.data, [effect]))
            }
            HubToMain::WriteExport => {
                let (path, content) = export_flow::export_file(&message.data).expect("a file");
                match export_flow::write_export(&self.fs, &path, &content) {
                    Ok(effect) => self.export_effects(window, None, &JsValue::Undefined, [effect]),
                    Err(e) => {
                        self.seen.uncaught.push(e.name.to_string());
                        Ok(())
                    }
                }
            }
            HubToMain::SetExporting => Ok(()),
            other => panic!("message {other:?}"),
        };
        if let Err(e) = handled {
            self.seen.rejections.push(e.name.to_string());
        }
    }

    fn dom_ready(&mut self, window: WindowId) {
        if let Some(prompt) = self.prompts.get(&window) {
            let message = ops::object([("supportsAkit", prompt.supports_akit.clone())]);
            self.seen.sent.push((window, cve(&message)));
            return;
        }
        let hub = self.new_hubs.remove(&window).expect("a new hub");
        let mut sends = Vec::new();
        let restore = HubRestore {
            first_load: true,
            layout_override: hub.layout.as_ref(),
            saved: hub.saved.as_ref(),
            cached: self.main.tracker.get_renderer_state(window as f64),
        };
        lifecycle::dom_ready_restores(
            &self.fs,
            &self.main.paths.type_memory(),
            &restore,
            |name, data| sends.push((name, data)),
        )
        .expect("dom-ready");
        for (name, data) in sends {
            self.send(window, name, data);
        }
        if let Some(files) = hub.files {
            self.send_files(window, &files);
        }
        if let Some(path) = hub.first_open
            && let Err(e) = self.open_files(&[path], Some(window))
        {
            self.seen.throw = Some(e.name.to_string());
        }
    }

    fn close(&mut self, window: WindowId) {
        if self.main.is_exporting(window as f64)
            && self.message_box(true, Some(window), &export_flow::closing_warning()) == 0
        {
            return;
        }
        self.manager.closed(window);
        self.seen.closed.push(window);
        if let Some(prompt) = self.prompts.get_mut(&window) {
            let hub = prompt.hub;
            let effect = prompt.flow.closed();
            self.export_effects(hub, Some(window), &JsValue::Undefined, effect)
                .expect("closed");
        }
    }

    fn page(&mut self, window: WindowId, data: &JsValue) {
        let prompt = self.prompts.get_mut(&window).expect("a prompt");
        let hub = prompt.hub;
        let result = prompt
            .flow
            .message(data)
            .and_then(|effects| self.export_effects(hub, Some(window), data, effects));
        if let Err(e) = result {
            self.seen.throw = Some(e.name.to_string());
        }
    }

    fn akit(&mut self) {
        let Some(path) = lifecycle::take_akit_input(&self.fs, &self.main.paths.akit_path_input())
        else {
            return;
        };
        let Some(window) = self.manager.hubs().first().copied() else {
            return;
        };
        self.seen.focused.push(window);
        self.manager.focus(window);
        if let Err(e) = self.open_files(&[path], Some(window)) {
            self.seen.uncaught.push(e.name.to_string());
        }
    }

    fn click<R>(&self, run: impl FnOnce(&ClickEnv<'_>, &mut Vec<Click>) -> R) -> (R, Vec<Click>) {
        let mut effects = Vec::new();
        let env = ClickEnv {
            platform: Platform::Mac,
            arch: "arm64",
            identity: AppIdentity::UPSTREAM,
            profile: ReleaseProfile::PARITY,
            fs: &self.fs,
            paths: &self.main.paths,
            windows: &self.manager,
            asset_status: "",
            layout_default_path: String::new(),
            application_state: &|| JsValue::Undefined,
        };
        let result = run(&env, &mut effects);
        (result, effects)
    }

    fn open_logs(&mut self, window: WindowId, gone: bool) {
        let (clicked, effects) =
            self.click(|env, out| click::click(Command::OpenLogs, false, Some(window), env, out));
        clicked.expect("a click");
        let [
            Click::OpenDialog {
                window: parent,
                options,
                then,
            },
        ] = &effects[..]
        else {
            panic!("{effects:?}");
        };
        let described = ops::object([
            ("title", JsValue::string(&options.title)),
            (
                "message",
                JsValue::string(options.message.expect("a message")),
            ),
            ("properties", strings(options.properties)),
            (
                "filters",
                filters(options.filters.as_ref().expect("filters")),
            ),
        ]);
        let reply = self.dialog("open", *parent, &described);
        let files = array(field(&reply, "filePaths")).iter().map(text).collect();
        if gone {
            self.manager.closed(window);
            self.new_hubs.remove(&window);
            self.seen.destroyed.push(window);
        }
        let (resumed, effects) =
            self.click(|env, out| click::resume(then.clone(), &Reply::Open(files), env, out));
        resumed.expect("a reply");
        for effect in effects {
            match effect {
                Click::OpenFilesSmart { files, window } => {
                    if let Err(e) = self.open_files(&files, Some(window)) {
                        self.seen.rejections.push(e.name.to_string());
                    }
                }
                other => panic!("{other:?}"),
            }
        }
    }
}

fn recorded(record: &Record) -> (Observed, VecDeque<Value>) {
    let value = |key: &str| {
        record
            .value(key)
            .unwrap_or_else(|_| panic!("line {}: {key}", record.line))
    };
    let window = |value: &Value| match value {
        Value::Null => None,
        id => Some(number(id) as WindowId),
    };
    let ids = |key: &str| -> Vec<WindowId> {
        record
            .field(key)
            .and_then(Json::as_array)
            .unwrap_or_else(|| panic!("line {}: {key}", record.line))
            .iter()
            .map(|id| id.as_f64().expect("an id") as WindowId)
            .collect()
    };
    let texts = |key: &str| -> Vec<String> { array(&value(key)).iter().map(text).collect() };
    let mut replies = VecDeque::new();
    let dialogs = array(&value("dialogs"))
        .iter()
        .map(|dialog| {
            replies.push_back(field(dialog, "reply").clone());
            (
                text(field(dialog, "kind")),
                window(field(dialog, "window")),
                sorted(field(dialog, "options")),
            )
        })
        .collect();
    let created = record
        .field("created")
        .and_then(Json::as_array)
        .expect("created")
        .iter()
        .map(|created| {
            let get = |key: &str| created.get(key).expect(key);
            (
                get("id").as_f64().expect("an id") as WindowId,
                get("kind").as_str().expect("a page").to_string_lossy(),
                get("parent").as_f64().map(|id| id as WindowId),
            )
        })
        .collect();
    let seen = Observed {
        dialogs,
        sent: array(&value("sent"))
            .iter()
            .map(|sent| {
                (
                    number(field(sent, "to")) as WindowId,
                    field(sent, "message").clone(),
                )
            })
            .collect(),
        created,
        focused: ids("focused"),
        destroyed: ids("destroyed"),
        closed: ids("closed"),
        external: texts("external"),
        files: array(&value("files"))
            .iter()
            .map(|file| {
                let data = match field(file, "bytes") {
                    Value::Null => None,
                    data => Some(bytes(data)),
                };
                (text(field(file, "name")), data)
            })
            .collect(),
        throw: match value("throw") {
            Value::Null => None,
            name => Some(text(&name)),
        },
        uncaught: texts("uncaught"),
        rejections: texts("rejections"),
    };
    (seen, replies)
}

fn replay(scenario: &str) -> BTreeMap<String, usize> {
    let records =
        vectors::load(&format!("files/routing-{scenario}")).unwrap_or_else(|e| panic!("{e}"));
    let (config, records) = records.split_first().expect("records");
    assert_eq!(config.string("op").as_deref(), Some("config"));
    let config = config.value("result").expect("result");
    let mut early = Vec::new();
    let mut world: Option<World> = None;
    let mut tally: BTreeMap<String, usize> = BTreeMap::new();
    for record in records {
        let op = record.string("op").expect("op");
        let at = format!("{scenario} line {} {op}", record.line);
        let value = |key: &str| record.value(key).unwrap_or_else(|_| panic!("{at}: {key}"));
        *tally.entry(op.clone()).or_default() += 1;
        match op.as_str() {
            "openFileEarly" => {
                early.push(text(&value("path")));
                continue;
            }
            "start" => {
                assert_eq!(record.string("scenario").as_deref(), Some(scenario));
                world = Some(World::start(&config, record, early.clone()));
                continue;
            }
            _ => {}
        }
        let world = world.as_mut().expect("start comes first");
        assert_eq!(world.early, early);
        let window = || {
            let id = record.field("window").and_then(Json::as_f64);
            id.unwrap_or_else(|| panic!("{at}: window")) as WindowId
        };
        match op.as_str() {
            "putFile" => {
                world.fs.put(&text(&value("name")), &bytes(&value("bytes")));
                continue;
            }
            "putDir" => {
                world.fs.put_dir(&text(&value("name")));
                continue;
            }
            "setFailure" => {
                let failure = match record.string("kind").as_deref() {
                    Some("read") => Failure::Read,
                    Some("readOpened") => Failure::ReadOpened,
                    Some("write") => Failure::Write,
                    other => panic!("{at}: {other:?}"),
                };
                world.fs.set_failure(&text(&value("name")), Some(failure));
                continue;
            }
            "saveState" => {
                let message = NamedMessage::new(HubToMain::SaveState, js(&value("state")));
                world
                    .main
                    .hub_message(&world.fs, window() as f64, &message)
                    .expect("save-state");
                continue;
            }
            _ => {}
        }
        world.seen = Observed::default();
        let (want, replies) = recorded(record);
        world.replies = replies;
        *tally.entry("asked".to_owned()).or_default() += want.dialogs.len();
        *tally.entry("sent".to_owned()).or_default() += want.sent.len();
        *tally.entry("created".to_owned()).or_default() += want.created.len();
        *tally.entry("written".to_owned()).or_default() += want.files.len();
        *tally.entry("errors".to_owned()).or_default() +=
            want.throw.iter().len() + want.uncaught.len() + want.rejections.len();
        match op.as_str() {
            "message" => {
                let name = record.string("name").expect("name");
                world.message(window(), &name, js(&value("data")));
            }
            "openFile" => {
                let path = text(&value("path"));
                let opened = world.route(std::slice::from_ref(&path), |hubs| hubs.open_file(&path));
                if let Err(e) = opened {
                    world.seen.throw = Some(e.name.to_string());
                }
            }
            "openLogs" => {
                let gone = value("gone").as_bool().expect("gone");
                world.open_logs(window(), gone);
            }
            "domReady" => world.dom_ready(window()),
            "close" => world.close(window()),
            "focus" => world.manager.focus(window()),
            "akit" => world.akit(),
            "page" => world.page(window(), &js(&value("data"))),
            other => panic!("{at}: unknown op {other}"),
        }
        for name in world.fs.take_writes() {
            let data = world.fs.read(&name).ok();
            world.seen.files.push((name, data));
        }
        assert_eq!(world.seen, want, "{at}");
        assert!(world.replies.is_empty(), "{at}: not asked for");
    }
    tally
}

#[test]
fn while_running() {
    let tally = replay("running");
    for (what, least) in [
        ("message", 155),
        ("openFile", 13),
        ("openLogs", 7),
        ("akit", 6),
        ("page", 76),
        ("domReady", 48),
        ("close", 12),
        ("asked", 124),
        ("sent", 201),
        ("created", 51),
        ("written", 6),
        ("errors", 8),
    ] {
        assert!(tally[what] >= least, "{what}: {tally:?}");
    }
}

#[test]
fn at_startup() {
    for scenario in [
        "startup-plain",
        "startup-argv",
        "startup-ds",
        "startup-ds-saved",
    ] {
        let tally = replay(scenario);
        assert!(tally["domReady"] > 0 && tally["sent"] > 0, "{tally:?}");
        assert_eq!(tally["asked"], usize::from(scenario != "startup-plain"));
    }
}
