//! Live checks of `asrs_ui::platform` on real windows, side by side with Electron when
//! `--electron <binary>` is given. Moves the cursor and posts mouse and wheel events through the
//! HID event tap, so leave the machine alone while it runs. Prints one line per check and exits
//! non-zero if any fails.

#![allow(clippy::disallowed_methods)]

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write as _};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use asrs_ui::platform::probe::{self, MouseAction, WheelSample, WindowImage};
use asrs_ui::platform::{self, PopupEntry, PopupMenu, WheelDelta};
use gpui_kit::{
    AnyWindowHandle, App, AppContext, AsyncApp, Bounds, Context, InteractiveElement, IntoElement,
    ParentElement, Render, ScrollDelta, Styled, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, div, point, px, rgb, size,
};

const PATCHES: [[u8; 3]; 4] = [[255, 0, 0], [0, 71, 206], [0, 255, 0], [128, 128, 128]];
const PATCH: f64 = 60.0;
const SIDEBAR: f64 = 120.0;
const HUB: (f64, f64, f64, f64) = (80.0, 120.0, 520.0, 220.0);
const ASPECT: (f64, f64, f64, f64) = (700.0, 120.0, 400.0, 300.0);
const ELECTRON_OFFSET: f64 = 300.0;
const RATIO: f64 = 16.0 / 9.0;
const DRAGS: [(f64, f64); 3] = [(150.0, 40.0), (-60.0, 90.0), (-400.0, -400.0)];

fn patch_origin(i: usize) -> (f64, f64) {
    (SIDEBAR + 20.0 + i as f64 * 80.0, 60.0)
}

type WheelLog = Rc<RefCell<Vec<(ScrollDelta, WheelDelta)>>>;

struct Probe {
    wheel: WheelLog,
    patches: bool,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        let wheel = self.wheel.clone();
        let mut root = div().size_full().relative().on_scroll(move |event, _, _| {
            wheel
                .borrow_mut()
                .push((event.delta, platform::wheel_delta(event)));
        });
        if !self.patches {
            return root.bg(rgb(0xffffff));
        }
        root = root.child(
            div()
                .absolute()
                .left(px(SIDEBAR as f32))
                .top_0()
                .bottom_0()
                .right_0()
                .bg(rgb(0xffffff)),
        );
        for (i, [r, g, b]) in PATCHES.into_iter().enumerate() {
            let (x, y) = patch_origin(i);
            root = root.child(
                div()
                    .absolute()
                    .left(px(x as f32))
                    .top(px(y as f32))
                    .size(px(PATCH as f32))
                    .bg(rgb(u32::from_be_bytes([0, r, g, b]))),
            );
        }
        root
    }
}

#[derive(Default)]
struct Report {
    failed: bool,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: impl std::fmt::Display) {
        println!("{} {name}: {detail}", if ok { "PASS" } else { "FAIL" });
        self.failed |= !ok;
    }
}

fn bounds((x, y, w, h): (f64, f64, f64, f64)) -> Option<WindowBounds> {
    Some(WindowBounds::Windowed(Bounds::new(
        point(px(x as f32), px(y as f32)),
        size(px(w as f32), px(h as f32)),
    )))
}

fn main() {
    let electron = std::env::args().skip_while(|a| a != "--electron").nth(1);
    gpui_kit::application().run(move |cx: &mut App| {
        asrs_ui::init(cx);
        let wheel = WheelLog::default();
        let options = WindowOptions {
            window_bounds: bounds(HUB),
            titlebar: Some(TitlebarOptions {
                title: Some("platform probe".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(13.0), px(13.0))),
            }),
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        };
        let probe_wheel = wheel.clone();
        let (hub, _) = asrs_ui::open_window(options, cx, |window, cx| {
            platform::install_sidebar_vibrancy(window);
            cx.new(|_| Probe {
                wheel: probe_wheel,
                patches: true,
            })
        })
        .expect("open probe window");
        cx.activate(true);
        cx.spawn(async move |cx| {
            let mut report = Report::default();
            let (cursor_x, cursor_y) = probe::cursor_location();
            checks(cx, hub, wheel, electron, &mut report).await;
            probe::warp_cursor(cursor_x, cursor_y);
            println!(
                "{}",
                if report.failed {
                    "FAILED"
                } else {
                    "ALL PASSED"
                }
            );
            std::process::exit(i32::from(report.failed));
        })
        .detach();
    });
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

