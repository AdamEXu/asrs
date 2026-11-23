//! The application menu template of `setupMenu` in `src/main/electron/main.ts`, and the in-place
//! rewrite `sendAllPreferences` applies to it.

use asrs_model::app_identity::AppIdentity;
use asrs_model::preferences::{LiveMode, Preferences, SystemcoreStaticAddress};
use asrs_model::tab_type::TabType;
use asrs_services::window_manager::Platform;

use super::model::{Command, HelpLink, ItemType, MenuItem};

const LIVE_MODES: [LiveMode; 6] = [
    LiveMode::Nt4,
    LiveMode::Nt4AKit,
    LiveMode::Nt4Systemcore,
    LiveMode::Phoenix,
    LiveMode::Rlog,
    LiveMode::FtcDashboard,
];

/// `setupMenu()`. `prefs` is the preferences file as it reads at startup; the FRC 6328
/// distribution is the one upstream ships with "Check for Updates...". `identity` names the app
/// in "About".
pub fn template(platform: Platform, prefs: &Preferences, identity: AppIdentity) -> Vec<MenuItem> {
    let mac = platform == Platform::Mac;
    vec![
        app(mac, identity),
        file(prefs),
        MenuItem::role("editMenu"),
        view(),
        tabs(),
        window(mac),
        help(),
    ]
}

/// Takes out "Check for Updates...", for a build that never checks.
pub fn remove_update_check(menu: &mut [MenuItem]) {
    for item in menu {
        if let Some(submenu) = &mut item.submenu {
            submenu.retain(|item| item.command != Some(Command::CheckForUpdates));
        }
    }
}

fn app(mac: bool, identity: AppIdentity) -> MenuItem {
    let mut items = vec![MenuItem::new(format!("About {}", identity.name)).command(Command::About)];
    if mac {
        items.push(MenuItem::separator());
    }
    items.extend([
        MenuItem::new(if mac {
            "Settings..."
        } else {
            "Show Preferences..."
        })
        .accelerator("CmdOrCtrl+,")
        .command(Command::OpenPreferences),
        MenuItem::new("Check for Updates...").command(Command::CheckForUpdates),
        MenuItem::new("Show Licenses...").command(Command::OpenLicenses),
        MenuItem::separator(),
        MenuItem::new("Show Assets Folder").command(Command::ShowAssetsFolder),
        MenuItem::new("Use Custom Assets Folder")
            .item_type(ItemType::Checkbox)
            .command(Command::UseCustomAssetsFolder),
        MenuItem::new("Asset Download Status...").command(Command::AssetDownloadStatus),
        MenuItem::new("Owlet Download Status...").command(Command::OwletDownloadStatus),
    ]);
    if mac {
        items.extend([
            MenuItem::separator(),
            MenuItem::role("services"),
            MenuItem::separator(),
            MenuItem::role("hide"),
            MenuItem::role("hideOthers"),
            MenuItem::role("unhide"),
            MenuItem::separator(),
            MenuItem::role("quit"),
        ]);
    }
    MenuItem {
        role: mac.then(|| "appMenu".to_owned()),
        label: Some(if mac { "" } else { "App" }.to_owned()),
        submenu: Some(items),
        ..MenuItem::default()
    }
}

fn connect(simulator: bool, accelerator: &str) -> MenuItem {
    let mut items = vec![
        MenuItem::new("Default")
            .accelerator(accelerator)
            .command(Command::StartLive {
                simulator,
                mode: None,
            }),
        MenuItem::separator(),
    ];
    items.extend(LIVE_MODES.map(|mode| {
        MenuItem::new(mode.display_name()).command(Command::StartLive {
            simulator,
            mode: Some(mode),
        })
    }));
    MenuItem::new(if simulator {
        "Connect to Simulator"
    } else {
        "Connect to Robot"
    })
    .item_type(ItemType::Submenu)
    .submenu(items)
}

fn file(prefs: &Preferences) -> MenuItem {
    let address = prefs.systemcore_static_address;
    MenuItem::new("File").submenu(vec![
        MenuItem::new("Open Log(s)...")
            .accelerator("CmdOrCtrl+O")
            .command(Command::OpenLogs),
        MenuItem::new("Add New Log(s)...")
            .accelerator("CmdOrCtrl+Shift+O")
            .command(Command::AddLogs),
        connect(false, "CmdOrCtrl+K"),
        connect(true, "CmdOrCtrl+Shift+K"),
        MenuItem::new("Connect to Driver Station")
            .accelerator("CmdOrCtrl+Alt+Shift+K")
            .command(Command::ConnectDriverStation),
        MenuItem::new("Download Logs...")
            .accelerator("CmdOrCtrl+D")
            .command(Command::DownloadLogs),
        MenuItem::separator(),
        MenuItem::new("Use Systemcore USB Address")
            .item_type(ItemType::Checkbox)
            .checked(address == SystemcoreStaticAddress::Usb)
            .command(Command::UseSystemcoreAddress(SystemcoreStaticAddress::Usb)),
        MenuItem::new("Use Systemcore Wi-Fi Address")
            .item_type(ItemType::Checkbox)
            .checked(address == SystemcoreStaticAddress::Wifi)
            .command(Command::UseSystemcoreAddress(SystemcoreStaticAddress::Wifi)),
        MenuItem::separator(),
        MenuItem::new("Export Data...")
            .accelerator("CmdOrCtrl+E")
            .command(Command::ExportData),
        MenuItem::new("Publish NT Data").submenu(vec![
            MenuItem::new("Connect to Robot")
                .accelerator("CmdOrCtrl+P")
                .command(Command::StartPublish { simulator: false }),
            MenuItem::new("Connect to Simulator")
                .accelerator("CmdOrCtrl+Shift+P")
                .command(Command::StartPublish { simulator: true }),
            MenuItem::new("Stop Publishing")
                .accelerator("CmdOrCtrl+Alt+P")
                .command(Command::StopPublish),
        ]),
        MenuItem::separator(),
        MenuItem::new("Export Layout...").command(Command::ExportLayout),
        MenuItem::new("Import Layout...").command(Command::ImportLayout),
        MenuItem::new("Reset to DS Layout").command(Command::ResetToDsLayout),
        MenuItem::separator(),
        MenuItem::new("New Window")
            .accelerator("CommandOrControl+N")
            .command(Command::NewWindow),
        MenuItem::new("Close Window")
            .accelerator("Shift+CmdOrCtrl+W")
            .command(Command::CloseWindow),
    ])
}

