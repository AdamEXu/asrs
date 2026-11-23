//! Replays `files/download-darwin` and `files/download-win32` (main.ts's download section against a
//! scripted ssh2 Client, fake timers and a scripted save folder) and `files/download-window`
//! (download.ts's start message and alert texts).

use std::collections::{HashMap, HashSet};

use asrs_js::string::JsString;
use asrs_parity::cve1::Value;
use asrs_parity::vectors::{check_records, load};
use asrs_parity::{Json, Record};
use asrs_services::dialog::OpenDialog;
use asrs_services::download::downloader::{
    ClientId, ConnectOptions, DirEntry, DownloadHost, Downloader, ListItem, OpenLogBox, Progress,
    SaveLogDialog, SftpId, Thrown, TimerId, Token, WindowMessage,
};
use asrs_services::download::window::{friendly_error, normalize_address};
use asrs_services::path::PathStyle;

fn text(json: &Json, key: &str) -> String {
    json.get(key)
        .and_then(Json::as_str)
        .unwrap_or_else(|| panic!("no string {key:?} in {}", json.to_compact()))
        .to_string_lossy()
}

fn number(json: &Json, key: &str) -> f64 {
    json.get(key)
        .and_then(Json::as_f64)
        .unwrap_or_else(|| panic!("no number {key:?} in {}", json.to_compact()))
}

fn index(json: &Json, key: &str) -> u32 {
    number(json, key) as u32
}

fn flag(json: &Json, key: &str) -> bool {
    match json.get(key) {
        Some(Json::Bool(value)) => *value,
        other => panic!("{key:?} is {other:?}"),
    }
}

fn optional_text(json: &Json, key: &str) -> Option<String> {
    match json.get(key) {
        Some(Json::Null) | None => None,
        Some(_) => Some(text(json, key)),
    }
}

fn tree(value: Value) -> Json {
    asrs_parity::json::parse(&value.encode()).expect("CVE-1 is JSON")
}

fn s(value: &str) -> Value {
    Value::string(value)
}

fn strings(values: &[&str]) -> Value {
    Value::Array(values.iter().map(|v| s(v)).collect())
}

fn n(value: f64) -> Json {
    Json::Number(value)
}

fn obj(pairs: Vec<(&str, Json)>) -> Json {
    Json::object(pairs)
}

struct Host {
    effects: Vec<Json>,
    clients: Vec<bool>,
    sftps: Vec<bool>,
    next_token: u32,
    next_timer: u32,
    next_dialog: u32,
    disk: HashMap<String, Option<f64>>,
    mkdir_throws: bool,
}

const DIR_SIZE: f64 = 64.0;

impl Host {
    fn token(&mut self) -> Token {
        self.next_token += 1;
        self.next_token - 1
    }

    fn dialog(&mut self, kind: &str, options: Value) -> Token {
        let token = self.next_dialog;
        self.next_dialog += 1;
        self.effects.push(obj(vec![
            ("k", Json::string(kind)),
            ("token", n(token.into())),
            ("options", tree(options)),
        ]));
        token
    }

    fn timer(&mut self, kind: &str, ms: f64) -> TimerId {
        self.next_timer += 1;
        self.effects.push(obj(vec![
            ("k", Json::string(kind)),
            ("id", n(self.next_timer.into())),
            ("ms", n(ms)),
        ]));
        self.next_timer
    }

    fn clear(&mut self, kind: &str, id: TimerId) {
        self.effects
            .push(obj(vec![("k", Json::string(kind)), ("id", n(id.into()))]));
    }

    fn request(
        &mut self,
        sftp: SftpId,
        mut effect: Vec<(&'static str, Json)>,
    ) -> Result<Token, String> {
        let dead = self.sftps[sftp as usize];
        if dead {
            let message = "Instance unusable after fatal error";
            effect.push(("threw", Json::string(message)));
            self.effects.push(obj(effect));
            return Err(message.to_owned());
        }
        let token = self.token();
        effect.push(("token", n(token.into())));
        self.effects.push(obj(effect));
        Ok(token)
    }
}

fn item_tree(item: &ListItem) -> Value {
    Value::object([
        ("name", s(&item.name)),
        ("size", item.size.map_or(Value::Undefined, Value::number)),
        ("isFolder", Value::Bool(item.is_folder)),
    ])
}

impl DownloadHost for Host {
    fn connect(&mut self, options: &ConnectOptions) -> ClientId {
        let client = self.clients.len() as u32;
        self.clients.push(true);
        let config = Value::object([
            ("host", s(&options.host)),
            ("port", Value::number(options.port.into())),
            ("forceIPv4", Value::Bool(true)),
            ("readyTimeout", Value::number(options.ready_timeout_ms)),
            ("username", s(options.username)),
            ("password", s(options.password)),
        ]);
        self.effects.push(obj(vec![
            ("k", Json::string("connect")),
            ("client", n(client.into())),
            ("config", tree(config)),
        ]));
        client
    }

