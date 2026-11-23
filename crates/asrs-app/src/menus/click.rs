//! The `click` handlers of `setupMenu` in `src/main/electron/main.ts`.
//!
//! [`click`] runs a handler up to the first dialog it waits for and pushes the [`Effect`]s it
//! has, in order. A dialog effect carries a [`Then`], which goes back to [`resume`] with the
//! dialog's reply to run the rest of the handler.

use asrs_js::ops;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsValue, ToJs};
use asrs_model::app_identity::AppIdentity;
use asrs_model::build_constants;
use asrs_model::json_file;
use asrs_model::messages::MainToHub;
use asrs_model::preferences::LiveMode;
use asrs_services::about::{AboutEnv, about};
use asrs_services::asset_loader::get_user_assets_path;
use asrs_services::dialog::{FileFilter, MessageBox, OpenDialog, SaveDialog};
use asrs_services::ds_layout::default_ds_layout;
use asrs_services::fs::Fs;
use asrs_services::historical_start::HOOT_UNSUPPORTED;
use asrs_services::layout::{self, Import};
use asrs_services::lifecycle::AppPaths;
use asrs_services::window_manager::{Platform, WindowId, WindowKind, WindowManager};

use super::model::{Command, HelpLink};

const ROBOT_LOGS: FileFilter = FileFilter {
    name: "Robot logs",
    extensions: &[
        "rlog", "wpilog", "wpilogxz", "hoot", "revlog", "log", "csv", "dslog", "dsevents",
    ],
};

const JSON_FILES: FileFilter = FileFilter {
    name: "JSON files",
    extensions: &["json"],
};

/// What the handlers read outside themselves.
pub struct ClickEnv<'a> {
    pub platform: Platform,
    /// `process.arch`.
    pub arch: &'a str,
    pub identity: AppIdentity,
    /// `GITHUB_REPOSITORY`.
    pub repository: &'a str,
    pub fs: &'a dyn Fs,
    pub paths: &'a AppPaths,
    pub windows: &'a WindowManager,
    /// `getAssetDownloadStatus()`.
    pub asset_status: &'a str,
    /// `"AdvantageScope " + formatDate() + ".json"`, before the identity applies.
    pub layout_default_path: String,
    /// `stateTracker.getCurrentApplicationState()`.
    pub application_state: &'a dyn Fn() -> JsValue,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// `sendMessage(window, name, data)`.
    Send {
        window: WindowId,
        name: MainToHub,
        data: JsValue,
    },
    /// `sendAllPreferences()`.
    SendAllPreferences,
    /// `advantageScopeAssets = await loadAssets(); sendAllPreferences(); sendAssets();`
    ReloadAssets,
    /// `dialog.showMessageBox` or, with `sync`, `showMessageBoxSync`.
    MessageBox {
        window: Option<WindowId>,
        sync: bool,
        options: MessageBox,
        then: Then,
    },
    OpenDialog {
        window: Option<WindowId>,
        options: OpenDialog,
        then: Then,
    },
    SaveDialog {
        window: Option<WindowId>,
        options: SaveDialog,
        then: Then,
    },
    /// `openFilesSmart(files, window)`.
    OpenFilesSmart {
        files: Vec<String>,
        window: WindowId,
    },
    /// `checkForUpdate(true)`.
    CheckForUpdate,
    /// `createHubWindow(state)`.
    OpenHub(Option<JsValue>),
    /// `createSatellite({ state })`.
    OpenSatellite(JsValue),
    /// `openPreferences(parent)`, `openDownload(parent)` or `openLicenses(parent)`.
    OpenWindow { kind: WindowKind, parent: WindowId },
    /// `newTabPopup(window, rect)` for a hub window.
    NewTabPopup { window: WindowId, rect: JsValue },
    /// `window.close()`.
    Close(WindowId),
    /// `window.destroy()`.
    Destroy(WindowId),
    /// `window.moveTop()`.
    MoveTop(WindowId),
    /// `shell.openPath(path)`.
    OpenPath(JsValue),
    /// `shell.openExternal(url)`.
    OpenExternal(String),
    /// `clipboard.writeText(text)`.
    WriteClipboard(String),
}

