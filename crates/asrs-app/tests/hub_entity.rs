#![allow(clippy::disallowed_methods)]

//! `asrs_app::hub::Hub` in a headless GPUI app: every task runs through the GPUI host (executor
//! timers, window frames, decode threads pumped by foreground tasks, `historical-start` on the
//! background executor).
//!
//! Each session of `parity/specs/hub/entity-sessions.requests.jsonl` runs twice: through `asrs-cli
//! historical-session`, whose trace must equal the oracle's (`hub/entity-sessions`), and
//! through the entity, driven by the main messages that make the hub do the same (`open-files`
//! for `open`, `prepare-export` for `loadAll`). The entity's status callbacks must equal the
//! trace's and its `Log` dumps the trace's dumps; its window titles must equal what upstream's
//! hub.ts set at the same points (`hub/entity-titles`).

mod functions {
    use asrs_parity::{Response, Value};

    pub type Function = fn(&[Value]) -> Response;

    pub fn arg(args: &[Value], i: usize) -> Value {
        args.get(i).cloned().unwrap_or(Value::Undefined)
    }
}

#[path = "../../asrs-cli/src/decode.rs"]
#[allow(dead_code)]
mod decode;
#[path = "../../asrs-cli/src/historical_session.rs"]
#[allow(dead_code)]
mod historical_session;
#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code, clippy::disallowed_methods)]
mod js_json;
#[path = "../../asrs-cli/src/log_field.rs"]
#[allow(dead_code)]
mod log_field;
#[path = "../../asrs-cli/src/log_run.rs"]
#[allow(dead_code)]
mod log_run;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use asrs_app::hub::{Hub, HubConfig, HubEvent};
use asrs_formats::historical::Status;
use asrs_hub::hub::HubView;
use asrs_hub::sidebar::{RowRect, RowValue, Sidebar};
use asrs_js::string::JsString;
use asrs_js::time::{SetClock, V8MathRandom};
use asrs_js::value::{JsObject, JsValue};
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage};
use asrs_parity::json::{self, Json};
use asrs_parity::vectors;
use asrs_ui::gpui::{
    AnyWindowHandle, AppContext, Context, Entity, HeadlessAppContext, IntoElement, NoopTextSystem,
    Render, Window, div, px, size,
};

const SESSIONS: &str = "parity/specs/hub/entity-sessions.requests.jsonl";
const TIMEOUT: Duration = Duration::from_secs(120);

fn root() -> PathBuf {
    vectors::workspace_root()
}

fn js(s: &str) -> JsString {
    JsString::from(s)
}

