#![allow(clippy::disallowed_methods)]

//! Opens every window `main.ts` creates in the `windows/*` vectors twice: through
//! `asrs_app::windows`, and in Electron 39 with the recorded `BrowserWindow` options and the
//! position the port's model gives it. The content sizes after open must be equal, and so must
//! the frames of windows that are not sheets. Then checks that an always-on-top window stays visible while
//! the app is inactive, which a bare `WindowKind::Floating` window does not.
//!
//! Needs a display and the oracle's Electron (`ASRS_ELECTRON` or `cargo xtask oracle setup`);
//! skipped under `ASRS_CI=1` and off macOS.

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write as _};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use asrs_app::windows::{
    self, FileFilter, MessageBoxOptions, MessageBoxResult, MessageBoxType, OpenDialogOptions, Rect,
    SatellitePlacement, SaveDialogOptions, WindowId, WindowKind, Windows,
};
use asrs_parity::cve1::Value;
use asrs_parity::json::Json;
use asrs_parity::vectors;
use asrs_services::window_manager::{Opened, OptionValue, Platform, WindowManager};
use asrs_ui::gpui::{
    AnyWindowHandle, App, AppContext, AsyncApp, Bounds, Context, IntoElement, Render, Styled,
    Window, WindowBounds, WindowOptions, div, point, px, size,
};
use asrs_ui::platform::probe;

const ELECTRON_MAIN: &str = r#"
const { app, BrowserWindow, dialog } = require("electron");
const readline = require("readline");
const out = (line) => process.stdout.write(line + "\n");
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
app.whenReady().then(() => {
  const parent = new BrowserWindow({ x: 60, y: 420, width: 1100, height: 650 });
  out("ready " + process.versions.electron);
  readline.createInterface({ input: process.stdin }).on("line", async (line) => {
    const request = JSON.parse(line);
    if (request.op === "open") {
      const options = { ...request.options, show: false };
      if (options.parent) options.parent = parent;
      const win = new BrowserWindow(options);
      win.show();
      await wait(500);
      out("size " + JSON.stringify([...win.getContentSize(), ...Object.values(win.getBounds())]));
      win.destroy();
      await wait(200);
    } else if (request.op === "cancelId") {
      const controller = new AbortController();
      controller.abort();
      const options = { message: "", buttons: request.buttons, signal: controller.signal };
      if (request.defaultId !== null) options.defaultId = request.defaultId;
      if (request.cancelId !== null) options.cancelId = request.cancelId;
      const result = await dialog.showMessageBox(options);
      out("cancelId " + (result.cancelId ?? result.response));
    } else if (request.op === "activate") {
      app.focus({ steal: true });
      parent.focus();
      await wait(800);
      out("activated");
    } else {
      app.quit();
    }
  });
});
"#;

struct Blank;

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div().size_full()
    }
}

fn electron_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("ASRS_ELECTRON") {
        return PathBuf::from(path);
    }
    let package = vectors::workspace_root().join("target/oracle/upstream/node_modules/electron");
    let rel = std::fs::read_to_string(package.join("path.txt"))
        .expect("Electron is not installed; run `cargo xtask oracle setup` or set ASRS_ELECTRON");
    package.join("dist").join(rel.trim())
}

struct Electron {
    child: RefCell<Child>,
    lines: mpsc::Receiver<String>,
}

