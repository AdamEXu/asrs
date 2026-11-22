#![allow(clippy::disallowed_methods)]

//! Replays `video/*` vectors against `asrs_services::video`. Flow vectors are upstream's
//! VideoProcessor running against scripted fakes: every effect it made (fs, spawn, dialogs,
//! download, fetch, menu, timers, logs, `dataCallback`) is compared in order with the port's
//! [`VideoHost`] calls, the recorded results are handed back, and every recorded event is
//! delivered to the port in the recorded order.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::collections::{BTreeMap, VecDeque};

use asrs_js::string::{JsError, JsString};
use asrs_js::value::JsValue;
use asrs_parity::cve1::Value;
use asrs_parity::json::Json;
use asrs_parity::utf16::Utf16String;
use asrs_parity::vectors::{self, Record};
use asrs_services::video::{
    self, FetchResponse, Handle, LogArg, LogLevel, MessageBox, OpenDialog, VideoConfig, VideoEvent,
    VideoHost, VideoProcessor,
};

/// A recorded JSON object whose every field must be read before [`Fields::finish`].
struct Fields {
    ctx: String,
    pairs: Vec<(Utf16String, Json)>,
    used: Vec<bool>,
}

impl Fields {
    fn new(json: &Json, ctx: &str) -> Fields {
        match json {
            Json::Object(pairs) => Fields {
                ctx: ctx.to_owned(),
                used: vec![false; pairs.len()],
                pairs: pairs.clone(),
            },
            other => panic!("{ctx}: expected an object, got {}", other.to_compact()),
        }
    }

    fn has(&self, key: &str) -> bool {
        self.pairs.iter().any(|(k, _)| k.eq_str(key))
    }

    fn take(&mut self, key: &str) -> Json {
        let i = self
            .pairs
            .iter()
            .position(|(k, _)| k.eq_str(key))
            .unwrap_or_else(|| panic!("{}: no field {key:?}", self.ctx));
        assert!(!self.used[i], "{}: field {key:?} read twice", self.ctx);
        self.used[i] = true;
        self.pairs[i].1.clone()
    }

    fn string(&mut self, key: &str) -> String {
        let ctx = self.ctx.clone();
        utf16(&self.take(key))
            .as_string()
            .unwrap_or_else(|| panic!("{ctx}: {key:?} is not valid UTF-16"))
    }

    fn js_string(&mut self, key: &str) -> JsString {
        JsString::from_units(utf16(&self.take(key)).units().to_vec())
    }

    fn opt_string(&mut self, key: &str) -> Option<String> {
        match self.take(key) {
            Json::Null => None,
            other => Some(
                utf16(&other)
                    .as_string()
                    .unwrap_or_else(|| panic!("{}: {key:?} is not valid UTF-16", self.ctx)),
            ),
        }
    }

    fn number(&mut self, key: &str) -> f64 {
        let ctx = self.ctx.clone();
        self.take(key)
            .as_f64()
            .unwrap_or_else(|| panic!("{ctx}: {key:?} is not a number"))
    }

    fn opt_number(&mut self, key: &str) -> Option<f64> {
        match self.take(key) {
            Json::Null => None,
            Json::Number(n) => Some(n),
            other => panic!("{}: {key:?} is {}", self.ctx, other.to_compact()),
        }
    }

    fn bool(&mut self, key: &str) -> bool {
        match self.take(key) {
            Json::Bool(b) => b,
            other => panic!("{}: {key:?} is {}", self.ctx, other.to_compact()),
        }
    }

    fn hex(&mut self, key: &str) -> Vec<u8> {
        let text = self.string(key);
        assert!(
            text.len().is_multiple_of(2),
            "{}: odd hex in {key:?}",
            self.ctx
        );
        (0..text.len())
            .step_by(2)
            .map(|i| {
                u8::from_str_radix(&text[i..i + 2], 16)
                    .unwrap_or_else(|e| panic!("{}: {key:?}: {e}", self.ctx))
            })
            .collect()
    }

    fn tree(&mut self, key: &str) -> Value {
        let ctx = self.ctx.clone();
        Value::from_json(&self.take(key)).unwrap_or_else(|e| panic!("{ctx}: {key:?}: {e:?}"))
    }