    fn end(&mut self, client: ClientId) {
        self.effects.push(obj(vec![
            ("k", Json::string("end")),
            ("client", n(client.into())),
        ]));
    }

    fn sftp(&mut self, client: ClientId) -> Result<Token, String> {
        let mut effect = vec![("k", Json::string("sftp")), ("client", n(client.into()))];
        if !self.clients[client as usize] {
            effect.push(("threw", Json::string("Not connected")));
            self.effects.push(obj(effect));
            return Err("Not connected".to_owned());
        }
        let token = self.token();
        effect.push(("token", n(token.into())));
        self.effects.push(obj(effect));
        Ok(token)
    }

    fn readdir(&mut self, sftp: SftpId, path: &str) -> Result<Token, String> {
        self.request(
            sftp,
            vec![
                ("k", Json::string("readdir")),
                ("sftp", n(sftp.into())),
                ("path", Json::string(path)),
            ],
        )
    }

    fn fast_get(&mut self, sftp: SftpId, remote: &str, local: &str) -> Result<Token, String> {
        self.request(
            sftp,
            vec![
                ("k", Json::string("fastGet")),
                ("sftp", n(sftp.into())),
                ("remote", Json::string(remote)),
                ("local", Json::string(local)),
            ],
        )
    }

    fn set_timeout(&mut self, ms: f64) -> TimerId {
        self.timer("timeout", ms)
    }

    fn clear_timeout(&mut self, id: TimerId) {
        self.clear("clearTimeout", id);
    }

    fn set_interval(&mut self, ms: f64) -> TimerId {
        self.timer("interval", ms)
    }

    fn clear_interval(&mut self, id: TimerId) {
        self.clear("clearInterval", id);
    }

    fn send(&mut self, message: WindowMessage) {
        let (name, data) = match message {
            WindowMessage::SetList(items) => (
                "set-list",
                Value::Array(items.iter().map(item_tree).collect()),
            ),
            WindowMessage::ShowError(text) => ("show-error", s(&text)),
            WindowMessage::ShowAlert(text) => ("show-alert", s(&text)),
            WindowMessage::SetProgress(progress) => (
                "set-progress",
                match progress {
                    Progress::Preparing => Value::number(0.0),
                    Progress::Finished => Value::number(1.0),
                    Progress::Transferring { current, total } => Value::object([
                        ("current", Value::number(current)),
                        ("total", Value::number(total)),
                    ]),
                },
            ),
        };
        self.effects.push(obj(vec![
            ("k", Json::string("send")),
            ("name", Json::string(name)),
            ("data", tree(data)),
        ]));
    }

    fn show_open_dialog(&mut self, dialog: &OpenDialog) -> Token {
        assert_eq!((dialog.message, dialog.filters), (None, None));
        let options = Value::object([
            ("title", s(&dialog.title)),
            ("buttonLabel", s(dialog.button_label.expect("buttonLabel"))),
            ("properties", strings(dialog.properties)),
        ]);
        self.dialog("open", options)
    }

    fn show_save_dialog(&mut self, dialog: &SaveLogDialog) -> Token {
        let filter = Value::object([
            ("name", s(dialog.filter_name)),
            ("extensions", strings(&[&dialog.extension])),
        ]);
        let options = Value::object([
            ("title", s(dialog.title)),
            ("defaultPath", s(&dialog.default_path)),
            ("properties", strings(dialog.properties)),
            ("filters", Value::Array(vec![filter])),
        ]);
        self.dialog("save", options)
    }

    fn show_message_box(&mut self, options: &OpenLogBox) -> Token {
        let options = Value::object([
            ("type", s(options.kind)),
            ("message", s(options.message)),
            ("detail", s(&options.detail)),
            ("icon", s("WINDOW_ICON")),
            ("buttons", strings(&options.buttons)),
            ("defaultId", Value::number(options.default_id as f64)),
        ]);
        self.dialog("box", options)
    }

    fn destroy_window(&mut self) {
        self.effects
            .push(obj(vec![("k", Json::string("destroyWindow"))]));
    }

    fn focus_hub(&mut self) {
        self.effects.push(obj(vec![("k", Json::string("focusHub"))]));
    }

    fn open_files(&mut self, files: &[String]) {
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        let data = Value::object([("files", strings(&files)), ("merge", Value::Bool(false))]);
        self.effects.push(obj(vec![
            ("k", Json::string("openFiles")),
            ("data", tree(data)),
        ]));
    }