/// The rest of a handler, after its dialog.
#[derive(Clone, Debug, PartialEq)]
pub enum Then {
    Nothing,
    About { copy_text: String },
    OpenLogs { window: WindowId },
    AddLogs { window: WindowId },
    CustomAssets { prefs: JsValue },
    ExportLayout,
    ImportLayout,
    ImportVersion { state: JsValue },
    ResetToDsLayout { window: WindowId },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    /// The button's index.
    Box(usize),
    /// `filePaths`, empty when cancelled.
    Open(Vec<String>),
    /// `filePath`, `None` when cancelled.
    Save(Option<String>),
}

/// How a handler ended early.
#[derive(Clone, Debug)]
pub enum Failure {
    /// A throw out of the handler, which Electron reports in an error dialog.
    Throw(JsError),
    /// A rejected promise, which upstream's `unhandledRejection` listener silences.
    Rejection(JsError),
}

fn send(out: &mut Vec<Effect>, window: WindowId, name: MainToHub, data: JsValue) {
    out.push(Effect::Send { window, name, data });
}

fn info_box(message: &str, detail: &str) -> MessageBox {
    MessageBox {
        kind: "info",
        title: "About",
        message: JsString::from(message),
        detail: JsString::from(detail),
        buttons: Some(vec!["Close"]),
        default_id: None,
        cancel_id: None,
        icon: true,
        modal: false,
    }
}

fn error_box(message: &str, detail: String) -> MessageBox {
    MessageBox {
        kind: "error",
        title: "Error",
        detail: JsString::from(detail),
        buttons: None,
        ..info_box(message, "")
    }
}

fn warning_box(message: &str, detail: String, buttons: [&'static str; 2]) -> MessageBox {
    MessageBox {
        kind: "warning",
        title: "Warning",
        detail: JsString::from(detail),
        buttons: Some(buttons.to_vec()),
        ..info_box(message, "")
    }
}

fn process_platform(platform: Platform) -> &'static str {
    match platform {
        Platform::Mac => "darwin",
        Platform::Windows => "win32",
        Platform::Linux => "linux",
    }
}

impl ClickEnv<'_> {
    fn is_hub(&self, window: WindowId) -> bool {
        self.windows.kind(window) == Some(WindowKind::Hub)
    }

    fn read_prefs(&self) -> Result<JsValue, JsError> {
        json_file::parse(&self.fs.read(&self.paths.prefs())?)
    }

    fn write_prefs(&self, prefs: &JsValue) -> Result<(), JsError> {
        self.fs
            .write(&self.paths.prefs(), &json_file::stringify(prefs)?)
    }

    /// `prefs = readFileSync(); prefs[key] = value; writeFileSync(prefs); sendAllPreferences();`
    fn set_pref(&self, key: &str, value: &str, out: &mut Vec<Effect>) -> Result<(), JsError> {
        let mut prefs = self.read_prefs()?;
        ops::set_key(&mut prefs, &JsString::from(key), JsValue::string(value))?;
        self.write_prefs(&prefs)?;
        out.push(Effect::SendAllPreferences);
        Ok(())
    }

    fn start_live(
        &self,
        window: WindowId,
        simulator: bool,
        mode: Option<LiveMode>,
        out: &mut Vec<Effect>,
    ) -> Result<(), JsError> {
        if let Some(mode) = mode {
            self.set_pref("liveMode", mode.as_str(), out)?;
        }
        let target = if simulator { "sim" } else { "robot" };
        send(out, window, MainToHub::StartLive, JsValue::string(target));
        Ok(())
    }
}

