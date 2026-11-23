use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use asrs_js::value::JsValue;
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage};
use asrs_model::selection::{Selection, SelectionMode};
use asrs_model::selection_impl::SelectionImpl;
use asrs_services::fs::RealFs;
use asrs_services::lifecycle::{AppPaths, MainState};
use asrs_services::path::PathStyle;
use asrs_services::windows::{Rect, WindowSystem};
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::gpui::{
    AnyWindowHandle, App, AppContext, Context, Entity, IntoElement, KeyUpEvent, Keystroke,
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement,
    PlatformInput, Render, ScrollDelta, ScrollEvent, Styled, TouchPhase, Window, WindowHandle, div,
    point, px, rgb, size,
};
use asrs_ui::parity::Recorder;
use asrs_ui::platform::dialog::{Reply, Script};
use asrs_ui::theme::ThemeSource;

use super::capture::{self, Probes, Shot};
use super::{
    Action, Button, COLUMNS, DialogReply, MouseKind, PATCH, Page, Scenario, Theme, Wheel,
    patch_colours,
};
use crate::hub::{Hub, HubEvent};
use crate::hub_dialogs;
use crate::main_process::{MainProcess, process_platform};
use crate::menus::bridge::ScriptedMenu;
use crate::views::field3d::WwwRoot;
use crate::views::frame::{FrameHost, HubFrame};
use crate::views::hub_host::{HubEntityHost, boot_messages};
use crate::views::prompts;
use crate::windows::{self, WindowId, WindowKind, Windows};

const USAGE: &str = "usage: asrs --parity-scenario <scenario.json> --out <dir> \
[--scale 1|2]... [--theme light|dark]... [--inputs <dir>] [--probes <probes.json>] \
[--bundled-assets <dir>] [--work-area <x>,<y>]

--inputs holds the logs `app:<name>` opens (<name>.wpilog, as the Electron driver writes them
under inputs/); --probes defaults to probes.json beside the scenario directory;
--bundled-assets is upstream's bundledAssets folder, which main loads with the user and
automatic folders of each variant's userData/; --work-area is the primary display's work area
origin on the Electron driver's host, which places its window 16 px inside it (default 0,0).";

/// A tick of the page: one animation frame and this much executor time, which the page's `Date`
/// and its scroll sensors' frame clock both run on.
const FRAME: Duration = Duration::from_millis(16);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const STATE_SAVE_TIMEOUT: Duration = Duration::from_secs(5);
const STABLE_ATTEMPTS: usize = 120;
/// The Electron driver's `--js-flags=--random-seed`.
const RANDOM_SEED: u64 = 1;
/// The Electron driver's `WINDOW_ORIGIN`: its window's offset into the primary work area.
const WINDOW_ORIGIN: f64 = 16.0;
/// The id `StateTracker` keys the one hub window's renderer state by.
const HUB_WINDOW_ID: f64 = 1.0;

/// What `StateTracker` reads of the screen in a run: the hub window where the Electron driver
/// places its own.
struct Screen {
    window: Rect,
}

impl WindowSystem for Screen {
    fn window_bounds(&self, id: f64) -> Option<Rect> {
        (id == HUB_WINDOW_ID).then_some(self.window)
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
    work_area: (f64, f64),
}

fn options(args: &[String]) -> Result<Options, String> {
    let (mut scenario, mut out, mut inputs, mut probes) = (None, None, None, None);
    let mut bundled_assets = None;
    let mut work_area = (0.0, 0.0);
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
            "--work-area" => {
                let text = value()?;
                work_area = text
                    .split_once(',')
                    .and_then(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?)))
                    .ok_or(format!("--work-area {text}: expected <x>,<y>"))?;
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
        work_area,
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
    headless.context().update(windows::init);
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
                probes: &probes,
                inputs: options.inputs.as_deref(),
                bundled_assets: options.bundled_assets.as_deref(),
                work_area: options.work_area,
                scale,
                theme,
                dir: &dir,
                window: None,
                prompt: None,
                hub: None,
                frame: None,
                saves: Rc::default(),
                exceptions: Rc::default(),
                pressed: None,
            };
            variant
                .run()
                .map_err(|e| format!("{name} {scale}x {}: {e}", theme.name()))?;
        }
    }
    Ok(())
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
    exceptions: Rc<RefCell<Vec<String>>>,
    /// The button a `mouse down` left pressed, which moves carry until `mouse up`.
    pressed: Option<MouseButton>,
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

