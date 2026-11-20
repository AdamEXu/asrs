//! `src/shared/NamedMessage.ts` and the message names each window kind exchanges with main
//! (`handleHubMessage`, `handleSatelliteMessage` and the download handler in
//! `src/main/electron/main.ts`; `handleMainMessage` in `src/hub/hub.ts`; the receivers in
//! `src/satellite.ts` and `src/download.ts`). XR and Lite messages are out of scope.

use asrs_js::value::JsValue;

/// `{ name, data }`. `data` is `undefined` for messages sent without one.
#[derive(Clone, Debug, PartialEq)]
pub struct NamedMessage<N> {
    pub name: N,
    pub data: JsValue,
}

impl<N> NamedMessage<N> {
    pub fn new(name: N, data: JsValue) -> Self {
        NamedMessage { name, data }
    }

    pub fn without_data(name: N) -> Self {
        NamedMessage {
            name,
            data: JsValue::Undefined,
        }
    }
}

macro_rules! message_names {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn name(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }

            pub fn from_name(name: &str) -> Option<$name> {
                match name {
                    $($text => Some($name::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

message_names!(HubToMain {
    Show = "show",
    Alert = "alert",
    OpenFilesRequest = "open-files-request",
    Error = "error",
    SaveState = "save-state",
    SaveTypeMemory = "save-type-memory",
    UpdatePreferences = "update-preferences",
    PromptUpdate = "prompt-update",
    OpenFeedback = "open-feedback",
    HistoricalStartRaw = "historical-start-raw",
    HistoricalStart = "historical-start",
    FrcCoordinateSystemWarning = "frc-coordinate-system-warning",
    FtcCoordinateSystemWarning = "ftc-coordinate-system-warning",
    LiveRlogStart = "live-rlog-start",
    LiveRlogStop = "live-rlog-stop",
    OpenLink = "open-link",
    AskOpenSidebarContextMenu = "ask-open-sidebar-context-menu",
    AskOpenTuningContextMenu = "ask-open-tuning-context-menu",
    OpenAppMenu = "open-app-menu",
    CloseAppMenu = "close-app-menu",
    AskPlaybackOptions = "ask-playback-options",
    AskNewTab = "ask-new-tab",
    SourceListTypePrompt = "source-list-type-prompt",
    SourceListClearPrompt = "source-list-clear-prompt",
    SourceListHelp = "source-list-help",
    AskEditAxis = "ask-edit-axis",
    AskRenameTab = "ask-rename-tab",
    CreateSatellite = "create-satellite",
    UpdateSatellite = "update-satellite",
    Ask3dCamera = "ask-3d-camera",
    Ask2dCamera = "ask-2d-camera",
    ExportConsole = "export-console",
    PromptExport = "prompt-export",
    WriteExport = "write-export",
    SetExporting = "set-exporting",
    SelectVideo = "select-video",
    UpdateTouchBarSlider = "update-touch-bar-slider",
});

message_names!(MainToHub {
    ShowWhenReady = "show-when-ready",
    RestoreState = "restore-state",
    RestoreTypeMemory = "restore-type-memory",
    SetFullscreen = "set-fullscreen",
    SetFocused = "set-focused",
    SetBattery = "set-battery",
    SetVersion = "set-version",
    SetPreferences = "set-preferences",
    SetAssets = "set-assets",
    SetActiveSatellites = "set-active-satellites",
    CallSelectionSetter = "call-selection-setter",
    Play = "play",
    Pause = "pause",
    Lock = "lock",
    Unlock = "unlock",
    ZoomEnabled = "zoom-enabled",
    ShowUpdateButton = "show-update-button",
    ShowFeedbackButton = "show-feedback-button",
    HistoricalData = "historical-data",
    LiveData = "live-data",
    OpenFiles = "open-files",
    StartLive = "start-live",
    StartPublish = "start-publish",
    StopPublish = "stop-publish",
    SetPlaybackOptions = "set-playback-options",
    ToggleSidebar = "toggle-sidebar",
    ToggleControls = "toggle-controls",
    NewTab = "new-tab",
    NewSatellite = "new-satellite",
    MoveTab = "move-tab",
    ShiftTab = "shift-tab",
    CloseTab = "close-tab",
    RenameTab = "rename-tab",
    SourceListTypeResponse = "source-list-type-response",
    SourceListClearResponse = "source-list-clear-response",
    SetRobotModeVisible = "set-robot-mode-visible",
    EditAxis = "edit-axis",
    ClearAxis = "clear-axis",
    Set3dCamera = "set-3d-camera",
    Set2dCamera = "set-2d-camera",
    EditFov = "edit-fov",
    AddTableRange = "add-table-range",
    VideoData = "video-data",
    StartExport = "start-export",
    CancelExport = "cancel-export",
    PrepareExport = "prepare-export",
    FinishExport = "finish-export",
    UpdateTouchBarSlider = "update-touch-bar-slider",
});

message_names!(SatelliteToMain {
    SetAspectRatio = "set-aspect-ratio",
    Ask3dCamera = "ask-3d-camera",
    Ask2dCamera = "ask-2d-camera",
    AddTableRange = "add-table-range",
    SaveState = "save-state",
    UpdatePreferences = "update-preferences",
    CallSelectionSetter = "call-selection-setter",
    OpenLink = "open-link",
});

message_names!(MainToSatellite {
    SetAssets = "set-assets",
    SetPreferences = "set-preferences",
    SetBattery = "set-battery",
    SetType = "set-type",
    RestoreState = "restore-state",
    Render = "render",
    Set3dCamera = "set-3d-camera",
    Set2dCamera = "set-2d-camera",
    EditFov = "edit-fov",
});

message_names!(DownloadToMain {
    Start = "start",
    Close = "close",
    Save = "save",
});

message_names!(MainToDownload {
    SetPlatform = "set-platform",
    SetPreferences = "set-preferences",
    SetFocused = "set-focused",
    ShowError = "show-error",
    ShowAlert = "show-alert",
    SetProgress = "set-progress",
    SetList = "set-list",
});
