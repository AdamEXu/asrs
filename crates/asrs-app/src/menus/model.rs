use asrs_model::preferences::{LiveMode, SystemcoreStaticAddress};
use asrs_model::tab_type::TabType;

/// Electron's `MenuItem.types`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemType {
    Normal,
    Separator,
    Submenu,
    Checkbox,
    Radio,
}

impl ItemType {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemType::Normal => "normal",
            ItemType::Separator => "separator",
            ItemType::Submenu => "submenu",
            ItemType::Checkbox => "checkbox",
            ItemType::Radio => "radio",
        }
    }

    pub fn parse(value: &str) -> Option<ItemType> {
        [
            ItemType::Normal,
            ItemType::Separator,
            ItemType::Submenu,
            ItemType::Checkbox,
            ItemType::Radio,
        ]
        .into_iter()
        .find(|t| t.as_str() == value)
    }
}

/// What a `click` handler in upstream's menu templates does. The handlers themselves live with
/// the app's command dispatch; the menu only names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    About,
    OpenPreferences,
    CheckForUpdates,
    OpenLicenses,
    ShowAssetsFolder,
    UseCustomAssetsFolder,
    AssetDownloadStatus,
    OwletDownloadStatus,
    OpenLogs,
    AddLogs,
    StartLive {
        simulator: bool,
        mode: Option<LiveMode>,
    },
    ConnectDriverStation,
    DownloadLogs,
    UseSystemcoreAddress(SystemcoreStaticAddress),
    ExportData,
    StartPublish {
        simulator: bool,
    },
    StopPublish,
    ExportLayout,
    ImportLayout,
    ResetToDsLayout,
    NewWindow,
    CloseWindow,
    ZoomToEnabledRange,
    ToggleSidebar,
    ToggleControls,
    NewTab(TabType),
    NewTabPopup,
    NewPopOut,
    MoveTab(i8),
    ShiftTab(i8),
    CloseTab,
    BringAllToFront,
    Help(HelpLink),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelpLink {
    ReportProblem,
    ContactUs,
    GitHubRepository,
    WpilibDocumentation,
    LittletonRobotics,
}

/// One entry of an Electron menu template (`MenuItemConstructorOptions`), as far as upstream
/// uses the options. `None` is an option the template leaves out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuItem {
    pub role: Option<String>,
    pub label: Option<String>,
    pub item_type: Option<ItemType>,
    pub accelerator: Option<String>,
    pub checked: Option<bool>,
    pub enabled: Option<bool>,
    pub visible: Option<bool>,
    pub command: Option<Command>,
    pub submenu: Option<Vec<MenuItem>>,
}

impl MenuItem {
    pub fn new(label: impl Into<String>) -> Self {
        MenuItem {
            label: Some(label.into()),
            ..MenuItem::default()
        }
    }

    pub fn separator() -> Self {
        MenuItem {
            item_type: Some(ItemType::Separator),
            ..MenuItem::default()
        }
    }

    pub fn role(role: &str) -> Self {
        MenuItem {
            role: Some(role.to_owned()),
            ..MenuItem::default()
        }
    }

    pub fn command(mut self, command: Command) -> Self {
        self.command = Some(command);
        self
    }

    pub fn accelerator(mut self, accelerator: &str) -> Self {
        self.accelerator = Some(accelerator.to_owned());
        self
    }

    pub fn item_type(mut self, item_type: ItemType) -> Self {
        self.item_type = Some(item_type);
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    pub fn visible(mut self, visible: bool) -> Self {
        self.visible = Some(visible);
        self
    }

    pub fn submenu(mut self, items: Vec<MenuItem>) -> Self {
        self.submenu = Some(items);
        self
    }

    /// The item at `path`, one index per menu level.
    pub fn at<'a>(items: &'a [MenuItem], path: &[usize]) -> Option<&'a MenuItem> {
        let (&first, rest) = path.split_first()?;
        let item = items.get(first)?;
        if rest.is_empty() {
            Some(item)
        } else {
            MenuItem::at(item.submenu.as_deref()?, rest)
        }
    }

    pub fn at_mut<'a>(items: &'a mut [MenuItem], path: &[usize]) -> Option<&'a mut MenuItem> {
        let (&first, rest) = path.split_first()?;
        let item = items.get_mut(first)?;
        if rest.is_empty() {
            Some(item)
        } else {
            MenuItem::at_mut(item.submenu.as_deref_mut()?, rest)
        }
    }
}