/// `prompt-export` up to the window: confirming it is P08's export flow.
fn open_export(id: WindowId, hub: &Entity<Hub>, data: &JsValue, cx: &mut App) {
    let supports_akit = data.get("supportsAkit").and_then(JsValue::as_bool) == Some(true);
    let hub = hub.downgrade();
    let opened = prompts::open_export(cx, id, supports_akit, move |value, prompt, cx| {
        if value.is_null() {
            windows::close(cx, prompt);
            if let Some(hub) = hub.upgrade() {
                let message = NamedMessage::new(MainToHub::CancelExport, JsValue::Undefined);
                hub.update(cx, |hub, cx| hub.handle_main_message(message, cx));
            }
        }
    });
    if let Err(e) = opened {
        eprintln!("asrs: opening the export prompt: {e}");
    }
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

    /// The hub, then the prompt while it is open.
    fn open_windows(&mut self) -> Vec<AnyWindowHandle> {
        let mut windows = vec![self.window(), self.active()];
        windows.dedup();
        windows
    }

    fn switch_prompt(&mut self, page: Option<WindowKind>) -> Result<(), String> {
        self.settle();
        self.prompt = match page {
            Some(kind) => {
                let window = self
                    .headless
                    .context()
                    .update(|cx| Windows::of_kind(cx, kind))
                    .ok_or_else(|| format!("no {kind:?} prompt is open"))?;
                self.headless
                    .set_scale_factor(window, self.scale as f32)
                    .map_err(|e| e.to_string())?;
                Some(window)
            }
            None if self.active() != self.window() => {
                return Err("the prompt is still open".to_owned());
            }
            None => None,
        };
        self.tick()
    }

    fn settle(&mut self) {
        self.headless.context().run_until_parked();
    }

    /// One animation frame, `FRAME` of executor time, and everything that then runs.
    fn tick(&mut self) -> Result<(), String> {
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
    ) -> Result<(), String> {
        let position = point(px(x as f32), px(y as f32));
        let button = match button {
            Button::Left => MouseButton::Left,
            Button::Right => MouseButton::Right,
            Button::Middle => MouseButton::Middle,
        };
        let down = PlatformInput::MouseDown(MouseDownEvent {
            button,
            position,
            modifiers: Modifiers::default(),
            click_count,
            first_mouse: false,
        });
        let up = PlatformInput::MouseUp(MouseUpEvent {
            button,
            position,
            modifiers: Modifiers::default(),
            click_count,
        });
        match kind {
            MouseKind::Move => self.dispatch(PlatformInput::MouseMove(MouseMoveEvent {
                position,
                pressed_button: self.pressed,
                modifiers: Modifiers::default(),
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

    fn perform(&mut self, action: &Action) -> Result<(), String> {
        match action {
            Action::Main { name, data } => self.main_message(*name, data.clone())?,
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
            } => self.mouse(*kind, *x, *y, *button, *click_count)?,
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
            Action::Key { key, modifiers } => self.key(key, modifiers)?,
            Action::Dialog(reply) => {
                let reply = match reply {
                    DialogReply::Button(index) => Reply::Button(*index),
                    DialogReply::Cancel => Reply::Save(None),
                    DialogReply::File(name) => {
                        let file = std::path::absolute(self.dir.join("files/dialog").join(name))
                            .map_err(|e| e.to_string())?;
                        if let Some(parent) = file.parent() {
                            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
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
            Action::Prompt { page } => self.switch_prompt(*page)?,
            Action::Capture {
                name,
                probes,
                masks,
            } => self.capture(name, probes, masks)?,
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

    /// Main's startup in the variant's `userData/`: the Electron driver's seeded `prefs.json`,
    /// then module load and `whenReady` up to the windows. Returns the `set-assets` and
    /// `set-preferences` data.
    fn start_main(&mut self) -> Result<(JsValue, JsValue), String> {
        let user_data = self.dir.join("userData");
        std::fs::create_dir_all(&user_data).map_err(|e| format!("{}: {e}", user_data.display()))?;
        capture::write(&user_data.join("prefs.json"), &self.prefs_seed()?)?;
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
            .update(|cx| cx.set_global(MainProcess::headless(main)));
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
        }
    }

    fn prefs_seed(&self) -> Result<String, String> {
        asrs_js::json::stringify(&seeded_prefs(self.scenario, self.theme)?)
            .map_err(|e| format!("prefs: {e:?}"))
            .map(Option::unwrap_or_default)
    }

    fn open(&mut self) -> Result<(), String> {
        let theme = self.theme;
        self.headless.context().update(|cx| {
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
                // `windows::open` makes the hub window transparent on macOS, for the vibrancy.
                self.headless
                    .set_transparent_windows(cfg!(target_os = "macos"));
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
                    let id = Windows::adopt_hub(cx, window.into());
                    cx.set_global(ScriptedMenu::default());
                    cx.set_global(Script::default());
                    cx.subscribe(&hub, move |hub, event, cx| match event {
                        HubEvent::Main(m) => {
                            hub_dialogs::show(m, handle, cx);
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
                            if m.name == HubToMain::PromptExport {
                                open_export(id, &hub, &m.data, cx);
                            }
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
                &capture::collect_files(
                    self.dir,
                    &[("prefs.json", self.prefs_seed()?.into_bytes())],
                )?,
                &dialogs,
            ),
        )
    }

    /// Waits for the hub's next `save-state` (every 250 ms) and returns its data.
    fn fresh_state(&mut self) -> Result<JsValue, String> {
        let before = self.saves.borrow().count;
        let mut waited = Duration::ZERO;
        while self.saves.borrow().count == before {
            if waited > STATE_SAVE_TIMEOUT {
                return Err("no save-state within 5 s of the capture".into());
            }
            self.tick()?;
            waited += FRAME;
        }
        Ok(self.saves.borrow().last.clone().unwrap_or_default())
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
            let same = next.same_as(&shot);
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