fn view() -> MenuItem {
    MenuItem {
        role: Some("viewMenu".to_owned()),
        submenu: Some(vec![
            MenuItem::role("reload"),
            MenuItem::role("toggleDevTools"),
            MenuItem::separator(),
            MenuItem::role("resetZoom"),
            MenuItem::role("zoomIn"),
            MenuItem::role("zoomOut"),
            MenuItem::separator(),
            MenuItem::new("Zoom to Enabled Range")
                .accelerator("CmdOrCtrl+\\")
                .command(Command::ZoomToEnabledRange),
            MenuItem::separator(),
            MenuItem::new("Toggle Sidebar")
                .accelerator("CmdOrCtrl+.")
                .command(Command::ToggleSidebar),
            MenuItem::new("Toggle Controls")
                .accelerator("CmdOrCtrl+/")
                .command(Command::ToggleControls),
            MenuItem::role("togglefullscreen"),
        ]),
        ..MenuItem::default()
    }
}

fn tabs() -> MenuItem {
    let new_tab = TabType::ALL[1..]
        .iter()
        .map(|&tab| {
            MenuItem::new(format!("{} {}", tab.icon(), tab.default_title()))
                .accelerator(tab.accelerator())
                .command(Command::NewTab(tab))
        })
        .collect();
    MenuItem::new("Tabs").submenu(vec![
        MenuItem::new("New Tab").submenu(new_tab),
        MenuItem::new("New Tab (Shortcut)")
            .visible(false)
            .accelerator("CmdOrCtrl+T")
            .command(Command::NewTabPopup),
        MenuItem::new("New Pop-Out")
            .accelerator("CmdOrCtrl+Shift+T")
            .command(Command::NewPopOut),
        MenuItem::separator(),
        MenuItem::new("Previous Tab")
            .accelerator("CmdOrCtrl+Left")
            .command(Command::MoveTab(-1)),
        MenuItem::new("Next Tab")
            .accelerator("CmdOrCtrl+Right")
            .command(Command::MoveTab(1)),
        MenuItem::separator(),
        MenuItem::new("Shift Left")
            .accelerator("CmdOrCtrl+[")
            .command(Command::ShiftTab(-1)),
        MenuItem::new("Shift Right")
            .accelerator("CmdOrCtrl+]")
            .command(Command::ShiftTab(1)),
        MenuItem::separator(),
        MenuItem::new("Close Tab")
            .accelerator("CmdOrCtrl+W")
            .command(Command::CloseTab),
    ])
}

fn window(mac: bool) -> MenuItem {
    if mac {
        return MenuItem::role("windowMenu");
    }
    MenuItem::new("Window").submenu(vec![
        MenuItem::role("minimize"),
        MenuItem::new("Bring All to Front")
            .accelerator("Ctrl+B")
            .command(Command::BringAllToFront),
        MenuItem::separator(),
    ])
}

fn help() -> MenuItem {
    let link = |label: &str, link| MenuItem::new(label).command(Command::Help(link));
    MenuItem {
        role: Some("help".to_owned()),
        submenu: Some(vec![
            link("Report a Problem", HelpLink::ReportProblem),
            link("Contact Us", HelpLink::ContactUs),
            link("GitHub Repository", HelpLink::GitHubRepository),
            link("WPILib Documentation", HelpLink::WpilibDocumentation),
            link("Littleton Robotics", HelpLink::LittletonRobotics),
        ]),
        ..MenuItem::default()
    }
}

/// The rewrite of the template in `sendAllPreferences()`. It writes by fixed index, so on
/// Windows and Linux, whose App menu has no separator after "About", the custom assets check
/// lands on "Asset Download Status..." (upstream main.ts:187).
pub fn apply_preferences(template: &mut [MenuItem], prefs: &Preferences) {
    let default = format!("Default: {}", prefs.live_mode.display_name());
    let mut set = |path: &[usize], f: &dyn Fn(&mut MenuItem)| {
        let item = MenuItem::at_mut(template, path)
            .unwrap_or_else(|| panic!("upstream's menu template has no item at {path:?}"));
        f(item);
    };
    set(&[1, 2, 0], &|item| item.label = Some(default.clone()));
    set(&[1, 3, 0], &|item| item.label = Some(default.clone()));
    set(&[0, 7], &|item| {
        item.checked = Some(prefs.user_assets_folder.is_some())
    });
    set(&[1, 7], &|item| {
        item.checked = Some(prefs.systemcore_static_address == SystemcoreStaticAddress::Usb)
    });
    set(&[1, 8], &|item| {
        item.checked = Some(prefs.systemcore_static_address == SystemcoreStaticAddress::Wifi)
    });
}
