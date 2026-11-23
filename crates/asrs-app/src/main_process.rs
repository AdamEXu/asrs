//! `main.ts`'s process around its hub windows: `whenReady`, the launch argument and `open-file`
//! events, the 5 s asset reload with `set-assets`, the 250 ms state save, the AdvantageKit path
//! poll, `window-all-closed`, `activate` and the quit cleanup.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use asrs_js::math;
use asrs_js::ops;
use asrs_js::string::JsError;
use asrs_js::time::{Clock, NamedZone, SystemClock};
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_model::messages::{HubToMain, MainToHub, MainToSatellite, NamedMessage, SatelliteToMain};
use asrs_model::preferences::{Preferences, Theme};
use asrs_services::asset_downloader;
use asrs_services::constants::{AKIT_PATH_INPUT_PERIOD_MS, layout_default_path};
use asrs_services::ds_layout::default_ds_layout;
use asrs_services::fs::RealFs;
use asrs_services::lifecycle::{self, AppPaths, HubRestore, MainState, StartupWindow};
use asrs_services::path::PathStyle;
use asrs_services::routing::{Action, Hubs, Route};
use asrs_services::update::ReleaseProfile;
use asrs_services::window_manager::Platform;
use asrs_services::windows::WindowSystem;
use asrs_ui::gpui::{AnyWindowHandle, App, Entity, Global, WeakEntity};
use asrs_ui::platform;
use asrs_ui::theme::{Appearance, ThemeSource};

use crate::download_flow;
use crate::export_flow;
use crate::hub::{Hub, HubEvent};
use crate::hub_dialogs;
use crate::menus::app_menu;
use crate::menus::bridge;
use crate::menus::click::ClickEnv;
use crate::menus::dispatch;
use crate::menus::model::MenuItem;
use crate::satellite::{Satellite, SatelliteEvent};
use crate::views::hub_host::{HubEntityHost, boot_messages, show_satellite_menu};
use crate::views::prompts;
use crate::views::satellite_host::SatelliteHost;
use crate::windows::{self, Rect, SatellitePlacement, WindowId, Windows};

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
    satellites: Vec<(WindowId, Entity<Satellite>)>,
    /// `--random-seed` of the pages a parity scenario opens.
    scenario_seed: Option<u64>,
}

