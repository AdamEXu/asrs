//! `main.ts`'s process around its hub windows: `whenReady`, the launch argument and `open-file`
//! events, the 5 s asset reload with `set-assets`, the 250 ms state save, the AdvantageKit path
//! poll, `window-all-closed`, `activate` and the quit cleanup.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use asrs_js::ops;
use asrs_js::string::JsError;
use asrs_js::time::{Clock, NamedZone, SystemClock};
use asrs_js::value::{JsValue, ToJs};
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage};
use asrs_model::preferences::{Preferences, Theme};
use asrs_services::asset_downloader;
use asrs_services::constants::{AKIT_PATH_INPUT_PERIOD_MS, layout_default_path};
use asrs_services::fs::RealFs;
use asrs_services::lifecycle::{self, AppPaths, HubRestore, MainState, StartupWindow};
use asrs_services::path::PathStyle;
use asrs_services::update::ReleaseProfile;
use asrs_services::window_manager::Platform;
use asrs_services::windows::WindowSystem;
use asrs_ui::gpui::{App, Entity, Global, WeakEntity};
use asrs_ui::theme::{Appearance, ThemeSource};

use crate::hub::{Hub, HubEvent};
use crate::menus::app_menu;
use crate::menus::bridge;
use crate::menus::click::ClickEnv;
use crate::menus::dispatch;
use crate::menus::model::MenuItem;
use crate::views::hub_host::{HubEntityHost, boot_messages};
use crate::views::prompts;
use crate::windows::{self, Rect, WindowId, Windows};

/// `app.isPackaged`: set by `cargo xtask bundle`.
pub const PACKAGED: bool = option_env!("ASRS_PACKAGED").is_some();

/// `isBeta()`: upstream's version is an alpha, and the beta flow is off in release builds.
const IS_BETA: bool = cfg!(feature = "parity");

const ASSET_RELOAD_PERIOD: Duration = Duration::from_millis(5000);
const STATE_SAVE_PERIOD: Duration = Duration::from_millis(250);

/// Upstream's repository in `parity` builds, asrs's otherwise.
pub const RELEASE_PROFILE: ReleaseProfile = if cfg!(feature = "parity") {
    ReleaseProfile::PARITY
} else {
    ReleaseProfile::RELEASE
};

pub struct MainProcess {
    state: MainState,
    /// `menuTemplate`.
    menu: Vec<MenuItem>,
    hubs: Vec<(WindowId, WeakEntity<Hub>)>,
}

impl Global for MainProcess {}

/// `process.platform`.
pub fn process_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

/// `process.arch`.
pub fn process_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    }
}

/// `path.join(__dirname, "..", "bundledAssets")`: beside the executable in a packaged build,
/// the workspace's copy of upstream's folder otherwise.
pub fn bundled_assets_dir() -> PathBuf {
    if PACKAGED {
        let exe = std::env::current_exe().unwrap_or_default();
        return exe.parent().unwrap_or(Path::new(".")).join("bundledAssets");
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let candidates = [
        root.join("assets/bundledAssets"),
        root.join("reference/AdvantageScope/bundledAssets"),
    ];
    let found = candidates.iter().find(|dir| dir.is_dir());
    let dir = found.unwrap_or(&candidates[0]);
    let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.clone());
    PathBuf::from(
        PathStyle::host()
            .normalize(&dir.to_string_lossy().as_ref().into())
            .to_string_lossy(),
    )
}

/// The host's `app.getPath` values, with the user-data directory created as Chromium does at
/// launch.
pub fn host_paths() -> Result<AppPaths, String> {
    let user_data = lifecycle::user_data_dir(Platform::current(), |name| std::env::var(name).ok())
        .ok_or("no home directory to put the user-data directory in")?;
    std::fs::create_dir_all(&user_data).map_err(|e| format!("{user_data}: {e}"))?;
    Ok(AppPaths {
        style: PathStyle::host(),
        user_data,
        temp: lifecycle::temp_dir(),
        bundled_assets: bundled_assets_dir().to_string_lossy().into_owned(),
        packaged: PACKAGED,
    })
}

fn rect(bounds: asrs_ui::gpui::Bounds<asrs_ui::gpui::Pixels>) -> Rect {
    let f = |p: asrs_ui::gpui::Pixels| f64::from(f32::from(p));
    Rect {
        x: f(bounds.origin.x),
        y: f(bounds.origin.y),
        width: f(bounds.size.width),
        height: f(bounds.size.height),
    }
}

/// The screens and window bounds `StateTracker` reads, as they are now.
struct Screens {
    windows: Vec<(WindowId, Rect)>,
    displays: Vec<Rect>,
    primary: Rect,
}