    fn handle(&mut self, key: &str) -> Handle {
        let n = self.number(key);
        assert!(n >= 0.0 && n.fract() == 0.0, "{}: bad handle {n}", self.ctx);
        n as Handle
    }

    fn expect_str(&mut self, key: &str, actual: &str) {
        let expected = self.string(key);
        assert_eq!(expected, actual, "{}: {key}", self.ctx);
    }

    fn expect_tree(&mut self, key: &str, actual: &Value) {
        let expected = self.tree(key);
        assert_eq!(expected.encode(), actual.encode(), "{}: {key}", self.ctx);
    }

    fn finish(self) {
        let unread: Vec<String> = self
            .pairs
            .iter()
            .zip(&self.used)
            .filter(|(_, used)| !**used)
            .map(|((k, _), _)| k.to_string_lossy())
            .collect();
        assert!(unread.is_empty(), "{}: unread fields {unread:?}", self.ctx);
    }
}

fn utf16(json: &Json) -> Utf16String {
    json.as_str()
        .unwrap_or_else(|| panic!("expected a string, got {}", json.to_compact()))
        .clone()
}

fn js(s: &JsString) -> Value {
    Value::String(Utf16String::from_units(s.units().to_vec()))
}

fn text(s: &str) -> Value {
    Value::string(s)
}

fn strings(items: &[String]) -> Value {
    Value::Array(items.iter().map(|s| text(s)).collect())
}

fn number(x: f64) -> Value {
    Value::number(x)
}

fn error_name(name: &str) -> &'static str {
    match name {
        "Error" => "Error",
        "TypeError" => "TypeError",
        "RangeError" => "RangeError",
        "SyntaxError" => "SyntaxError",
        other => panic!("unexpected recorded error name {other:?}"),
    }
}

fn recorded_error(json: &Json, ctx: &str) -> JsError {
    let mut f = Fields::new(json, ctx);
    let name = error_name(&f.string("name"));
    let code = f.opt_string("code").unwrap_or_default();
    f.finish();
    JsError::new(name, code)
}

fn message_box_tree(options: &MessageBox) -> Value {
    let mut pairs: Vec<(Utf16String, Value)> = vec![
        ("type".into(), text(&options.kind)),
        ("title".into(), text(&options.title)),
        ("message".into(), text(&options.message)),
        ("detail".into(), text(&options.detail)),
    ];
    if let Some(buttons) = &options.buttons {
        pairs.push(("buttons".into(), strings(buttons)));
    }
    if let Some(id) = options.default_id {
        pairs.push(("defaultId".into(), number(id)));
    }
    pairs.push((
        "icon".into(),
        options
            .icon
            .as_deref()
            .map(text)
            .unwrap_or(Value::Undefined),
    ));
    Value::Object(pairs)
}

fn open_dialog_tree(options: &OpenDialog) -> Value {
    Value::object([
        ("title", text(&options.title)),
        ("properties", strings(&options.properties)),
        (
            "filters",
            Value::Array(
                options
                    .filters
                    .iter()
                    .map(|(name, extensions)| {
                        Value::object([("name", text(name)), ("extensions", strings(extensions))])
                    })
                    .collect(),
            ),
        ),
    ])
}

fn log_level(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Log => "log",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    }
}

#[derive(Default)]
struct Tally {
    recorded: BTreeMap<String, usize>,
    compared: BTreeMap<String, usize>,
}

impl Tally {
    fn record(&mut self, kind: &str) {
        *self.recorded.entry(kind.to_owned()).or_default() += 1;
    }

    fn compare(&mut self, kind: &str) {
        *self.compared.entry(kind.to_owned()).or_default() += 1;
    }

    fn assert_complete(&self, id: &str) {
        assert_eq!(
            self.recorded, self.compared,
            "{id}: compared counts differ from recorded counts"
        );
    }
}

struct ReplayHost<'t> {
    ctx: String,
    expected: VecDeque<Json>,
    tally: &'t mut Tally,
}

