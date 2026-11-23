//! What `src/main/electron/main.ts` does around its windows: where the app keeps its files
//! (`ElectronConstants.ts`), the `whenReady` preferences rewrite and saved-window plan, the hub's
//! `dom-ready` restores, the argv scan, the AdvantageKit path handoff through the temp directory,
//! and the quit cleanup.

use asrs_js::ops;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsValue, ToJs};
use asrs_model::app_identity::USER_DATA_DIR;
use asrs_model::json_file;
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage};
use asrs_model::preferences::{Preferences, merge_preferences};

use crate::asset_loader::{AssetFolders, AssetService};
use crate::fs::Fs;
use crate::path::PathStyle;
use crate::state_tracker::StateTracker;
use crate::window_manager::Platform;
use crate::windows::WindowSystem;
use crate::{constants, recent_units, type_memory};

/// The extensions `whenReady` takes a launch argument as a file to open by.
pub const FILE_ARG_EXTENSIONS: [&str; 9] = [
    ".wpilog",
    ".wpilogxz",
    ".rlog",
    ".hoot",
    ".revlog",
    ".log",
    ".csv",
    ".dslog",
    ".dsevents",
];

/// `app.getPath("userData")`, `app.getPath("temp")`, the bundled assets folder and
/// `app.isPackaged`, which every `ElectronConstants` path derives from.
#[derive(Clone, Debug, PartialEq)]
pub struct AppPaths {
    pub style: PathStyle,
    pub user_data: String,
    pub temp: String,
    pub bundled_assets: String,
    pub packaged: bool,
}

impl AppPaths {
    pub fn app_version(&self) -> &'static str {
        constants::app_version(self.packaged)
    }

    pub fn prefs(&self) -> String {
        constants::prefs_filename(self.style, &self.user_data)
    }

    /// `STATE_FILENAME`: `state-dev.json` unless packaged.
    pub fn state(&self) -> String {
        constants::state_filename(self.style, &self.user_data, self.app_version())
    }

    pub fn type_memory(&self) -> String {
        constants::type_memory_filename(self.style, &self.user_data)
    }

    pub fn recent_units(&self) -> String {
        constants::recent_units_filename(self.style, &self.user_data)
    }

    pub fn akit_path_input(&self) -> String {
        constants::akit_path_input(self.style, &self.temp)
    }

    pub fn akit_path_output(&self) -> String {
        constants::akit_path_output(self.style, &self.temp)
    }

    pub fn asset_folders(&self) -> AssetFolders {
        AssetFolders::new(self.style, &self.user_data, &self.bundled_assets)
    }
}

/// `app.getPath("appData")` as Chromium resolves it: `~/Library/Application Support` on macOS,
/// the roaming `%APPDATA%` on Windows, `$XDG_CONFIG_HOME` or `~/.config` on Linux.
pub fn app_data_dir(platform: Platform, env: impl Fn(&str) -> Option<String>) -> Option<String> {
    let set = |name: &str| env(name).filter(|v| !v.is_empty());
    match platform {
        Platform::Mac => {
            set("HOME").map(|home| join(PathStyle::Posix, &home, "Library/Application Support"))
        }
        Platform::Windows => set("APPDATA"),
        Platform::Linux => set("XDG_CONFIG_HOME")
            .or_else(|| set("HOME").map(|home| join(PathStyle::Posix, &home, ".config"))),
    }
}

/// `app.getPath("userData")`: the app's own directory under `appData`.
pub fn user_data_dir(platform: Platform, env: impl Fn(&str) -> Option<String>) -> Option<String> {
    let style = match platform {
        Platform::Windows => PathStyle::Win32,
        _ => PathStyle::Posix,
    };
    app_data_dir(platform, env).map(|dir| join(style, &dir, USER_DATA_DIR))
}

/// `app.getPath("temp")`: Chromium's temp directory, without the trailing separator the OS
/// hands back.
pub fn temp_dir() -> String {
    let dir = std::env::temp_dir().to_string_lossy().into_owned();
    let trimmed = dir.trim_end_matches(['/', std::path::MAIN_SEPARATOR]);
    if trimmed.is_empty() {
        dir
    } else {
        trimmed.to_owned()
    }
}

