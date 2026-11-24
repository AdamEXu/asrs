#![allow(clippy::disallowed_methods)]

//! The Windows window and menu checks on a real desktop, beside the oracle's Electron: the content
//! size, bounds, track sizes, `WS_EX_TOPMOST` and DWM backdrop of every window `main.ts` opens,
//! satellites' aspect ratios in a live resize, `isVisible()`, wheel deltas as Chromium's DOM
//! events carry them, native menus driven by real pointer and key events, and the hub's own
//! window against the capture scenario mode makes of it.
//!
//! Needs the logged-in desktop at scale 1 and the oracle's Electron (`ASRS_ELECTRON` or
//! `cargo xtask oracle setup`); skipped under `ASRS_CI=1` and off Windows. It moves the pointer.

#[cfg(windows)]
mod desktop {
    use std::cell::RefCell;
    use std::io::{BufRead, BufReader, Write as _};
    use std::net::{TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::rc::Rc;
    use std::sync::mpsc;
    use std::time::Duration;

    use asrs_app::main_process;
    use asrs_app::menus::accelerator::Accelerator;
    use asrs_app::scenario;
    use asrs_app::views::hub_host::{HubEntityHost, boot_messages};
    use asrs_app::windows::{self, Rect, SatellitePlacement, WindowId, WindowKind, Windows};
    use asrs_model::messages::{MainToHub, NamedMessage};
    use asrs_parity::json::{self, Json};
    use asrs_parity::pixel::{self, Rgb, Rgba};
    use asrs_parity::vectors;
    use asrs_services::fs::RealFs;
    use asrs_services::lifecycle::{AppPaths, MainState};
    use asrs_services::path::PathStyle;
    use asrs_services::window_manager::{OptionValue, Platform, WindowOptions};
    use asrs_services::windows::WindowSystem;
    use asrs_ui::gpui::{
        AnyWindowHandle, App, AppContext, AsyncApp, Context, Entity, InteractiveElement,
        IntoElement, Render, ScrollEvent, Styled, Window, div, px, size,
    };
    use asrs_ui::platform::probe::{self, MouseAction, WheelSample};
    use asrs_ui::platform::{
        self, MenuPath, PopupEntry, PopupMenu, SelectMenu, SelectMenuItem, SelectMenuItemKind,
    };
    use asrs_ui::selection::colors::ForcedActive;
    use asrs_ui::theme::ThemeSource;

    const ELECTRON_MAIN: &str = r##"
const { app, BrowserWindow, Menu } = require("electron");
const readline = require("readline");
// The main process has no readable stdin on Windows.
const socket = require("net").connect(Number(process.argv.at(-1)), "127.0.0.1");
const out = (line) => socket.write(line + "\n");
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const id = (win) => win.getNativeWindowHandle().readUInt32LE(0);
app.whenReady().then(() => {
  Menu.setApplicationMenu(Menu.buildFromTemplate([{ label: "App", submenu: [{ label: "Item" }] }]));
  const parent = new BrowserWindow({ x: 60, y: 420, width: 1100, height: 650 });
  let win = null;
  out("ready " + process.versions.electron);
  readline.createInterface({ input: socket }).on("line", async (line) => {
    const request = JSON.parse(line);
    if (request.op === "open") {
      const options = { ...request.options };
      if (options.parent) options.parent = parent;
      if (request.hub) Object.assign(options, { backgroundMaterial: "acrylic", titleBarStyle: "hidden", titleBarOverlay: { color: "#00000000", symbolColor: "#000000", height: 38 } });
      win = new BrowserWindow({ ...options, show: false });
      if (!request.hub) win.setMenu(null);
      win.setMenuBarVisibility(false);
      win.once("ready-to-show", () => win.show());
      await win.loadURL("data:text/html,<body style='margin:0;background:white" + (request.hub ? "" : ";height:5000px") + "'>");
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
    } else if (request.op === "state") {
      win[request.to](...(request.args ?? []));
      await wait(1500);
      out("state " + JSON.stringify([...win.getContentSize(), ...Object.values(win.getBounds())]));
    } else if (request.op === "wheel") {
      await win.webContents.executeJavaScript(
        "window.__wheel = []; window.addEventListener('wheel', (e) => window.__wheel.push([e.deltaX, e.deltaY, e.deltaMode])); true"
      );
      win.focus();
      await wait(300);
      out("wheel ready");
    } else if (request.op === "wheels") {
      out("wheels " + JSON.stringify(await win.webContents.executeJavaScript("window.__wheel")));
    } else if (request.op === "focus") {
      win.setAlwaysOnTop(true);
      win.focus();
      await wait(600);
      out("focused");
    } else if (request.op === "close") {
      win.destroy();
      await wait(300);
      out("closed");
    } else {
      app.quit();
    }
  });
});
"##;

    fn electron_binary() -> PathBuf {
        if let Some(path) = std::env::var_os("ASRS_ELECTRON") {
            return PathBuf::from(path);
        }
        let package =
            vectors::workspace_root().join("target/oracle/upstream/node_modules/electron");
        let rel = std::fs::read_to_string(package.join("path.txt")).expect(
            "Electron is not installed; run `cargo xtask oracle setup` or set ASRS_ELECTRON",
        );
        package.join("dist").join(rel.trim())
    }

    struct Electron {
        child: RefCell<Child>,
        socket: RefCell<TcpStream>,
        lines: mpsc::Receiver<String>,
    }

    impl Electron {
        fn start() -> Self {
            let dir =
                std::env::temp_dir().join(format!("asrs-windows-desktop-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("temp dir");
            std::fs::write(dir.join("main.js"), ELECTRON_MAIN).expect("main.js");
            let listener = TcpListener::bind("127.0.0.1:0").expect("port");
            let port = listener.local_addr().expect("port").port();
            let child = Command::new(electron_binary())
                .args(["--force-device-scale-factor=1"])
                .arg(format!("--user-data-dir={}", dir.join("data").display()))
                .arg(dir.join("main.js"))
                .arg(port.to_string())
                .env_remove("ELECTRON_RUN_AS_NODE")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("Electron");
            let (socket, _) = listener.accept().expect("Electron's connection");
            let replies = BufReader::new(socket.try_clone().expect("socket"));
            let (send, lines) = mpsc::channel();
            std::thread::spawn(move || {
                for line in replies.lines().map_while(Result::ok) {
                    if send.send(line).is_err() {
                        return;
                    }
                }
            });
            Electron {
                child: RefCell::new(child),
                socket: RefCell::new(socket),
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
            writeln!(self.socket.borrow_mut(), "{request}").expect("request");
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

    /// Where Electron's parent window is, which is as large as ours.
    const PARENT: (f64, f64) = (60.0, 420.0);

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
            let backdrop = probe::backdrop_type(number);
            let theirs = electron
                .numbers(cx, &electron_options(kind, &options), "open ")
                .await;
            let number = theirs[0] as u32;
            let their_hints =
                with_window(cx, handle, |w| probe::size_hints(w, number)).expect("hints");
            report.check(
                &format!("{name} content size"),
                ours == (theirs[1], theirs[2]),
                format!("ours {ours:?}, Electron {:?}", &theirs[1..3]),
            );
            let mut bounds = [bounds.0, bounds.1, bounds.2, bounds.3];
            if options.modal {
                let parent = cx.update(|cx| Windows::manager(cx).bounds(parent));
                let parent = parent.expect("the parent's bounds");
                bounds[0] += PARENT.0 - parent.x;
                bounds[1] += PARENT.1 - parent.y;
            }
            report.check(
                &format!("{name} bounds"),
                bounds == theirs[3..7],
                format!("ours {bounds:?}, Electron {:?}", &theirs[3..7]),
            );
            let frame = |window: u32| {
                let [window, client] = probe::rects(window).expect("rects");
                let side = |r: [i32; 4], i: usize| r[i + 2] - r[i];
                [0, 1].map(|i| side(window, i) - side(client, i))
            };
            let ours = with_window(cx, handle, |w| probe::window_number(w)).expect("window id");
            let content = |min: Option<(i32, i32)>, frame: [i32; 2]| {
                min.map(|(w, h)| (w - frame[0], h - frame[1]))
            };
            let limited = options.min_width.is_some() || options.max_width.is_some();
            if limited {
                let mine = content(hints.min, frame(ours));
                let other = content(their_hints.min, frame(number));
                report.check(
                    &format!("{name} minimum content size"),
                    mine == other,
                    format!("ours {mine:?}, Electron {other:?}"),
                );
            }
            report.check(
                &format!("{name} WS_EX_TOPMOST"),
                hints.above == their_hints.above && hints.above == options.always_on_top,
                format!("ours {}, Electron {}", hints.above, their_hints.above),
            );
            if kind == WindowKind::Hub {
                report.check(
                    "the hub's backdrop type",
                    backdrop == Some(3),
                    format!("{backdrop:?}"),
                );
            }
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
        let opened = cx
            .update(|cx| windows::open_satellite(cx, "a", SatellitePlacement::Saved(rect), blank));
        let (id, _) = opened.expect("open window");
        let handle = handle_of(cx, id);
        sleep(cx, 800).await;
        let number = with_window(cx, handle, |w| probe::window_number(w)).expect("window id");
        let request = r#"{"op":"open","options":{"x":160,"y":160,"width":300,"height":160,"minWidth":200,"minHeight":100,"resizable":true}}"#;
        let theirs = electron.numbers(cx, request, "open ").await[0] as u32;
        for (ratio, lock) in RATIOS {
            let content =
                with_window(cx, handle, |w| probe::content_size(w)).expect("content size");
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
            report.check(
                &format!("satellite ratio {ratio} lock {lock} content"),
                [ours.0, ours.1] == size[..],
                format!("ours {ours:?}, Electron {size:?}"),
            );
            let mut answers = Vec::new();
            for edge in 1..=8 {
                let dragged = [160, 160, 160 + 431, 160 + 277];
                answers.push((
                    edge,
                    probe::sizing(number, edge, dragged),
                    probe::sizing(theirs, edge, dragged),
                ));
            }
            let differing: Vec<_> = answers.iter().filter(|(_, a, b)| a != b).collect();
            report.check(
                &format!("satellite ratio {ratio} lock {lock} live resize"),
                differing.is_empty(),
                format!("(edge, ours, Electron) {differing:?}"),
            );
        }
        let mut ours = vec![cx.update(|cx| Windows::is_visible(cx, id))];
        with_window(cx, handle, |w| w.minimize_window());
        sleep(cx, 1200).await;
        ours.push(cx.update(|cx| Windows::is_visible(cx, id)));
        let reply = electron
            .request(cx, r#"{"op":"visible"}"#, "visible ")
            .await;
        report.check(
            "isVisible() shown and minimized",
            reply == format!("[{},{}]", ours[0], ours[1]),
            format!("ours {ours:?}, Electron {reply}"),
        );
        electron.request(cx, r#"{"op":"close"}"#, "closed").await;
        cx.update(|cx| windows::close(cx, id));
        sleep(cx, 400).await;
    }

    /// `getContentSize()` and `getBounds()` through every window state, which `state.json` gets.
    async fn states(cx: &mut AsyncApp, electron: &Electron, report: &mut Report) {
        const STEPS: [&str; 8] = [
            "minimize",
            "restore",
            "maximize",
            "minimize",
            "restore",
            "unmaximize",
            "minimize",
            "restore",
        ];
        for kind in [WindowKind::Hub, WindowKind::Satellite] {
            let hub = kind == WindowKind::Hub;
            let rect = Rect {
                x: 160.0,
                y: 160.0,
                width: if hub { 800.0 } else { 200.0 },
                height: if hub { 400.0 } else { 100.0 },
            };
            let opened = cx.update(|cx| match kind {
                WindowKind::Hub => windows::open_hub(cx, Some(rect), blank),
                _ => windows::open_satellite(cx, "m", SatellitePlacement::Saved(rect), blank),
            });
            let id = opened.expect("open window").0;
            let handle = handle_of(cx, id);
            sleep(cx, 1200).await;
            let number = with_window(cx, handle, |w| probe::window_number(w)).expect("window id");
            let request = format!(
                r#"{{"op":"open","hub":{hub},"options":{{"x":160,"y":160,"width":{},"height":{},"minWidth":{},"minHeight":{},"resizable":true}}}}"#,
                rect.width, rect.height, rect.width, rect.height
            );
            electron.numbers(cx, &request, "open ").await;
            for (step, to) in STEPS.into_iter().enumerate() {
                probe::show_window(number, to);
                let request = format!(r#"{{"op":"state","to":"{to}"}}"#);
                let theirs = electron.numbers(cx, &request, "state ").await;
                let content =
                    with_window(cx, handle, |w| probe::content_size(w)).expect("content size");
                let bounds = cx
                    .update(|cx| Windows::manager(cx).bounds(id))
                    .expect("bounds");
                let ours = [
                    content.0,
                    content.1,
                    bounds.x,
                    bounds.y,
                    bounds.width,
                    bounds.height,
                ];
                report.check(
                    &format!("{kind:?} state {step} {to}"),
                    ours == theirs[..],
                    format!("ours {ours:?}, Electron {theirs:?}"),
                );
            }
            electron.request(cx, r#"{"op":"close"}"#, "closed").await;
            cx.update(|cx| windows::close(cx, id));
            sleep(cx, 400).await;
        }
    }

    async fn wheel(cx: &mut AsyncApp, electron: &Electron, report: &mut Report) {
        const UNITS: [[i32; 2]; 9] = [
            [120, 0],
            [-120, 0],
            [-360, 0],
            [40, 0],
            [-40, 0],
            [-80, 0],
            [0, 120],
            [0, -120],
            [0, 40],
        ];
        let rect = Rect {
            x: 2200.0,
            y: 200.0,
            width: 400.0,
            height: 300.0,
        };
        let opened = cx
            .update(|cx| windows::open_satellite(cx, "w", SatellitePlacement::Saved(rect), blank));
        let (id, view) = opened.expect("open window");
        let handle = handle_of(cx, id);
        let log = cx.update(|cx| view.read(cx).wheel.clone());
        let request = r#"{"op":"open","options":{"x":2200,"y":700,"width":400,"height":300}}"#;
        let opened = electron.numbers(cx, request, "open ").await;
        let theirs = (opened[3] + 200.0, opened[4] + 150.0);
        electron
            .request(cx, r#"{"op":"wheel"}"#, "wheel ready")
            .await;
        let (x, y, w, h) = with_window(cx, handle, |w| platform::window_bounds(w)).expect("bounds");
        for (x, y) in [(x + w / 2.0, y + h / 2.0), theirs] {
            probe::post_mouse(MouseAction::Drag, x - 3.0, y);
            sleep(cx, 200).await;
            for delta in UNITS {
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
            "wheel notches and 40 units",
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

    const ESCAPE: u32 = 0x1b;
    const DOWN: u32 = 0x28;
    const RETURN: u32 = 0x0d;
    const CONTROL: u32 = 0x11;
    const SHIFT: u32 = 0x10;

    fn rich_menu() -> PopupMenu {
        menu()
            .entry(
                PopupEntry::new("&Open\u{2026}")
                    .shortcut("Ctrl+O")
                    .checked(true),
            )
            .entry(PopupEntry::new("Red").swatch([0xd0, 0x20, 0x18]))
    }

    async fn menus(cx: &mut AsyncApp, report: &mut Report, hub: WindowId) {
        let rich = rich_menu();
        let read = probe::native_menu_roundtrip(&rich);
        report.check(
            "the native menu reads back as it was built",
            read.as_ref() == Some(&rich),
            format!("{read:?}"),
        );
        let handle = handle_of(cx, hub);
        with_window(cx, handle, |w| w.activate_window());
        sleep(cx, 800).await;
        let number = with_window(cx, handle, |w| probe::window_number(w)).expect("window id");
        let [_, client] = probe::rects(number).expect("rects");
        let (x, y) = (f64::from(client[0]), f64::from(client[1]));
        let closed: Rc<RefCell<Vec<Option<MenuPath>>>> = Rc::default();
        let names = [
            "a click chooses an entry",
            "a click on a disabled entry chooses nothing until Escape closes the menu",
            "a click outside closes the menu",
            "keys drive the menu",
            "a submenu takes keys",
            "closePopup",
        ];
        for (index, name) in names.into_iter().enumerate() {
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
            let frames = probe::menu_window_frames();
            let Some(frame) = frames.first().copied() else {
                report.check(&format!("{name}: opens"), false, "no menu window");
                continue;
            };
            let at = frame.x == x + 100.0 && frame.y == y + 120.0;
            report.check(
                &format!("{name}: opens at its position"),
                at,
                format!("{frame:?}"),
            );
            let row = frame.height / 5.0;
            let want = match index {
                0 => {
                    click(cx, (frame.x + 40.0, frame.y + row / 2.0)).await;
                    Some(vec![0])
                }
                1 => {
                    click(cx, (frame.x + 40.0, frame.y + 1.6 * row)).await;
                    let open = !probe::menu_window_frames().is_empty();
                    report.check("a disabled entry keeps the menu open", open, "");
                    probe::post_keys(&[ESCAPE]);
                    None
                }
                2 => {
                    click(cx, (x + 700.0, y + 450.0)).await;
                    None
                }
                3 => {
                    for key in [DOWN, DOWN, DOWN, DOWN, RETURN] {
                        probe::post_keys(&[key]);
                        sleep(cx, 200).await;
                    }
                    Some(vec![4])
                }
                4 => {
                    for key in [DOWN, DOWN, DOWN, RETURN, DOWN, RETURN] {
                        probe::post_keys(&[key]);
                        sleep(cx, 300).await;
                    }
                    Some(vec![3, 1])
                }
                _ => {
                    with_window(cx, handle, |w| platform::close_popup_menu(w));
                    None
                }
            };
            sleep(cx, 900).await;
            let left = probe::menu_window_frames().len();
            let got = closed.take();
            report.check(
                name,
                got == [want] && left == 0,
                format!("{got:?}, {left} menus left"),
            );
        }
    }

    /// The accelerator table of the hidden menu bar, on real keys.
    async fn accelerators(cx: &mut AsyncApp, report: &mut Report, hub: WindowId) {
        let cases: [(&str, &[u32]); 4] = [
            ("CommandOrControl+Plus", &[CONTROL, SHIFT, 0xbb]),
            ("CmdOrCtrl+Shift+O", &[CONTROL, SHIFT, 'O' as u32]),
            ("CmdOrCtrl+,", &[CONTROL, 0xbc]),
            ("F11", &[0x7a]),
        ];
        let table = cases
            .iter()
            .enumerate()
            .map(|(i, (accelerator, _))| {
                let parsed =
                    Accelerator::parse(accelerator, Platform::Windows).expect("accelerator");
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

    /// The window's premultiplied pixels as a capture has them.
    fn straight(width: u32, height: u32, pixels: &[u8]) -> Rgba {
        let mut data = pixels.to_vec();
        for pixel in data.as_chunks_mut::<4>().0 {
            let alpha = u32::from(pixel[3]);
            if alpha != 0 && alpha != 255 {
                for c in &mut pixel[..3] {
                    *c = ((u32::from(*c) * 255 + alpha / 2) / alpha).min(255) as u8;
                }
            }
        }
        Rgba {
            width,
            height,
            data,
        }
    }

    /// Leaves two differing images in the temp folder as PPM, for whoever looks at them.
    fn dump(name: &str, mine: &Rgb, other: &Rgb) {
        let dir = std::env::temp_dir().join("asrs-captions");
        std::fs::create_dir_all(&dir).expect("temp dir");
        for (side, image) in [("ours", mine), ("other", other)] {
            let mut ppm = format!("P6 {} {} 255\n", image.width, image.height).into_bytes();
            ppm.extend(image.data.iter().flatten());
            let name = name.replace([' ', ','], "-");
            std::fs::write(dir.join(format!("{name}-{side}.ppm")), ppm).expect("ppm");
        }
    }

    const STRIP: (i32, i32) = (140, 38);

    /// The screen's pixels over the caption buttons of a window's content, less the row and
    /// column of the border DWM draws around a window that is not maximized.
    fn strip(number: u32, maximized: bool) -> Rgb {
        let [_, client] = probe::rects(number).expect("rects");
        let border = i32::from(!maximized);
        let (width, height) = (STRIP.0 - border, STRIP.1 - border);
        let x = client[2] - STRIP.0;
        let image = probe::screen_image(x, client[1] + border, width, height).expect("screen");
        let image = Rgba {
            width: image.width,
            height: image.height,
            data: image.rgba,
        };
        pixel::composite(&image, [0, 0, 0])
    }

    /// The hub's caption buttons on screen, under the real pointer, beside Electron's.
    async fn captions(cx: &mut AsyncApp, electron: &Electron, report: &mut Report) {
        let rect =
            r#"{"op":"open","hub":true,"options":{"x":200,"y":150,"width":1100,"height":650}}"#;
        let theirs = electron.numbers(cx, rect, "open ").await[0] as u32;
        let dir = std::env::temp_dir().join(format!("asrs-captions-{}", std::process::id()));
        cx.update(|cx| cx.set_global(ThemeSource::Light));
        let opened = cx.update(|cx| {
            windows::open_hub(cx, None, |window, cx| {
                let boot = fresh_boot(&dir);
                HubEntityHost::open_with(window, false, boot, None, cx)
            })
        });
        let id = opened.expect("hub").0;
        let handle = handle_of(cx, id);
        let ours = with_window(cx, handle, |w| probe::window_number(w)).expect("window id");
        sleep(cx, 4000).await;
        let mut shots: Vec<Vec<(String, Rgb)>> = Vec::new();
        for (window, mine) in [(ours, true), (theirs, false)] {
            if mine {
                with_window(cx, handle, |w| w.activate_window());
            } else {
                electron.request(cx, r#"{"op":"focus"}"#, "focused").await;
            }
            sleep(cx, 1000).await;
            let mut list = Vec::new();
            for maximized in [false, true] {
                // WARP takes over a second for a maximized frame in a debug build.
                let settle = if maximized { 4000 } else { 900 };
                if maximized {
                    let [_, client] = probe::rects(window).expect("rects");
                    click(cx, (f64::from(client[2] - 70), f64::from(client[1] + 32))).await;
                    sleep(cx, 4000).await;
                }
                let [_, client] = probe::rects(window).expect("rects");
                let at = |dx: i32, y: i32| (f64::from(client[2] - dx), f64::from(client[1] + y));
                let away = at(400, 300);
                for (name, dx) in [
                    ("rest", 0),
                    ("minimize", 115),
                    ("maximize", 70),
                    ("close", 24),
                ] {
                    if dx == 0 {
                        probe::post_mouse(MouseAction::Drag, away.0, away.1);
                        sleep(cx, settle).await;
                        list.push((
                            format!("{name}, maximized {maximized}"),
                            strip(window, maximized),
                        ));
                        continue;
                    }
                    let (x, y) = at(dx, 32);
                    probe::post_mouse(MouseAction::Drag, x, y);
                    sleep(cx, settle).await;
                    list.push((
                        format!("{name} hovered, maximized {maximized}"),
                        strip(window, maximized),
                    ));
                    probe::post_mouse(MouseAction::Down, x, y);
                    sleep(cx, settle).await;
                    list.push((
                        format!("{name} pressed, maximized {maximized}"),
                        strip(window, maximized),
                    ));
                    probe::post_mouse(MouseAction::Drag, away.0, away.1);
                    sleep(cx, settle).await;
                    let dragged = format!("{name} dragged off, maximized {maximized}");
                    list.push((dragged, strip(window, maximized)));
                    probe::post_mouse(MouseAction::Up, away.0, away.1);
                    sleep(cx, 400).await;
                }
            }
            if mine {
                with_window(cx, handle, |w| w.minimize_window());
                sleep(cx, 1500).await;
            }
            shots.push(list);
        }
        electron.request(cx, r#"{"op":"close"}"#, "closed").await;
        for ((name, mine), (_, other)) in shots[0].iter().zip(&shots[1]) {
            let compared = pixel::compare(mine, other, [0, 0, 0]);
            let (width, height) = (mine.width as usize, mine.height as usize);
            let (ok, detail) = match compared {
                Ok(compared) => {
                    let differing = compared.mask.iter().filter(|d| **d).count();
                    let blob = pixel::max_blob(&compared.mask, width, height);
                    if blob > 0 {
                        dump(name, mine, other);
                    }
                    let rate = 100.0 * differing as f64 / compared.mask.len() as f64;
                    let detail = format!("{differing} px differ ({rate:.3} %), blob {blob}");
                    (rate <= 0.2 && blob <= 64, detail)
                }
                Err(e) => (false, e),
            };
            report.check(&format!("caption buttons, {name}"), ok, detail);
        }
        with_window(cx, handle, |w| w.activate_window());
        sleep(cx, 4000).await;
        let [_, client] = probe::rects(ours).expect("rects");
        let at = |dx: i32| (f64::from(client[2] - dx), f64::from(client[1] + 32));
        let maximized = with_window(cx, handle, |w| w.is_maximized());
        report.check("the maximize button maximizes", maximized, "");
        click(cx, at(70)).await;
        sleep(cx, 1500).await;
        let restored = !with_window(cx, handle, |w| w.is_maximized());
        report.check("the maximize button restores", restored, "");
        let [_, client] = probe::rects(ours).expect("rects");
        let at = |dx: i32| (f64::from(client[2] - dx), f64::from(client[1] + 32));
        let dragged = at(500);
        for (action, (x, y)) in [
            (MouseAction::Drag, dragged),
            (MouseAction::Down, dragged),
            (MouseAction::Drag, (dragged.0 + 20.0, dragged.1 + 20.0)),
            (MouseAction::Drag, (dragged.0 + 60.0, dragged.1 + 40.0)),
            (MouseAction::Up, (dragged.0 + 60.0, dragged.1 + 40.0)),
        ] {
            probe::post_mouse(action, x, y);
            sleep(cx, 400).await;
        }
        let [_, moved] = probe::rects(ours).expect("rects");
        report.check(
            "the title bar drags the window",
            [moved[0] - client[0], moved[1] - client[1]] == [60, 40],
            format!("{client:?} to {moved:?}"),
        );
        let at = |dx: i32| (f64::from(moved[2] - dx), f64::from(moved[1] + 32));
        click(cx, at(115)).await;
        sleep(cx, 1500).await;
        let hidden = !cx.update(|cx| Windows::is_visible(cx, id));
        report.check("the minimize button minimizes", hidden, "");
        with_window(cx, handle, |w| w.activate_window());
        sleep(cx, 1500).await;
        let [_, client] = probe::rects(ours).expect("rects");
        click(cx, (f64::from(client[2] - 24), f64::from(client[1] + 32))).await;
        sleep(cx, 1500).await;
        let closed = cx.update(|cx| Windows::handle(cx, id).is_none());
        report.check("the close button closes", closed, "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The drawn select list: below its control, with keys, and closed by a press outside.
    async fn lists(cx: &mut AsyncApp, report: &mut Report, hub: WindowId) {
        let handle = handle_of(cx, hub);
        with_window(cx, handle, |w| w.activate_window());
        sleep(cx, 800).await;
        let others = cx.update(|cx| cx.windows().len());
        let number = with_window(cx, handle, |w| probe::window_number(w)).expect("window id");
        let [_, client] = probe::rects(number).expect("rects");
        let (x, y) = (f64::from(client[0]), f64::from(client[1]));
        let chosen: Rc<RefCell<Vec<Option<usize>>>> = Rc::default();
        for (name, keys, want) in [
            ("keys choose an option", &[DOWN, DOWN, RETURN][..], Some(12)),
            (
                "Escape keeps what keys chose",
                &[DOWN, ESCAPE][..],
                Some(11),
            ),
            ("a press outside chooses nothing", &[][..], None),
        ] {
            let items = (0..30).map(|i| SelectMenuItem {
                label: format!("Option {i}"),
                kind: SelectMenuItemKind::Option,
                enabled: true,
                tool_tip: String::new(),
            });
            let menu = SelectMenu {
                items: items.collect(),
                font_size: 13.0,
                selected: Some(10),
                right_aligned: false,
            };
            let sink = chosen.clone();
            cx.update(|cx| {
                handle.update(cx, |_, window, cx| {
                    let at = [100.0, 120.0, 200.0, 22.0];
                    platform::select_popup(window, cx, menu, at, move |row, _| {
                        sink.borrow_mut().push(row);
                    });
                })
            })
            .expect("window");
            sleep(cx, 1500).await;
            let windows = cx.update(|cx| cx.windows());
            let list = windows.into_iter().find(|w| *w != handle);
            let open = cx.update(|cx| cx.windows().len()) == others + 1;
            report.check(&format!("select list, {name}: opens"), open, "");
            if let Some(list) = list.filter(|_| others == 1) {
                let bounds = with_window(cx, list, |w| platform::window_bounds(w));
                let placed =
                    bounds.is_some_and(|b| (b.0, b.1, b.2) == (x + 100.0, y + 142.0, 200.0));
                report.check(
                    &format!("select list, {name}: opens below its control"),
                    placed,
                    format!("{bounds:?}"),
                );
            }
            for key in keys {
                probe::post_keys(&[*key]);
                sleep(cx, 400).await;
            }
            if keys.is_empty() {
                click(cx, (x + 700.0, y + 500.0)).await;
            }
            sleep(cx, 1000).await;
            let got = chosen.take();
            let left = cx.update(|cx| cx.windows().len());
            report.check(
                &format!("select list, {name}"),
                got == [want] && left == others,
                format!("{got:?}, {left} windows left of {others}"),
            );
        }
    }

    struct NoScreen;

    impl WindowSystem for NoScreen {
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
                width: 1280.0,
                height: 800.0,
            }
        }
    }

    /// What main sends the hub of a first launch in the light theme.
    fn fresh_boot(user_data: &Path) -> Vec<NamedMessage<MainToHub>> {
        std::fs::create_dir_all(user_data).expect("userData");
        std::fs::write(user_data.join("prefs.json"), r#"{"theme":"light"}"#).expect("prefs.json");
        let path = |path: &Path| path.to_str().expect("path").to_owned();
        let assets = vectors::workspace_root().join("reference/AdvantageScope/bundledAssets");
        let paths = AppPaths {
            style: PathStyle::host(),
            user_data: path(user_data),
            temp: path(&user_data.join("temp")),
            bundled_assets: path(&assets),
            packaged: false,
        };
        let started = MainState::start(&RealFs, &NoScreen, paths, main_process::process_platform());
        let (main, _) = started.unwrap_or_else(|e| panic!("{e:?}"));
        let preferences = main
            .preferences(&RealFs)
            .unwrap_or_else(|e| panic!("{e:?}"));
        boot_messages(main.assets.set_assets_data(), true, preferences, Vec::new())
    }

    /// The hub as `windows::open_hub` opens it, drawn by GPUI in its own window, against the capture
    /// scenario mode makes of the same page.
    async fn own_window(cx: &mut AsyncApp, report: &mut Report) {
        let out = std::env::temp_dir().join(format!("asrs-own-window-{}", std::process::id()));
        let status = Command::new(std::env::current_exe().expect("test binary"))
            .arg("--capture")
            .arg(&out)
            .status()
            .expect("the capture process");
        report.check("the scenario runs", status.success(), status);
        cx.update(|cx| {
            cx.set_global(ThemeSource::Light);
            cx.set_global(ForcedActive(false));
        });
        let opened = cx.update(|cx| {
            windows::open_hub(cx, None, |window, cx| {
                let boot = fresh_boot(&out.join("own"));
                HubEntityHost::open_with(window, false, boot, None, cx)
            })
        });
        let handle = handle_of(cx, opened.expect("hub").0);
        let number = with_window(cx, handle, |w| probe::window_number(w)).expect("window id");
        for scale in [1u32, 2] {
            probe::change_scale(number, scale);
            with_window(cx, handle, |w| w.resize(size(px(1100.0), px(650.0))));
            probe::post_mouse(MouseAction::Drag, 2870.0, 10.0);
            sleep(cx, 6000).await;
            let own = with_window(cx, handle, |w| w.render_to_image()).expect("render_to_image");
            let own = straight(own.width(), own.height(), own.as_raw());
            let file = out.join(format!("{scale}x-light/captures/launch.png"));
            let scenario = pixel::read_png(&file).expect("the scenario's capture");
            let same_size = (own.width, own.height) == (scenario.width, scenario.height);
            let differing = own
                .data
                .chunks(4)
                .zip(scenario.data.chunks(4))
                .filter(|(a, b)| a != b)
                .count();
            let off = own.data.iter().zip(&scenario.data);
            let off = off.map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
            let pixels: Vec<(u32, u32)> = (0..own.data.len().min(scenario.data.len()))
                .step_by(4)
                .filter(|i| own.data[*i..i + 4] != scenario.data[*i..i + 4])
                .map(|i| (i as u32 / 4 % own.width, i as u32 / 4 / own.width))
                .collect();
            let columns = pixels.iter().map(|p| p.0);
            let rows = pixels.iter().map(|p| p.1);
            let min = columns.clone().min().zip(rows.clone().min());
            let max = columns.max().zip(rows.max());
            if off > 1 {
                let flat = |image: &Rgba| pixel::composite(image, [255, 255, 255]);
                dump(
                    &format!("own window {scale}x"),
                    &flat(&own),
                    &flat(&scenario),
                );
            }
            report.check(
                &format!("the hub's own window at {scale}x equals the scenario's capture"),
                same_size && off <= 1,
                format!("{differing} px differ, by at most {off}, in {min:?} to {max:?}"),
            );
        }
        let _ = std::fs::remove_dir_all(&out);
    }

    pub fn capture(out: &str) {
        let root = vectors::workspace_root();
        let scenario = root.join("parity/scenarios/hub-empty.json");
        let assets = root.join("reference/AdvantageScope/bundledAssets");
        let args = [
            "--parity-scenario",
            scenario.to_str().expect("path"),
            "--out",
            out,
            "--bundled-assets",
            assets.to_str().expect("path"),
            "--theme",
            "light",
        ];
        scenario::run(&args.map(str::to_owned)).unwrap_or_else(|e| panic!("{e}"));
    }

    pub fn run() {
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
                sleep(cx, 1500).await;
                if std::env::var_os("ASRS_STATES_ONLY").is_some() {
                    states(cx, &electron, &mut report).await;
                    println!("{} passed, {} failed", report.passed, report.failed.len());
                    std::process::exit(0);
                }
                captions(cx, &electron, &mut report).await;
                menus(cx, &mut report, hub).await;
                lists(cx, &mut report, hub).await;
                accelerators(cx, &mut report, hub).await;
                wheel(cx, &electron, &mut report).await;
                aspects(cx, &electron, &mut report).await;
                sizes(cx, &electron, &mut report, hub).await;
                states(cx, &electron, &mut report).await;
                drop(electron);
                own_window(cx, &mut report).await;
                println!(
                    "windows desktop: {} passed, {} failed {:?}",
                    report.passed,
                    report.failed.len(),
                    report.failed
                );
                std::process::exit(i32::from(!report.failed.is_empty()));
            })
            .detach();
        });
    }
}

fn main() {
    if asrs_parity::ci::skip_in_ci("windows desktop", "a desktop and the oracle's Electron") {
        return;
    }
    #[cfg(not(windows))]
    println!("windows desktop: skipped on this platform");
    #[cfg(windows)]
    {
        let args: Vec<String> = std::env::args().collect();
        match args.iter().position(|a| a == "--capture") {
            Some(at) => desktop::capture(&args[at + 1]),
            None => desktop::run(),
        }
    }
}