fn open_plain(cx: &mut AsyncApp, options: WindowOptions) -> AnyWindowHandle {
    cx.update(|cx| {
        asrs_ui::open_window(options, cx, |_, cx| {
            cx.new(|_| Probe {
                wheel: WheelLog::default(),
                patches: false,
            })
        })
    })
    .map(|(handle, _)| handle)
    .expect("open window")
}

async fn checks(
    cx: &mut AsyncApp,
    hub: AnyWindowHandle,
    wheel: WheelLog,
    electron: Option<String>,
    report: &mut Report,
) {
    sleep(cx, 600).await;
    with_window(cx, hub, |w| {
        w.resize(size(px(HUB.2 as f32), px(HUB.3 as f32)))
    });
    sleep(cx, 300).await;

    let vibrancy = with_window(cx, hub, |w| probe::has_sidebar_vibrancy(w));
    report.check(
        "vibrancy",
        vibrancy,
        "NSVisualEffectView Sidebar/BehindWindow/FollowsWindowActiveState is the bottom subview",
    );

    floating(cx, report).await;
    popup(cx, hub, report).await;

    let electron = electron.and_then(|path| match Electron::start(&path) {
        Ok(e) => Some(e),
        Err(e) => {
            report.check("electron", false, format!("could not start {path}: {e}"));
            None
        }
    });
    let ready = match &electron {
        Some(e) => e.ready(cx).await,
        None => None,
    };
    if electron.is_some() && ready.is_none() {
        report.check("electron", false, "no ready line within 10 s");
    }
    let electron = electron.as_ref();
    srgb(cx, hub, electron.zip(ready.as_ref()), report).await;
    wheel_events(cx, hub, &wheel, electron.zip(ready.as_ref()), report).await;
    aspect(cx, electron.zip(ready.as_ref()), report).await;
}

async fn floating(cx: &mut AsyncApp, report: &mut Report) {
    let handle = open_plain(
        cx,
        WindowOptions {
            window_bounds: bounds((700.0, 500.0, 200.0, 100.0)),
            kind: WindowKind::Floating,
            ..Default::default()
        },
    );
    sleep(cx, 300).await;
    let before = with_window(cx, handle, |w| probe::hides_on_deactivate(w));
    with_window(cx, handle, |w| platform::keep_visible_when_inactive(w));
    let after = with_window(cx, handle, |w| probe::hides_on_deactivate(w));
    report.check(
        "floating stays visible when inactive",
        before == Some(true) && after == Some(false),
        format!("hidesOnDeactivate {before:?} -> {after:?}"),
    );
    with_window(cx, handle, |w| w.remove_window());
}

async fn popup(cx: &mut AsyncApp, hub: AnyWindowHandle, report: &mut Report) {
    let menu = PopupMenu::new()
        .entry(PopupEntry::new("Copy \"Pose\" to clipboard"))
        .entry(PopupEntry::new("Loop Visible Range").checked(true))
        .entry(PopupEntry::new("(No Unit Metadata)").enabled(false))
        .separator()
        .entry(PopupEntry::new("Blue").swatch([0x2b, 0x66, 0xa2]))
        .entry(
            PopupEntry::new("Driver Station")
                .submenu(PopupMenu::new().entry(PopupEntry::new("Blue 1"))),
        );
    for (x, y) in [(123, 77), (300, 150), (0, 0)] {
        let closed = Rc::new(RefCell::new(None));
        let closed_in = closed.clone();
        cx.update(|cx| {
            hub.update(cx, |_, window, cx| {
                platform::popup_menu(window, cx, menu.clone(), x, y, move |selection, _| {
                    *closed_in.borrow_mut() = Some(selection);
                });
            })
        })
        .expect("hub window");
        sleep(cx, 700).await;
        let frames = probe::menu_window_frames();
        let window = with_window(cx, hub, |w| probe::window_frame(w)).expect("frame");
        with_window(cx, hub, |w| platform::close_popup_menu(w));
        sleep(cx, 400).await;
        let selection = closed.borrow().clone();
        let menu_frame = frames.first().copied();
        let ok = frames.len() == 1
            && menu_frame.is_some_and(|m| m.x == window.x + f64::from(x))
            && selection == Some(None);
        let top = menu_frame.map(|m| window.y + window.height - (m.y + m.height));
        report.check(
            &format!("popup at ({x}, {y})"),
            ok,
            format!(
                "menu x {:?} = window x {} + {x}; menu top {top:?} below window top; frames {frames:?}; closed with {selection:?}",
                menu_frame.map(|m| m.x),
                window.x
            ),
        );
    }
}