fn join(style: PathStyle, dir: &str, name: &str) -> String {
    style
        .join(&[&JsString::from(dir), &JsString::from(name)])
        .to_string_lossy()
}

/// The `whenReady` preferences step: the defaults (light theme on Linux), merged with
/// `prefs.json` when it exists and with a missing custom assets folder reset, written back. An
/// error is a throw that ends upstream's startup before any window opens.
pub fn startup_preferences(
    fs: &dyn Fs,
    prefs_file: &str,
    platform: &str,
) -> Result<Preferences, JsError> {
    let file = if fs.exists(prefs_file) {
        Some(json_file::parse(&fs.read(prefs_file)?)?)
    } else {
        None
    };
    let prefs = Preferences::startup(platform, file.as_ref(), |folder| {
        fs.exists(&folder.to_string_lossy())
    })?;
    fs.write(prefs_file, &json_file::stringify(&prefs.to_js())?)?;
    Ok(prefs)
}

/// A window `whenReady` creates.
#[derive(Clone, Debug, PartialEq)]
pub enum StartupWindow {
    /// `createHubWindow()`: no saved state, or a state without hubs.
    DefaultHub,
    /// `createHubWindow(hubState)`.
    Hub(JsValue),
    /// `createSatellite({ state: satelliteState })`.
    Satellite(JsValue),
}

/// The windows `whenReady` creates from `getSavedApplicationState()`, in order. `create` runs
/// for each before the next is looked at, so a malformed `satellites` throws after the hubs are
/// open, as upstream's `forEach` calls do.
pub fn startup_windows(
    application_state: &JsValue,
    mut create: impl FnMut(StartupWindow),
) -> Result<(), JsError> {
    if application_state.is_null() || array_len(application_state, "hubs")? == 0 {
        create(StartupWindow::DefaultHub);
        return Ok(());
    }
    for hub in array_items(application_state, "hubs")? {
        create(StartupWindow::Hub(hub));
    }
    for satellite in array_items(application_state, "satellites")? {
        create(StartupWindow::Satellite(satellite));
    }
    Ok(())
}

fn not_array(v: &JsValue, reading: &str) -> JsError {
    JsError::type_error(format!(
        "Cannot read properties of {} (reading '{reading}')",
        if v.is_null() { "null" } else { "undefined" }
    ))
}

fn array_len(value: &JsValue, key: &str) -> Result<usize, JsError> {
    let v = ops::get(value, key)?;
    v.as_array()
        .map(<[JsValue]>::len)
        .ok_or_else(|| not_array(&v, "length"))
}

fn array_items(value: &JsValue, key: &str) -> Result<Vec<JsValue>, JsError> {
    let v = ops::get(value, key)?;
    v.as_array()
        .map(<[JsValue]>::to_vec)
        .ok_or_else(|| not_array(&v, "forEach"))
}

/// What a hub's `dom-ready` handler picks its `restore-state` from.
#[derive(Clone, Debug, Default)]
pub struct HubRestore<'a> {
    /// The handler has not run for this window before.
    pub first_load: bool,
    /// `createHubWindow`'s `layoutOverride`.
    pub layout_override: Option<&'a JsValue>,
    /// `createHubWindow`'s `state`: a saved `WindowState`.
    pub saved: Option<&'a JsValue>,
    /// `stateTracker.getRendererState(window)`.
    pub cached: JsValue,
}

/// The `dom-ready` messages after the fixed init sends (`set-assets` through
/// `set-active-satellites`): `restore-type-memory` when `type-memory.json` exists, then
/// `restore-state` from the override, the saved state or the cached renderer state. A read or
/// parse error is the handler's throw, after the messages before it were sent.
pub fn dom_ready_restores(
    fs: &dyn Fs,
    type_memory_file: &str,
    restore: &HubRestore<'_>,
    mut send: impl FnMut(MainToHub, JsValue),
) -> Result<(), JsError> {
    if fs.exists(type_memory_file) {
        send(
            MainToHub::RestoreTypeMemory,
            json_file::parse(&fs.read(type_memory_file)?)?,
        );
    }
    let state = match (restore.first_load, restore.layout_override, restore.saved) {
        (true, Some(layout), _) => Some(layout.clone()),
        (true, None, Some(saved)) => Some(ops::get(saved, "state")?),
        _ => (!restore.cached.is_undefined()).then(|| restore.cached.clone()),
    };
    if let Some(state) = state {
        send(MainToHub::RestoreState, state);
    }
    Ok(())
}