impl Electron {
    fn start() -> Self {
        let dir = std::env::temp_dir().join(format!("asrs-window-sizes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("main.js"), ELECTRON_MAIN).expect("main.js");
        std::fs::write(
            dir.join("package.json"),
            r#"{"name":"asrs-window-sizes","main":"main.js"}"#,
        )
        .expect("package.json");
        let mut child = Command::new(electron_binary())
            .arg(&dir)
            .env_remove("ELECTRON_RUN_AS_NODE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start Electron");
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child: RefCell::new(child),
            lines,
        }
    }

    async fn reply(&self, cx: &mut AsyncApp, prefix: &str) -> String {
        for _ in 0..200 {
            sleep(cx, 50).await;
            if let Some(rest) = self
                .lines
                .try_iter()
                .find_map(|l| l.strip_prefix(prefix).map(str::to_owned))
            {
                return rest;
            }
        }
        panic!("no {prefix:?} line from Electron within 10 s");
    }

    async fn request(&self, cx: &mut AsyncApp, request: &str, prefix: &str) -> String {
        self.lines.try_iter().for_each(drop);
        writeln!(
            self.child.borrow_mut().stdin.as_mut().expect("stdin"),
            "{request}"
        )
        .expect("write to Electron");
        self.reply(cx, prefix).await
    }
}

impl Drop for Electron {
    fn drop(&mut self) {
        let child = self.child.get_mut();
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = writeln!(stdin, r#"{{"op":"quit"}}"#);
        }
        std::thread::sleep(Duration::from_millis(300));
        let _ = child.kill();
        let _ = child.wait();
    }
}

async fn sleep(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

fn with_window<R>(
    cx: &mut AsyncApp,
    handle: AnyWindowHandle,
    f: impl FnOnce(&mut Window) -> R,
) -> R {
    cx.update(|cx| handle.update(cx, |_, window, _| f(window)))
        .expect("window is open")
}

/// One window `main.ts` created, with its options minus the placement-only `x`, `y` and
/// `parent`.
#[derive(Clone, Debug)]
struct Case {
    options: Vec<(String, Value)>,
}

impl Case {
    fn number(&self, key: &str) -> Option<f64> {
        self.options
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.as_f64())
    }

    fn flag(&self, key: &str) -> bool {
        self.options
            .iter()
            .any(|(k, v)| k == key && *v == Value::Bool(true))
    }

    fn has(&self, key: &str) -> bool {
        self.options.iter().any(|(k, _)| k == key)
    }

    fn kind(&self) -> WindowKind {
        if self.has("vibrancy") {
            return WindowKind::Hub;
        }
        if self.flag("modal") {
            return match self.number("height") {
                Some(108.0) => WindowKind::EditRange,
                Some(162.0) => WindowKind::UnitConversion,
                Some(81.0) => WindowKind::RenameTab,
                Some(189.0) => WindowKind::Export,
                _ => WindowKind::BetaWelcome,
            };
        }
        if self.has("maxWidth") {
            return WindowKind::SourceListHelp;
        }
        match self.number("minWidth") {
            Some(200.0) => WindowKind::Satellite,
            Some(500.0) => WindowKind::Download,
            Some(600.0) => WindowKind::Licenses,
            _ => WindowKind::Preferences,
        }
    }

    fn electron_json(&self, with_parent: bool, origin: Option<(f64, f64)>) -> String {
        let mut pairs: Vec<(&str, Json)> = Vec::new();
        if let Some((x, y)) = origin {
            pairs.push(("x", Json::Number(x)));
            pairs.push(("y", Json::Number(y)));
        }
        for (key, value) in &self.options {
            let json = match value {
                Value::Bool(b) => Json::Bool(*b),
                Value::String(s) => Json::string(&s.to_string_lossy()),
                Value::Undefined => continue,
                other => Json::Number(other.as_f64().expect("numeric option")),
            };
            pairs.push((key, json));
        }
        if with_parent {
            pairs.push(("parent", Json::Bool(true)));
        }
        Json::object([
            ("op", Json::string("open")),
            ("options", Json::object(pairs)),
        ])
        .to_compact()
    }
}

fn cases() -> Vec<(Case, Option<(f64, f64)>)> {
    let mut out: Vec<(Case, Option<(f64, f64)>)> = Vec::new();
    for id in ["windows/fresh-darwin", "windows/saved-darwin"] {
        for record in vectors::load(id).unwrap_or_else(|e| panic!("{e}")) {
            let Some(created) = record.field("created").and_then(Json::as_array) else {
                continue;
            };
            for window in created {
                let options = Value::from_json(window.get("options").expect("options"))
                    .expect("CVE-1 options");
                let Value::Object(pairs) = options else {
                    panic!("options are not an object");
                };
                let get = |key: &str| {
                    pairs
                        .iter()
                        .find(|(k, _)| k.to_string_lossy() == key)
                        .and_then(|(_, v)| v.as_f64())
                };
                let origin = get("x").zip(get("y"));
                let mut options: Vec<(String, Value)> = pairs
                    .iter()
                    .map(|(k, v)| (k.to_string_lossy(), v.clone()))
                    .filter(|(k, _)| !matches!(k.as_str(), "x" | "y" | "parent"))
                    .collect();
                options.sort_by(|a, b| a.0.cmp(&b.0));
                let case = Case { options };
                if !out.iter().any(|(c, _)| c.options == case.options) {
                    out.push((case, origin));
                }
            }
        }
    }
    out
}

fn port_options(options: &windows::WindowOptions) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = options
        .electron_entries()
        .into_iter()
        .filter(|(k, _)| !matches!(*k, "x" | "y" | "parent"))
        .map(|(k, v)| {
            let value = match v {
                OptionValue::Undefined => Value::Undefined,
                OptionValue::Bool(b) => Value::Bool(b),
                OptionValue::Number(n) => Value::number(n),
                OptionValue::String(s) => Value::string(s),
                OptionValue::Window(id) => Value::number(id as f64),
            };
            (k.to_owned(), value)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn open_case(
    cx: &mut App,
    case: &Case,
    origin: Option<(f64, f64)>,
    parent: WindowId,
) -> (WindowId, AnyWindowHandle) {
    let rect = case_rect(case, origin);
    let build = |_: &mut Window, cx: &mut App| cx.new(|_| Blank);
    let id =
        match case.kind() {
            WindowKind::Hub => windows::open_hub(cx, Some(rect), build).map(|(id, _)| id),
            WindowKind::Satellite => {
                windows::open_satellite(cx, "case", SatellitePlacement::Saved(rect), build)
                    .map(|(id, _)| id)
            }
            WindowKind::SourceListHelp => {
                windows::open_source_list_help(cx, parent, build).map(|(id, _)| id)
            }
            WindowKind::Preferences => windows::open_preferences(cx, parent, build)
                .map(|o| o.expect("a new preferences window").0),
            WindowKind::Download => windows::open_download(cx, parent, build)
                .map(|o| o.expect("a new download window").0),
            WindowKind::Licenses => windows::open_licenses(cx, parent, build)
                .map(|o| o.expect("a new licenses window").0),
            kind => windows::open_modal(cx, kind, parent, build).map(|(id, _)| id),
        }
        .expect("open window");
    (id, Windows::handle(cx, id).expect("handle"))
}

#[derive(Default)]
struct Report {
    failed: Vec<String>,
    passed: usize,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: impl std::fmt::Display) {
        println!("{} {name}: {detail}", if ok { "PASS" } else { "FAIL" });
        if ok {
            self.passed += 1;
        } else {
            self.failed.push(name.to_owned());
        }
    }
}

struct Measured {
    content: (f64, f64),
    frame: (f64, f64, f64, f64),
}

async fn measure_electron(cx: &mut AsyncApp, electron: &Electron, request: &str) -> Measured {
    let reply = electron.request(cx, request, "size ").await;
    let values: Vec<f64> = asrs_parity::json::parse(&reply)
        .expect("size json")
        .as_array()
        .expect("size array")
        .iter()
        .map(|v| v.as_f64().expect("number"))
        .collect();
    let [w, h, x, y, fw, fh] = values[..] else {
        panic!("size reply {reply}");
    };
    Measured {
        content: (w, h),
        frame: (x, y, fw, fh),
    }
}

fn frame_of(cx: &mut AsyncApp, handle: AnyWindowHandle) -> (f64, f64, f64, f64) {
    with_window(cx, handle, |w| asrs_ui::platform::window_bounds(w)).expect("window bounds")
}

async fn sizes(cx: &mut AsyncApp, electron: &Electron, report: &mut Report) {
    let parent = cx
        .update(|cx| windows::open_hub(cx, None, |_, cx| cx.new(|_| Blank)))
        .expect("parent hub")
        .0;
    sleep(cx, 600).await;
    let parent_handle = cx
        .update(|cx| Windows::handle(cx, parent))
        .expect("parent handle");
    let parent_bounds = cx
        .update(|cx| Windows::manager(cx).bounds(parent))
        .expect("parent bounds");
    let cases = cases();
    assert!(cases.len() >= 12, "only {} distinct windows", cases.len());
    for (case, origin) in &cases {
        let kind = case.kind();
        let name = format!(
            "{kind:?} {}x{}",
            case.number("width").unwrap_or(0.0),
            case.number("height").unwrap_or(0.0)
        );
        with_window(cx, parent_handle, |w| w.activate_window());
        sleep(cx, 300).await;
        let (id, handle) = cx.update(|cx| open_case(cx, case, *origin, parent));
        let model = cx.update(|cx| Windows::manager(cx).kind(id));
        sleep(cx, 700).await;
        let ours = with_window(cx, handle, |w| probe::content_size(w)).expect("content size");
        let ours_frame = frame_of(cx, handle);
        if kind.is_modal() {
            let attached = cx
                .update(|cx| {
                    parent_handle.update(cx, |_, parent, cx| {
                        handle.update(cx, |_, window, _| probe::is_sheet_of(window, parent))
                    })
                })
                .ok()
                .and_then(Result::ok)
                .unwrap_or(false);
            report.check(&format!("{name} is a sheet on its parent"), attached, "");
        }
        let placed = model_options(case, *origin, Some(parent_bounds));
        let request = case.electron_json(kind.is_modal(), placed.x.zip(placed.y));
        let theirs = measure_electron(cx, electron, &request).await;
        report.check(
            &format!("{name} content size"),
            ours == theirs.content,
            format!("ours {ours:?}, Electron {:?}", theirs.content),
        );
        if !kind.is_modal() {
            report.check(
                &format!("{name} frame"),
                ours_frame == theirs.frame,
                format!("ours {ours_frame:?}, Electron {:?}", theirs.frame),
            );
        }
        report.check(
            &format!("{name} model kind"),
            model == Some(kind),
            format!("{model:?}"),
        );
        cx.update(|cx| windows::close(cx, id));
        sleep(cx, 300).await;
        if kind.is_modal() {
            let stuck = with_window(cx, parent_handle, |w| probe::has_attached_sheet(w));
            report.check(&format!("{name} detaches when closed"), !stuck, "");
        }
    }
    let mismatched: Vec<String> = cases
        .iter()
        .filter(|(case, origin)| port_options(&model_options(case, *origin, None)) != case.options)
        .map(|(case, _)| format!("{:?}", case.kind()))
        .collect();
    report.check(
        "every measured window has the options main.ts gives its kind",
        mismatched.is_empty(),
        format!(
            "{} distinct windows, mismatched {mismatched:?}",
            cases.len()
        ),
    );
}

fn case_rect(case: &Case, origin: Option<(f64, f64)>) -> Rect {
    let (x, y) = origin.unwrap_or((140.0, 140.0));
    Rect {
        x,
        y,
        width: case.number("width").expect("width"),
        height: case.number("height").expect("height"),
    }
}

/// The options the port's model gives the window `open_case` opens, with the parent at
/// `parent_bounds` when given.
fn model_options(
    case: &Case,
    origin: Option<(f64, f64)>,
    parent_bounds: Option<Rect>,
) -> windows::WindowOptions {
    let mut manager = WindowManager::for_platform(Platform::Mac);
    let (parent, _) = manager.create_hub(None);
    if let Some(bounds) = parent_bounds {
        manager.set_bounds(parent, bounds);
    }
    let rect = case_rect(case, origin);
    let opened = |opened: Opened| match opened {
        Opened::New(_, options) => options,
        Opened::Focus(id) => panic!("window {id} already open"),
    };
    match case.kind() {
        WindowKind::Hub => manager.create_hub(Some(rect)).1,
        WindowKind::Satellite => {
            manager
                .create_satellite("case", SatellitePlacement::Saved(rect))
                .1
        }
        WindowKind::SourceListHelp => manager.open_source_list_help(parent).1,
        WindowKind::Preferences => opened(manager.open_preferences(parent)),
        WindowKind::Download => opened(manager.open_download(parent)),
        WindowKind::Licenses => opened(manager.open_licenses(parent)),
        kind => manager.create_modal(kind, parent).1,
    }
}

async fn floating(cx: &mut AsyncApp, electron: &Electron, report: &mut Report) {
    let parent = cx.update(|cx| Windows::manager(cx).hubs()[0]);
    let prefs = cx
        .update(|cx| windows::open_preferences(cx, parent, |_, cx| cx.new(|_| Blank)))
        .expect("open preferences")
        .expect("new preferences window")
        .0;
    let prefs = cx.update(|cx| Windows::handle(cx, prefs)).expect("handle");
    let bare = cx
        .update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(900.0), px(500.0)),
                        size(px(200.0), px(100.0)),
                    ))),
                    kind: asrs_ui::gpui::WindowKind::Floating,
                    ..Default::default()
                },
                |_, cx| cx.new(|_| Blank),
            )
        })
        .expect("bare floating window");
    sleep(cx, 600).await;
    activate(cx, report).await;
    let before = (
        with_window(cx, prefs, |w| probe::is_visible(w)),
        cx.update(|_| probe::app_is_active()),
    );
    electron
        .request(cx, r#"{"op":"activate"}"#, "activated")
        .await;
    sleep(cx, 500).await;
    let active = cx.update(|_| probe::app_is_active());
    let visible = with_window(cx, prefs, |w| probe::is_visible(w));
    let bare_visible = cx
        .update(|cx| bare.update(cx, |_, w, _| probe::is_visible(w)))
        .expect("bare window");
    report.check(
        "the app went inactive",
        active == Some(false),
        format!("active before {:?}, after {active:?}", before.1),
    );
    report.check(
        "always-on-top window stays visible while the app is inactive",
        before.0 == Some(true) && visible == Some(true),
        format!("visible {:?} -> {visible:?}", before.0),
    );
    report.check(
        "a bare Floating window hides while the app is inactive",
        bare_visible == Some(false),
        format!(
            "visible {bare_visible:?}, hidesOnDeactivate {:?}",
            cx.update(|cx| bare.update(cx, |_, w, _| probe::hides_on_deactivate(w)))
                .ok()
        ),
    );
}

