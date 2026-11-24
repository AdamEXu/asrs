use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use asrs_js::value::JsValue;
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage, SatelliteToMain};
use asrs_model::preferences::Preferences;
use asrs_model::selection::{Selection, SelectionMode};
use asrs_model::selection_impl::SelectionImpl;
use asrs_services::download::page::alert_runs;
use asrs_services::fs::RealFs;
use asrs_services::lifecycle::{AppPaths, MainState};
use asrs_services::path::PathStyle;
use asrs_services::window_manager::Platform;
use asrs_services::windows::{Rect, WindowSystem};
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::gpui::{
    AnyWindowHandle, App, AppContext, ClipboardItem, Context, Entity, IntoElement, KeyUpEvent,
    Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement,
    PlatformInput, Render, ScrollDelta, ScrollEvent, Styled, TouchPhase, Window, WindowHandle, div,
    point, px, rgb, size,
};
use asrs_ui::parity::Recorder;
use asrs_ui::platform::dialog::{Reply, Script};
use asrs_ui::platform::{self, EditCommand};
use asrs_ui::scroll::{Style, set_style};
use asrs_ui::selection::colors::ForcedActive;
use asrs_ui::theme::ThemeSource;
use asrs_ui::tooltip::{ForcedHost, Host};

mod tooltips;

use super::capture::{self, Probes, Shot};
use super::{
    Action, BETA_FILE, Button, COLUMNS, DialogReply, DownloadShows, MouseKind, PATCH, Page,
    Scenario, Seed, Theme, Wheel, patch_colours,
};
use crate::download_flow::{self, RobotPort};
use crate::export_flow;
use crate::hub::{Hub, HubEvent};
use crate::hub_dialogs;
use crate::licenses::LicensesFile;
use crate::main_process::{BetaTime, MainProcess, process_platform};
use crate::menus::app_menu;
use crate::menus::bridge::{MenuAction, ScriptedMenu};
use crate::menus::dispatch::{dispatch, run_edit};
use crate::menus::model::{Command, MenuItem};
use crate::satellite::{Satellite, SatelliteEvent};
use crate::views::field3d::WwwRoot;
use crate::views::frame::{FrameHost, HubFrame};
use crate::views::hub_host::{HubEntityHost, boot_messages};
use crate::views::satellite_host::SatelliteHost;
use crate::windows::{self, ScriptedFrame, WindowId, WindowKind, Windows};

const USAGE: &str = "usage: asrs --parity-scenario <scenario.json> --out <dir> \
[--scale 1|2]... [--theme light|dark]... [--inputs <dir>] [--probes <probes.json>] \
[--bundled-assets <dir>] [--licenses <file>] [--work-area [<scale>:]<x>,<y>]... [--robot <port>] \
[--tooltips <reference>]

--inputs holds the logs `app:<name>` opens (<name>.wpilog, as the Electron driver writes them
under inputs/); --probes defaults to probes.json beside the scenario directory;
--bundled-assets is upstream's bundledAssets folder, which main loads with the user and
automatic folders of each variant's userData/; --licenses is the licenses.json the Licenses window
lists; --work-area is the primary display's work area origin on the Electron driver's host, which
places its window 16 px inside it (default 0,0), at one scale or at every scale;
--robot is where the scenario's robot listens on 127.0.0.1;
--tooltips writes the tooltips (<name>.tooltips.json) at each capture the reference has, as the
Electron driver does, and captures nothing else.";

/// A tick of the page: one animation frame and this much executor time, which the page's `Date`
/// and its scroll sensors' frame clock both run on.
const FRAME: Duration = Duration::from_millis(16);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// Long enough for a satellite's last state to reach `state.json`.
const SAVE_SETTLE: Duration = Duration::from_millis(1500);
const STATE_SAVE_TIMEOUT: Duration = Duration::from_secs(5);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(90);
const STABLE_ATTEMPTS: usize = 120;
/// The Electron driver's `--js-flags=--random-seed`.
const RANDOM_SEED: u64 = 1;
/// The Electron driver's `WINDOW_ORIGIN`: its window's offset into the primary work area.
const WINDOW_ORIGIN: f64 = 16.0;
/// 2026-11-01T00:00:00Z, before upstream's beta expires.
const BETA_TIME: f64 = 1_793_491_200_000.0;
/// The id `StateTracker` keys the one hub window's renderer state by.
const HUB_WINDOW_ID: f64 = 1.0;

/// What `StateTracker` reads of the screen in a run: the hub window where the Electron driver
/// places its own, and the satellites main placed around it.
#[derive(Clone)]
struct Screen {
    window: Rect,
    satellites: Vec<(WindowId, Rect)>,
}

impl Screen {
    fn with_satellites(&self, cx: &App) -> Screen {
        let manager = Windows::manager(cx);
        let ids = manager.satellites().into_iter().flat_map(|(_, list)| list);
        Screen {
            window: self.window,
            satellites: ids
                .filter_map(|id| Some((*id, manager.bounds(*id)?)))
                .collect(),
        }
    }
}

impl WindowSystem for Screen {
    fn window_bounds(&self, id: f64) -> Option<Rect> {
        if id == HUB_WINDOW_ID {
            return Some(self.window);
        }
        let mut satellites = self.satellites.iter();
        satellites
            .find(|(window, _)| *window as f64 == id)
            .map(|(_, bounds)| *bounds)
    }

    fn displays(&self) -> Vec<Rect> {
        vec![self.primary_display()]
    }

    fn primary_display(&self) -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            width: self.window.x + self.window.width + WINDOW_ORIGIN,
            height: self.window.y + self.window.height + WINDOW_ORIGIN,
        }
    }
}

/// `Some(exit code)` when the command line asks for a scenario run.
pub fn run_from_args() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|a| a == "--parity-scenario") {
        return None;
    }
    Some(match run(&args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("asrs --parity-scenario: {e}");
            1
        }
    })
}

struct Options {
    scenario: PathBuf,
    out: PathBuf,
    scales: Vec<u32>,
    themes: Vec<String>,
    inputs: Option<PathBuf>,
    probes: Option<PathBuf>,
    bundled_assets: Option<PathBuf>,
    licenses: Option<PathBuf>,
    work_areas: Vec<(Option<u32>, (f64, f64))>,
    robot: Option<u16>,
    tooltips: Option<PathBuf>,
}