/// `firstOpenPath` from `process.argv`: the first argument naming a log by its extension.
pub fn first_file_arg(argv: &[String]) -> Option<String> {
    argv.iter()
        .find(|arg| FILE_ARG_EXTENSIONS.iter().any(|ext| arg.ends_with(ext)))
        .cloned()
}

/// At launch, a stale `ascope-log-path.txt` from an earlier request is deleted unread.
pub fn clear_akit_input(fs: &dyn Fs, akit_path_input: &str) {
    if fs.exists(akit_path_input) {
        fs.remove(akit_path_input);
    }
}

/// One tick of the `AKIT_PATH_INPUT_PERIOD` poll: the path AdvantageKit wrote, trimmed, with the
/// file deleted. An unreadable file stays for the next tick.
pub fn take_akit_input(fs: &dyn Fs, akit_path_input: &str) -> Option<String> {
    if !fs.exists(akit_path_input) {
        return None;
    }
    let bytes = fs.read(akit_path_input).ok()?;
    fs.remove(akit_path_input);
    Some(
        JsString::from(String::from_utf8_lossy(&bytes).as_ref())
            .trim()
            .to_string_lossy(),
    )
}

/// `app.on("quit")`: the log path handed to AdvantageKit is withdrawn. The official app writes
/// the same temp file, so it is removed only while it still holds the path this process wrote.
pub fn quit_cleanup(fs: &dyn Fs, akit_path_output: &str, written: Option<&str>) {
    let Some(written) = written else {
        return;
    };
    if fs
        .read(akit_path_output)
        .is_ok_and(|bytes| bytes == written.as_bytes())
    {
        fs.remove(akit_path_output);
    }
}

/// What main keeps in files: the `StateTracker` made at module load, the assets `whenReady`
/// loads, and the preferences, type memory and recent units files.
pub struct MainState {
    pub paths: AppPaths,
    pub tracker: StateTracker,
    pub assets: AssetService,
    /// The log path this process last wrote to `akit-log-path.txt`.
    pub akit_path_written: Option<String>,
}

impl MainState {
    /// `new StateTracker()` at module load, then `whenReady` up to the menu: the preferences
    /// rewrite, `createAssetFolders()` and the first `loadAssets()`. An error is a throw that
    /// leaves upstream running without a window.
    pub fn start(
        fs: &dyn Fs,
        ws: &dyn WindowSystem,
        paths: AppPaths,
        platform: &str,
    ) -> Result<(MainState, Preferences), JsError> {
        let tracker = StateTracker::new(fs, ws, &paths.state())?;
        let prefs = startup_preferences(fs, &paths.prefs(), platform)?;
        let mut assets = AssetService::new(paths.asset_folders());
        assets.start(fs)?;
        let main = MainState {
            paths,
            tracker,
            assets,
            akit_path_written: None,
        };
        Ok((main, prefs))
    }

    /// `jsonfile.readFileSync(PREFS_FILENAME)`, as `sendAllPreferences` sends it.
    pub fn preferences(&self, fs: &dyn Fs) -> Result<JsValue, JsError> {
        json_file::parse(&fs.read(&self.paths.prefs())?)
    }

    /// `jsonfile.writeFileSync(PREFS_FILENAME, prefs)`.
    pub fn write_preferences(&self, fs: &dyn Fs, prefs: &JsValue) -> Result<(), JsError> {
        fs.write(&self.paths.prefs(), &json_file::stringify(prefs)?)
    }

    /// `handleHubMessage`'s file effects for the hub window with numeric id `window`.
    pub fn hub_message(
        &mut self,
        fs: &dyn Fs,
        window: f64,
        message: &NamedMessage<HubToMain>,
    ) -> Result<(), JsError> {
        match message.name {
            HubToMain::SaveState => {
                self.tracker
                    .save_renderer_state(window, message.data.clone());
                Ok(())
            }
            HubToMain::SaveTypeMemory => {
                type_memory::save(fs, &self.paths.type_memory(), &message.data)
            }
            HubToMain::UpdatePreferences => {
                let mut prefs = self.preferences(fs)?;
                merge_preferences(&mut prefs, &message.data)?;
                self.write_preferences(fs, &prefs)
            }
            _ => Ok(()),
        }
    }