/// One click on a template item. `checked` is the item's state as the handler sees it, after
/// Electron flipped it; `window` is the focused window.
pub fn click(
    command: Command,
    checked: bool,
    window: Option<WindowId>,
    env: &ClickEnv<'_>,
    out: &mut Vec<Effect>,
) -> Result<(), Failure> {
    let hub = window.filter(|w| env.is_hub(*w));
    let to_hub = |out: &mut Vec<Effect>, name: MainToHub, data: JsValue| {
        if let Some(hub) = hub {
            send(out, hub, name, data);
        }
    };
    match command {
        Command::About => {
            let about = about(&AboutEnv {
                app_name: env.identity.name,
                distribution: build_constants::DISTRIBUTION,
                platform: process_platform(env.platform),
                arch: env.arch,
                packaged: env.paths.packaged,
                version: build_constants::VERSION,
            });
            out.push(Effect::MessageBox {
                window: None,
                sync: false,
                options: about.message_box,
                then: Then::About {
                    copy_text: about.copy_text,
                },
            });
        }
        Command::OpenPreferences | Command::OpenLicenses | Command::DownloadLogs => {
            let kind = match command {
                Command::OpenPreferences => WindowKind::Preferences,
                Command::OpenLicenses => WindowKind::Licenses,
                _ => WindowKind::Download,
            };
            if let Some(parent) = window {
                out.push(Effect::OpenWindow { kind, parent });
            }
        }
        Command::CheckForUpdates => out.push(Effect::CheckForUpdate),
        Command::ShowAssetsFolder => {
            let folder =
                get_user_assets_path(env.fs, &env.paths.asset_folders()).map_err(Failure::Throw)?;
            out.push(Effect::OpenPath(folder));
        }
        Command::UseCustomAssetsFolder => {
            let mut prefs = env.read_prefs().map_err(Failure::Rejection)?;
            if checked {
                out.push(Effect::OpenDialog {
                    window: None,
                    options: OpenDialog {
                        title: env
                            .identity
                            .text("Select folder containing custom AdvantageScope assets"),
                        message: None,
                        button_label: Some("Open"),
                        properties: &["openDirectory", "createDirectory", "dontAddToRecent"],
                        filters: None,
                    },
                    then: Then::CustomAssets { prefs },
                });
            } else {
                ops::set_key(
                    &mut prefs,
                    &JsString::from("userAssetsFolder"),
                    JsValue::Null,
                )
                .map_err(Failure::Rejection)?;
                finish_custom_assets(&prefs, env, out).map_err(Failure::Rejection)?;
            }
        }
        Command::AssetDownloadStatus => out.push(Effect::MessageBox {
            window: None,
            sync: false,
            options: info_box("Asset Download Status", env.asset_status),
            then: Then::Nothing,
        }),
        Command::OwletDownloadStatus => out.push(Effect::MessageBox {
            window: None,
            sync: false,
            options: info_box("Owlet Download Status", HOOT_UNSUPPORTED),
            then: Then::Nothing,
        }),
        Command::OpenLogs | Command::AddLogs => {
            let Some(window) = hub else { return Ok(()) };
            let add = command == Command::AddLogs;
            out.push(Effect::OpenDialog {
                window: Some(window),
                options: OpenDialog {
                    title: if add {
                        "Select the robot log file(s) to add to the current log"
                    } else {
                        "Select the robot log file(s) to open"
                    }
                    .to_owned(),
                    message: (!add).then_some(
                        "If multiple files are selected, timestamps will be aligned automatically",
                    ),
                    button_label: None,
                    properties: &["openFile", "multiSelections"],
                    filters: Some(ROBOT_LOGS),
                },
                then: if add {
                    Then::AddLogs { window }
                } else {
                    Then::OpenLogs { window }
                },
            });
        }
        Command::StartLive { simulator, mode } => {
            if let Some(hub) = hub {
                env.start_live(hub, simulator, mode, out)
                    .map_err(Failure::Throw)?;
            }
        }
        Command::ConnectDriverStation => to_hub(out, MainToHub::StartLive, JsValue::string("ds")),
        Command::UseSystemcoreAddress(address) => {
            let value = if checked { address.as_str() } else { "" };
            env.set_pref("systemcoreStaticAddress", value, out)
                .map_err(Failure::Throw)?;
        }
        Command::ExportData => to_hub(out, MainToHub::StartExport, JsValue::Undefined),
        Command::StartPublish { simulator } => {
            to_hub(out, MainToHub::StartPublish, JsValue::Bool(simulator));
        }
        Command::StopPublish => to_hub(out, MainToHub::StopPublish, JsValue::Undefined),
        Command::ExportLayout => out.push(Effect::SaveDialog {
            window: None,
            options: SaveDialog {
                title: "Select export location for layout file",
                default_path: env.identity.text(&env.layout_default_path),
                properties: &[
                    "createDirectory",
                    "showOverwriteConfirmation",
                    "dontAddToRecent",
                ],
                filters: JSON_FILES,
            },
            then: Then::ExportLayout,
        }),
        Command::ImportLayout => out.push(Effect::OpenDialog {
            window: None,
            options: OpenDialog {
                title: "Select one or more layout files to import".to_owned(),
                message: None,
                button_label: None,
                properties: &["openFile", "multiSelections"],
                filters: Some(JSON_FILES),
            },
            then: Then::ImportLayout,
        }),
        Command::ResetToDsLayout => {
            let Some(window) = hub else { return Ok(()) };
            out.push(Effect::MessageBox {
                window: Some(window),
                sync: true,
                options: MessageBox {
                    default_id: Some(0.0),
                    cancel_id: Some(1.0),
                    modal: true,
                    ..warning_box(
                        "Reset to DS Layout?",
                        "The layout of this window will be replaced with the DS layout. This cannot be undone."
                            .to_owned(),
                        ["Reset", "Cancel"],
                    )
                },
                then: Then::ResetToDsLayout { window },
            });
        }
        Command::NewWindow => out.push(Effect::OpenHub(None)),
        Command::CloseWindow => out.extend(window.map(Effect::Close)),
        Command::ZoomToEnabledRange => to_hub(out, MainToHub::ZoomEnabled, JsValue::Undefined),
        Command::ToggleSidebar => to_hub(out, MainToHub::ToggleSidebar, JsValue::Undefined),
        Command::ToggleControls => to_hub(out, MainToHub::ToggleControls, JsValue::Undefined),
        Command::NewTab(tab) => to_hub(out, MainToHub::NewTab, tab.to_js()),
        Command::NewTabPopup => {
            if let Some(window) = hub {
                let width = env.windows.bounds(window).map_or(0.0, |b| b.width);
                let y = if env.platform == Platform::Windows {
                    48.0
                } else {
                    10.0
                };
                out.push(Effect::NewTabPopup {
                    window,
                    rect: ops::object([
                        ("x", JsValue::Number(width - 12.0)),
                        ("y", JsValue::Number(y)),
                        ("width", JsValue::Number(0.0)),
                        ("height", JsValue::Number(0.0)),
                    ]),
                });
            }
        }
        Command::NewPopOut => to_hub(out, MainToHub::NewSatellite, JsValue::Undefined),
        Command::MoveTab(delta) => {
            to_hub(out, MainToHub::MoveTab, JsValue::Number(f64::from(delta)))
        }
        Command::ShiftTab(delta) => {
            to_hub(out, MainToHub::ShiftTab, JsValue::Number(f64::from(delta)))
        }
        Command::CloseTab => match (hub, window) {
            (Some(hub), _) => send(out, hub, MainToHub::CloseTab, JsValue::Bool(false)),
            (None, Some(window)) => out.push(Effect::Destroy(window)),
            (None, None) => {}
        },
        Command::BringAllToFront => {
            out.extend(
                env.windows
                    .bring_to_front_order()
                    .into_iter()
                    .map(Effect::MoveTop),
            );
            out.extend(window.map(Effect::MoveTop));
        }
        Command::Help(link) => out.push(Effect::OpenExternal(match link {
            HelpLink::ReportProblem => format!("https://github.com/{}/issues", env.repository),
            HelpLink::ContactUs => "mailto:software@team6328.org".to_owned(),
            HelpLink::GitHubRepository => format!("https://github.com/{}", env.repository),
            HelpLink::WpilibDocumentation => "https://docs.wpilib.org".to_owned(),
            HelpLink::LittletonRobotics => "https://littletonrobotics.org".to_owned(),
        })),
        Command::ContextItem(_) => {
            unreachable!("context menu items carry effects, not application menu commands")
        }
    }
    Ok(())
}