/// Activation requests are ignored until the process has a window on screen.
async fn activate(cx: &mut AsyncApp, report: &mut Report) {
    cx.update(|cx| cx.activate(true));
    sleep(cx, 500).await;
    let active = cx.update(|_| probe::app_is_active());
    report.check(
        "the app is active",
        active == Some(true),
        format!("{active:?}"),
    );
}

const CANCEL_CASES: &[(&[&str], Option<usize>, Option<usize>)] = &[
    (&[], None, None),
    (&["OK"], None, None),
    (&["OK"], Some(0), None),
    (&["Open", "Cancel"], None, None),
    (&["Open", "Cancel"], Some(0), None),
    (&["NO", "Yes"], Some(1), None),
    (&["Continue", "cancel", "No"], None, None),
    (&["Don't Close", "Close"], Some(0), None),
    (&["Quit", "Ignore"], Some(0), None),
    (&["A", "B"], Some(1), None),
    (&["A", "B", "C"], Some(0), Some(2)),
    (
        &["Open in DS Window", "Open in This Window", "Cancel"],
        Some(0),
        Some(2),
    ),
];

async fn wait_for<T: Clone>(cx: &mut AsyncApp, slot: &Rc<RefCell<Option<T>>>) -> Option<T> {
    for _ in 0..40 {
        if let Some(value) = slot.borrow().clone() {
            return Some(value);
        }
        sleep(cx, 50).await;
    }
    None
}

