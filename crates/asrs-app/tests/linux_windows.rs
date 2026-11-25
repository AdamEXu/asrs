#![allow(clippy::disallowed_methods)]

//! The Linux window and menu checks on a real X11 display, beside the oracle's Electron: the
//! content size and `WM_NORMAL_HINTS` of every window `main.ts` opens, the always-on-top state,
//! satellites' aspect ratio hints, `isVisible()`, wheel notches as Chromium's DOM events carry
//! them, and the drawn menus driven by real pointer and key events.
//!
//! Needs a display with a window manager and the oracle's Electron (`ASRS_ELECTRON` or
//! `cargo xtask oracle setup`); skipped under `ASRS_CI=1`, off Linux, and on Wayland. It moves the
//! pointer.

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write as _};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use asrs_app::main_process;
use asrs_app::menus::accelerator::Accelerator;
use asrs_app::windows::{self, Rect, SatellitePlacement, WindowId, WindowKind, Windows};
use asrs_parity::json::{self, Json};
use asrs_parity::vectors;
use asrs_services::window_manager::{OptionValue, Platform, WindowOptions};
use asrs_ui::gpui::{
    AnyWindowHandle, App, AppContext, AsyncApp, Context, Entity, InteractiveElement, IntoElement,
    Render, ScrollEvent, Styled, Window, div,
};
use asrs_ui::platform::probe::{self, MouseAction, WheelSample};
use asrs_ui::platform::{
    self, MenuPath, PopupEntry, PopupMenu, SelectMenu, SelectMenuItem, SelectMenuItemKind,
};

const ELECTRON_MAIN: &str = r#"
const { app, BrowserWindow, Menu, screen } = require("electron");
const readline = require("readline");
const out = (line) => process.stdout.write(line + "\n");
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const id = (win) => win.getNativeWindowHandle().readUInt32LE(0);
app.whenReady().then(() => {
  Menu.setApplicationMenu(Menu.buildFromTemplate([{ label: "App", submenu: [{ label: "Item" }] }]));
  const parent = new BrowserWindow({ x: 60, y: 420, width: 1100, height: 650 });
  let win = null;
  out("ready " + process.versions.electron);
  readline.createInterface({ input: process.stdin }).on("line", async (line) => {
    const request = JSON.parse(line);
    if (request.op === "open") {
      const options = { ...request.options };
      if (options.parent) options.parent = parent;
      win = new BrowserWindow({ ...options, show: false });
      if (!request.hub) win.setMenu(null);
      win.setMenuBarVisibility(false);
      win.once("ready-to-show", () => win.show());
      await win.loadURL("data:text/html,<body style='margin:0;height:5000px'>");
      await wait(900);
      out("open " + JSON.stringify([id(win), ...win.getContentSize(), ...Object.values(win.getBounds())]));
    } else if (request.op === "aspect") {
      const [ratio, lock, width, height] = request.step;
      win.setAspectRatio(lock ? ratio : 0);
      if (ratio !== 0) win.setContentSize(width, height);
      await wait(500);
      out("aspect " + JSON.stringify(win.getContentSize()));
    } else if (request.op === "visible") {
      const seen = [win.isVisible()];
      win.minimize();
      await wait(1200);
      seen.push(win.isVisible());
      out("visible " + JSON.stringify(seen));
    } else if (request.op === "wheel") {
      await win.webContents.executeJavaScript(
        "window.__wheel = []; window.addEventListener('wheel', (e) => window.__wheel.push([e.deltaX, e.deltaY, e.deltaMode])); true"
      );
      win.focus();
      await wait(300);
      out("wheel ready");
    } else if (request.op === "wheels") {
      out("wheels " + JSON.stringify(await win.webContents.executeJavaScript("window.__wheel")));
    } else if (request.op === "displays") {
      const areas = screen.getAllDisplays().map(({ workArea: a }) => [a.x, a.y, a.width, a.height].map((v) => v.toFixed(1)));
      out("displays [" + areas.map((a) => "[" + a.join(", ") + "]").join(", ") + "]");
    } else if (request.op === "centred") {
      const win = new BrowserWindow({ width: 900, height: 500, minWidth: 200, minHeight: 100, show: false });
      await wait(300);
      const { x, y } = win.getBounds();
      out("centred [" + x.toFixed(1) + ", " + y.toFixed(1) + "]");
      win.destroy();
    } else if (request.op === "close") {
      win.destroy();
      await wait(300);
      out("closed");
    } else {
      app.quit();
    }
  });
});
"#;

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
        let dir = std::env::temp_dir().join(format!("asrs-linux-windows-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("main.js"), ELECTRON_MAIN).expect("main.js");
        let mut child = Command::new(electron_binary())
            .args(["--ozone-platform=x11", "--force-device-scale-factor=1"])
            .arg(format!("--user-data-dir={}", dir.join("data").display()))
            .arg(dir.join("main.js"))
            .env_remove("ELECTRON_RUN_AS_NODE")
            .env_remove("WAYLAND_DISPLAY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("Electron");
        let stdout = child.stdout.take().expect("stdout");
        let (send, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if send.send(line).is_err() {
                    return;
                }
            }
        });
        Electron {
            child: RefCell::new(child),
            lines,
        }
    }

    async fn reply(&self, cx: &mut AsyncApp, prefix: &str) -> String {
        for _ in 0..600 {
            while let Ok(line) = self.lines.try_recv() {
                if let Some(rest) = line.strip_prefix(prefix) {
                    return rest.to_owned();
                }
            }
            sleep(cx, 50).await;
        }
        panic!("Electron did not answer {prefix:?}");
    }

    async fn request(&self, cx: &mut AsyncApp, request: &str, prefix: &str) -> String {
        {
            let mut child = self.child.borrow_mut();
            let stdin = child.stdin.as_mut().expect("stdin");
            writeln!(stdin, "{request}").expect("request");
        }
        self.reply(cx, prefix).await
    }

    async fn numbers(&self, cx: &mut AsyncApp, request: &str, prefix: &str) -> Vec<f64> {
        let reply = self.request(cx, request, prefix).await;
        let values = json::parse(&reply).expect("json");
        let values = values.as_array().expect("array").iter();
        values.map(|v| v.as_f64().expect("number")).collect()
    }
}