fn options(args: &[String]) -> Result<Options, String> {
    let (mut scenario, mut out, mut inputs, mut probes) = (None, None, None, None);
    let (mut bundled_assets, mut licenses) = (None, None);
    let mut work_areas = Vec::new();
    let mut robot = None;
    let mut tooltips = None;
    let (mut scales, mut themes) = (Vec::new(), Vec::new());
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = || iter.next().cloned().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--parity-scenario" => scenario = Some(PathBuf::from(value()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--scale" => scales.push(value()?.parse().map_err(|_| "--scale is 1 or 2")?),
            "--theme" => themes.push(value()?),
            "--inputs" => inputs = Some(PathBuf::from(value()?)),
            "--probes" => probes = Some(PathBuf::from(value()?)),
            "--bundled-assets" => bundled_assets = Some(PathBuf::from(value()?)),
            "--licenses" => licenses = Some(PathBuf::from(value()?)),
            "--robot" => robot = Some(value()?.parse().map_err(|_| "--robot is a port")?),
            "--tooltips" => tooltips = Some(PathBuf::from(value()?)),
            "--work-area" => {
                let text = value()?;
                let (scale, at) = match text.split_once(':') {
                    Some((scale, at)) => (
                        Some(scale.parse().map_err(|_| "--work-area's scale is 1 or 2")?),
                        at,
                    ),
                    None => (None, text.as_str()),
                };
                let origin = at
                    .split_once(',')
                    .and_then(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?)))
                    .ok_or(format!("--work-area {text}: expected [<scale>:]<x>,<y>"))?;
                work_areas.push((scale, origin));
            }
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    Ok(Options {
        scenario: scenario.ok_or(USAGE)?,
        out: out.ok_or(USAGE)?,
        scales,
        themes,
        inputs,
        probes,
        bundled_assets,
        licenses,
        work_areas,
        robot,
        tooltips,
    })
}

struct ColourPatches(Vec<[u8; 3]>);

impl Render for ColourPatches {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x000000))
            .children(self.0.iter().enumerate().map(|(i, c)| {
                let i = i as u32;
                div()
                    .absolute()
                    .left(px(((i % COLUMNS) * PATCH) as f32))
                    .top(px(((i / COLUMNS) * PATCH) as f32))
                    .size(px(PATCH as f32))
                    .bg(rgb(u32::from_be_bytes([0, c[0], c[1], c[2]])))
            }))
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    let options = options(args)?;
    let name = options
        .scenario
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("scenario file has no name")?
        .to_owned();
    let text = std::fs::read_to_string(&options.scenario)
        .map_err(|e| format!("{}: {e}", options.scenario.display()))?;
    let scenario = Scenario::parse(&name, &text)?;
    let probes_file = match &options.probes {
        Some(file) => file.clone(),
        None => options
            .scenario
            .parent()
            .and_then(Path::parent)
            .map(|parity| parity.join("probes.json"))
            .ok_or("no probes.json beside the scenario directory; pass --probes")?,
    };
    let probes = Probes::load(&probes_file)?;
    let mut headless = HeadlessCapture::new(Arc::new(())).map_err(|e| e.to_string())?;
    headless.context().update(|cx| {
        windows::init(cx);
        cx.set_global(ForcedActive(false));
        cx.set_global(ForcedHost(Host::Native));
    });
    if let Some(file) = options.licenses.clone() {
        headless
            .context()
            .update(|cx| cx.set_global(LicensesFile(file)));
    }
    match options.robot {
        Some(port) => headless
            .context()
            .update(|cx| cx.set_global(RobotPort(port))),
        None if scenario.robot => return Err(format!("{name} needs --robot")),
        None => {}
    }
    let tooltips = match &options.tooltips {
        Some(file) => Some(capture_names(file)?),
        None => None,
    };
    for &scale in &scenario.scales {
        if !options.scales.is_empty() && !options.scales.contains(&scale) {
            continue;
        }
        for &theme in &scenario.themes {
            if !options.themes.is_empty() && !options.themes.iter().any(|t| t == theme.name()) {
                continue;
            }
            let dir = options.out.join(format!("{scale}x-{}", theme.name()));
            let mut variant = Variant {
                headless: &mut headless,
                scenario: &scenario,
                scenario_dir: options.scenario.parent().unwrap_or(Path::new("")),
                probes: &probes,
                inputs: options.inputs.as_deref(),
                bundled_assets: options.bundled_assets.as_deref(),
                work_area: options
                    .work_areas
                    .iter()
                    .find(|(only, _)| only.is_none_or(|only| only == scale))
                    .map_or((0.0, 0.0), |(_, origin)| *origin),
                scale,
                theme,
                dir: &dir,
                window: None,
                prompt: None,
                hub: None,
                frame: None,
                saves: Rc::default(),
                saved: Vec::new(),
                exceptions: Rc::default(),
                pressed: None,
                pointer: (0.0, 0.0),
                tooltips: tooltips.as_deref(),
            };
            variant
                .run()
                .map_err(|e| format!("{name} {scale}x {}: {e}", theme.name()))?;
        }
    }
    Ok(())
}

/// The captures of a tooltip reference.
fn capture_names(file: &Path) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let reference =
        asrs_js::json::parse_str(&text).map_err(|e| format!("{}: {e:?}", file.display()))?;
    let captures = reference.as_object().ok_or("the reference is no object")?;
    Ok(captures
        .iter()
        .map(|(name, _)| name.to_string_lossy())
        .collect())
}

/// The `save-state` messages the hub has sent main.
#[derive(Default)]
struct Saves {
    count: usize,
    last: Option<JsValue>,
}

struct Variant<'a> {
    headless: &'a mut HeadlessCapture,
    scenario: &'a Scenario,
    scenario_dir: &'a Path,
    probes: &'a Probes,
    inputs: Option<&'a Path>,
    bundled_assets: Option<&'a Path>,
    work_area: (f64, f64),
    scale: u32,
    theme: Theme,
    dir: &'a Path,
    window: Option<AnyWindowHandle>,
    /// The prompt window input and captures go to until it closes.
    prompt: Option<AnyWindowHandle>,
    hub: Option<(Entity<Hub>, crate::views::frame::FrameGeometry)>,
    frame: Option<WindowHandle<HubFrame<HubEntityHost>>>,
    saves: Rc<RefCell<Saves>>,
    /// What each satellite saves.
    saved: Vec<(AnyWindowHandle, Rc<RefCell<Saves>>)>,
    exceptions: Rc<RefCell<Vec<String>>>,
    /// The button a `mouse down` left pressed, which moves carry until `mouse up`.
    pressed: Option<MouseButton>,
    pointer: (f64, f64),
    /// The captures that record the tooltips, where none is taken.
    tooltips: Option<&'a [String]>,
}