fn patch_colors(image: &WindowImage, top: f64) -> Vec<[u8; 4]> {
    let scale = f64::from(image.width) / HUB.2;
    (0..PATCHES.len())
        .map(|i| {
            let (x, y) = patch_origin(i);
            image.pixel(
                ((x + PATCH / 2.0) * scale) as u32,
                ((top + y + PATCH / 2.0) * scale) as u32,
            )
        })
        .collect()
}

async fn srgb(
    cx: &mut AsyncApp,
    hub: AnyWindowHandle,
    electron: Option<(&Electron, &Ready)>,
    report: &mut Report,
) {
    let tagged = with_window(cx, hub, |w| probe::is_tagged_srgb(w));
    report.check("sRGB tag", tagged == Some(true), format!("{tagged:?}"));
    if !probe::can_capture_screen() {
        println!("SKIP sRGB patches: no screen capture access");
        return;
    }
    let capture = |cx: &mut AsyncApp| {
        with_window(cx, hub, |w| probe::window_number(w))
            .and_then(probe::capture_window)
            .map(|image| patch_colors(&image, 0.0))
    };
    let ours = capture(cx);
    with_window(cx, hub, |w| probe::untag_colorspace(w));
    with_window(cx, hub, |w| w.refresh());
    sleep(cx, 300).await;
    let raw = capture(cx);
    with_window(cx, hub, |w| platform::tag_srgb_colorspace(w));
    with_window(cx, hub, |w| w.refresh());
    sleep(cx, 300).await;

    let Some((_, ready)) = electron else {
        println!(
            "SKIP sRGB patches against Electron: no --electron; ours {ours:?}, untagged {raw:?}"
        );
        return;
    };
    let capture_electron = || {
        probe::capture_window(ready.hub_window).map(|image| {
            let scale = f64::from(image.width) / HUB.2;
            let title_bar = f64::from(image.height) / scale - ready.hub_content.3;
            patch_colors(&image, title_bar)
        })
    };
    let theirs = capture_electron();
    let max_delta = ours.as_ref().zip(theirs.as_ref()).map(|(a, b)| {
        a.iter()
            .flatten()
            .zip(b.iter().flatten())
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0)
    });
    report.check(
        "sRGB patches on screen match Electron's",
        max_delta.is_some_and(|d| d <= 1),
        format!(
            "sources {PATCHES:?}; ours {ours:?}; Electron {theirs:?}; max channel delta {max_delta:?}; ours untagged {raw:?}"
        ),
    );
}

fn expected_chromium(sample: &WheelSample) -> f32 {
    if sample.continuous {
        sample.point_delta[1] as f32
    } else {
        sample.fixed_delta[1] as f32 * 40.0
    }
}

async fn wheel_events(
    cx: &mut AsyncApp,
    hub: AnyWindowHandle,
    wheel: &WheelLog,
    electron: Option<(&Electron, &Ready)>,
    report: &mut Report,
) {
    if !probe::can_post_events() {
        println!("SKIP wheel: no event posting access");
        return;
    }
    let samples = probe::wheel_gesture_samples();
    let expected: Vec<f32> = samples.iter().map(expected_chromium).collect();
    wheel.borrow_mut().clear();
    for sample in &samples {
        with_window(cx, hub, |w| probe::post_wheel(w, sample));
        sleep(cx, 30).await;
    }
    sleep(cx, 300).await;
    let got = wheel.borrow().clone();
    let ours: Vec<f32> = got.iter().map(|(_, d)| d.y).collect();
    let gpui: Vec<String> = got.iter().map(|(d, _)| format!("{d:?}")).collect();
    report.check(
        "wheel deltas through the window server",
        ours == expected,
        format!("ours {ours:?}, Chromium's formula {expected:?}; GPUI {gpui:?}"),
    );

    let Some((electron, ready)) = electron else {
        return;
    };
    electron.drain();
    let (x, y, w, h) = ready.hub_content;
    for sample in &samples {
        probe::post_wheel_at(x + w / 2.0, y + h / 2.0, sample);
        sleep(cx, 30).await;
    }
    sleep(cx, 600).await;
    let dom: Vec<f32> = electron
        .drain()
        .iter()
        .filter_map(|line| line.strip_prefix("wheel "))
        .filter_map(|dy| dy.trim().parse::<f32>().ok())
        .map(|dy| -dy)
        .collect();
    let nonzero = |v: &[f32]| v.iter().copied().filter(|d| *d != 0.0).collect::<Vec<_>>();
    report.check(
        "Electron wheel deltas equal ours",
        !dom.is_empty() && nonzero(&dom) == nonzero(&ours),
        format!("Electron -deltaY {dom:?}; ours {ours:?}"),
    );
}