impl ReplayHost<'_> {
    fn next(&mut self, kind: &str, actual: impl FnOnce() -> String) -> Fields {
        let ctx = self.ctx.clone();
        let Some(effect) = self.expected.pop_front() else {
            panic!("{ctx}: port made an extra effect {kind}: {}", actual());
        };
        let mut f = Fields::new(&effect, &format!("{ctx}: effect {}", effect.to_compact()));
        let recorded = f.string("k");
        assert_eq!(
            recorded,
            kind,
            "{ctx}: expected effect {}, port made {kind}: {}",
            effect.to_compact(),
            actual()
        );
        self.tally.compare(&format!("effect:{kind}"));
        f
    }

    fn unit_result(f: &mut Fields) -> Result<(), JsError> {
        if f.has("throw") {
            let ctx = f.ctx.clone();
            Err(recorded_error(&f.take("throw"), &ctx))
        } else {
            Ok(())
        }
    }

    fn finish_step(&self) {
        assert!(
            self.expected.is_empty(),
            "{}: port did not make {} recorded effects, first {}",
            self.ctx,
            self.expected.len(),
            self.expected[0].to_compact()
        );
    }
}

impl VideoHost for ReplayHost<'_> {
    fn exists(&mut self, path: &str) -> bool {
        let mut f = self.next("exists", || path.to_owned());
        f.expect_str("path", path);
        let r = f.bool("r");
        f.finish();
        r
    }

    fn rm_recursive(&mut self, path: &str) -> Result<(), JsError> {
        let mut f = self.next("rmSync", || path.to_owned());
        f.expect_str("path", path);
        f.expect_tree(
            "options",
            &Value::object([("recursive", Value::Bool(true))]),
        );
        let r = Self::unit_result(&mut f);
        f.finish();
        r
    }

    fn mkdir_recursive(&mut self, path: &str) -> Result<(), JsError> {
        let mut f = self.next("mkdirSync", || path.to_owned());
        f.expect_str("path", path);
        f.expect_tree(
            "options",
            &Value::object([("recursive", Value::Bool(true))]),
        );
        let r = Self::unit_result(&mut f);
        f.finish();
        r
    }

    fn read_file(&mut self, path: &str) -> Result<Vec<u8>, JsError> {
        let mut f = self.next("readFile", || path.to_owned());
        f.expect_str("path", path);
        let r = Self::unit_result(&mut f).map(|()| f.hex("r"));
        f.finish();
        r
    }

    fn read_file_sync(&mut self, path: &str) -> Result<Vec<u8>, JsError> {
        let mut f = self.next("readFileSync", || path.to_owned());
        f.expect_str("path", path);
        f.expect_tree("options", &Value::Object(Vec::new()));
        let r = Self::unit_result(&mut f).map(|()| f.hex("r"));
        f.finish();
        r
    }

    fn unlink(&mut self, path: &str) -> Result<(), JsError> {
        let mut f = self.next("unlink", || path.to_owned());
        f.expect_str("path", path);
        let r = Self::unit_result(&mut f);
        f.finish();
        r
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), JsError> {
        let mut f = self.next("rename", || format!("{from} -> {to}"));
        f.expect_str("from", from);
        f.expect_str("to", to);
        let r = Self::unit_result(&mut f);
        f.finish();
        r
    }

    fn chmod(&mut self, path: &str, mode: u32) -> Result<(), JsError> {
        let mut f = self.next("chmod", || format!("{path} {mode:o}"));
        f.expect_str("path", path);
        assert_eq!(f.number("mode"), f64::from(mode), "{}: mode", f.ctx);
        let r = Self::unit_result(&mut f);
        f.finish();
        r
    }

    fn sha256_hex(&mut self, data: &[u8]) -> String {
        let mut f = self.next("hash", || format!("{} bytes", data.len()));
        f.expect_str("alg", "sha256");
        f.expect_str("encoding", "hex");
        assert_eq!(f.hex("data"), data, "{}: hashed bytes", f.ctx);
        let r = f.string("r");
        f.finish();
        r
    }

    fn random(&mut self) -> f64 {
        let mut f = self.next("random", String::new);
        let r = f
            .tree("r")
            .as_f64()
            .unwrap_or_else(|| panic!("{}: random is not a number", f.ctx));
        f.finish();
        r
    }

    fn read_clipboard(&mut self) -> JsString {
        let mut f = self.next("clipboard", String::new);
        let r = f.js_string("r");
        f.finish();
        r
    }

    fn window_destroyed(&mut self) -> bool {
        let mut f = self.next("isDestroyed", String::new);
        let r = f.bool("r");
        f.finish();
        r
    }

    fn spawn(
        &mut self,
        command: &str,
        args: &[JsString],
        cwd: Option<&str>,
    ) -> Result<Handle, JsError> {
        let mut f = self.next("spawn", || format!("{command} {args:?} {cwd:?}"));
        f.expect_str("cmd", command);
        f.expect_tree("args", &Value::Array(args.iter().map(js).collect()));
        let recorded_cwd = f.opt_string("cwd");
        assert_eq!(recorded_cwd.as_deref(), cwd, "{}: cwd", f.ctx);
        let r = if f.has("throw") {
            let ctx = f.ctx.clone();
            Err(recorded_error(&f.take("throw"), &ctx))
        } else {
            Ok(f.handle("id"))
        };
        f.finish();
        r
    }

    fn kill(&mut self, process: Handle) -> bool {
        let mut f = self.next("kill", || process.to_string());
        assert_eq!(f.handle("id"), process, "{}: killed process", f.ctx);
        let r = f.bool("r");
        f.finish();
        r
    }

    fn message_box(&mut self, options: &MessageBox) -> Handle {
        let mut f = self.next("messageBox", || format!("{options:?}"));
        f.expect_tree("options", &message_box_tree(options));
        let id = f.handle("id");
        f.finish();
        id
    }

    fn open_dialog(&mut self, options: &OpenDialog) -> Handle {
        let mut f = self.next("openDialog", || format!("{options:?}"));
        f.expect_tree("options", &open_dialog_tree(options));
        let id = f.handle("id");
        f.finish();
        id
    }

    fn download(&mut self, url: &str, folder: &str, filename: &str) -> Handle {
        let mut f = self.next("download", || format!("{url} {folder} {filename}"));
        f.expect_str("url", url);
        f.expect_str("folder", folder);
        f.expect_tree("options", &Value::object([("filename", text(filename))]));
        let id = f.handle("id");
        f.finish();
        id
    }

    fn fetch(
        &mut self,
        url: &JsString,
        headers: &[(JsString, JsValue)],
        timeout_ms: f64,
    ) -> Handle {
        let mut f = self.next("fetch", || url.to_string_lossy());
        f.expect_tree("url", &js(url));
        f.expect_tree("method", &text("GET"));
        f.expect_tree(
            "headers",
            &Value::Array(
                headers
                    .iter()
                    .map(|(name, value)| Value::Array(vec![js(name), js_json::from_js(value)]))
                    .collect(),
            ),
        );
        f.expect_tree("timeout", &number(timeout_ms));
        let id = f.handle("id");
        f.finish();
        id
    }

    fn popup_menu(&mut self, labels: &[JsString], x: f64, y: f64) -> Handle {
        let mut f = self.next("menu", || format!("{labels:?} {x} {y}"));
        f.expect_tree("labels", &Value::Array(labels.iter().map(js).collect()));
        f.expect_tree("x", &number(x));
        f.expect_tree("y", &number(y));
        let id = f.handle("id");
        f.finish();
        id
    }

    fn set_timeout(&mut self, ms: f64) -> Handle {
        let mut f = self.next("setTimeout", || ms.to_string());
        f.expect_tree("ms", &number(ms));
        let id = f.handle("id");
        f.finish();
        id
    }

    fn read_timer_text(&mut self, path: &JsString, width: f64, height: f64) -> Handle {
        let mut f = self.next("timerText", || {
            format!("{} {width} {height}", path.to_string_lossy())
        });
        f.expect_tree("path", &js(path));
        f.expect_tree("width", &number(width));
        f.expect_tree("height", &number(height));
        let id = f.handle("id");
        f.finish();
        id
    }

    fn log(&mut self, level: LogLevel, args: &[LogArg]) {
        let mut f = self.next("log", || format!("{level:?} {args:?}"));
        f.expect_str("level", log_level(level));
        let recorded = f.take("args");
        let recorded = recorded
            .as_array()
            .unwrap_or_else(|| panic!("{}: log args", f.ctx));
        assert_eq!(
            recorded.len(),
            args.len(),
            "{}: log argument count, port {args:?}",
            f.ctx
        );
        for (json, arg) in recorded.iter().zip(args) {
            let mut a = Fields::new(json, &f.ctx);
            match arg {
                LogArg::Text(s) => {
                    let expected = a.js_string("s");
                    assert_eq!(expected, *s, "{}: log text", f.ctx);
                }
                LogArg::Error(name) => {
                    let expected = a.js_string("error");
                    assert_eq!(expected, *name, "{}: logged error name", f.ctx);
                }
            }
            a.finish();
        }
        f.finish();
    }

    fn send(&mut self, data: JsValue) {
        let mut f = self.next("data", || js_json::from_js(&data).encode());
        f.expect_tree("v", &js_json::from_js(&data));
        f.finish();
    }
}