async fn dialogs(cx: &mut AsyncApp, electron: &Electron, report: &mut Report) {
    for (buttons, default_id, cancel_id) in CANCEL_CASES {
        let options = MessageBoxOptions {
            buttons: buttons.iter().map(|b| (*b).to_owned()).collect(),
            default_id: *default_id,
            cancel_id: *cancel_id,
            ..Default::default()
        };
        let number = |n: Option<usize>| n.map_or(Json::Null, |n| Json::Number(n as f64));
        let request = Json::object([
            ("op", Json::string("cancelId")),
            (
                "buttons",
                Json::Array(buttons.iter().map(|b| Json::string(b)).collect()),
            ),
            ("defaultId", number(*default_id)),
            ("cancelId", number(*cancel_id)),
        ])
        .to_compact();
        let theirs = electron.request(cx, &request, "cancelId ").await;
        report.check(
            &format!("cancelId for {buttons:?} default {default_id:?}"),
            theirs == options.cancel_id().to_string(),
            format!("ours {}, Electron {theirs}", options.cancel_id()),
        );
    }

    let hub = cx.update(|cx| Windows::hubs(cx)[0]);
    let result: Rc<RefCell<Option<MessageBoxResult>>> = Rc::default();
    for (checkbox, click) in [(true, "Cancel"), (false, "Open")] {
        result.borrow_mut().take();
        let slot = result.clone();
        cx.update(|cx| {
            hub.update(cx, |_, window, cx| {
                windows::message_box(
                    Some(window),
                    cx,
                    MessageBoxOptions {
                        kind: MessageBoxType::Info,
                        title: "Alert".to_owned(),
                        message: "Limited Signals Available".to_owned(),
                        detail: "Detail".to_owned(),
                        buttons: vec!["Open".to_owned(), "Cancel".to_owned()],
                        default_id: Some(0),
                        checkbox_label: if checkbox {
                            "Don't Show Again".to_owned()
                        } else {
                            String::new()
                        },
                        ..Default::default()
                    },
                    move |r, _| *slot.borrow_mut() = Some(r),
                )
            })
        })
        .expect("hub");
        sleep(cx, 800).await;
        let clicked = with_window(cx, hub, |w| {
            (!checkbox || probe::click_sheet_button(w, "Don't Show Again"))
                && probe::click_sheet_button(w, click)
        });
        let got = wait_for(cx, &result).await;
        let want = MessageBoxResult {
            response: usize::from(click == "Cancel"),
            checkbox_checked: checkbox,
        };
        report.check(
            &format!("message box sheet answers {click} with checkbox {checkbox}"),
            clicked && got == Some(want),
            format!("{got:?}"),
        );
        sleep(cx, 300).await;
    }

    let dir = std::env::temp_dir().canonicalize().expect("temp dir");
    let saved: Rc<RefCell<Option<Option<PathBuf>>>> = Rc::default();
    let slot = saved.clone();
    let save = SaveDialogOptions {
        title: "Select export location for layout file".to_owned(),
        default_path: Some(dir.join("AdvantageScope 2026-11-21.json")),
        filters: vec![FileFilter {
            name: "JSON files".to_owned(),
            extensions: vec!["json".to_owned()],
        }],
        create_directory: true,
        ..Default::default()
    };
    cx.update(|cx| {
        hub.update(cx, |_, window, cx| {
            windows::save_dialog(Some(window), cx, save, move |p, _| {
                *slot.borrow_mut() = Some(p)
            })
        })
    })
    .expect("hub");
    sleep(cx, 1000).await;
    let state = with_window(cx, hub, |w| probe::sheet_panel_state(w));
    let want = probe::PanelState {
        open: false,
        title: "Select export location for layout file".to_owned(),
        allowed_file_types: vec!["json".to_owned()],
        directory: Some(dir.to_string_lossy().into_owned()),
        name: "AdvantageScope 2026-11-21.json".to_owned(),
        can_create_directories: true,
        ..Default::default()
    };
    let matches = state.as_ref().is_some_and(|s| {
        probe::PanelState {
            message: String::new(),
            prompt: String::new(),
            ..s.clone()
        } == want
    });
    report.check(
        "save panel sheet is set up from the options",
        matches,
        format!("{state:?}"),
    );
    with_window(cx, hub, |w| probe::cancel_sheet_panel(w));
    let got = wait_for(cx, &saved).await;
    report.check(
        "cancelled save panel gives no path",
        got == Some(None),
        format!("{got:?}"),
    );
    sleep(cx, 500).await;

    let opened: Rc<RefCell<Option<Vec<PathBuf>>>> = Rc::default();
    let slot = opened.clone();
    let open = OpenDialogOptions {
        title: "Select the robot log file(s) to open".to_owned(),
        message: "If multiple files are selected, timestamps will be aligned automatically"
            .to_owned(),
        filters: vec![FileFilter {
            name: "Robot logs".to_owned(),
            extensions: ["rlog", "wpilog", "wpilogxz", "*"]
                .map(str::to_owned)
                .to_vec(),
        }],
        open_file: true,
        multi_selections: true,
        ..Default::default()
    };
    cx.update(|cx| {
        hub.update(cx, |_, window, cx| {
            windows::open_dialog(Some(window), cx, open, move |p, _| {
                *slot.borrow_mut() = Some(p)
            })
        })
    })
    .expect("hub");
    sleep(cx, 1000).await;
    let state = with_window(cx, hub, |w| probe::sheet_panel_state(w));
    let matches = state.as_ref().is_some_and(|s| {
        s.open
            && s.message
                == "If multiple files are selected, timestamps will be aligned automatically"
            && s.allowed_file_types.is_empty()
            && s.can_choose_files
            && !s.can_choose_directories
            && s.allows_multiple_selection
            && !s.can_create_directories
    });
    report.check(
        "open panel sheet is set up from the options, `*` allowing every type",
        matches,
        format!("{state:?}"),
    );
    with_window(cx, hub, |w| probe::cancel_sheet_panel(w));
    let got = wait_for(cx, &opened).await;
    report.check(
        "cancelled open panel gives no paths",
        got == Some(Vec::new()),
        format!("{got:?}"),
    );
}

fn main() {
    if asrs_parity::ci::skip_in_ci("window sizes", "a display and the oracle's Electron") {
        return;
    }
    if !cfg!(target_os = "macos") {
        println!("window sizes: skipped on this platform");
        return;
    }
    asrs_ui::gpui::application().run(|cx: &mut App| {
        asrs_ui::init(cx);
        windows::init(cx);
        cx.activate(true);
        cx.spawn(async move |cx| {
            let electron = Electron::start();
            let version = electron.reply(cx, "ready ").await;
            let mut report = Report::default();
            report.check("Electron is upstream's", version == "39.2.7", &version);
            sizes(cx, &electron, &mut report).await;
            dialogs(cx, &electron, &mut report).await;
            floating(cx, &electron, &mut report).await;
            drop(electron);
            println!(
                "window sizes: {} passed, {} failed {:?}",
                report.passed,
                report.failed.len(),
                report.failed
            );
            std::process::exit(i32::from(!report.failed.is_empty()));
        })
        .detach();
    });
}