impl Screens {
    fn now(cx: &App) -> Screens {
        let manager = Windows::manager(cx);
        let windows = manager
            .hubs()
            .iter()
            .filter_map(|id| Some((*id, manager.bounds(*id)?)))
            .collect();
        let displays: Vec<Rect> = cx.displays().iter().map(|d| rect(d.bounds())).collect();
        let primary = cx
            .primary_display()
            .map(|d| rect(d.bounds()))
            .or_else(|| displays.first().copied())
            .unwrap_or(Rect {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            });
        Screens {
            windows,
            displays: if displays.is_empty() {
                vec![primary]
            } else {
                displays
            },
            primary,
        }
    }
}

impl WindowSystem for Screens {
    fn window_bounds(&self, id: f64) -> Option<Rect> {
        self.windows
            .iter()
            .find(|(w, _)| *w as f64 == id)
            .map(|(_, r)| *r)
    }

    fn displays(&self) -> Vec<Rect> {
        self.displays.clone()
    }

    fn primary_display(&self) -> Rect {
        self.primary
    }
}

fn theme_source(theme: Theme) -> ThemeSource {
    match theme {
        Theme::Light => ThemeSource::Light,
        Theme::Dark => ThemeSource::Dark,
        Theme::System => ThemeSource::System,
    }
}

/// A saved `WindowState`'s bounds, which `createHubWindow` passes to the window.
fn saved_bounds(state: &JsValue) -> Option<Rect> {
    let n = |key: &str| state.get(key).and_then(JsValue::as_f64);
    Some(Rect {
        x: n("x")?,
        y: n("y")?,
        width: n("width")?,
        height: n("height")?,
    })
}