fn event(json: &Json, ctx: &str) -> (String, VideoEvent) {
    let mut f = Fields::new(json, ctx);
    let kind = f.string("e");
    let event = match kind.as_str() {
        "stdout" => VideoEvent::Stdout {
            process: f.handle("id"),
            data: f.hex("data"),
        },
        "stderr" => VideoEvent::Stderr {
            process: f.handle("id"),
            data: f.hex("data"),
        },
        "exit" | "close" => {
            let process = f.handle("id");
            let code = f.opt_number("code").map(|c| c as i32);
            let signal = f.opt_string("signal");
            if kind == "exit" {
                VideoEvent::Exit {
                    process,
                    code,
                    signal,
                }
            } else {
                VideoEvent::Close {
                    process,
                    code,
                    signal,
                }
            }
        }
        "error" => VideoEvent::SpawnError {
            process: f.handle("id"),
            code: f.string("code"),
            errno: f.number("errno") as i32,
            syscall: f.string("syscall"),
            path: f.string("path"),
        },
        "messageBox" => VideoEvent::MessageBoxClosed {
            dialog: f.handle("id"),
            response: f.number("response") as u32,
        },
        "openDialog" => {
            let dialog = f.handle("id");
            let canceled = f.bool("canceled");
            let paths = f.take("filePaths");
            let file_paths = paths
                .as_array()
                .unwrap_or_else(|| panic!("{ctx}: filePaths"))
                .iter()
                .map(|p| {
                    utf16(p)
                        .as_string()
                        .unwrap_or_else(|| panic!("{ctx}: path"))
                })
                .collect();
            VideoEvent::OpenDialogClosed {
                dialog,
                canceled,
                file_paths,
            }
        }
        "download" => VideoEvent::DownloadFinished {
            download: f.handle("id"),
            ok: f.bool("ok"),
        },
        "fetch" => {
            let fetch = f.handle("id");
            let response = if f.bool("ok") {
                Some(FetchResponse {
                    status: f.number("status") as u16,
                    body: f.hex("body"),
                })
            } else {
                None
            };
            VideoEvent::FetchFinished { fetch, response }
        }
        "menuClick" => VideoEvent::MenuClick {
            menu: f.handle("id"),
            index: f.number("index") as usize,
        },
        "menuClose" => VideoEvent::MenuWillClose {
            menu: f.handle("id"),
        },
        "timer" => VideoEvent::TimerFired {
            timer: f.handle("id"),
        },
        "timerText" => VideoEvent::TimerText {
            job: f.handle("id"),
            text: f.js_string("text"),
        },
        other => panic!("{ctx}: unknown event kind {other:?}"),
    };
    f.finish();
    (kind, event)
}