impl Drop for Electron {
    fn drop(&mut self) {
        let child = self.child.get_mut();
        let _ = child.kill();
        let _ = child.wait();
    }
}

async fn sleep(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

type WheelLog = Rc<RefCell<Vec<(f64, f64)>>>;

#[derive(Default)]
struct Blank {
    wheel: WheelLog,
}

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        let wheel = self.wheel.clone();
        div()
            .size_full()
            .on_scroll(move |event: &ScrollEvent, _, _| {
                let delta = platform::wheel_delta(event);
                wheel
                    .borrow_mut()
                    .push((-f64::from(delta.x), -f64::from(delta.y)));
            })
    }
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

fn blank(_: &mut Window, cx: &mut App) -> Entity<Blank> {
    cx.new(|_| Blank::default())
}

fn with_window<R>(
    cx: &mut AsyncApp,
    handle: AnyWindowHandle,
    f: impl FnOnce(&mut Window) -> R,
) -> R {
    cx.update(|cx| handle.update(cx, |_, window, _| f(window)))
        .expect("window")
}

fn handle_of(cx: &mut AsyncApp, id: WindowId) -> AnyWindowHandle {
    cx.update(|cx| Windows::handle(cx, id)).expect("handle")
}

fn electron_options(kind: WindowKind, options: &WindowOptions) -> String {
    let entries = options.electron_entries().into_iter().filter_map(|(k, v)| {
        let json = match v {
            OptionValue::Undefined => return None,
            OptionValue::Bool(b) => Json::Bool(b),
            OptionValue::Number(n) => Json::Number(n),
            OptionValue::String(s) => Json::string(s),
            OptionValue::Window(_) => Json::Bool(true),
        };
        Some((k, json))
    });
    Json::object([
        ("op", Json::string("open")),
        ("hub", Json::Bool(kind == WindowKind::Hub)),
        ("options", Json::object(entries)),
    ])
    .to_compact()
}

const KINDS: [WindowKind; 11] = [
    WindowKind::Hub,
    WindowKind::Satellite,
    WindowKind::EditRange,
    WindowKind::EditFov,
    WindowKind::UnitConversion,
    WindowKind::RenameTab,
    WindowKind::Export,
    WindowKind::SourceListHelp,
    WindowKind::Preferences,
    WindowKind::Download,
    WindowKind::Licenses,
];

const SATELLITE: Rect = Rect {
    x: 160.0,
    y: 160.0,
    width: 900.0,
    height: 500.0,
};