struct View;

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

    fn can_scroll_into_view(&self) -> bool {
        false
    }

    fn sidebar_rects(&self, sidebar: &Sidebar) -> Vec<RowRect> {
        vec![RowRect::default(); sidebar.rows().len()]
    }

    fn measure_value(&self, _value: &RowValue) -> f64 {
        0.0
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

#[derive(Clone, Debug)]
enum Seen {
    Event(HubEvent),
    Title(String),
}

struct Page {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    hub: Entity<Hub>,
    seen: Rc<RefCell<Vec<Seen>>>,
}

impl Page {
    fn new() -> Page {
        let mut cx = HeadlessAppContext::new(Arc::new(NoopTextSystem::new()));
        let slot = Rc::new(RefCell::new(None));
        let built = slot.clone();
        let window = cx
            .open_window(size(px(1100.0), px(650.0)), move |window, cx| {
                let mut config = HubConfig::new(Box::new(View));
                config.worker_realm = Box::new(|| {
                    (
                        Box::new(V8MathRandom::from_entropy()),
                        Box::new(SetClock(decode::WORKER_FROZEN_MS)),
                    )
                });
                let hub = cx.new(|cx| Hub::new(Some(window), config, cx));
                *built.borrow_mut() = Some(hub.clone());
                cx.new(|_| Shell(hub))
            })
            .expect("open a headless window");
        let hub = slot.borrow_mut().take().expect("the window built the hub");
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = seen.clone();
        cx.update(|cx| {
            cx.subscribe(&hub, move |hub, event, cx| {
                let mut sink = sink.borrow_mut();
                if *event == HubEvent::Title {
                    let title = hub.read(cx).model().title().to_string_lossy();
                    sink.push(Seen::Title(title));
                }
                sink.push(Seen::Event(event.clone()));
            })
            .detach();
        });
        Page {
            cx,
            window: window.into(),
            hub,
            seen,
        }
    }

    fn main(&mut self, name: MainToHub, data: JsValue) {
        let hub = self.hub.clone();
        self.cx.update(|cx| {
            hub.update(cx, |hub, cx| {
                hub.handle_main_message(NamedMessage::new(name, data), cx)
            })
        });
    }

    fn read<R>(&mut self, f: impl FnOnce(&Hub) -> R) -> R {
        let hub = self.hub.clone();
        self.cx.update(|cx| f(hub.read(cx)))
    }

    /// Runs the page until `done`, waiting on the decode and export threads in between.
    fn settle(&mut self, what: &str, done: impl Fn(&Hub, &[Seen]) -> bool) {
        let start = Instant::now();
        loop {
            self.cx.run_until_parked();
            let seen = self.seen.clone();
            if self.read(|hub| done(hub, &seen.borrow())) {
                return;
            }
            assert!(start.elapsed() < TIMEOUT, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn frame(&mut self) -> usize {
        self.cx
            .update_window(self.window, |_, window, cx| window.simulate_next_frame(cx))
            .expect("the window is open")
    }

    fn take_seen(&mut self) -> Vec<Seen> {
        std::mem::take(&mut self.seen.borrow_mut())
    }

    fn exceptions(&self) -> Vec<String> {
        self.seen
            .borrow()
            .iter()
            .filter_map(|s| match s {
                Seen::Event(HubEvent::Exception { task, error }) => {
                    Some(format!("{task}: {error}"))
                }
                _ => None,
            })
            .collect()
    }

    fn dump(&mut self, phase: &str) -> Vec<String> {
        self.read(|hub| {
            let model = hub.model();
            let statuses: Vec<&str> = model
                .historical_sources()
                .map(|s| s.status().name())
                .collect();
            let mut loading: Vec<JsString> = Vec::new();
            for source in model.historical_sources() {
                for field in source.get_loading_fields() {
                    if !loading.contains(&field) {
                        loading.push(field);
                    }
                }
            }
            let serialized = model.page().log().to_serialized();
            decode::dump_log(&serialized, phase, &statuses.join(","), &loading)
        })
    }
}

fn open_files(path: &str) -> JsValue {
    let mut o = JsObject::new();
    o.insert("files", JsValue::Array(vec![JsValue::String(js(path))]));
    o.insert("merge", JsValue::Bool(false));
    JsValue::Object(o)
}

fn prepare_export(name: &str) -> JsValue {
    let mut options = JsObject::new();
    options.insert("format", JsValue::string("csv-table"));
    options.insert("samplingMode", JsValue::string("changes"));
    options.insert("samplingPeriod", JsValue::Number(20.0));
    options.insert("prefixes", JsValue::string(""));
    options.insert("includeGenerated", JsValue::Bool(true));
    let mut o = JsObject::new();
    o.insert("path", JsValue::String(js(&format!("/out/{name}.csv"))));
    o.insert("options", JsValue::Object(options));
    JsValue::Object(o)
}

fn last_status(seen: &[Seen]) -> Option<Status> {
    seen.iter().rev().find_map(|s| match s {
        Seen::Event(HubEvent::SourceStatus { source: 0, status }) => Some(*status),
        _ => None,
    })
}

fn wrote_export(seen: &[Seen]) -> bool {
    seen.iter().any(|s| {
        matches!(s, Seen::Event(HubEvent::Main(m))
            if m.name == HubToMain::WriteExport || m.name == HubToMain::Error)
    })
}

/// A CVE-1 trace line's object pairs.
fn pairs(line: &str) -> Vec<(String, Json)> {
    let json = json::parse(line).unwrap_or_else(|e| panic!("{line}: {e}"));
    json.get("o")
        .and_then(Json::as_array)
        .unwrap_or_else(|| panic!("{line}: not an object"))
        .iter()
        .map(|pair| {
            let pair = pair.as_array().expect("a key-value pair");
            let key = pair[0].as_str().expect("a string key").to_string_lossy();
            (key, pair[1].clone())
        })
        .collect()
}

fn text(json: &Json) -> String {
    json.as_str()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default()
}

/// What the entity must reproduce from one session's trace.
struct Expected {
    statuses: Vec<String>,
    dumps: Vec<(String, Vec<String>)>,
}

fn expected(trace: &[&str]) -> Expected {
    let mut statuses = Vec::new();
    let mut dumps: Vec<(String, Vec<String>)> = Vec::new();
    let mut in_dump = false;
    for line in trace {
        let pairs = pairs(line);
        let get = |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v);
        let first = pairs.first().map(|(k, _)| k.as_str()).unwrap_or_default();
        if first == "step" || first == "end" {
            in_dump = get("op").map(text).as_deref() == Some("dump");
            if in_dump {
                dumps.push((String::new(), Vec::new()));
            }
            continue;
        }
        if in_dump {
            let dump = dumps.last_mut().expect("a dump step");
            if let Some(phase) = get("phase") {
                dump.0 = text(phase);
            }
            dump.1.push((*line).to_owned());
            continue;
        }
        if get("event").map(text).as_deref() == Some("status") {
            assert_eq!(
                get("source").map(Json::to_compact).as_deref(),
                Some(r#"{"n":"0000000000000000"}"#),
                "{line}"
            );
            statuses.push(get("status").map(text).unwrap_or_default());
        }
    }
    Expected { statuses, dumps }
}

/// The title changes `titles` makes to a page showing `before`.
fn changes(before: &str, titles: &[String]) -> Vec<String> {
    let mut current = before;
    let mut out = Vec::new();
    for title in titles {
        if title != current {
            out.push(title.clone());
            current = title;
        }
    }
    out
}

/// `window.__titles.splice(0)` at each labelled capture of `hub/entity-titles`.
fn upstream_titles() -> Vec<(String, Vec<String>)> {
    let records = vectors::load("hub/entity-titles").expect("hub/entity-titles");
    let mut label = None;
    let mut out = Vec::new();
    for record in &records {
        if let Some(capture) = record.string("capture") {
            label = Some(capture);
        } else if record.string("eval").as_deref() == Some("window.__titles.splice(0)") {
            let values = record
                .field("value")
                .and_then(Json::as_array)
                .expect("a title list")
                .iter()
                .map(text)
                .collect();
            out.push((label.clone().expect("a capture label"), values));
        }
    }
    out
}

#[test]
fn sessions_match_the_cli_and_upstream() {
    std::env::set_current_dir(root()).expect("enter the workspace");
    let sessions = std::fs::read_to_string(SESSIONS).expect("read the session specs");
    let cli = historical_session::traces(&sessions).expect("asrs-cli historical-session");
    let oracle = vectors::read_text(
        &vectors::vector_path(&vectors::vectors_dir(), "hub/entity-sessions")
            .expect("hub/entity-sessions"),
    )
    .expect("read hub/entity-sessions");
    assert_eq!(
        cli.lines().collect::<Vec<_>>(),
        oracle.lines().collect::<Vec<_>>(),
        "asrs-cli historical-session differs from the oracle"
    );
    let titles = upstream_titles();

    let mut traces = Vec::new();
    for line in cli.lines() {
        if pairs(line).first().map(|(k, _)| k.as_str()) == Some("session") {
            traces.push(Vec::new());
        }
        traces.last_mut().expect("a session line first").push(line);
    }
    let specs: Vec<Json> = sessions
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| json::parse(l).expect("a session spec"))
        .collect();
    assert_eq!(specs.len(), traces.len());
    assert_eq!(specs.len(), 2);

    let mut compared_dumps = 0;
    let mut compared_titles = 0;
    for (spec, trace) in specs.iter().zip(&traces) {
        let name = spec.get("name").map(text).expect("a session name");
        let expected = expected(trace);
        let mut page = Page::new();
        let mut dumps = Vec::new();
        let mut entity_titles = Vec::new();
        for step in spec
            .get("steps")
            .and_then(Json::as_array)
            .expect("session steps")
        {
            let before = page.read(|hub| hub.model().title().to_string_lossy());
            match step.get("op").map(text).as_deref() {
                Some("open") => {
                    assert_eq!(step.get("prefix").map(text).as_deref(), Some(""));
                    let path = step.get("path").map(text).expect("an open path");
                    page.main(MainToHub::OpenFiles, open_files(&path));
                    page.settle("the initial decode", |_, seen| {
                        matches!(last_status(seen), Some(Status::Idle | Status::Error))
                    });
                    entity_titles.push(("open", before, page.take_seen()));
                }
                Some("loadAll") => {
                    page.main(MainToHub::PrepareExport, prepare_export(&name));
                    page.settle("loadAllFields and the export", |_, seen| wrote_export(seen));
                    entity_titles.push(("all", before, page.take_seen()));
                }
                Some("dump") => {
                    let phase = step.get("phase").map(text).expect("a dump phase");
                    dumps.push((phase.clone(), page.dump(&phase)));
                }
                other => panic!("{name}: no entity step for {other:?}"),
            }
            assert_eq!(page.exceptions(), Vec::<String>::new(), "{name}");
        }

        let statuses: Vec<String> = entity_titles
            .iter()
            .flat_map(|(_, _, seen)| seen)
            .filter_map(|s| match s {
                Seen::Event(HubEvent::SourceStatus { source: 0, status }) => {
                    Some(status.name().to_owned())
                }
                _ => None,
            })
            .collect();
        assert_eq!(statuses, expected.statuses, "{name}: status callbacks");

        assert_eq!(dumps.len(), expected.dumps.len(), "{name}: dump count");
        for ((phase, actual), (expected_phase, lines)) in dumps.iter().zip(&expected.dumps) {
            assert_eq!(phase, expected_phase);
            assert_eq!(actual.len(), lines.len(), "{name} {phase}: dump length");
            for (a, e) in actual.iter().zip(lines) {
                assert_eq!(a, e, "{name} {phase}: dump line");
            }
            compared_dumps += 1;
        }

        for (phase, before, seen) in &entity_titles {
            let label = format!("{name}-{phase}");
            let (_, upstream) = titles
                .iter()
                .find(|(l, _)| *l == label)
                .unwrap_or_else(|| panic!("no capture {label} in hub/entity-titles"));
            let actual: Vec<String> = seen
                .iter()
                .filter_map(|s| match s {
                    Seen::Title(t) => Some(t.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(actual, changes(before, upstream), "{label}: window titles");
            compared_titles += 1;
        }
    }
    assert_eq!(compared_dumps, 4);
    assert_eq!(compared_titles, titles.len());
}

fn wpilog_record(out: &mut Vec<u8>, entry: u8, timestamp: u32, payload: &[u8]) {
    out.push(0x30);
    out.push(entry);
    out.push(payload.len() as u8);
    out.extend(timestamp.to_le_bytes());
    out.extend(payload);
}

/// A 43 MB WPILOG of one double entry, the size of the largest public corpus log.
fn synthetic_wpilog(path: &Path) {
    let mut out = b"WPILOG".to_vec();
    out.extend(0x0100u16.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    let mut start = vec![0u8];
    start.extend(1u32.to_le_bytes());
    for text in ["/Big/Value", "double", ""] {
        start.extend((text.len() as u32).to_le_bytes());
        start.extend(text.as_bytes());
    }
    wpilog_record(&mut out, 0, 0, &start);
    let mut timestamp = 0u32;
    while out.len() < 43_000_000 {
        timestamp += 1_000;
        wpilog_record(&mut out, 1, timestamp, &f64::from(timestamp).to_le_bytes());
    }
    std::fs::write(path, out).expect("write the synthetic log");
}

fn closing_frees_the_decode_thread(log: &Path) {
    let mut page = Page::new();
    page.main(MainToHub::OpenFiles, open_files(&log.to_string_lossy()));
    page.settle("the decode thread to start indexing", |_, seen| {
        last_status(seen) == Some(Status::DecodingInitial)
            && seen
                .iter()
                .any(|s| matches!(s, Seen::Event(HubEvent::Loading)))
    });
    assert_eq!(page.read(Hub::decode_threads), 1);

    let small = root().join("parity/specs/hub/logs/corpus-a-struct-wpilib.wpilog");
    let start = Instant::now();
    page.main(MainToHub::OpenFiles, open_files(&small.to_string_lossy()));
    while page.read(Hub::decode_threads) > 0 {
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "{}: the terminated decode thread still runs after {:?}",
            log.display(),
            start.elapsed()
        );
        std::thread::sleep(Duration::from_micros(200));
    }
    eprintln!(
        "{}: decode thread gone {:?} after the close",
        log.display(),
        start.elapsed()
    );

    page.settle("the next log", |_, seen| {
        matches!(last_status(seen), Some(Status::Idle))
    });
    assert_eq!(
        page.read(|hub| hub.model().title().to_string_lossy()),
        "corpus-a-struct-wpilib.wpilog \u{2014} AdvantageScope"
    );
    assert_eq!(page.read(Hub::decode_threads), 1);
    assert_eq!(page.exceptions(), Vec::<String>::new());
}

#[test]
fn closing_a_43_mb_log_frees_its_decode_thread_within_100_ms() {
    let dir = std::env::temp_dir().join(format!("asrs-hub-entity-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create a scratch directory");
    let synthetic = dir.join("big.wpilog");
    synthetic_wpilog(&synthetic);
    closing_frees_the_decode_thread(&synthetic);
    let _ = std::fs::remove_file(&synthetic);
    let _ = std::fs::remove_dir(&dir);

    match asrs_parity::cache::public_log("rar1741/FRC_20250315_191317_INKOK_Q21.wpilog") {
        Ok(public) if public.exists() => closing_frees_the_decode_thread(&public),
        _ => eprintln!("the 43 MB public log is not fetched; only the synthetic one ran"),
    }
}

#[test]
fn frames_and_intervals_run_on_the_gpui_executor() {
    let mut page = Page::new();
    for _ in 0..3 {
        assert_eq!(
            page.frame(),
            1,
            "the Tabs loop requests one frame per frame"
        );
    }

    page.take_seen();
    for _ in 0..100 {
        page.cx.advance_clock(Duration::from_millis(10));
        page.cx.run_until_parked();
    }
    let count = |name: HubToMain, seen: &[Seen]| {
        seen.iter()
            .filter(|s| matches!(s, Seen::Event(HubEvent::Main(m)) if m.name == name))
            .count()
    };
    let seen = page.take_seen();
    assert_eq!(count(HubToMain::SaveState, &seen), 4);
    assert_eq!(count(HubToMain::SaveTypeMemory, &seen), 1);
}