fn config(json: &Json, ctx: &str) -> VideoConfig {
    let mut f = Fields::new(json, ctx);
    let config = VideoConfig {
        platform: f.string("platform"),
        arch: f.string("arch"),
        user_data: f.string("userData"),
        temp: f.string("temp"),
        yt_dlp_binary: f.string("ytDlpBinary"),
        window_icon: match f.tree("windowIcon") {
            Value::Undefined => None,
            Value::String(s) => Some(s.as_string().unwrap_or_else(|| panic!("{ctx}: windowIcon"))),
            other => panic!("{ctx}: windowIcon {}", other.encode()),
        },
    };
    f.finish();
    config
}

/// Replays every case of a flow vector and returns the number of cases.
fn replay_flows(id: &str) -> usize {
    let records = vectors::load(id).unwrap_or_else(|e| panic!("{e}"));
    let mut tally = Tally::default();
    for record in &records {
        if record.field("config").is_some() {
            tally.record("case");
            continue;
        }
        tally.record("step");
        let effects = record
            .field("effects")
            .and_then(|e| e.as_array())
            .unwrap_or_else(|| panic!("{id} line {}: no effects", record.line));
        for effect in effects {
            let kind = effect
                .get("k")
                .and_then(|k| k.as_str())
                .map(|k| k.to_string_lossy());
            tally.record(&format!(
                "effect:{}",
                kind.unwrap_or_else(|| panic!("{id} line {}: effect kind", record.line))
            ));
        }
    }

    let mut port: Option<(usize, String, VideoProcessor)> = None;
    for record in &records {
        let ctx = format!("{id} line {}", record.line);
        let mut f = Fields::new(&record.json, &ctx);
        let case = f.number("case") as usize;
        if f.has("config") {
            let name = f.string("name");
            let config = config(&f.take("config"), &format!("{ctx} ({name})"));
            f.finish();
            let processor = VideoProcessor::new(config.clone());
            assert_eq!(processor.config(), &config, "{ctx}: config");
            port = Some((case, name, processor));
            tally.compare("case");
            continue;
        }
        let (port_case, name, processor) = port
            .as_mut()
            .unwrap_or_else(|| panic!("{ctx}: step before a case header"));
        assert_eq!(*port_case, case, "{ctx}: step for another case");
        let step = f.number("step");
        let ctx = format!("{ctx} ({name} step {step})");
        let effects = f.take("effects");
        let mut host = ReplayHost {
            ctx: ctx.clone(),
            expected: effects
                .as_array()
                .unwrap_or_else(|| panic!("{ctx}: effects"))
                .iter()
                .cloned()
                .collect(),
            tally: &mut tally,
        };
        if f.has("op") {
            let op = f.string("op");
            match op.as_str() {
                "prepare" => {
                    let uuid = f.js_string("uuid");
                    let to_js =
                        |v: Value| js_json::to_js(&v).unwrap_or_else(|e| panic!("{ctx}: {e}"));
                    let source = to_js(f.tree("source"));
                    let match_info = to_js(f.tree("matchInfo"));
                    let menu_coordinates = to_js(f.tree("menuCoordinates"));
                    processor.prepare(&mut host, &uuid, &source, &match_info, &menu_coordinates);
                }
                "cleanup" => processor.cleanup(&mut host),
                other => panic!("{ctx}: unknown op {other:?}"),
            }
        } else {
            let (_, event) = event(&f.take("event"), &ctx);
            processor.handle(&mut host, event);
        }
        host.finish_step();
        drop(host);
        f.finish();
        tally.compare("step");
    }
    tally.assert_complete(id);
    tally.recorded.get("case").copied().unwrap_or(0)
}