fn open_kind(cx: &mut App, kind: WindowKind, parent: WindowId) -> (WindowId, WindowOptions) {
    let mut manager = asrs_services::window_manager::WindowManager::new();
    let (model_parent, _) = manager.create_hub(None);
    if let Some(bounds) = Windows::manager(cx).bounds(parent) {
        manager.set_bounds(model_parent, bounds);
        manager.focus(model_parent);
    }
    let new = |opened| match opened {
        asrs_services::window_manager::Opened::New(_, options) => options,
        asrs_services::window_manager::Opened::Focus(_) => panic!("already open"),
    };
    let only = |opened: asrs_ui::gpui::Result<Option<(WindowId, Entity<Blank>)>>| {
        opened.expect("open").expect("new").0
    };
    match kind {
        WindowKind::Hub => (
            windows::open_hub(cx, None, blank).expect("open").0,
            manager.create_hub(None).1,
        ),
        WindowKind::Satellite => (
            windows::open_satellite(cx, "s", SatellitePlacement::Saved(SATELLITE), blank)
                .expect("open")
                .0,
            manager
                .create_satellite("s", SatellitePlacement::Saved(SATELLITE))
                .1,
        ),
        WindowKind::SourceListHelp => (
            windows::open_source_list_help(cx, parent, blank)
                .expect("open")
                .0,
            manager.open_source_list_help(model_parent).1,
        ),
        WindowKind::Preferences => (
            only(windows::open_preferences(cx, parent, blank)),
            new(manager.open_preferences(model_parent)),
        ),
        WindowKind::Download => (
            only(windows::open_download(cx, parent, blank)),
            new(manager.open_download(model_parent)),
        ),
        WindowKind::Licenses => (
            only(windows::open_licenses(cx, parent, blank)),
            new(manager.open_licenses(model_parent)),
        ),
        kind => (
            windows::open_modal(cx, kind, parent, blank)
                .expect("open")
                .0,
            manager.create_modal(kind, model_parent).1,
        ),
    }
}