    fn exists(&mut self, path: &str) -> bool {
        self.disk.contains_key(path)
    }

    fn file_size(&mut self, path: &str) -> Result<f64, String> {
        self.disk
            .get(path)
            .copied()
            .flatten()
            .ok_or_else(|| format!("stat {path}"))
    }

    fn mkdir_all(&mut self, path: &str) -> Thrown {
        self.effects.push(obj(vec![
            ("k", Json::string("mkdir")),
            ("path", Json::string(path)),
            ("recursive", Json::Bool(true)),
        ]));
        if self.mkdir_throws {
            return Err("EACCES".to_owned());
        }
        self.disk.insert(path.to_owned(), Some(DIR_SIZE));
        Ok(())
    }
}

struct Replay {
    host: Host,
    downloader: Downloader,
    window_open: bool,
    saves: HashSet<Token>,
    /// Clients whose `ready` fired: upstream listens once.
    ready: HashSet<ClientId>,
    effects: HashMap<String, usize>,
}

fn entries(list: &Json) -> Vec<DirEntry> {
    let Json::Array(list) = list else {
        panic!("list is {}", list.to_compact());
    };
    list.iter()
        .map(|entry| DirEntry {
            filename: text(entry, "filename"),
            mode: match entry.get("mode") {
                Some(Json::Null) => None,
                _ => Some(index(entry, "mode")),
            },
            size: match entry.get("size") {
                Some(Json::Null) => None,
                _ => Some(number(entry, "size")),
            },
        })
        .collect()
}

impl Replay {
    fn step(&mut self, record: &Record) -> Result<(), String> {
        let op = record.string("op").ok_or("no op")?;
        let empty = Json::Object(Vec::new());
        let args = record.field("args").unwrap_or(&empty);
        let host = &mut self.host;
        let downloader = &mut self.downloader;
        let mut threw = None;
        match op.as_str() {
            "open" => {
                if !self.window_open {
                    downloader.window_opened();
                    self.window_open = true;
                }
            }
            "userClose" => {
                downloader.window_closed(host);
                self.window_open = false;
            }
            "licenses" => downloader.stop(host),
            "start" => downloader.start_message(&text(args, "address"), &text(args, "path"), host),
            "close" => downloader.close_message(host),
            "save" => {
                let files = args
                    .get("files")
                    .and_then(Json::as_array)
                    .ok_or("files")?
                    .iter()
                    .map(|f| f.as_str().expect("a name").to_string_lossy())
                    .collect();
                downloader.save_message(files, host);
            }
            "ready" => {
                if self.ready.insert(index(args, "client")) {
                    threw = downloader.on_ready(host).err();
                }
            }
            "error" => downloader.on_error(&text(args, "message"), host),
            "setConnected" => {
                host.clients[index(args, "client") as usize] = flag(args, "connected");
            }
            "setDead" => host.sftps[index(args, "sftp") as usize] = flag(args, "dead"),
            "sftpResult" => {
                let result = match optional_text(args, "error") {
                    Some(message) => Err(message),
                    None => {
                        host.sftps.push(false);
                        Ok(host.sftps.len() as u32 - 1)
                    }
                };
                threw = downloader.on_sftp(index(args, "token"), result, host).err();
            }
            "readdirResult" => {
                let result = match optional_text(args, "error") {
                    Some(message) => Err(message),
                    None => Ok(entries(args.get("list").ok_or("list")?)),
                };
                downloader.on_readdir(index(args, "token"), result, host);
            }
            "progress" => downloader.on_step(
                index(args, "token"),
                number(args, "transferred"),
                number(args, "total"),
                host,
            ),
            "getResult" => {
                let result = optional_text(args, "error").map_or(Ok(()), Err);
                downloader.on_got(index(args, "token"), result, host);
            }
            "timer" => threw = downloader.on_timer(index(args, "id"), host).err(),
            "answer" => {
                let value = args.get("value").ok_or("value")?;
                let token = index(args, "token");
                if value.get("response").is_some() {
                    downloader.on_open_log(token, index(value, "response") as usize, host);
                }
                if value.get("canceled").is_some() {
                    let path = (!flag(value, "canceled")).then(|| {
                        value
                            .get("filePath")
                            .filter(|_| self.saves.contains(&token))
                            .map_or_else(
                                || {
                                    value
                                        .get("filePaths")
                                        .and_then(Json::as_array)
                                        .expect("filePaths")[0]
                                        .as_str()
                                        .expect("a path")
                                        .to_string_lossy()
                                },
                                |path| path.as_str().expect("a path").to_string_lossy(),
                            )
                    });
                    downloader.on_save_location(token, path, host);
                }
            }
            "local" => {
                let path = text(args, "path");
                match args.get("entry").ok_or("entry")? {
                    Json::Null => host.disk.remove(&path),
                    Json::Number(size) => host.disk.insert(path, Some(*size)),
                    _ if text(args, "entry") == "dir" => host.disk.insert(path, Some(DIR_SIZE)),
                    _ => host.disk.insert(path, None),
                };
            }
            "setMkdirThrows" => host.mkdir_throws = flag(args, "throws"),
            other => return Err(format!("unknown op {other:?}")),
        }
        for effect in &host.effects {
            let kind = text(effect, "k");
            match kind.as_str() {
                "save" => {
                    self.saves.insert(index(effect, "token"));
                }
                "destroyWindow" => self.window_open = false,
                _ => {}
            }
            *self.effects.entry(kind).or_default() += 1;
        }
        if threw != record.string("threw") {
            return Err(format!("threw {threw:?}, expected {:?}", record.string("threw")));
        }
        let expected = record.field("effects").ok_or("effects")?.to_compact();
        let actual = Json::Array(std::mem::take(&mut host.effects)).to_compact();
        if expected != actual {
            return Err(format!("{op}\nexpected {expected}\nactual   {actual}"));
        }
        Ok(())
    }
}

fn replay(id: &str, style: PathStyle, platform: &str) {
    let records = load(id).expect("vector");
    assert_eq!(records[0].string("platform").as_deref(), Some(platform));
    let mut replay = Replay {
        host: Host {
            effects: Vec::new(),
            clients: Vec::new(),
            sftps: Vec::new(),
            next_token: 0,
            next_timer: 0,
            next_dialog: 0,
            disk: HashMap::new(),
            mkdir_throws: false,
        },
        downloader: Downloader::new(style),
        window_open: false,
        saves: HashSet::new(),
        ready: HashSet::new(),
        effects: HashMap::new(),
    };
    for record in &records[1..] {
        if let Err(e) = replay.step(record) {
            panic!("{id} line {}: {e}", record.line);
        }
    }
    for kind in [
        "connect", "end", "sftp", "readdir", "fastGet", "timeout", "interval", "clearTimeout",
        "clearInterval", "send", "open", "save", "box", "destroyWindow", "focusHub", "openFiles",
        "mkdir",
    ] {
        assert!(replay.effects.contains_key(kind), "{id} never reached {kind}");
    }
}

#[test]
fn main_process_posix() {
    replay("files/download-darwin", PathStyle::Posix, "darwin");
}

#[test]
fn main_process_windows() {
    replay("files/download-win32", PathStyle::Win32, "win32");
}

#[test]
fn window() {
    let (mut starts, mut errors, mut alerts) = (0, 0, 0);
    check_records("files/download-window", |record| {
        let preferences = record.field("preferences").ok_or("preferences")?;
        let platform = record.string("platform").ok_or("platform")?;
        let mut model = asrs_model::preferences::Preferences::default();
        let fixed = text(preferences, "systemcoreStaticAddress");
        model.systemcore_static_address = match fixed.as_str() {
            "" => asrs_model::preferences::SystemcoreStaticAddress::Off,
            "usb" => asrs_model::preferences::SystemcoreStaticAddress::Usb,
            "wifi" => asrs_model::preferences::SystemcoreStaticAddress::Wifi,
            other => return Err(format!("static address {other}")),
        };
        model.robot_address = JsString::from(text(preferences, "robotAddress"));
        let path = JsString::from(text(preferences, "remotePath"));
        let address = normalize_address(&model.robot_address_for(&platform));
        if let Some(start) = record.field("start") {
            starts += 1;
            let expected = Json::Array(vec![obj(vec![
                ("name", Json::string("start")),
                (
                    "data",
                    obj(vec![
                        ("address", Json::string(&address.to_string_lossy())),
                        ("path", Json::string(&path.to_string_lossy())),
                    ]),
                ),
            ])]);
            return if start.to_compact() == expected.to_compact() {
                Ok(())
            } else {
                Err(format!("{} vs {}", start.to_compact(), expected.to_compact()))
            };
        }
        let message = record.string("message").ok_or("message")?;
        let actual = match record.string("name").as_deref() {
            Some("show-error") => {
                errors += 1;
                friendly_error(&message, &address, &path).to_string_lossy()
            }
            Some("show-alert") => {
                alerts += 1;
                message
            }
            other => return Err(format!("message {other:?}")),
        };
        if record.field("hidden") != Some(&Json::Bool(false)) {
            return Err("the alert stayed hidden".to_owned());
        }
        match record.string("text") {
            Some(text) if text == actual => Ok(()),
            other => Err(format!("expected {other:?}, got {actual:?}")),
        }
    });
    assert!(starts > 50 && errors > 200 && alerts > 200, "{starts} {errors} {alerts}");
}