/// `file:` URLs from the platform's open event, as paths.
fn file_paths(urls: Vec<String>) -> Vec<String> {
    urls.into_iter()
        .filter_map(|url| {
            let rest = url.strip_prefix("file://")?;
            let bytes = rest.as_bytes();
            let mut out = Vec::with_capacity(bytes.len());
            let mut i = 0;
            while i < bytes.len() {
                let hex = |b: u8| (b as char).to_digit(16);
                if bytes[i] == b'%'
                    && let (Some(hi), Some(lo)) = (
                        bytes.get(i + 1).copied().and_then(hex),
                        bytes.get(i + 2).copied().and_then(hex),
                    )
                {
                    out.push((hi * 16 + lo) as u8);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            Some(String::from_utf8_lossy(&out).into_owned())
        })
        .collect()
}

impl MainProcess {
    /// Main in a parity scenario, which opens its hub window itself.
    #[cfg(feature = "parity")]
    pub(crate) fn headless(state: MainState) -> MainProcess {
        MainProcess {
            state,
            menu: Vec::new(),
            hubs: Vec::new(),
        }
    }

    #[cfg(feature = "parity")]
    pub(crate) fn state_mut(&mut self) -> &mut MainState {
        &mut self.state
    }

    fn send(cx: &mut App, hub: &WeakEntity<Hub>, name: MainToHub, data: JsValue) {
        if let Some(hub) = hub.upgrade() {
            hub.update(cx, |hub, cx| {
                hub.handle_main_message(NamedMessage::new(name, data), cx)
            });
        }
    }

    /// `hubWindows`, most recently focused first.
    pub fn hub_windows(cx: &App) -> Vec<(WindowId, WeakEntity<Hub>)> {
        let Some(main) = cx.try_global::<MainProcess>() else {
            return Vec::new();
        };
        Windows::manager(cx)
            .hubs()
            .iter()
            .filter_map(|id| main.hubs.iter().find(|(w, _)| w == id).cloned())
            .collect()
    }

    /// `ask-edit-axis`'s `recent-units.json`; an error is the handler's throw.
    pub fn recent_units(cx: &App) -> Result<Option<JsValue>, String> {
        match cx.try_global::<MainProcess>() {
            Some(main) => main
                .state
                .recent_units(&RealFs)
                .map_err(|e| format!("{e:?}")),
            None => Ok(None),
        }
    }

    /// `updateRecents(preset)`.
    pub fn update_recent_units(cx: &App, preset: &JsValue) {
        if let Some(main) = cx.try_global::<MainProcess>()
            && let Err(e) = main.state.update_recent_units(&RealFs, preset)
        {
            eprintln!("asrs: recent-units.json: {e:?}");
        }
    }

    fn hub_message(cx: &mut App, id: WindowId, message: &NamedMessage<HubToMain>) {
        if !cx.has_global::<MainProcess>() {
            return;
        }
        let handled = cx
            .global_mut::<MainProcess>()
            .state
            .hub_message(&RealFs, id as f64, message);
        match handled {
            Err(e) => eprintln!("asrs: {}: {e:?}", message.name.name()),
            Ok(()) if message.name == HubToMain::UpdatePreferences => {
                Self::send_all_preferences(cx);
            }
            Ok(()) => {}
        }
    }

    /// `sendMessage(window, name, data)` to a hub window.
    pub fn send_to_hub(cx: &mut App, window: WindowId, name: MainToHub, data: JsValue) {
        let hub = Self::hub_windows(cx)
            .into_iter()
            .find(|(id, _)| *id == window);
        if let Some((_, hub)) = hub {
            Self::send(cx, &hub, name, data);
        }
    }

    /// `sendAllPreferences()`: the theme, every hub window, and the application menu.
    pub fn send_all_preferences(cx: &mut App) {
        let Some(main) = cx.try_global::<MainProcess>() else {
            return;
        };
        let data = match main.state.preferences(&RealFs) {
            Ok(data) => data,
            Err(e) => {
                eprintln!("asrs: prefs.json: {e:?}");
                return;
            }
        };
        let prefs = Preferences::from_js(&data);
        cx.set_global(theme_source(prefs.theme));
        for (_, hub) in Self::hub_windows(cx) {
            Self::send(cx, &hub, MainToHub::SetPreferences, data.clone());
        }
        let menu = &mut cx.global_mut::<MainProcess>().menu;
        if !menu.is_empty() {
            app_menu::apply_preferences(menu, &prefs);
            Self::install_menu(cx);
        }
    }

    /// `openPreferences(parent)`; an error is `dom-ready`'s throw on `prefs.json`, after which the
    /// window shows its page's defaults.
    pub fn open_preferences(cx: &mut App, parent: WindowId) -> Result<(), JsError> {
        let (id, view) = match prompts::open_preferences(cx, parent, Self::write_preferences) {
            Ok(Some(opened)) => opened,
            Ok(None) => return Ok(()),
            Err(e) => {
                eprintln!("asrs: opening the preferences window: {e}");
                return Ok(());
            }
        };
        let Some(main) = cx.try_global::<MainProcess>() else {
            return Ok(());
        };
        let message = ops::object([
            ("platform", JsValue::string(process_platform())),
            ("prefs", main.state.preferences(&RealFs)?),
        ]);
        if let Err(e) = prompts::post(cx, id, &view, &message) {
            eprintln!("asrs: the preferences window: {e}");
        }
        Ok(())
    }

    /// `jsonfile.writeFileSync(PREFS_FILENAME, event.data); sendAllPreferences();`
    fn write_preferences(prefs: JsValue, cx: &mut App) {
        let Some(main) = cx.try_global::<MainProcess>() else {
            return;
        };
        match main.state.write_preferences(&RealFs, &prefs) {
            Ok(()) => Self::send_all_preferences(cx),
            Err(e) => eprintln!("asrs: prefs.json: {e:?}"),
        }
    }

    /// `menuTemplate`, as the last `sendAllPreferences` left it.
    pub fn menu(cx: &App) -> &[MenuItem] {
        &cx.global::<MainProcess>().menu
    }

    fn install_menu(cx: &mut App) {
        let menu = cx.global::<MainProcess>().menu.clone();
        bridge::install_application_menu(
            cx,
            Platform::current(),
            &menu,
            Rc::new(dispatch::dispatch),
        );
    }

    /// `loadAssets()`, then `sendAllPreferences()` and `sendAssets()`.
    pub fn reload_assets(cx: &mut App) {
        if Self::load_assets(cx) {
            Self::send_all_preferences(cx);
            Self::send_assets(cx);
        }
    }

    /// Runs `f` with what the application menu's click handlers read.
    pub fn with_click_env<R>(cx: &App, f: impl FnOnce(&ClickEnv<'_>) -> R) -> Option<R> {
        let main = cx.try_global::<MainProcess>()?;
        let application_state = || {
            main.state
                .tracker
                .current_application_state(&Screens::now(cx))
        };
        Some(f(&ClickEnv {
            platform: Platform::current(),
            arch: process_arch(),
            identity: crate::IDENTITY,
            profile: RELEASE_PROFILE,
            fs: &RealFs,
            paths: &main.state.paths,
            windows: Windows::manager(cx),
            asset_status: asset_downloader::INITIAL_STATUS,
            layout_default_path: layout_default_path(SystemClock.now(), &NamedZone::system()),
            application_state: &application_state,
        }))
    }

    /// `createHubWindow(state)` with its `dom-ready` sends; `files` are opened after them.
    pub fn open_hub(cx: &mut App, saved: Option<&JsValue>, files: Vec<String>) -> Option<WindowId> {
        Self::send_all_preferences(cx);
        let main = cx.try_global::<MainProcess>()?;
        let preferences = main.state.preferences(&RealFs).unwrap_or_else(|e| {
            eprintln!("asrs: prefs.json: {e:?}");
            Preferences::default().to_js()
        });
        let mut boot = boot_messages(
            main.state.assets.set_assets_data(),
            IS_BETA,
            preferences,
            Vec::new(),
        );
        let restore = HubRestore {
            first_load: true,
            layout_override: None,
            saved,
            cached: JsValue::Undefined,
        };
        let restored = lifecycle::dom_ready_restores(
            &RealFs,
            &main.state.paths.type_memory(),
            &restore,
            |name, data| boot.push(NamedMessage::new(name, data)),
        );
        if let Err(e) = restored {
            eprintln!("asrs: dom-ready: {e:?}");
        }
        if !files.is_empty() {
            boot.push(open_files_message(files));
        }
        let opened = windows::open_hub(cx, saved.and_then(saved_bounds), |window, cx| {
            let dark = Appearance::of(window, cx) == Appearance::Dark;
            HubEntityHost::open_with(window, dark, boot, None, cx)
        });
        let (id, frame) = match opened {
            Ok(opened) => opened,
            Err(e) => {
                eprintln!("asrs: opening a hub window: {e}");
                return None;
            }
        };
        let hub: Entity<Hub> = frame.read(cx).host().hub().clone();
        cx.subscribe(&hub, move |hub, event, cx| match event {
            HubEvent::Main(message) => Self::hub_message(cx, id, message),
            HubEvent::Title => {
                let title =
                    crate::IDENTITY.window_title(&hub.read(cx).model().title().to_string_lossy());
                if let Some(handle) = Windows::handle(cx, id) {
                    let _ = handle.update(cx, |_, window, _| window.set_window_title(&title));
                }
            }
            _ => {}
        })
        .detach();
        let weak = hub.downgrade();
        let main = cx.global_mut::<MainProcess>();
        main.hubs.retain(|(_, h)| h.upgrade().is_some());
        main.hubs.push((id, weak));
        Some(id)
    }

    /// `openFilesSmart(files, sourceWindow)` for anything but a single Driver Station log, which
    /// takes the same route here.
    pub fn open_files(cx: &mut App, files: Vec<String>, source: Option<WindowId>) {
        let target = source.and_then(|id| {
            Self::hub_windows(cx)
                .into_iter()
                .find(|(w, _)| *w == id)
                .map(|(_, hub)| hub)
        });
        match target {
            Some(hub) => {
                let message = open_files_message(files);
                Self::send(cx, &hub, message.name, message.data);
            }
            None => {
                Self::open_hub(cx, None, files);
            }
        }
    }

    /// `advantageScopeAssets = await loadAssets()`; false when it failed.
    fn load_assets(cx: &mut App) -> bool {
        if !cx.has_global::<MainProcess>() {
            return false;
        }
        let main = cx.global_mut::<MainProcess>();
        let loaded = main.state.assets.reload(&RealFs);
        if let Err(e) = &loaded {
            eprintln!("asrs: loading assets: {e:?}");
        }
        loaded.is_ok()
    }

    fn send_assets(cx: &mut App) {
        let Some(main) = cx.try_global::<MainProcess>() else {
            return;
        };
        let data = main.state.assets.set_assets_data();
        for (_, hub) in Self::hub_windows(cx) {
            Self::send(cx, &hub, MainToHub::SetAssets, data.clone());
        }
    }

    fn save_state(cx: &mut App) {
        let screens = Screens::now(cx);
        if cx.has_global::<MainProcess>()
            && let Err(e) = cx
                .global_mut::<MainProcess>()
                .state
                .tracker
                .tick(&RealFs, &screens)
        {
            eprintln!("asrs: saving state: {e:?}");
        }
    }

    fn poll_akit(cx: &mut App) {
        let Some(main) = cx.try_global::<MainProcess>() else {
            return;
        };
        let Some(path) = lifecycle::take_akit_input(&RealFs, &main.state.paths.akit_path_input())
        else {
            return;
        };
        let Some((id, _)) = Self::hub_windows(cx).into_iter().next() else {
            return;
        };
        if let Some(handle) = Windows::handle(cx, id) {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
        }
        Self::open_files(cx, vec![path], Some(id));
    }
}

fn open_files_message(files: Vec<String>) -> NamedMessage<MainToHub> {
    let files = files.iter().map(|f| JsValue::string(f)).collect();
    NamedMessage::new(
        MainToHub::OpenFiles,
        ops::object([
            ("files", JsValue::Array(files)),
            ("merge", JsValue::Bool(false)),
        ]),
    )
}

fn every(cx: &mut App, period: Duration, mut tick: impl FnMut(&mut App) + 'static) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(period).await;
            cx.update(&mut tick);
        }
    })
    .detach();
}