/// `{...scenario.prefs, theme}`, the `prefs.json` the Electron driver seeds.
fn seeded_prefs(scenario: &Scenario, theme: Theme) -> Result<JsValue, String> {
    let mut seeded = scenario.prefs.clone();
    seeded
        .as_object_mut()
        .ok_or("prefs is not an object")?
        .insert("theme", JsValue::string(theme.name()));
    Ok(seeded)
}

fn number(args: &[JsValue], i: usize, method: &str) -> Result<f64, String> {
    args.get(i)
        .and_then(JsValue::as_f64)
        .ok_or_else(|| format!("selection.{method}: argument {i} is not a number"))
}

fn boolean(args: &[JsValue], i: usize, method: &str) -> Result<bool, String> {
    args.get(i)
        .and_then(JsValue::as_bool)
        .ok_or_else(|| format!("selection.{method}: argument {i} is not a boolean"))
}

/// `window.selection[method](...args)` for the methods a scenario can call.
fn call_selection(
    selection: &mut SelectionImpl,
    method: &str,
    args: &[JsValue],
) -> Result<(), String> {
    match method {
        "setSelectedTime" => selection.set_selected_time(number(args, 0, method)?),
        "setHoveredTime" => selection.set_hovered_time(match args.first() {
            None | Some(JsValue::Null) => None,
            Some(_) => Some(number(args, 0, method)?),
        }),
        "goIdle" => selection.go_idle(),
        "play" => selection.play(),
        "pause" => selection.pause(),
        "togglePlayback" => selection.toggle_playback(),
        "lock" => selection.lock(),
        "unlock" => selection.unlock(),
        "toggleLock" => selection.toggle_lock(),
        "stepCycle" => selection.step_cycle(boolean(args, 0, method)?),
        "setPlaybackSpeed" => selection.set_playback_speed(number(args, 0, method)?),
        "setPlaybackLooping" => selection.set_playback_looping(boolean(args, 0, method)?),
        "setTimelineRange" => {
            let range = args
                .first()
                .and_then(JsValue::as_array)
                .filter(|r| r.len() == 2)
                .and_then(|r| Some([r[0].as_f64()?, r[1].as_f64()?]))
                .ok_or("selection.setTimelineRange: argument 0 is not a [min, max] pair")?;
            selection.set_timeline_range(range, boolean(args, 1, method)?);
        }
        "applyTimelineScroll" => selection.apply_timeline_scroll(
            number(args, 0, method)?,
            number(args, 1, method)?,
            number(args, 2, method)?,
        ),
        other => return Err(format!("selection.{other} is not a method scenarios call")),
    }
    Ok(())
}

/// Electron's `keyCode` names for the keys GPUI names differently.
fn gpui_key(key: &str) -> String {
    match key {
        "Enter" | "Return" => "enter".into(),
        "Escape" => "escape".into(),
        "Backspace" => "backspace".into(),
        "Delete" => "delete".into(),
        "Tab" => "tab".into(),
        "Space" | " " => "space".into(),
        "Up" | "ArrowUp" => "up".into(),
        "Down" | "ArrowDown" => "down".into(),
        "Left" | "ArrowLeft" => "left".into(),
        "Right" | "ArrowRight" => "right".into(),
        "Home" => "home".into(),
        "End" => "end".into(),
        "PageUp" => "pageup".into(),
        "PageDown" => "pagedown".into(),
        other => other.to_lowercase(),
    }
}

fn modifiers(names: &[String]) -> Result<Modifiers, String> {
    let mut m = Modifiers::default();
    for name in names {
        match name.as_str() {
            "shift" => m.shift = true,
            "control" | "ctrl" => m.control = true,
            "alt" => m.alt = true,
            "meta" | "cmd" | "command" => m.platform = true,
            "commandOrControl" if cfg!(target_os = "macos") => m.platform = true,
            "commandOrControl" => m.control = true,
            other => return Err(format!("unknown modifier {other:?}")),
        }
    }
    Ok(m)
}

/// What GPUI reports for the native event Chromium turned into `wheel`: notches arrive as lines,
/// continuous (trackpad) scrolling as pixels. Chromium's notched delta is 40 px a line on macOS
/// and X11, so a scenario whose `deltaY` and `wheelTicksY` disagree describes no real event.
fn scroll_delta(wheel: &Wheel) -> Result<ScrollDelta, String> {
    if wheel.precise {
        return Ok(ScrollDelta::Pixels(point(
            px(wheel.delta_x as f32),
            px(wheel.delta_y as f32),
        )));
    }
    if cfg!(any(target_os = "macos", target_os = "linux"))
        && (wheel.delta_x != wheel.wheel_ticks_x * 40.0
            || wheel.delta_y != wheel.wheel_ticks_y * 40.0)
    {
        return Err(format!(
            "a notched wheel event's deltas are 40 px a tick here, not {:?} for {:?} ticks",
            (wheel.delta_x, wheel.delta_y),
            (wheel.wheel_ticks_x, wheel.wheel_ticks_y)
        ));
    }
    Ok(ScrollDelta::Lines(point(
        wheel.wheel_ticks_x as f32,
        wheel.wheel_ticks_y as f32,
    )))
}

/// The lines GPUI makes of `WM_MOUSEHWHEEL` and `WM_MOUSEWHEEL` at the system's default of three
/// a notch, in the order the driver sends them.
fn native_wheel_lines(units_x: f64, units_y: f64) -> Vec<asrs_ui::gpui::Point<f32>> {
    let lines = |units: f64| units as f32 / 120.0 * 3.0;
    let events = [
        (units_x, point(-lines(units_x), 0.0)),
        (units_y, point(0.0, lines(units_y))),
    ];
    let sent = events.into_iter().filter(|(units, _)| *units != 0.0);
    sent.map(|(_, lines)| lines).collect()
}