    /// `ask-edit-axis`'s read of `recent-units.json`.
    pub fn recent_units(&self, fs: &dyn Fs) -> Result<Option<JsValue>, JsError> {
        recent_units::read(fs, &self.paths.recent_units())
    }

    /// `updateRecents(preset)`.
    pub fn update_recent_units(&self, fs: &dyn Fs, preset: &JsValue) -> Result<(), JsError> {
        recent_units::update(fs, &self.paths.recent_units(), preset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::MemFs;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn user_data_is_asrs_under_app_data() {
        assert_eq!(
            user_data_dir(Platform::Mac, env(&[("HOME", "/Users/a")])).as_deref(),
            Some("/Users/a/Library/Application Support/asrs")
        );
        assert_eq!(
            user_data_dir(
                Platform::Windows,
                env(&[("APPDATA", "C:\\Users\\a\\AppData\\Roaming")])
            )
            .as_deref(),
            Some("C:\\Users\\a\\AppData\\Roaming\\asrs")
        );
        assert_eq!(
            user_data_dir(Platform::Linux, env(&[("HOME", "/home/a")])).as_deref(),
            Some("/home/a/.config/asrs")
        );
        assert_eq!(
            user_data_dir(
                Platform::Linux,
                env(&[("HOME", "/home/a"), ("XDG_CONFIG_HOME", "/cfg")])
            )
            .as_deref(),
            Some("/cfg/asrs")
        );
        assert_eq!(
            user_data_dir(
                Platform::Linux,
                env(&[("HOME", "/home/a"), ("XDG_CONFIG_HOME", "")])
            )
            .as_deref(),
            Some("/home/a/.config/asrs")
        );
    }

    #[test]
    fn upstream_file_names_inside_user_data() {
        let paths = AppPaths {
            style: PathStyle::Posix,
            user_data: "/u/asrs".into(),
            temp: "/t".into(),
            bundled_assets: "/b".into(),
            packaged: false,
        };
        assert_eq!(paths.prefs(), "/u/asrs/prefs.json");
        assert_eq!(paths.state(), "/u/asrs/state-dev.json");
        assert_eq!(paths.type_memory(), "/u/asrs/type-memory.json");
        assert_eq!(paths.recent_units(), "/u/asrs/recent-units.json");
        assert_eq!(paths.akit_path_input(), "/t/ascope-log-path.txt");
        assert_eq!(paths.akit_path_output(), "/t/akit-log-path.txt");
        let packaged = AppPaths {
            packaged: true,
            ..paths
        };
        assert_eq!(
            packaged.state(),
            format!(
                "/u/asrs/state-{}.json",
                asrs_model::build_constants::VERSION.replace('.', "_")
            )
        );
    }

    #[test]
    fn first_file_arg_matches_by_suffix() {
        let argv: Vec<String> = ["/x/asrs", "-v", "a.txt", "b.LOG", "c.dslog", "d.wpilog"]
            .map(String::from)
            .to_vec();
        assert_eq!(first_file_arg(&argv).as_deref(), Some("c.dslog"));
        assert_eq!(first_file_arg(&argv[..4]), None);
    }

    #[test]
    fn akit_input_is_trimmed_and_consumed() {
        let fs = MemFs::new();
        assert_eq!(take_akit_input(&fs, "/t/in"), None);
        fs.put("/t/in", "\u{feff} /logs/a.wpilog\r\n".as_bytes());
        assert_eq!(
            take_akit_input(&fs, "/t/in").as_deref(),
            Some("/logs/a.wpilog")
        );
        assert!(!fs.exists("/t/in"));
        fs.put("/t/in", b"x");
        clear_akit_input(&fs, "/t/in");
        assert!(!fs.exists("/t/in"));
        fs.put("/t/out", b"/logs/a.wpilog");
        quit_cleanup(&fs, "/t/out", None);
        assert!(fs.exists("/t/out"));
        quit_cleanup(&fs, "/t/out", Some("/logs/b.wpilog"));
        assert!(fs.exists("/t/out"));
        quit_cleanup(&fs, "/t/out", Some("/logs/a.wpilog"));
        assert!(!fs.exists("/t/out"));
    }
}