async fn drag(cx: &mut AsyncApp, from: (f64, f64), by: (f64, f64)) {
    probe::warp_cursor(from.0, from.1);
    sleep(cx, 100).await;
    probe::post_mouse(MouseAction::Down, from.0, from.1);
    sleep(cx, 100).await;
    for step in 1..=12 {
        let t = f64::from(step) / 12.0;
        probe::post_mouse(MouseAction::Drag, from.0 + by.0 * t, from.1 + by.1 * t);
        sleep(cx, 20).await;
    }
    probe::post_mouse(MouseAction::Up, from.0 + by.0, from.1 + by.1);
    sleep(cx, 500).await;
}

async fn aspect(cx: &mut AsyncApp, electron: Option<(&Electron, &Ready)>, report: &mut Report) {
    if !probe::can_post_events() {
        println!("SKIP aspect: no event posting access");
        return;
    }
    let handle = open_plain(
        cx,
        WindowOptions {
            window_bounds: bounds(ASPECT),
            window_min_size: Some(size(px(200.0), px(100.0))),
            ..Default::default()
        },
    );
    sleep(cx, 400).await;
    with_window(cx, handle, |w| {
        w.resize(size(px(ASPECT.2 as f32), px(ASPECT.3 as f32)))
    });
    with_window(cx, handle, |w| platform::set_aspect_ratio(w, RATIO));
    sleep(cx, 200).await;
    let screen_height = probe::screen_height();
    let mut ours = Vec::new();
    for (dx, dy) in DRAGS {
        let frame = with_window(cx, handle, |w| probe::window_frame(w)).expect("frame");
        let corner = (frame.x + frame.width - 3.0, screen_height - frame.y - 3.0);
        drag(cx, corner, (dx, dy)).await;
        ours.push(with_window(cx, handle, |w| probe::content_size(w)).expect("size"));
    }
    with_window(cx, handle, |w| platform::set_aspect_ratio(w, 0.0));
    let frame = with_window(cx, handle, |w| probe::window_frame(w)).expect("frame");
    let before = with_window(cx, handle, |w| probe::content_size(w)).expect("size");
    let corner = (frame.x + frame.width - 3.0, screen_height - frame.y - 3.0);
    drag(cx, corner, (90.0, 10.0)).await;
    let unlocked = with_window(cx, handle, |w| probe::content_size(w)).expect("size");
    with_window(cx, handle, |w| w.remove_window());

    let ratios: Vec<String> = ours
        .iter()
        .map(|(w, h)| format!("{w}x{h} ({:.4})", w / h))
        .collect();
    let locked = ours[..2].iter().all(|(w, h)| (w / h - RATIO).abs() < 0.01);
    let free = unlocked.0 > before.0 && (unlocked.0 / unlocked.1 - RATIO).abs() > 0.05;
    report.check(
        "aspect lock holds during live resize",
        locked && free,
        format!("locked {ratios:?}; after unlocking {before:?} -> {unlocked:?}"),
    );

    let Some((electron, _)) = electron else {
        return;
    };
    let mut theirs = Vec::new();
    for (dx, dy) in DRAGS {
        let Some(frame) = electron.query(cx, "aspect").await else {
            break;
        };
        let corner = (frame.0 + frame.2 - 3.0, frame.1 + frame.3 - 3.0);
        drag(cx, corner, (dx, dy)).await;
        if let Some(content) = electron.query(cx, "aspect-content").await {
            theirs.push((content.2, content.3));
        }
    }
    report.check(
        "aspect live resize equals Electron's",
        theirs == ours,
        format!("ours {ours:?}; Electron {theirs:?}"),
    );
}