fn finish_custom_assets(
    prefs: &JsValue,
    env: &ClickEnv<'_>,
    out: &mut Vec<Effect>,
) -> Result<(), JsError> {
    env.write_prefs(prefs)?;
    out.push(Effect::ReloadAssets);
    Ok(())
}

/// The rest of a handler once its dialog closed.
pub fn resume(
    then: Then,
    reply: &Reply,
    env: &ClickEnv<'_>,
    out: &mut Vec<Effect>,
) -> Result<(), Failure> {
    match (then, reply) {
        (Then::Nothing, Reply::Box(_)) => {}
        (Then::About { copy_text }, Reply::Box(response)) => {
            if *response == 1 {
                out.push(Effect::WriteClipboard(copy_text));
            }
        }
        (Then::OpenLogs { window }, Reply::Open(files)) => {
            if !files.is_empty() {
                out.push(Effect::OpenFilesSmart {
                    files: files.clone(),
                    window,
                });
            }
        }
        (Then::AddLogs { window }, Reply::Open(files)) => {
            if !files.is_empty() {
                send(
                    out,
                    window,
                    MainToHub::OpenFiles,
                    ops::object([
                        (
                            "files",
                            JsValue::Array(files.iter().map(|f| JsValue::string(f)).collect()),
                        ),
                        ("merge", JsValue::Bool(true)),
                    ]),
                );
            }
        }
        (Then::CustomAssets { mut prefs }, Reply::Open(files)) => {
            if let Some(folder) = files.first() {
                ops::set_key(
                    &mut prefs,
                    &JsString::from("userAssetsFolder"),
                    JsValue::string(folder),
                )
                .map_err(Failure::Rejection)?;
            }
            finish_custom_assets(&prefs, env, out).map_err(Failure::Rejection)?;
        }
        (Then::ExportLayout, Reply::Save(path)) => {
            if let Some(path) = path {
                let mut state = (env.application_state)();
                layout::export_bytes(&mut state, env.paths.app_version())
                    .and_then(|bytes| env.fs.write(path, &bytes))
                    .map_err(Failure::Rejection)?;
            }
        }
        (Then::ImportLayout, Reply::Open(files)) => {
            if files.is_empty() {
                return Ok(());
            }
            let import = layout::import(env.fs, files, env.paths.packaged, env.paths.app_version())
                .map_err(Failure::Rejection)?;
            let failed = |detail: &str| Effect::MessageBox {
                window: None,
                sync: false,
                options: error_box("Failed to import layout", env.identity.text(detail)),
                then: Then::Nothing,
            };
            match import {
                Import::OldFormat => out.push(failed(
                    "The selected layout file uses an older format which is not compatible with the current version of AdvantageScope.",
                )),
                Import::Unrecognized => out.push(failed(
                    "The selected layout file was not a recognized format.",
                )),
                Import::Windows {
                    state,
                    version_mismatch: true,
                } => out.push(Effect::MessageBox {
                    window: None,
                    sync: true,
                    options: warning_box(
                        "Version mismatch",
                        env.identity.text(
                            "The layout file was generated by a different version of AdvantageScope. Compatibility is not guaranteed.",
                        ),
                        ["Continue", "Cancel"],
                    ),
                    then: Then::ImportVersion { state },
                }),
                Import::Windows {
                    state,
                    version_mismatch: false,
                } => replace_windows(&state, env, out).map_err(Failure::Rejection)?,
            }
        }
        (Then::ImportVersion { state }, Reply::Box(response)) => {
            if *response == 0 {
                replace_windows(&state, env, out).map_err(Failure::Rejection)?;
            }
        }
        (Then::ResetToDsLayout { window }, Reply::Box(response)) => {
            if *response == 0 {
                send(
                    out,
                    window,
                    MainToHub::RestoreState,
                    default_ds_layout(true).to_js(),
                );
            }
        }
        (then, reply) => panic!("{then:?} does not take {reply:?}"),
    }
    Ok(())
}

fn replace_windows(
    state: &JsValue,
    env: &ClickEnv<'_>,
    out: &mut Vec<Effect>,
) -> Result<(), JsError> {
    out.extend(env.windows.hubs().iter().copied().map(Effect::Close));
    for (_, satellites) in env.windows.satellites() {
        out.extend(satellites.iter().copied().map(Effect::Close));
    }
    let items = |key: &str| -> Result<Vec<JsValue>, JsError> {
        Ok(ops::get(state, key)?
            .as_array()
            .expect("layout::import checked it is an array")
            .to_vec())
    };
    out.extend(items("hubs")?.into_iter().map(|s| Effect::OpenHub(Some(s))));
    out.extend(items("satellites")?.into_iter().map(Effect::OpenSatellite));
    Ok(())
}