/// Registers the platform's open event before the app runs: paths that arrive before
/// [`start`] are its `firstOpenPath`, later ones each open a new hub window.
pub fn open_file_events() -> (
    impl FnMut(Vec<String>) + 'static,
    async_channel::Receiver<String>,
) {
    let (send, receive) = async_channel::unbounded();
    let handler = move |urls: Vec<String>| {
        for path in file_paths(urls) {
            let _ = send.try_send(path);
        }
    };
    (handler, receive)
}

/// Module load and `app.whenReady`: the stale AdvantageKit request is cleared, preferences
/// rewritten and applied, assets loaded, and the saved windows (or one new hub) opened, the first
/// with the launch file. `argv` is `process.argv`.
pub fn start(cx: &mut App, argv: &[String], open_events: async_channel::Receiver<String>) {
    let paths = match host_paths() {
        Ok(paths) => paths,
        Err(e) => {
            eprintln!("asrs: {e}");
            cx.quit();
            return;
        }
    };
    lifecycle::clear_akit_input(&RealFs, &paths.akit_path_input());
    let screens = Screens::now(cx);
    let (state, prefs) = match MainState::start(&RealFs, &screens, paths, process_platform()) {
        Ok(started) => started,
        Err(e) => {
            eprintln!("asrs: startup: {e:?}");
            cx.quit();
            return;
        }
    };
    cx.set_global(theme_source(prefs.theme));
    let saved = state
        .tracker
        .get_saved_application_state(&RealFs, &screens, true);
    cx.set_global(MainProcess {
        state,
        menu: app_menu::template(Platform::current(), &prefs, crate::IDENTITY),
        hubs: Vec::new(),
    });
    MainProcess::install_menu(cx);
    let mut first_open_path = None;
    while let Ok(path) = open_events.try_recv() {
        first_open_path = Some(path);
    }
    if let Some(path) = lifecycle::first_file_arg(argv) {
        first_open_path = Some(path);
    }

    let mut plan = Vec::new();
    let planned = saved.and_then(|saved| lifecycle::startup_windows(&saved, |w| plan.push(w)));
    let mut target = None;
    let mut files = first_open_path.into_iter().collect::<Vec<_>>();
    for window in plan {
        let (saved, is_target) = match &window {
            StartupWindow::DefaultHub => (None, true),
            StartupWindow::Hub(state) => (Some(state), target.is_none()),
            StartupWindow::Satellite(_) => {
                eprintln!("asrs: saved satellite windows are not restored yet");
                continue;
            }
        };
        let opened = MainProcess::open_hub(
            cx,
            saved,
            if is_target {
                std::mem::take(&mut files)
            } else {
                Vec::new()
            },
        );
        if is_target {
            target = opened;
        }
    }
    if let Err(e) = planned {
        eprintln!("asrs: restoring windows: {e:?}");
    }

    every(cx, ASSET_RELOAD_PERIOD, |cx| {
        if MainProcess::load_assets(cx) {
            MainProcess::send_assets(cx);
        }
    });
    every(cx, STATE_SAVE_PERIOD, MainProcess::save_state);
    every(
        cx,
        Duration::from_millis(AKIT_PATH_INPUT_PERIOD_MS as u64),
        MainProcess::poll_akit,
    );
    cx.spawn(async move |cx| {
        while let Ok(path) = open_events.recv().await {
            cx.update(|cx| MainProcess::open_hub(cx, None, vec![path]));
        }
    })
    .detach();
    cx.on_window_closed(|cx, _| {
        if Platform::current() != Platform::Mac && cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
    cx.on_app_quit(|cx| {
        if let Some(main) = cx.try_global::<MainProcess>() {
            lifecycle::quit_cleanup(
                &RealFs,
                &main.state.paths.akit_path_output(),
                main.state.akit_path_written.as_deref(),
            );
        }
        async {}
    })
    .detach();
    cx.activate(true);
}

/// `app.on("activate")`: a new hub window when none is open.
pub fn reopen(cx: &mut App) {
    if cx.windows().is_empty() {
        MainProcess::open_hub(cx, None, Vec::new());
    }
}