fn click_app_menu(labels: &[String], cx: &mut App) -> Result<(), String> {
    fn find(items: &[MenuItem], labels: &[String], found: &mut Vec<Command>) {
        for item in items {
            if let Some(submenu) = &item.submenu {
                find(submenu, labels, found);
            } else if item.label.as_ref().is_some_and(|l| labels.contains(l)) {
                found.extend(item.command);
            }
        }
    }
    let prefs = Preferences::default();
    let menu = app_menu::template(Platform::current(), &prefs, crate::IDENTITY);
    let mut found = Vec::new();
    find(&menu, labels, &mut found);
    let &[command] = found.as_slice() else {
        return Err(format!(
            "{} application menu items are labelled {labels:?}",
            found.len()
        ));
    };
    dispatch(
        MenuAction::Command {
            command,
            checked: false,
        },
        cx,
    );
    Ok(())
}

impl Variant<'_> {
    fn window(&self) -> AnyWindowHandle {
        self.window.expect("the window is open")
    }

    fn is_open(&mut self, window: AnyWindowHandle) -> bool {
        self.headless
            .context()
            .update_window(window, |_, _, _| ())
            .is_ok()
    }

    /// The window input goes to: the prompt while it is open, else the hub.
    fn active(&mut self) -> AnyWindowHandle {
        if let Some(prompt) = self.prompt
            && !self.is_open(prompt)
        {
            self.prompt = None;
        }
        self.prompt.unwrap_or(self.window())
    }

    /// The hub, then the prompt while it is open, then the satellites.
    fn open_windows(&mut self) -> Vec<AnyWindowHandle> {
        let mut windows = vec![self.window(), self.active()];
        windows.extend(self.satellites().iter().map(|(window, _)| *window));
        let mut seen = Vec::new();
        windows.retain(|window| {
            let new = !seen.contains(window);
            seen.push(*window);
            new
        });
        windows
    }

    fn satellites(&mut self) -> Vec<(AnyWindowHandle, Entity<Satellite>)> {
        self.headless.context().update(|cx| {
            let pages = MainProcess::satellite_windows(cx);
            let pages = pages.into_iter();
            pages
                .filter_map(|(id, page)| Some((Windows::handle(cx, id)?, page)))
                .collect()
        })
    }

    /// New satellites get the run's scale before anything is rendered in them, and their saved
    /// states are kept.
    fn adopt_satellites(&mut self) -> Result<(), String> {
        for (window, page) in self.satellites() {
            if self.saved.iter().any(|(known, _)| *known == window) {
                continue;
            }
            let saves = Rc::<RefCell<Saves>>::default();
            let kept = saves.clone();
            self.headless.context().update(|cx| {
                cx.subscribe(&page, move |_, event, _| {
                    if let SatelliteEvent::Main(message) = event
                        && message.name == SatelliteToMain::SaveState
                    {
                        let mut kept = kept.borrow_mut();
                        kept.count += 1;
                        kept.last = Some(message.data.clone());
                    }
                })
                .detach();
            });
            self.saved.push((window, saves));
            self.headless
                .set_scale_factor(window, self.scale as f32)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn switch_to_hub(&mut self) -> Result<(), String> {
        self.settle();
        let active = self.active();
        if active != self.window() && !self.satellites().iter().any(|(w, _)| *w == active) {
            return Err("the prompt is still open".to_owned());
        }
        self.prompt = None;
        self.tick()
    }

    fn switch_prompt(
        &mut self,
        page: Option<WindowKind>,
        index: usize,
        content: Option<(f64, f64)>,
    ) -> Result<(), String> {
        self.settle();
        self.adopt_satellites()?;
        self.prompt = match page {
            Some(kind) => {
                let window = self
                    .headless
                    .context()
                    .update(|cx| Windows::of_kind(cx, kind, index))
                    .ok_or_else(|| format!("no {kind:?} prompt is open"))?;
                if matches!(kind, WindowKind::Licenses | WindowKind::SourceListHelp) {
                    self.leave_title_bar(window)?;
                }
                if let Some((width, height)) = content {
                    self.headless
                        .context()
                        .update_window(window, |_, window, _| {
                            window.resize(size(px(width as f32), px(height as f32)));
                        })
                        .map_err(|e| e.to_string())?;
                }
                self.headless
                    .set_scale_factor(window, self.scale as f32)
                    .map_err(|e| e.to_string())?;
                Some(window)
            }
            None => {
                let start = Instant::now();
                while self.active() != self.window() {
                    if start.elapsed() > IDLE_TIMEOUT {
                        return Err("the prompt is still open".to_owned());
                    }
                    std::thread::sleep(Duration::from_millis(1));
                    self.tick()?;
                }
                None
            }
        };
        self.tick()
    }

    /// Electron sizes windows without `useContentSize` by their frame, and a headless window has
    /// none to take the title bar from.
    fn leave_title_bar(&mut self, window: AnyWindowHandle) -> Result<(), String> {
        self.headless
            .context()
            .update_window(window, |_, window, _| {
                let frame = window.viewport_size();
                let side = |v| f64::from(f32::from(v));
                let (width, height) =
                    platform::titled_content_size(side(frame.width), side(frame.height));
                window.resize(size(px(width as f32), px(height as f32)));
            })
            .map_err(|e| e.to_string())
    }

    fn settle(&mut self) {
        self.headless.context().run_until_parked();
    }

    /// One animation frame, `FRAME` of executor time, and everything that then runs.
    fn tick(&mut self) -> Result<(), String> {
        self.adopt_satellites()?;
        self.headless.context().advance_clock(FRAME);
        for window in self.open_windows() {
            self.headless
                .context()
                .update_window(window, |_, window, cx| window.simulate_next_frame(cx))
                .map_err(|e| e.to_string())?;
        }
        self.settle();
        Ok(())
    }

    /// The download service runs on its own thread, so this waits in real time.
    fn wait_download(&mut self, shows: &DownloadShows) -> Result<(), String> {
        let view = self
            .headless
            .context()
            .update(|cx| download_flow::view(cx))
            .ok_or("the download window is not open")?;
        let start = Instant::now();
        loop {
            self.tick()?;
            let shown = self.headless.context().update(|cx| {
                let page = view.read(cx).page();
                let alert = page.alert().map(|html| {
                    let runs = alert_runs(html).into_iter().map(|run| run.text);
                    runs.collect::<String>()
                });
                let progress = page.progress();
                match shows {
                    DownloadShows::Listed => {
                        !page.loading() && alert.is_none() && progress.is_none()
                    }
                    DownloadShows::Text(text) => alert
                        .as_deref()
                        .into_iter()
                        .chain(progress.map(|(_, details)| details))
                        .any(|shown| shown.starts_with(text)),
                    DownloadShows::Progress(value) => {
                        progress.is_some_and(|(shown, _)| shown == *value)
                    }
                }
            });
            if shown {
                return Ok(());
            }
            if start.elapsed() > DOWNLOAD_TIMEOUT {
                return Err(format!(
                    "the download window did not show {shows:?} within {} s",
                    DOWNLOAD_TIMEOUT.as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn with_hub<R>(
        &mut self,
        f: impl FnOnce(&mut Hub, &crate::views::frame::FrameGeometry, &mut Context<'_, Hub>) -> R,
    ) -> Result<R, String> {
        let (hub, geometry) = self.hub.clone().ok_or("the colour-patch page has no hub")?;
        let result = self
            .headless
            .context()
            .update(|cx| hub.update(cx, |hub, cx| f(hub, &geometry, cx)));
        self.settle();
        Ok(result)
    }

    fn main_message(&mut self, name: MainToHub, data: JsValue) -> Result<(), String> {
        self.with_hub(|hub, geometry, cx| {
            geometry.sync(hub.model());
            hub.handle_main_message(NamedMessage::new(name, data), cx);
        })
    }

    /// Upstream's `busyReasons()` read from the model, plus the decode the port runs on threads.
    fn busy(&mut self) -> Result<Vec<&'static str>, String> {
        if self.hub.is_none() {
            return Ok(Vec::new());
        }
        self.with_hub(|hub, _, _| {
            let model = hub.model();
            let mut reasons = Vec::new();
            if !model.loading_fields().is_empty() {
                reasons.push("fields loading");
            }
            if model.loading().show == Some(true) {
                reasons.push("log loading");
            }
            if model.title().to_string_lossy().contains("(Loading)") {
                reasons.push("title loading");
            }
            if matches!(
                model.page().selection().get_mode(),
                SelectionMode::Playback | SelectionMode::Locked
            ) {
                reasons.push("playback or live");
            }
            reasons
        })
        .and_then(|mut reasons| {
            if let Some(frame) = self.frame {
                let busy = self
                    .headless
                    .context()
                    .update(|cx| frame.read_with(cx, |frame, _| frame.field3d_busy()))
                    .map_err(|e| e.to_string())?;
                let mut busy = busy;
                for (window, _) in self.satellites() {
                    if let Some(frame) = window.downcast::<HubFrame<SatelliteHost>>() {
                        busy |= self
                            .headless
                            .context()
                            .update(|cx| frame.read_with(cx, |frame, _| frame.field3d_busy()))
                            .map_err(|e| e.to_string())?;
                    }
                }
                if busy {
                    reasons.push("3D loading or rendering");
                }
            }
            Ok(reasons)
        })
    }

    fn wait_idle(&mut self) -> Result<(), String> {
        let start = Instant::now();
        let mut quiet = 0;
        loop {
            self.tick()?;
            let busy = self.busy()?;
            quiet = if busy.is_empty() { quiet + 1 } else { 0 };
            if quiet >= 2 {
                break;
            }
            if start.elapsed() > IDLE_TIMEOUT {
                return Err(format!(
                    "timed out after {} s waiting for the page to go idle (busy: {})",
                    IDLE_TIMEOUT.as_secs(),
                    busy.join(", ")
                ));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        self.tick()?;
        self.tick()
    }

    fn resolve_log(&self, spec: &str) -> Result<String, String> {
        let Some(generated) = spec.strip_prefix("app:") else {
            return std::path::absolute(spec)
                .map(|p| p.to_string_lossy().into_owned())
                .map_err(|e| format!("{spec}: {e}"));
        };
        let inputs = self.inputs.ok_or_else(|| {
            format!("{spec} is a generated log; pass --inputs with the driver's inputs directory")
        })?;
        let file = inputs.join(format!("{generated}.wpilog"));
        if !file.is_file() {
            return Err(format!("{spec}: no {}", file.display()));
        }
        std::path::absolute(&file)
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(|e| format!("{}: {e}", file.display()))
    }

    /// `{"$clip": name}` is the folder of a clip the Electron driver generated, as `imgFolder`.
    fn resolve_clips(&self, value: &JsValue) -> Result<JsValue, String> {
        match value {
            JsValue::Array(items) => {
                let items = items.iter().map(|v| self.resolve_clips(v));
                items.collect::<Result<_, _>>().map(JsValue::Array)
            }
            JsValue::Object(object) => {
                if let Some(clip) = value.get("$clip").and_then(JsValue::as_str) {
                    let clip = clip.to_string_lossy();
                    let inputs = self.inputs.ok_or_else(|| {
                        format!("{clip} is a generated clip; pass --inputs with the driver's inputs directory")
                    })?;
                    let folder = std::path::absolute(inputs.join(&clip))
                        .map_err(|e| format!("{clip}: {e}"))?;
                    if !folder.is_dir() {
                        return Err(format!("no clip {}", folder.display()));
                    }
                    let folder = format!("{}{}", folder.display(), std::path::MAIN_SEPARATOR);
                    return Ok(JsValue::string(&folder));
                }
                let mut resolved = asrs_js::value::JsObject::new();
                for (key, v) in object.iter() {
                    resolved.insert(key.clone(), self.resolve_clips(v)?);
                }
                Ok(JsValue::Object(resolved))
            }
            other => Ok(other.clone()),
        }
    }

    fn dispatch(&mut self, input: PlatformInput) -> Result<(), String> {
        let window = self.active();
        self.headless
            .context()
            .update_window(window, |_, window, cx| {
                window.dispatch_event(input, cx);
            })
            .map_err(|e| e.to_string())?;
        self.settle();
        Ok(())
    }

    fn mouse(
        &mut self,
        kind: MouseKind,
        x: f64,
        y: f64,
        button: Button,
        click_count: usize,
        names: &[String],
    ) -> Result<(), String> {
        self.pointer = (x, y);
        let modifiers = modifiers(names)?;
        let position = point(px(x as f32), px(y as f32));
        let button = match button {
            Button::Left => MouseButton::Left,
            Button::Right => MouseButton::Right,
            Button::Middle => MouseButton::Middle,
        };
        let down = PlatformInput::MouseDown(MouseDownEvent {
            button,
            position,
            modifiers,
            click_count,
            first_mouse: false,
        });
        let up = PlatformInput::MouseUp(MouseUpEvent {
            button,
            position,
            modifiers,
            click_count,
        });
        match kind {
            MouseKind::Move => self.dispatch(PlatformInput::MouseMove(MouseMoveEvent {
                position,
                pressed_button: self.pressed,
                modifiers,
            }))?,
            MouseKind::Down => {
                self.dispatch(down)?;
                self.pressed = Some(button);
            }
            MouseKind::Up => {
                self.dispatch(up)?;
                self.pressed = None;
            }
            MouseKind::Click => {
                self.dispatch(down)?;
                self.dispatch(up)?;
            }
        }
        Ok(())
    }

    fn key(&mut self, key: &str, names: &[String]) -> Result<(), String> {
        let modifiers = modifiers(names)?;
        let key_char = (key.chars().count() == 1).then(|| {
            if modifiers.shift {
                key.to_uppercase()
            } else {
                key.to_owned()
            }
        });
        let keystroke = Keystroke {
            modifiers,
            key: gpui_key(key),
            key_char,
        };
        let window = self.active();
        let down = keystroke.clone();
        self.headless
            .context()
            .update_window(window, |_, window, cx| window.dispatch_keystroke(down, cx))
            .map_err(|e| e.to_string())?;
        self.settle();
        if self.is_open(window) {
            self.dispatch(PlatformInput::KeyUp(KeyUpEvent { keystroke }))?;
        }
        Ok(())
    }

    fn edit(&mut self, command: EditCommand) -> Result<(), String> {
        let window = self.active();
        self.headless
            .context()
            .update_window(window, |_, window, cx| run_edit(command, window, cx))
            .map_err(|e| e.to_string())?;
        self.settle();
        self.tick()?;
        self.tick()
    }

    fn perform(&mut self, action: &Action) -> Result<(), String> {
        match action {
            Action::Main { name, data } => {
                let data = self.resolve_clips(data)?;
                self.main_message(*name, data)?
            }
            Action::Open { files, merge } => {
                let files = files
                    .iter()
                    .map(|f| self.resolve_log(f).map(|p| JsValue::string(&p)))
                    .collect::<Result<_, _>>()?;
                self.main_message(
                    MainToHub::OpenFiles,
                    asrs_js::ops::object([
                        ("files", JsValue::Array(files)),
                        ("merge", JsValue::Bool(*merge)),
                    ]),
                )?;
            }
            Action::Selection { method, args } => {
                self.with_hub(|hub, _, cx| {
                    hub.update(cx, |model, _| {
                        call_selection(model.page_mut().selection_mut(), method, args)
                    })
                })??;
            }
            Action::Wait { ms } => {
                let deadline = Duration::from_secs_f64(ms / 1000.0);
                let mut waited = Duration::ZERO;
                while waited < deadline {
                    self.tick()?;
                    waited += FRAME;
                }
            }
            Action::Idle => self.wait_idle()?,
            Action::Mouse {
                kind,
                x,
                y,
                button,
                click_count,
                modifiers,
            } => self.mouse(*kind, *x, *y, *button, *click_count, modifiers)?,
            Action::Wheel(wheel) => {
                let delta = scroll_delta(wheel)?;
                self.dispatch(PlatformInput::Scroll(ScrollEvent {
                    position: point(px(wheel.x as f32), px(wheel.y as f32)),
                    delta,
                    modifiers: Modifiers::default(),
                    touch_phase: TouchPhase::Moved,
                    ..Default::default()
                }))?;
            }
            Action::NativeWheel {
                x,
                y,
                units_x,
                units_y,
            } => {
                for delta in native_wheel_lines(*units_x, *units_y) {
                    self.dispatch(PlatformInput::Scroll(ScrollEvent {
                        position: point(px(*x as f32), px(*y as f32)),
                        delta: ScrollDelta::Lines(delta),
                        modifiers: Modifiers::default(),
                        touch_phase: TouchPhase::Moved,
                        ..Default::default()
                    }))?;
                }
                self.tick()?;
            }
            Action::Key { key, modifiers } => self.key(key, modifiers)?,
            Action::Dialog(reply) => {
                let reply = match reply {
                    DialogReply::Button(index) => Reply::Button(*index),
                    DialogReply::Cancel => Reply::Save(None),
                    DialogReply::File(name) => {
                        let file = std::path::absolute(self.dir.join("files/dialog").join(name))
                            .map_err(|e| e.to_string())?;
                        let folder = if name.ends_with('/') {
                            Some(file.as_path())
                        } else {
                            file.parent()
                        };
                        if let Some(folder) = folder {
                            std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
                        }
                        Reply::Save(Some(file))
                    }
                };
                self.headless.context().update(|cx| {
                    cx.default_global::<Script>().replies.push_back(reply);
                });
            }
            Action::Menu { path } => {
                self.headless
                    .context()
                    .update(|cx| ScriptedMenu::click(cx, path))?;
                self.settle();
            }
            Action::AppMenu { labels } => {
                self.headless
                    .context()
                    .update(|cx| click_app_menu(labels, cx))?;
                self.settle();
            }
            Action::Prompt { page, index, size } => self.switch_prompt(*page, *index, *size)?,
            Action::Hub => self.switch_to_hub()?,
            Action::Download(shows) => self.wait_download(shows)?,
            Action::SelectAll => self.edit(EditCommand::SelectAll)?,
            Action::Copy(name) => {
                self.headless.context().update(|cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
                });
                self.edit(EditCommand::Copy)?;
                let copied = self.headless.context().update(|cx| {
                    let item = cx.read_from_clipboard();
                    item.and_then(|item| item.text()).unwrap_or_default()
                });
                let folder = self.dir.join("files/clipboard");
                std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
                std::fs::write(folder.join(format!("{name}.txt")), copied)
                    .map_err(|e| e.to_string())?;
            }
            Action::Focus(active) => {
                let active = ForcedActive(*active);
                self.headless.context().update(|cx| cx.set_global(active));
                self.tick()?;
            }
            Action::Capture {
                name,
                probes,
                masks,
            } => {
                if let Some(names) = self.tooltips {
                    if names.contains(name) {
                        self.capture_tooltips(name)?;
                    }
                } else {
                    self.capture(name, probes, masks)?;
                }
            }
        }
        if matches!(
            action,
            Action::Mouse { .. } | Action::Wheel(_) | Action::Key { .. }
        ) {
            self.tick()?;
            self.tick()?;
        }
        Ok(())
    }

    /// Main's startup in the variant's `userData/`: the Electron driver's seeded files, then module load and `whenReady` up to the windows. Returns the `set-assets` and
    /// `set-preferences` data.
    fn start_main(&mut self) -> Result<(JsValue, JsValue), String> {
        let user_data = self.dir.join("userData");
        std::fs::create_dir_all(&user_data).map_err(|e| format!("{}: {e}", user_data.display()))?;
        for (file, bytes) in self.seeds()? {
            let file = user_data.join(file);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            std::fs::write(&file, bytes).map_err(|e| format!("{}: {e}", file.display()))?;
        }
        let absolute = |p: &Path| {
            std::path::absolute(p)
                .map(|p| p.to_string_lossy().into_owned())
                .map_err(|e| e.to_string())
        };
        let bundled = match self.bundled_assets {
            Some(dir) => absolute(dir)?,
            None => absolute(&self.dir.join("bundledAssets"))?,
        };
        let paths = AppPaths {
            style: PathStyle::host(),
            user_data: absolute(&user_data)?,
            temp: absolute(&self.dir.join("temp"))?,
            bundled_assets: bundled,
            packaged: false,
        };
        let (main, _) = MainState::start(&RealFs, &self.screen(), paths, process_platform())
            .map_err(|e| format!("main startup: {e:?}"))?;
        let assets = main.assets.set_assets_data();
        let preferences = main
            .preferences(&RealFs)
            .map_err(|e| format!("prefs.json: {e:?}"))?;
        self.headless
            .context()
            .update(|cx| cx.set_global(MainProcess::headless(main, RANDOM_SEED)));
        Ok((assets, preferences))
    }

    fn screen(&self) -> Screen {
        Screen {
            window: Rect {
                x: self.work_area.0 + WINDOW_ORIGIN,
                y: self.work_area.1 + WINDOW_ORIGIN,
                width: self.scenario.width,
                height: self.scenario.height,
            },
            satellites: Vec::new(),
        }
    }

    /// What the Electron driver seeds in userData.
    fn seeds(&self) -> Result<Vec<(String, Vec<u8>)>, String> {
        let json = |file: &str, value: &JsValue| {
            asrs_js::json::stringify(value)
                .map_err(|e| format!("{file}: {e:?}"))
                .map(|text| text.unwrap_or_default().into_bytes())
        };
        let prefs = seeded_prefs(self.scenario, self.theme)?;
        let mut seeds = vec![
            ("prefs.json".to_owned(), json("prefs", &prefs)?),
            (BETA_FILE.to_owned(), json(BETA_FILE, &self.scenario.beta)?),
        ];
        for (file, seed) in &self.scenario.user_assets {
            let bytes = match seed {
                Seed::Json(value) => json(file, value)?,
                Seed::File(path) => std::fs::read(self.scenario_dir.join(path))
                    .map_err(|e| format!("{file}: {path}: {e}"))?,
            };
            seeds.push((file.clone(), bytes));
        }
        Ok(seeds)
    }

    fn open(&mut self) -> Result<(), String> {
        let theme = self.theme;
        let classic = self.scenario.classic_scrollbars;
        self.headless.context().update(|cx| {
            set_style(Style::of(classic), false, cx);
            cx.set_global(match theme {
                Theme::Light => ThemeSource::Light,
                Theme::Dark => ThemeSource::Dark,
            });
        });
        let content = size(
            px(self.scenario.width as f32),
            px(self.scenario.height as f32),
        );
        let scale = self.scale as f32;
        match self.scenario.page {
            Page::App => {
                // `windows::open` makes the hub window transparent for the vibrancy on macOS and
                // the acrylic on Windows.
                self.headless
                    .set_transparent_windows(cfg!(target_os = "macos") || platform::has_acrylic());
                let (assets, preferences) = self.start_main()?;
                if let Some(root) = self.bundled_assets.and_then(Path::parent) {
                    let root = std::path::absolute(root).map_err(|e| e.to_string())?;
                    self.headless
                        .context()
                        .update(|cx| cx.set_global(WwwRoot(root)));
                }
                let boot = boot_messages(assets, true, preferences, Vec::new());
                let window = self
                    .headless
                    .open_window(content, scale, |window, cx| {
                        HubEntityHost::open_with(
                            window,
                            theme == Theme::Dark,
                            boot,
                            Some(RANDOM_SEED),
                            cx,
                        )
                    })
                    .map_err(|e| e.to_string())?;
                self.headless.set_transparent_windows(false);
                let (hub, geometry) = self
                    .headless
                    .context()
                    .update(|cx| {
                        window.update(cx, |frame, _, _| {
                            (frame.host().hub().clone(), frame.host().geometry())
                        })
                    })
                    .map_err(|e| e.to_string())?;
                let (saves, exceptions) = (self.saves.clone(), self.exceptions.clone());
                let handle = window.into();
                let screen = self.screen();
                self.headless.context().update(|cx| {
                    let id = Windows::adopt_hub(cx, window.into(), screen.window);
                    cx.set_global(ScriptedFrame {
                        size: platform::probe::framed_window().unwrap_or((0.0, 0.0)),
                    });
                    cx.global_mut::<MainProcess>()
                        .adopt_hub(id, hub.downgrade());
                    cx.set_global(ScriptedMenu::default());
                    cx.set_global(Script::default());
                    cx.subscribe(&hub, move |_, event, cx| match event {
                        HubEvent::SatelliteUpdate(_) => {
                            MainProcess::satellite_request(cx, id, event)
                        }
                        HubEvent::Main(m) => {
                            MainProcess::satellite_request(cx, id, event);
                            hub_dialogs::show(m, handle, cx);
                            let screen = screen.with_satellites(cx);
                            let main = cx.global_mut::<MainProcess>().state_mut();
                            let saved =
                                main.hub_message(&RealFs, HUB_WINDOW_ID, m)
                                    .and_then(|()| match m.name {
                                        HubToMain::SaveState => main.tracker.tick(&RealFs, &screen),
                                        _ => Ok(()),
                                    });
                            if let Err(e) = saved {
                                exceptions
                                    .borrow_mut()
                                    .push(format!("main {}: {e:?}", m.name.name()));
                            }
                            export_flow::hub_message(id, m, cx);
                            if m.name == HubToMain::SaveState {
                                let mut saves = saves.borrow_mut();
                                saves.count += 1;
                                saves.last = Some(m.data.clone());
                            }
                        }
                        HubEvent::Exception { task, error } => {
                            exceptions.borrow_mut().push(format!("{task}: {error}"));
                        }
                        _ => {}
                    })
                    .detach();
                    cx.set_global(BetaTime(BETA_TIME));
                    MainProcess::beta_init(cx, id);
                });
                self.window = Some(window.into());
                self.hub = Some((hub, geometry));
                self.frame = Some(window);
            }
            Page::ColourPatch => {
                self.headless.set_transparent_windows(false);
                let window = self
                    .headless
                    .open_window(content, scale, |_, cx| {
                        cx.new(|_| ColourPatches(patch_colours()))
                    })
                    .map_err(|e| e.to_string())?;
                self.window = Some(window.into());
            }
        }
        self.settle();
        self.tick()
    }

    fn run(&mut self) -> Result<(), String> {
        let captures = self.dir.join("captures");
        std::fs::create_dir_all(&captures).map_err(|e| format!("{}: {e}", captures.display()))?;
        self.open()?;
        for (i, action) in self.scenario.actions.iter().enumerate() {
            self.perform(action)
                .map_err(|e| format!("action {i} ({}): {e}", capture::kind(action)))?;
        }
        if !self.satellites().is_empty() {
            let mut waited = Duration::ZERO;
            while waited < SAVE_SETTLE {
                self.tick()?;
                waited += FRAME;
            }
        }
        for window in self.open_windows() {
            self.headless
                .context()
                .update_window(window, |_, window, _| window.remove_window())
                .map_err(|e| e.to_string())?;
        }
        self.settle();
        let exceptions = self.exceptions.borrow().clone();
        let dialogs: Vec<JsValue> = self.headless.context().update(|cx| {
            let script = cx.try_global::<Script>();
            let shown = script.map_or(&[][..], |s| &s.shown);
            shown
                .iter()
                .map(|(_, s)| capture::dialog_record(s))
                .collect()
        });
        capture::write(
            &self.dir.join("run.json"),
            &capture::run_record(
                self.scenario,
                self.scale,
                self.theme,
                &exceptions,
                &capture::collect_files(self.dir, &self.seeds()?)?,
                &dialogs,
            ),
        )
    }

    /// Waits for the next `save-state` (every 250 ms) of the satellite input goes to, or of the
    /// hub, and returns its data.
    fn fresh_state(&mut self) -> Result<JsValue, String> {
        let active = self.active();
        let mut satellites = self.saved.iter();
        let saves = satellites
            .find(|(window, _)| *window == active)
            .map_or_else(|| self.saves.clone(), |(_, saves)| saves.clone());
        let before = saves.borrow().count;
        let mut waited = Duration::ZERO;
        while saves.borrow().count == before {
            if waited > STATE_SAVE_TIMEOUT {
                return Err("no save-state within 5 s of the capture".into());
            }
            self.tick()?;
            waited += FRAME;
        }
        let last = saves.borrow().last.clone();
        Ok(last.unwrap_or_default())
    }

    fn shoot(&mut self) -> Result<Shot, String> {
        let window = self.active();
        self.headless
            .context()
            .update(|cx| cx.set_global(Recorder::default()));
        let image = self.headless.capture(window).map_err(|e| e.to_string())?;
        let recorder = self
            .headless
            .context()
            .update(|cx| cx.remove_global::<Recorder>());
        let viewport = self
            .headless
            .context()
            .update_window(window, |_, window, _| window.viewport_size())
            .map_err(|e| e.to_string())?;
        Ok(Shot {
            image,
            recorder,
            viewport: (
                f64::from(f32::from(viewport.width)),
                f64::from(f32::from(viewport.height)),
            ),
        })
    }

    fn capture(&mut self, name: &str, probes: &[String], masks: &[String]) -> Result<(), String> {
        if self.scenario.page == Page::App {
            self.wait_idle()?;
        }
        let mut shot = self.shoot()?;
        let mut stable = false;
        for _ in 0..STABLE_ATTEMPTS {
            self.tick()?;
            let next = self.shoot()?;
            let rects = next.masks(self.probes, masks)?;
            let same = next.same_as(&shot, &rects, f64::from(self.scale));
            shot = next;
            if same {
                stable = true;
                break;
            }
        }
        if !stable {
            return Err(format!(
                "capture {name}: the frame still changed after {STABLE_ATTEMPTS} frames"
            ));
        }
        let (scenario, scale) = (self.scenario, self.scale);
        let content = if self.active() == self.window() {
            (scenario.width, scenario.height)
        } else {
            shot.viewport
        };
        let expected = (
            (content.0 * f64::from(scale)) as u32,
            (content.1 * f64::from(scale)) as u32,
        );
        if (shot.image.width, shot.image.height) != expected || shot.viewport != content {
            return Err(format!(
                "capture {name}: content is {}x{} px ({:?} logical), expected {}x{}",
                shot.image.width, shot.image.height, shot.viewport, expected.0, expected.1
            ));
        }
        let (state, timeline) = match self.hub {
            Some(_) => {
                let state = self.fresh_state()?;
                let timeline = self.with_hub(|hub, _, cx| {
                    hub.update(cx, |model, _| {
                        model.page_mut().selection_mut().get_timeline_range()
                    })
                })?;
                (Some(state), Some(timeline))
            }
            None => (None, None),
        };
        capture::write_capture(
            &self.dir.join("captures"),
            name,
            capture::Context {
                scenario,
                scale,
                theme: self.theme,
                probes: self.probes,
            },
            &shot,
            probes,
            masks,
            state.as_ref(),
            timeline,
        )
    }
}