/// `createSatellite`'s `config`.
#[derive(Clone, Debug)]
pub enum SatelliteConfig {
    /// `create-satellite` from a hub window.
    New {
        parent: WindowId,
        uuid: JsValue,
        type_value: JsValue,
    },
    /// A saved `SatelliteWindowState`.
    State(JsValue),
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
        let satellites = manager.satellites().into_iter().flat_map(|(_, list)| list);
        let windows = manager
            .hubs()
            .iter()
            .chain(satellites)
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

/// `set-aspect-ratio` from a satellite: the content keeps its area at the new ratio.
pub fn set_aspect_ratio(cx: &mut App, id: WindowId, aspect_ratio: Option<f64>, lock: bool) {
    let Some(aspect_ratio) = aspect_ratio else {
        windows::set_aspect_ratio(cx, id, 0.0, |_| None);
        return;
    };
    let ratio = if lock { aspect_ratio } else { 0.0 };
    windows::set_aspect_ratio(cx, id, ratio, move |size| content_size(size, aspect_ratio));
}

/// `None` where Electron refuses the size (`NaN`).
pub fn content_size(original: (f64, f64), aspect_ratio: f64) -> Option<(f64, f64)> {
    let area = original.0 * original.1;
    let height = math::sqrt(area / aspect_ratio);
    let width = aspect_ratio * height;
    let size = (math::round(width), math::round(height));
    (size.0.is_finite() && size.1.is_finite()).then_some(size)
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
    pub(crate) fn headless(state: MainState, scenario_seed: u64) -> MainProcess {
        MainProcess {
            state,
            menu: Vec::new(),
            hubs: Vec::new(),
            satellites: Vec::new(),
            scenario_seed: Some(scenario_seed),
        }
    }

    /// Registers the hub window a parity scenario opened.
    #[cfg(feature = "parity")]
    pub(crate) fn adopt_hub(&mut self, id: WindowId, hub: WeakEntity<Hub>) {
        self.hubs.push((id, hub));
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

    /// The hub's messages about satellites: `create-satellite` and `update-satellite`.
    pub(crate) fn satellite_request(cx: &mut App, id: WindowId, event: &HubEvent) {
        match event {
            HubEvent::Main(message) if message.name == HubToMain::CreateSatellite => {
                let field = |key| message.data.get(key).cloned().unwrap_or_default();
                let config = SatelliteConfig::New {
                    parent: id,
                    uuid: field("uuid"),
                    type_value: field("type"),
                };
                Self::open_satellite(cx, config);
            }
            HubEvent::SatelliteUpdate(update) => {
                let uuid = ops::to_string(&update.uuid).to_string_lossy();
                let satellites = Self::satellites(cx, &uuid);
                for (window, satellite) in satellites {
                    if Windows::is_visible(cx, window) {
                        satellite.update(cx, |page, cx| page.render_command(update, cx));
                    }
                }
            }
            _ => {}
        }
    }

    /// `satelliteWindows[uuid]`.
    fn satellites(cx: &App, uuid: &str) -> Vec<(WindowId, Entity<Satellite>)> {
        let Some(main) = cx.try_global::<MainProcess>() else {
            return Vec::new();
        };
        let windows = Windows::manager(cx).satellites();
        let windows = windows.into_iter().find(|(key, _)| key == uuid);
        let pages = &main.satellites;
        windows
            .into_iter()
            .flat_map(|(_, list)| list)
            .filter_map(|id| pages.iter().find(|(window, _)| window == id).cloned())
            .collect()
    }

    /// Every satellite window's page.
    pub fn satellite_windows(cx: &App) -> Vec<(WindowId, Entity<Satellite>)> {
        cx.try_global::<MainProcess>()
            .map(|main| main.satellites.clone())
            .unwrap_or_default()
    }

    fn send_satellites(cx: &mut App, name: MainToSatellite, data: &JsValue) {
        let message = NamedMessage::new(name, data.clone());
        for (_, satellite) in Self::satellite_windows(cx) {
            satellite.update(cx, |page, cx| page.handle_main_message(&message, cx));
        }
    }

    /// `stateTracker.saveSatelliteIds(satelliteWindows); sendActiveSatellites();`
    fn satellites_changed(cx: &mut App) {
        let mut ids = JsObject::new();
        for (uuid, windows) in Windows::manager(cx).satellites() {
            let windows = windows
                .iter()
                .map(|id| ops::object([("id", JsValue::Number(*id as f64))]))
                .collect();
            ids.insert(uuid.as_str(), JsValue::Array(windows));
        }
        if !cx.has_global::<MainProcess>() {
            return;
        }
        let tracker = &mut cx.global_mut::<MainProcess>().state.tracker;
        if let Err(e) = tracker.save_satellite_ids(&JsValue::Object(ids)) {
            eprintln!("asrs: saveSatelliteIds: {e:?}");
        }
        let active = Self::active_satellites(cx);
        for (_, hub) in Self::hub_windows(cx) {
            Self::send(cx, &hub, MainToHub::SetActiveSatellites, active.clone());
        }
    }

    fn active_satellites(cx: &App) -> JsValue {
        let active = Windows::manager(cx).active_satellites();
        JsValue::Array(active.iter().map(|uuid| JsValue::string(uuid)).collect())
    }

    /// `createSatellite(config)` with its `dom-ready` sends.
    pub fn open_satellite(cx: &mut App, config: SatelliteConfig) -> Option<WindowId> {
        let main = cx.try_global::<MainProcess>()?;
        let seed = main.scenario_seed;
        let (uuid, placement, restore) = match &config {
            SatelliteConfig::New {
                parent,
                uuid,
                type_value,
            } => (
                uuid.clone(),
                SatellitePlacement::Parent(*parent),
                NamedMessage::new(MainToSatellite::SetType, type_value.clone()),
            ),
            SatelliteConfig::State(saved) => {
                let field = |key| saved.get(key).cloned().unwrap_or_default();
                (
                    field("uuid"),
                    saved_bounds(saved)
                        .map_or(SatellitePlacement::Unplaced, SatellitePlacement::Saved),
                    NamedMessage::new(MainToSatellite::RestoreState, field("state")),
                )
            }
        };
        let boot = [
            NamedMessage::new(
                MainToSatellite::SetAssets,
                main.state.assets.set_assets_data(),
            ),
            NamedMessage::new(MainToSatellite::SetBattery, JsValue::Bool(false)),
            restore,
        ];
        let key = ops::to_string(&uuid).to_string_lossy();
        let opened = windows::open_satellite(cx, &key, placement, |window, cx| {
            let dark = Appearance::of(window, cx) == Appearance::Dark;
            SatelliteHost::open(dark, &boot, seed, cx)
        });
        let (id, frame) = match opened {
            Ok(opened) => opened,
            Err(e) => {
                eprintln!("asrs: opening a satellite window: {e}");
                return None;
            }
        };
        let satellite = frame.read(cx).host().satellite().clone();
        cx.subscribe(&satellite, move |satellite, event, cx| match event {
            SatelliteEvent::Main(message) => {
                Self::satellite_message(cx, id, &uuid, satellite.downgrade(), message);
            }
            SatelliteEvent::Title(title) => Self::set_title(cx, id, title),
        })
        .detach();
        let title = satellite.read(cx).model().title().to_string_lossy();
        Self::set_title(cx, id, &title);
        if let Some(handle) = Windows::handle(cx, id) {
            let _ = handle.update(cx, |_, window, cx| {
                window.on_window_should_close(cx, move |_, cx| {
                    Self::before_unload(cx, id);
                    true
                });
            });
        }
        Windows::on_closed(cx, id, move |cx| {
            let main = cx.global_mut::<MainProcess>();
            main.satellites.retain(|(window, _)| *window != id);
            Self::satellites_changed(cx);
        });
        let main = cx.global_mut::<MainProcess>();
        main.satellites.push((id, satellite));
        Self::satellites_changed(cx);
        Self::send_all_preferences(cx);
        Some(id)
    }

    fn set_title(cx: &mut App, id: WindowId, title: &str) {
        let title = crate::IDENTITY.window_title(title);
        if let Some(handle) = Windows::handle(cx, id) {
            let _ = handle.update(cx, |_, window, _| window.set_window_title(&title));
        }
    }

    /// The page's `beforeunload` event, which `destroy()` does not fire.
    pub fn before_unload(cx: &mut App, window: WindowId) {
        let mut satellites = Self::satellite_windows(cx).into_iter();
        if let Some((_, satellite)) = satellites.find(|(id, _)| *id == window) {
            satellite.update(cx, |page, cx| page.before_unload(cx));
        }
    }

    fn satellite_message(
        cx: &mut App,
        id: WindowId,
        uuid: &JsValue,
        satellite: WeakEntity<Satellite>,
        message: &NamedMessage<SatelliteToMain>,
    ) {
        if !cx.has_global::<MainProcess>() {
            return;
        }
        let data = &message.data;
        let field = |key| data.get(key).cloned().unwrap_or_default();
        match message.name {
            SatelliteToMain::SetAspectRatio => set_aspect_ratio(
                cx,
                id,
                field("aspectRatio").as_f64(),
                field("lock").as_bool().unwrap_or(false),
            ),
            SatelliteToMain::Ask3dCamera | SatelliteToMain::Ask2dCamera => {
                if let Some(handle) = Windows::handle(cx, id) {
                    show_satellite_menu(handle, satellite, message, cx);
                }
            }
            SatelliteToMain::AddTableRange => {
                let range = ops::object([
                    ("controllerUUID", uuid.clone()),
                    ("rendererUUID", field("uuid")),
                    ("range", field("range")),
                ]);
                for (_, hub) in Self::hub_windows(cx) {
                    Self::send(cx, &hub, MainToHub::AddTableRange, range.clone());
                }
            }
            SatelliteToMain::SaveState => cx
                .global_mut::<MainProcess>()
                .state
                .tracker
                .save_renderer_state(id as f64, data.clone()),
            SatelliteToMain::UpdatePreferences => {
                let message = NamedMessage::new(HubToMain::UpdatePreferences, data.clone());
                let state = &mut cx.global_mut::<MainProcess>().state;
                match state.hub_message(&RealFs, id as f64, &message) {
                    Ok(()) => Self::send_all_preferences(cx),
                    Err(e) => eprintln!("asrs: update-preferences: {e:?}"),
                }
            }
            SatelliteToMain::CallSelectionSetter => {
                let mut call = data.clone();
                if let Some(call) = call.as_object_mut() {
                    call.insert("uuid", uuid.clone());
                }
                for (_, hub) in Self::hub_windows(cx) {
                    Self::send(cx, &hub, MainToHub::CallSelectionSetter, call.clone());
                }
            }
            SatelliteToMain::OpenLink => {
                if let Some(url) = data.as_str() {
                    cx.open_url(&url.to_string_lossy());
                }
            }
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
            Ok(()) if message.name == HubToMain::OpenFilesRequest => {
                let files = message.data.get("files").and_then(JsValue::as_array);
                let files = files.into_iter().flatten().filter_map(JsValue::as_str);
                let files = files.map(|f| f.to_string_lossy()).collect();
                Self::open_files(cx, files, Some(id));
            }
            Ok(()) => {
                export_flow::hub_message(id, message, cx);
                if let Some(window) = Windows::handle(cx, id) {
                    hub_dialogs::show(message, window, cx);
                    title_bar_menu_message(message, window, cx);
                }
            }
        }
    }

    /// `hubExportingIds.has(window.id)`.
    pub fn is_exporting(cx: &App, window: WindowId) -> bool {
        cx.try_global::<MainProcess>()
            .is_some_and(|main| main.state.is_exporting(window as f64))
    }

    /// `historical-start`'s `app.addRecentDocument(path)`, after the hub wrote the path to
    /// `akit-log-path.txt`.
    fn opened_document(cx: &mut App, path: &str) {
        if cx.has_global::<MainProcess>() {
            cx.global_mut::<MainProcess>().state.akit_path_written = Some(path.to_owned());
        }
        platform::add_recent_document(cx, Path::new(path));
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
        Self::send_satellites(cx, MainToSatellite::SetPreferences, &data);
        download_flow::send_preferences(&prefs, cx);
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

    /// `downloadStop`, which a closing licenses or source list help window also calls.
    pub fn download_stop(cx: &App) {
        download_flow::stop(cx);
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
        Self::open_hub_with(cx, saved, None, files)
    }

    fn open_hub_with(
        cx: &mut App,
        saved: Option<&JsValue>,
        layout_override: Option<&JsValue>,
        files: Vec<String>,
    ) -> Option<WindowId> {
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
        let active = Self::active_satellites(cx);
        for message in &mut boot {
            if message.name == MainToHub::SetActiveSatellites {
                message.data = active.clone();
            }
        }
        let main = cx.try_global::<MainProcess>()?;
        let restore = HubRestore {
            first_load: true,
            layout_override,
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
            HubEvent::Main(message) if message.name != HubToMain::CreateSatellite => {
                Self::hub_message(cx, id, message);
            }
            HubEvent::Main(_) | HubEvent::SatelliteUpdate(_) => {
                Self::satellite_request(cx, id, event);
            }
            HubEvent::Title => {
                let title = hub.read(cx).model().title().to_string_lossy();
                Self::set_title(cx, id, &title);
            }
            HubEvent::RecentDocument(path) => Self::opened_document(cx, &path.to_string_lossy()),
            _ => {}
        })
        .detach();
        if let Some(handle) = Windows::handle(cx, id) {
            let _ = handle.update(cx, |_, window, cx| {
                window.on_window_should_close(cx, move |_, cx| {
                    let exporting = MainProcess::is_exporting(cx, id);
                    if exporting {
                        cx.defer(move |cx| {
                            export_flow::may_close(id, cx);
                        });
                    }
                    !exporting
                });
            });
        }
        let weak = hub.downgrade();
        let main = cx.global_mut::<MainProcess>();
        main.hubs.retain(|(_, h)| h.upgrade().is_some());
        main.hubs.push((id, weak));
        Some(id)
    }

    /// `openFilesSmart(files, sourceWindow)`.
    pub fn open_files(cx: &mut App, files: Vec<String>, source: Option<WindowId>) {
        Self::route(cx, files, |hubs, files| {
            hubs.open_files_smart(files, source)
        });
    }

    /// The `open-file` event once the app is ready.
    fn open_file(cx: &mut App, path: String) {
        Self::route(cx, vec![path], |hubs, files| hubs.open_file(&files[0]));
    }

    fn route(
        cx: &mut App,
        files: Vec<String>,
        decide: impl FnOnce(&Hubs<'_>, &[String]) -> Result<Route, JsError>,
    ) {
        let Some(main) = cx.try_global::<MainProcess>() else {
            return;
        };
        let route = decide(
            &Hubs {
                fs: &RealFs,
                windows: Windows::manager(cx).hubs(),
                state: &|window| main.state.tracker.get_renderer_state(window as f64),
            },
            &files,
        );
        match route {
            Ok(Route::Open(action)) => Self::act(cx, action, files),
            Ok(Route::Ask {
                window,
                options,
                actions,
            }) => {
                let options = dispatch::message_box_options(&options);
                dispatch::show_message_box(window, options, cx, move |response, cx| {
                    if let Some(action) = actions.get(response) {
                        Self::act(cx, *action, files);
                    }
                });
            }
            Err(e) => eprintln!("asrs: opening {files:?}: {e:?}"),
        }
    }

    fn act(cx: &mut App, action: Action, files: Vec<String>) {
        match action {
            Action::Send(window) => Self::send_files(cx, window, files),
            Action::FocusAndSend(window) => {
                Self::focus(cx, window);
                Self::send_files(cx, window, files);
            }
            Action::NewHub => {
                Self::open_hub(cx, None, files);
            }
            Action::NewDsHub => {
                let layout = default_ds_layout(true).to_js();
                Self::open_hub_with(cx, None, Some(&layout), files);
            }
        }
    }

    pub(crate) fn send_files(cx: &mut App, window: WindowId, files: Vec<String>) {
        let message = open_files_message(files);
        Self::send_to_hub(cx, window, message.name, message.data);
    }

    pub(crate) fn focus(cx: &mut App, window: WindowId) {
        if let Some(handle) = Windows::handle(cx, window) {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
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
        Self::send_satellites(cx, MainToSatellite::SetAssets, &data);
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
        Self::focus(cx, id);
        Self::open_files(cx, vec![path], Some(id));
    }
}

/// `open-app-menu` and `close-app-menu`: the `title-bar-menu`'s buttons, which leave out the edit
/// menu.
fn title_bar_menu_message(
    message: &NamedMessage<HubToMain>,
    window: AnyWindowHandle,
    cx: &mut App,
) {
    let number = |value: Option<&JsValue>| value.and_then(JsValue::as_f64).unwrap_or(f64::NAN);
    match message.name {
        HubToMain::OpenAppMenu => {
            let mut index = number(message.data.get("index"));
            if index >= 2.0 {
                index += 1.0;
            }
            let rect = message.data.get("rect").cloned().unwrap_or_default();
            let x = math::round(number(rect.get("x")));
            let y = (number(rect.get("y")) + number(rect.get("height"))).ceil();
            cx.defer(move |cx| {
                let _ = window.update(cx, |_, window, cx| {
                    bridge::popup_application_submenu(
                        window,
                        cx,
                        index as usize,
                        x as i32,
                        y as i32,
                    );
                });
            });
        }
        HubToMain::CloseAppMenu => cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, _| platform::close_popup_menu(window));
        }),
        _ => {}
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
/// [`start`] are its `firstOpenPath`, later ones each go where [`Hubs::open_file`] sends them.
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
        satellites: Vec::new(),
        scenario_seed: None,
    });
    MainProcess::install_menu(cx);
    let opened = std::iter::from_fn(|| open_events.try_recv().ok());
    let first_open_path = lifecycle::first_open_path(opened, argv);

    let mut plan = Vec::new();
    let planned = saved.and_then(|saved| lifecycle::startup_windows(&saved, |w| plan.push(w)));
    let mut target = None;
    for window in plan {
        let (saved, is_target) = match &window {
            StartupWindow::DefaultHub => (None, true),
            StartupWindow::Hub(state) => (Some(state), target.is_none()),
            StartupWindow::Satellite(state) => {
                MainProcess::open_satellite(cx, SatelliteConfig::State(state.clone()));
                continue;
            }
        };
        let opened = MainProcess::open_hub(cx, saved, Vec::new());
        if is_target {
            target = opened;
        }
    }
    match planned {
        Ok(()) => {
            if let (Some(path), Some(target)) = (first_open_path, target) {
                MainProcess::open_files(cx, vec![path], Some(target));
            }
        }
        Err(e) => eprintln!("asrs: restoring windows: {e:?}"),
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
            cx.update(|cx| MainProcess::open_file(cx, path));
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