const ELECTRON_MAIN: &str = r#"
const { app, BrowserWindow } = require("electron");
const readline = require("readline");
const out = (s) => process.stdout.write(s + "\n");
const rect = (b) => [b.x, b.y, b.width, b.height].join(" ");
app.whenReady().then(async () => {
  const [hx, hy, hw, hh] = JSON.parse(process.env.PROBE_HUB);
  const hub = new BrowserWindow({ x: hx, y: hy, width: hw, height: hh });
  hub.webContents.on("console-message", (e, level, message) => {
    const text = typeof message === "string" ? message : e.message;
    if (text.startsWith("wheel ")) out(text);
  });
  await hub.loadURL("data:text/html," + encodeURIComponent(process.env.PROBE_HTML));
  hub.focus();
  out("ready " + hub.getMediaSourceId().split(":")[1] + " " + rect(hub.getContentBounds()));
  let aspect = null;
  const openAspect = async () => {
    if (aspect) return;
    const [ax, ay, aw, ah] = JSON.parse(process.env.PROBE_ASPECT);
    aspect = new BrowserWindow({ x: ax, y: ay, width: aw, height: ah, useContentSize: true, minWidth: 200, minHeight: 100, resizable: true });
    await aspect.loadURL("data:text/html,<body style='background:#fff'></body>");
    aspect.setAspectRatio(Number(process.env.PROBE_RATIO));
  };
  readline.createInterface({ input: process.stdin }).on("line", async (line) => {
    if (line === "aspect") { await openAspect(); out("aspect " + rect(aspect.getBounds())); }
    else if (line === "aspect-content") out("aspect-content " + rect(aspect.getContentBounds()));
    else app.quit();
  });
});
"#;

fn electron_html() -> String {
    let mut html = String::from(
        "<html><body style=\"margin:0;width:100vw;height:100vh;background:#fff;overflow:hidden\">",
    );
    for (i, [r, g, b]) in PATCHES.into_iter().enumerate() {
        let (x, y) = patch_origin(i);
        html.push_str(&format!(
            "<div style=\"position:absolute;left:{x}px;top:{y}px;width:{PATCH}px;height:{PATCH}px;background:rgb({r},{g},{b})\"></div>"
        ));
    }
    html.push_str(
        "<script>addEventListener('wheel', e => console.log('wheel ' + e.deltaY), {passive: true});</script></body></html>",
    );
    html
}

struct Ready {
    hub_window: u32,
    hub_content: (f64, f64, f64, f64),
}

struct Electron {
    child: RefCell<Child>,
    lines: mpsc::Receiver<String>,
}

fn rect_of(text: &str) -> Option<(f64, f64, f64, f64)> {
    let v: Vec<f64> = text
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    (v.len() == 4).then(|| (v[0], v[1], v[2], v[3]))
}

impl Electron {
    fn start(binary: &str) -> std::io::Result<Self> {
        let dir = std::env::temp_dir().join(format!("asrs-platform-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("main.js"), ELECTRON_MAIN)?;
        std::fs::write(
            dir.join("package.json"),
            r#"{"name":"asrs-platform-probe","main":"main.js"}"#,
        )?;
        let shifted =
            |(x, y, w, h): (f64, f64, f64, f64)| format!("[{x},{},{w},{h}]", y + ELECTRON_OFFSET);
        let mut child = Command::new(binary)
            .arg(&dir)
            .env("PROBE_HTML", electron_html())
            .env("PROBE_HUB", shifted(HUB))
            .env("PROBE_ASPECT", shifted(ASPECT))
            .env("PROBE_RATIO", RATIO.to_string())
            .env_remove("ELECTRON_RUN_AS_NODE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child: RefCell::new(child),
            lines,
        })
    }

    fn drain(&self) -> Vec<String> {
        self.lines.try_iter().collect()
    }

    async fn ready(&self, cx: &mut AsyncApp) -> Option<Ready> {
        for _ in 0..100 {
            sleep(cx, 100).await;
            if let Some(line) = self.drain().iter().find_map(|l| l.strip_prefix("ready ")) {
                let (id, rest) = line.split_once(' ')?;
                sleep(cx, 800).await;
                return Some(Ready {
                    hub_window: id.parse().ok()?,
                    hub_content: rect_of(rest)?,
                });
            }
        }
        None
    }

    async fn query(&self, cx: &mut AsyncApp, what: &str) -> Option<(f64, f64, f64, f64)> {
        self.drain();
        writeln!(self.child.borrow_mut().stdin.as_mut()?, "{what}").ok()?;
        let prefix = format!("{what} ");
        for _ in 0..50 {
            sleep(cx, 20).await;
            if let Some(rect) = self
                .drain()
                .iter()
                .find_map(|l| l.strip_prefix(&prefix).and_then(rect_of))
            {
                return Some(rect);
            }
        }
        None
    }
}

impl Drop for Electron {
    fn drop(&mut self) {
        let child = self.child.get_mut();
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = writeln!(stdin, "quit");
        }
        std::thread::sleep(Duration::from_millis(300));
        let _ = child.kill();
        let _ = child.wait();
    }
}