async fn sizes(cx: &mut AsyncApp, electron: &Electron, report: &mut Report, parent: WindowId) {
    for kind in KINDS {
        let (id, options) = cx.update(|cx| open_kind(cx, kind, parent));
        let name = format!("{kind:?} {}x{}", options.width, options.height);
        let handle = handle_of(cx, id);
        sleep(cx, 1200).await;
        let ours = with_window(cx, handle, |w| probe::content_size(w)).expect("content size");
        let bounds = with_window(cx, handle, |w| platform::window_bounds(w)).expect("bounds");
        let number = with_window(cx, handle, |w| probe::window_number(w)).expect("window id");
        let hints = with_window(cx, handle, |w| probe::size_hints(w, number)).expect("hints");
        let theirs = electron
            .numbers(cx, &electron_options(kind, &options), "open ")
            .await;
        let their_hints =
            with_window(cx, handle, |w| probe::size_hints(w, theirs[0] as u32)).expect("hints");
        report.check(
            &format!("{name} content size"),
            ours == (theirs[1], theirs[2]),
            format!("ours {ours:?}, Electron {:?}", &theirs[1..3]),
        );
        let bounds = [bounds.0, bounds.1, bounds.2, bounds.3];
        report.check(
            &format!("{name} bounds"),
            bounds == theirs[3..7],
            format!("ours {bounds:?}, Electron {:?}", &theirs[3..7]),
        );
        let limited = options.resizable == Some(false) || options.max_width.is_some();
        report.check(
            &format!("{name} size limits"),
            hints.min == their_hints.min && (!limited || hints.max == their_hints.max),
            format!("ours {hints:?}, Electron {their_hints:?}"),
        );
        report.check(
            &format!("{name} always on top"),
            hints.above == their_hints.above && hints.above == options.always_on_top,
            "",
        );
        electron.request(cx, r#"{"op":"close"}"#, "closed").await;
        cx.update(|cx| windows::close(cx, id));
        sleep(cx, 400).await;
    }
}

async fn aspects(cx: &mut AsyncApp, electron: &Electron, report: &mut Report) {
    const RATIOS: [(f64, bool); 7] = [
        (1.0, true),
        (5.0, true),
        (0.5, true),
        (16.0 / 9.0, true),
        (16.0 / 9.0, false),
        (0.0, false),
        (1.3333, true),
    ];
    let rect = Rect {
        x: 160.0,
        y: 160.0,
        width: 300.0,
        height: 160.0,
    };
    let opened =
        cx.update(|cx| windows::open_satellite(cx, "a", SatellitePlacement::Saved(rect), blank));
    let (id, _) = opened.expect("open window");
    let handle = handle_of(cx, id);
    let number = with_window(cx, handle, |w| probe::window_number(w)).expect("window id");
    let request = r#"{"op":"open","options":{"x":160,"y":160,"width":300,"height":160,"minWidth":200,"minHeight":100,"resizable":true}}"#;
    let theirs = electron.numbers(cx, request, "open ").await[0] as u32;
    for (ratio, lock) in RATIOS {
        let content = with_window(cx, handle, |w| probe::content_size(w)).expect("content size");
        let (width, height) = match ratio {
            0.0 => (0.0, 0.0),
            _ => main_process::content_size(content, ratio).expect("a size"),
        };
        let message = (ratio != 0.0).then_some(ratio);
        cx.update(|cx| main_process::set_aspect_ratio(cx, id, message, lock));
        let request = format!(r#"{{"op":"aspect","step":[{ratio},{lock},{width},{height}]}}"#);
        let size = electron.numbers(cx, &request, "aspect ").await;
        sleep(cx, 500).await;
        let ours = with_window(cx, handle, |w| probe::content_size(w)).expect("content size");
        let (mine, other) = with_window(cx, handle, |w| {
            (probe::size_hints(w, number), probe::size_hints(w, theirs))
        });
        let (mine, other) = (mine.expect("hints"), other.expect("hints"));
        report.check(
            &format!("satellite ratio {ratio} lock {lock} content"),
            [ours.0, ours.1] == size[..],
            format!("ours {ours:?}, Electron {size:?}"),
        );
        report.check(
            &format!("satellite ratio {ratio} lock {lock} hint"),
            mine.aspect == other.aspect,
            format!("ours {:?}, Electron {:?}", mine.aspect, other.aspect),
        );
    }
    let mut ours = vec![cx.update(|cx| Windows::is_visible(cx, id))];
    with_window(cx, handle, |w| w.minimize_window());
    sleep(cx, 1200).await;
    let mapped = with_window(cx, handle, |w| probe::is_visible(w));
    ours.push(cx.update(|cx| Windows::is_visible(cx, id)));
    let reply = electron
        .request(cx, r#"{"op":"visible"}"#, "visible ")
        .await;
    report.check(
        "isVisible() shown and minimized",
        reply == format!("[{},{}]", ours[0], ours[1]) && mapped == Some(false),
        format!("ours {ours:?}, Electron {reply}, mapped while minimized {mapped:?}"),
    );
    electron.request(cx, r#"{"op":"close"}"#, "closed").await;
    cx.update(|cx| windows::close(cx, id));
    sleep(cx, 400).await;
}

async fn wheel(cx: &mut AsyncApp, electron: &Electron, report: &mut Report) {
    const NOTCHES: [[i32; 2]; 5] = [[1, 0], [-1, 0], [-1, 0], [0, 1], [0, -1]];
    let rect = Rect {
        x: 1300.0,
        y: 200.0,
        width: 400.0,
        height: 300.0,
    };
    let opened =
        cx.update(|cx| windows::open_satellite(cx, "w", SatellitePlacement::Saved(rect), blank));
    let (id, view) = opened.expect("open window");
    let handle = handle_of(cx, id);
    let log = cx.update(|cx| view.read(cx).wheel.clone());
    let request = r#"{"op":"open","options":{"x":1300,"y":700,"width":400,"height":300}}"#;
    let opened = electron.numbers(cx, request, "open ").await;
    let theirs = (opened[3] + 200.0, opened[4] + 150.0);
    electron
        .request(cx, r#"{"op":"wheel"}"#, "wheel ready")
        .await;
    let (x, y, w, h) = with_window(cx, handle, |w| platform::window_bounds(w)).expect("bounds");
    for (x, y) in [(x + w / 2.0, y + h / 2.0), theirs] {
        probe::post_mouse(MouseAction::Drag, x - 3.0, y);
        sleep(cx, 200).await;
        for delta in NOTCHES {
            probe::post_wheel_at(
                x,
                y,
                &WheelSample {
                    delta,
                    ..WheelSample::default()
                },
            );
            sleep(cx, 700).await;
        }
    }
    let reply = electron.request(cx, r#"{"op":"wheels"}"#, "wheels ").await;
    let dom = json::parse(&reply).expect("json");
    let number = |v: &Json, i: usize| v.as_array().expect("event")[i].as_f64().expect("number");
    let ours = log.borrow().clone();
    let events: Vec<(f64, f64)> = dom
        .as_array()
        .expect("events")
        .iter()
        .map(|e| (number(e, 0) + 0.0, number(e, 1) + 0.0))
        .collect();
    let plain: Vec<(f64, f64)> = ours.iter().map(|(x, y)| (x + 0.0, y + 0.0)).collect();
    report.check(
        "wheel notches",
        !plain.is_empty() && plain == events,
        format!("ours {plain:?}, Chromium {events:?}"),
    );
    electron.request(cx, r#"{"op":"close"}"#, "closed").await;
    cx.update(|cx| windows::close(cx, id));
    sleep(cx, 400).await;
}

fn menu() -> PopupMenu {
    let inner = PopupMenu::new()
        .entry(PopupEntry::new("Inner"))
        .entry(PopupEntry::new("Inner two"));
    PopupMenu::new()
        .entry(PopupEntry::new("First"))
        .entry(PopupEntry::new("Disabled").enabled(false))
        .separator()
        .entry(PopupEntry::new("More").submenu(inner))
        .entry(PopupEntry::new("Last"))
}

const ESCAPE: u32 = 0xff1b;
const DOWN: u32 = 0xff54;
const RETURN: u32 = 0xff0d;
const CONTROL: u32 = 0xffe3;
const SHIFT: u32 = 0xffe1;

/// The accelerator table of the hidden menu bar, on real keys.
async fn accelerators(cx: &mut AsyncApp, report: &mut Report, hub: WindowId) {
    let cases: [(&str, &[u32]); 4] = [
        ("CommandOrControl+Plus", &[CONTROL, SHIFT, '=' as u32]),
        ("CmdOrCtrl+Shift+O", &[CONTROL, SHIFT, 'o' as u32]),
        ("CmdOrCtrl+,", &[CONTROL, ',' as u32]),
        ("F11", &[0xffc8]),
    ];
    let table = cases
        .iter()
        .enumerate()
        .map(|(i, (accelerator, _))| {
            let parsed = Accelerator::parse(accelerator, Platform::Linux).expect("accelerator");
            (parsed.gpui_keystroke().expect("keystroke"), vec![i])
        })
        .collect();
    let seen: Rc<RefCell<Vec<MenuPath>>> = Rc::default();
    let sink = seen.clone();
    cx.update(|cx| {
        asrs_ui::menu::install_accelerators(cx, table, move |path, _, _| {
            sink.borrow_mut().push(path);
        });
    });
    let handle = handle_of(cx, hub);
    with_window(cx, handle, |w| w.activate_window());
    sleep(cx, 600).await;
    for (i, (accelerator, keys)) in cases.iter().enumerate() {
        probe::post_keys(keys);
        sleep(cx, 400).await;
        let got = seen.take();
        report.check(
            &format!("accelerator {accelerator}"),
            got == [vec![i]],
            format!("{got:?}"),
        );
    }
}

async fn click(cx: &mut AsyncApp, (x, y): (f64, f64)) {
    for action in [MouseAction::Drag, MouseAction::Down, MouseAction::Up] {
        probe::post_mouse(action, x, y);
        sleep(cx, 250).await;
    }
}

async fn menus(cx: &mut AsyncApp, report: &mut Report, hub: WindowId) {
    use asrs_ui::menu::{PADDING, ROW_HEIGHT, SEPARATOR_HEIGHT};
    let handle = handle_of(cx, hub);
    with_window(cx, handle, |w| w.activate_window());
    sleep(cx, 600).await;
    let (x, y, _, _) = with_window(cx, handle, |w| platform::window_bounds(w)).expect("bounds");
    let (row, pad, line) = (
        f64::from(ROW_HEIGHT),
        f64::from(PADDING),
        f64::from(SEPARATOR_HEIGHT),
    );
    let at = (x + 100.0, y + 120.0);
    let first = (at.0 + 30.0, at.1 + pad + row / 2.0);
    let disabled = (first.0, first.1 + row);
    let more = (first.0, at.1 + pad + 2.0 * row + line + row / 2.0);
    let last = (first.0, more.1 + row);
    let outside = (x + 600.0, y + 400.0);
    let closed: Rc<RefCell<Vec<Option<MenuPath>>>> = Rc::default();
    let mut cases: Vec<(&str, Option<MenuPath>)> = Vec::new();
    for name in [
        "a click chooses an entry",
        "a click on a disabled entry chooses nothing until Escape closes the menu",
        "a click outside closes the menu",
        "keys of the parent window drive the menu",
        "a submenu opens under the pointer and takes the click",
        "a click on the last entry",
        "closePopup",
    ] {
        let sink = closed.clone();
        cx.update(|cx| {
            handle.update(cx, |_, window, cx| {
                platform::popup_menu(window, cx, menu(), 100, 120, move |path, _| {
                    sink.borrow_mut().push(path);
                });
            })
        })
        .expect("window");
        sleep(cx, 900).await;
        let open = cx.update(|cx| cx.windows().len());
        report.check(
            &format!("{name}: opens"),
            open == 2,
            format!("{open} windows"),
        );
        let want = match cases.len() {
            0 => {
                click(cx, first).await;
                Some(vec![0])
            }
            1 => {
                click(cx, disabled).await;
                let still = closed.borrow().len() == 1;
                report.check("a disabled entry keeps the menu open", still, "");
                probe::post_keys(&[ESCAPE]);
                None
            }
            2 => {
                click(cx, outside).await;
                None
            }
            3 => {
                for key in [DOWN, DOWN, DOWN, RETURN] {
                    probe::post_keys(&[key]);
                    sleep(cx, 200).await;
                }
                Some(vec![4])
            }
            4 => {
                probe::post_mouse(MouseAction::Drag, first.0, first.1);
                sleep(cx, 200).await;
                probe::post_mouse(MouseAction::Drag, more.0, more.1);
                sleep(cx, 900).await;
                let windows = cx.update(|cx| cx.windows().len());
                report.check(
                    "the submenu opens",
                    windows == 3,
                    format!("{windows} windows"),
                );
                let inner = (
                    at.0 + f64::from(asrs_ui::menu::MIN_WIDTH) + 10.0,
                    more.1 + row,
                );
                click(cx, inner).await;
                Some(vec![3, 1])
            }
            5 => {
                click(cx, last).await;
                Some(vec![4])
            }
            _ => {
                with_window(cx, handle, |w| platform::close_popup_menu(w));
                None
            }
        };
        sleep(cx, 700).await;
        let left = cx.update(|cx| cx.windows().len());
        let got = closed.borrow().last().cloned();
        report.check(
            name,
            got == Some(want.clone()) && closed.borrow().len() == cases.len() + 1 && left == 1,
            format!("{got:?}, {left} windows left"),
        );
        cases.push((name, want));
        while closed.borrow().len() < cases.len() {
            closed.borrow_mut().push(None);
        }
    }
    let active = with_window(cx, handle, |w| w.is_window_active());
    report.check("the hub keeps the focus", active, "");
}

fn popup_bounds(cx: &mut AsyncApp, parent: AnyWindowHandle) -> Option<(f64, f64, f64, f64)> {
    let popup = cx.update(|cx| cx.windows().into_iter().find(|w| *w != parent))?;
    with_window(cx, popup, |w| platform::window_bounds(w))
}

/// A menu and a select list at the edge of the left of two monitors, and a list longer than it
/// shows that opens on its selected option.
async fn lists(cx: &mut AsyncApp, report: &mut Report, hub: WindowId) {
    const ROWS: f64 = 20.0;
    let handle = handle_of(cx, hub);
    with_window(cx, handle, |w| w.activate_window());
    sleep(cx, 600).await;
    let (x, y, _, _) = with_window(cx, handle, |w| platform::window_bounds(w)).expect("bounds");
    let chosen: Rc<RefCell<Vec<Option<usize>>>> = Rc::default();
    let open = |cx: &mut AsyncApp, at: [f64; 4]| {
        let sink = chosen.clone();
        let items = (0..60).map(|i| SelectMenuItem {
            label: format!("Option {i}"),
            kind: SelectMenuItemKind::Option,
            enabled: true,
            tool_tip: String::new(),
        });
        let menu = SelectMenu {
            items: items.collect(),
            font_size: 13.0,
            selected: Some(50),
            right_aligned: false,
        };
        cx.update(|cx| {
            handle.update(cx, |_, window, cx| {
                platform::select_popup(window, cx, menu, at, move |row, _| {
                    sink.borrow_mut().push(row);
                });
            })
        })
        .expect("window");
    };
    let row = f64::from(asrs_ui::menu::ROW_HEIGHT);
    let padding = 2.0 * f64::from(asrs_ui::menu::PADDING);
    let at = [100.0, 120.0, 200.0, 22.0];

    open(cx, at);
    sleep(cx, 900).await;
    let bounds = popup_bounds(cx, handle).expect("a list");
    let want = (x + 100.0, y + 142.0, 200.0, ROWS * row + padding);
    report.check(
        "the list shows 20 rows",
        bounds == want,
        format!("{bounds:?}"),
    );
    click(cx, (x + 140.0, bounds.1 + bounds.3 - row / 2.0)).await;
    sleep(cx, 700).await;
    let got = chosen.take();
    report.check(
        "the list opens on its selected option",
        got == [Some(50)],
        format!("{got:?}"),
    );

    open(cx, at);
    sleep(cx, 900).await;
    for key in [DOWN, ' ' as u32, DOWN, RETURN] {
        probe::post_keys(&[key]);
        sleep(cx, 250).await;
    }
    sleep(cx, 700).await;
    let got = chosen.take();
    report.check(
        "Space chooses nothing in a list",
        got == [Some(52)],
        format!("{got:?}"),
    );

    let Some(port) = std::env::var_os("ASRS_RFB_PORT") else {
        println!("linux windows: one screen only, ASRS_RFB_PORT is not set");
        return;
    };
    let edge = x + 400.0;
    let screens = [
        format!("{edge}x1800+0+0"),
        format!("{}x1800+{edge}+0", 2880.0 - edge),
    ];
    report.check("two screens", set_screens(&port, &screens), "");
    sleep(cx, 900).await;
    let (x, _, _, _) = with_window(cx, handle, |w| platform::window_bounds(w)).expect("bounds");
    with_window(cx, handle, |w| w.activate_window());
    sleep(cx, 600).await;
    for (name, left, inside) in [
        ("left", edge - x - 50.0, 0.0..edge),
        ("right", edge - x + 5.0, edge..f64::MAX),
    ] {
        open(cx, [left, 120.0, 200.0, 22.0]);
        sleep(cx, 900).await;
        let list = popup_bounds(cx, handle).expect("a list");
        let ok = list.0 >= inside.start && list.0 + list.2 <= inside.end;
        report.check(
            &format!("the list stays on the {name} monitor"),
            ok,
            format!("{list:?}"),
        );
        probe::post_keys(&[ESCAPE]);
        sleep(cx, 700).await;
        cx.update(|cx| {
            handle.update(cx, |_, window, cx| {
                platform::popup_menu(window, cx, menu(), left as i32, 120, |_, _| {});
            })
        })
        .expect("window");
        sleep(cx, 900).await;
        let menu = popup_bounds(cx, handle).expect("a menu");
        let ok = menu.0 >= inside.start && menu.0 + menu.2 <= inside.end;
        report.check(
            &format!("the menu stays on the {name} monitor"),
            ok,
            format!("{menu:?}"),
        );
        probe::post_mouse(MouseAction::Drag, menu.0 + 20.0, menu.1 + 15.0);
        sleep(cx, 200).await;
        probe::post_mouse(
            MouseAction::Drag,
            menu.0 + 20.0,
            menu.1 + menu.3 - 4.0 - 1.5 * row,
        );
        sleep(cx, 900).await;
        let more = cx.update(|cx| cx.windows()).into_iter();
        let boxes: Vec<_> = more
            .filter(|w| *w != handle)
            .filter_map(|w| with_window(cx, w, |w| platform::window_bounds(w)))
            .collect();
        let ok = boxes.len() == 2
            && boxes
                .iter()
                .all(|b| b.0 >= inside.start && b.0 + b.2 <= inside.end);
        report.check(
            &format!("the submenu stays on the {name} monitor"),
            ok,
            format!("{boxes:?}"),
        );
        for _ in 0..2 {
            probe::post_keys(&[ESCAPE]);
            sleep(cx, 400).await;
        }
    }
    with_window(cx, handle, |w| w.activate_window());
    sleep(cx, 600).await;
    let (x, y, _, _) = with_window(cx, handle, |w| platform::window_bounds(w)).expect("bounds");
    // A panel under the left screen, which ends a row below where the menu opens.
    let bottom = y + 120.0 + 2.0 * row;
    let panel = [0.0, 0.0, edge, bottom].map(|v| format!("{v}")).join(", ");
    let set = Command::new("xprop")
        .args(["-root", "-f", "_NET_WORKAREA", "32c"])
        .args(["-set", "_NET_WORKAREA", &panel])
        .status();
    report.check("a work area", set.is_ok_and(|s| s.success()), "");
    sleep(cx, 600).await;
    let electron = Electron::start();
    electron.reply(cx, "ready ").await;
    let theirs = electron
        .request(cx, r#"{"op":"displays"}"#, "displays ")
        .await;
    let displays = platform::x11_displays(2880.0).expect("displays");
    let ours: Vec<_> = displays.displays.iter().map(|d| d.work_area).collect();
    report.check(
        "work areas",
        format!("{ours:?}") == theirs,
        format!("{ours:?}, Electron {theirs}"),
    );
    cx.update(|cx| {
        handle.update(cx, |_, window, cx| {
            platform::popup_menu(window, cx, menu(), (edge - x) as i32 - 300, 120, |_, _| {});
        })
    })
    .expect("window");
    sleep(cx, 900).await;
    let menu = popup_bounds(cx, handle).expect("a menu");
    report.check(
        "the menu stays above the panel",
        menu.1 + menu.3 <= bottom,
        format!("{menu:?} {bottom}"),
    );
    probe::post_keys(&[ESCAPE]);
    sleep(cx, 700).await;
    let satellite = cx
        .update(|cx| windows::open_satellite(cx, "u", SatellitePlacement::Unplaced, blank))
        .expect("satellite")
        .0;
    sleep(cx, 900).await;
    let placed = handle_of(cx, satellite);
    let bounds = with_window(cx, placed, |w| platform::window_bounds(w)).expect("bounds");
    let request = r#"{"op":"centred"}"#;
    let theirs = electron.request(cx, request, "centred ").await;
    let ours = format!("{:?}", [bounds.0, bounds.1]);
    report.check(
        "a window without a place",
        ours == theirs,
        format!("{ours}, Electron {theirs}"),
    );
    cx.update(|cx| windows::close(cx, satellite));
    sleep(cx, 600).await;
    let one = set_screens(&port, &["2880x1800+0+0".to_owned()]);
    report.check("one screen", one, "");
    sleep(cx, 900).await;
}

fn set_screens(port: &std::ffi::OsStr, screens: &[String]) -> bool {
    Command::new(vectors::workspace_root().join("xtask/hosts/linux/screens.py"))
        .arg(port)
        .args(screens)
        .status()
        .is_ok_and(|status| status.success())
}

fn main() {
    if asrs_parity::ci::skip_in_ci("linux windows", "a display and the oracle's Electron") {
        return;
    }
    if !cfg!(target_os = "linux") {
        println!("linux windows: skipped on this platform");
        return;
    }
    if std::env::var_os("DISPLAY").is_none() || std::env::var_os("WAYLAND_DISPLAY").is_some() {
        println!("linux windows: skipped, not an X11 session");
        return;
    }
    asrs_ui::gpui::application().run(|cx: &mut App| {
        asrs_ui::init(cx);
        windows::init(cx);
        cx.spawn(async move |cx| {
            let electron = Electron::start();
            let version = electron.reply(cx, "ready ").await;
            let mut report = Report::default();
            report.check("Electron is upstream's", version == "39.2.7", &version);
            let hub = cx
                .update(|cx| windows::open_hub(cx, None, blank))
                .expect("hub")
                .0;
            sleep(cx, 1000).await;
            menus(cx, &mut report, hub).await;
            lists(cx, &mut report, hub).await;
            accelerators(cx, &mut report, hub).await;
            wheel(cx, &electron, &mut report).await;
            aspects(cx, &electron, &mut report).await;
            sizes(cx, &electron, &mut report, hub).await;
            drop(electron);
            println!(
                "linux windows: {} passed, {} failed {:?}",
                report.passed,
                report.failed.len(),
                report.failed
            );
            std::process::exit(i32::from(!report.failed.is_empty()));
        })
        .detach();
    });
}