fn check_simple(id: &str, mut f: impl FnMut(&Record, &mut Fields)) -> usize {
    let records = vectors::load(id).unwrap_or_else(|e| panic!("{e}"));
    let mut compared = 0;
    for record in &records {
        let mut fields = Fields::new(&record.json, &format!("{id} line {}", record.line));
        f(record, &mut fields);
        fields.finish();
        compared += 1;
    }
    assert_eq!(compared, records.len());
    compared
}

#[test]
fn electron_platform() {
    let n = check_simple("video/platform", |_, f| {
        let platform = f.string("platform");
        let arch = f.string("arch");
        let expected = f.string("result");
        assert_eq!(
            video::get_electron_platform(&platform, &arch),
            expected,
            "{}",
            f.ctx
        );
    });
    assert_eq!(n, 121);
}

#[test]
fn youtube_video_id() {
    let n = check_simple("video/video-id", |_, f| {
        let url = f.js_string("url");
        let expected = match f.tree("result") {
            Value::Null => None,
            Value::String(s) => Some(JsString::from_units(s.units().to_vec())),
            other => panic!("{}: result {}", f.ctx, other.encode()),
        };
        assert_eq!(
            video::get_video_id(&url),
            expected,
            "{}: {}",
            f.ctx,
            url.to_string_lossy()
        );
    });
    assert!(n >= 300, "{n} urls");
}

#[test]
fn flows() {
    assert!(replay_flows("video/flows") >= 150);
}

#[test]
fn recorded_ffmpeg_stderr() {
    assert!(replay_flows("video/stderr") >= 150);
}

#[test]
fn youtube_formats() {
    assert!(replay_flows("video/youtube") >= 120);
}
