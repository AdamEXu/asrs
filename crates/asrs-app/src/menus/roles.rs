//! Electron's `Menu.buildFromTemplate` and `MenuItem` constructor (Electron 39,
//! `lib/browser/api/menu.ts`, `menu-item.ts` and `menu-item-roles.ts`): the menu Electron makes
//! of a template, with the labels, types, accelerators and submenus it fills in for roles.

use asrs_services::window_manager::Platform;

use super::model::{Command, ItemType, MenuItem};

/// A `MenuItem` as Electron holds it after construction.
#[derive(Clone, Debug, PartialEq)]
pub struct BuiltItem {
    pub label: String,
    pub role: Option<String>,
    pub item_type: ItemType,
    pub accelerator: Option<String>,
    pub default_role_accelerator: Option<String>,
    pub checked: bool,
    pub enabled: bool,
    pub visible: bool,
    pub register_accelerator: bool,
    pub accelerator_works_when_hidden: bool,
    pub icon: Option<[u8; 3]>,
    pub command: Option<Command>,
    pub submenu: Option<Vec<BuiltItem>>,
}

impl BuiltItem {
    /// `Menu.prototype._getAcceleratorForCommandId`.
    pub fn accelerator_for(&self, use_default_accelerator: bool) -> Option<&str> {
        match &self.accelerator {
            Some(accelerator) => Some(accelerator),
            None if use_default_accelerator => self.default_role_accelerator.as_deref(),
            None => None,
        }
    }
}

struct Role {
    label: String,
    accelerator: Option<&'static str>,
    register_accelerator: Option<bool>,
    has_checked: bool,
    submenu: Option<Vec<MenuItem>>,
}

fn role(name: &str, platform: Platform, app_name: &str) -> Option<Role> {
    let mac = platform == Platform::Mac;
    let windows = platform == Platform::Windows;
    let linux = platform == Platform::Linux;
    let plain = |label: &str| Role {
        label: label.to_owned(),
        accelerator: None,
        register_accelerator: None,
        has_checked: false,
        submenu: None,
    };
    let keyed = |label: &str, accelerator: &'static str| Role {
        accelerator: Some(accelerator),
        ..plain(label)
    };
    let menu = |label: &str, items: Vec<MenuItem>| Role {
        submenu: Some(items),
        ..plain(label)
    };
    let clipboard = |label: &str, accelerator: &'static str| Role {
        register_accelerator: Some(false),
        ..keyed(label, accelerator)
    };
    let roles = |names: &[&str]| -> Vec<MenuItem> {
        names
            .iter()
            .map(|&n| {
                if n == "-" {
                    MenuItem::separator()
                } else {
                    MenuItem::role(n)
                }
            })
            .collect()
    };
    Some(match name {
        "about" => plain(&if linux {
            "About".to_owned()
        } else {
            format!("About {app_name}")
        }),
        "close" => keyed(
            if mac { "Close Window" } else { "Close" },
            "CommandOrControl+W",
        ),
        "copy" => clipboard("Copy", "CommandOrControl+C"),
        "cut" => clipboard("Cut", "CommandOrControl+X"),
        "delete" => plain("Delete"),
        "forcereload" => keyed("Force Reload", "Shift+CmdOrCtrl+R"),
        "front" => plain("Bring All to Front"),
        "help" => plain("Help"),
        "hide" => keyed(&format!("Hide {app_name}"), "Command+H"),
        "hideothers" => keyed("Hide Others", "Command+Alt+H"),
        "minimize" => keyed("Minimize", "CommandOrControl+M"),
        "paste" => clipboard("Paste", "CommandOrControl+V"),
        "pasteandmatchstyle" => clipboard(
            "Paste and Match Style",
            if mac {
                "Cmd+Option+Shift+V"
            } else {
                "Shift+CommandOrControl+V"
            },
        ),
        "quit" => Role {
            accelerator: (!windows).then_some("CommandOrControl+Q"),
            ..plain(&match platform {
                Platform::Mac => format!("Quit {app_name}"),
                Platform::Windows => "Exit".to_owned(),
                Platform::Linux => "Quit".to_owned(),
            })
        },
        "redo" => keyed(
            "Redo",
            if windows {
                "Control+Y"
            } else {
                "Shift+CommandOrControl+Z"
            },
        ),
        "reload" => keyed("Reload", "CmdOrCtrl+R"),
        "resetzoom" => keyed("Actual Size", "CommandOrControl+0"),
        "selectall" => keyed("Select All", "CommandOrControl+A"),
        "services" => plain("Services"),
        "recentdocuments" => plain("Open Recent"),
        "clearrecentdocuments" => plain("Clear Menu"),
        "showsubstitutions" => plain("Show Substitutions"),
        "togglesmartquotes" => plain("Smart Quotes"),
        "togglesmartdashes" => plain("Smart Dashes"),
        "toggletextreplacement" => plain("Text Replacement"),
        "startspeaking" => plain("Start Speaking"),
        "stopspeaking" => plain("Stop Speaking"),
        "toggledevtools" => keyed(
            "Toggle Developer Tools",
            if mac { "Alt+Command+I" } else { "Ctrl+Shift+I" },
        ),
        "togglefullscreen" => keyed(
            "Toggle Full Screen",
            if mac { "Control+Command+F" } else { "F11" },
        ),
        "undo" => keyed("Undo", "CommandOrControl+Z"),
        "unhide" => plain("Show All"),
        "window" => plain("Window"),
        "zoom" => plain("Zoom"),
        "zoomin" => keyed("Zoom In", "CommandOrControl+Plus"),
        "zoomout" => keyed("Zoom Out", "CommandOrControl+-"),
        "togglespellchecker" => Role {
            has_checked: true,
            ..plain("Check Spelling While Typing")
        },
        "appmenu" => menu(
            app_name,
            roles(&[
                "about",
                "-",
                "services",
                "-",
                "hide",
                "hideOthers",
                "unhide",
                "-",
                "quit",
            ]),
        ),
        "filemenu" => menu("File", roles(&[if mac { "close" } else { "quit" }])),
        "editmenu" => {
            let mut items = roles(&["undo", "redo", "-", "cut", "copy", "paste"]);
            if mac {
                items.extend(roles(&["pasteAndMatchStyle", "delete", "selectAll", "-"]));
                items.push(MenuItem::new("Substitutions").submenu(roles(&[
                    "showSubstitutions",
                    "-",
                    "toggleSmartQuotes",
                    "toggleSmartDashes",
                    "toggleTextReplacement",
                ])));
                items.push(
                    MenuItem::new("Speech").submenu(roles(&["startSpeaking", "stopSpeaking"])),
                );
            } else {
                items.extend(roles(&["delete", "-", "selectAll"]));
            }
            menu("Edit", items)
        }
        "viewmenu" => menu(
            "View",
            roles(&[
                "reload",
                "forceReload",
                "toggleDevTools",
                "-",
                "resetZoom",
                "zoomIn",
                "zoomOut",
                "-",
                "togglefullscreen",
            ]),
        ),
        "windowmenu" => {
            let mut items = roles(&["minimize", "zoom"]);
            items.extend(roles(if mac { &["-", "front"] } else { &["close"] }));
            menu("Window", items)
        }
        "sharemenu" => menu("Share", Vec::new()),
        _ => return None,
    })
}

/// `Menu.buildFromTemplate(template)`, for templates without `before`/`after` positioning.
pub fn build_from_template(
    template: &[MenuItem],
    platform: Platform,
    app_name: &str,
) -> Vec<BuiltItem> {
    for item in template {
        assert!(
            item.label.is_some()
                || item.role.is_some()
                || item.item_type == Some(ItemType::Separator),
            "Invalid template for MenuItem: must have at least one of label, role or type"
        );
    }
    remove_extra_separators(template)
        .into_iter()
        .map(|item| new_item(item, platform, app_name))
        .collect()
}

/// `new Menu()` followed by `menu.append(new MenuItem(options))` for each item: unlike
/// `buildFromTemplate`, appending folds no separators at this level.
pub fn build_appended(items: &[MenuItem], platform: Platform, app_name: &str) -> Vec<BuiltItem> {
    items
        .iter()
        .map(|item| new_item(item, platform, app_name))
        .collect()
}

fn is_separator(item: &MenuItem) -> bool {
    item.item_type == Some(ItemType::Separator)
}

/// `removeExtraSeparators`: folds adjacent separators, then drops edge separators, leaving
/// invisible items alone.
fn remove_extra_separators(items: &[MenuItem]) -> Vec<&MenuItem> {
    let folded: Vec<&MenuItem> = items
        .iter()
        .enumerate()
        .filter(|&(i, e)| {
            e.visible == Some(false) || !is_separator(e) || i == 0 || !is_separator(&items[i - 1])
        })
        .map(|(_, e)| e)
        .collect();
    let last = folded.len().wrapping_sub(1);
    folded
        .iter()
        .enumerate()
        .filter(|&(i, e)| e.visible == Some(false) || !is_separator(e) || (i != 0 && i != last))
        .map(|(_, e)| *e)
        .collect()
}

fn new_item(item: &MenuItem, platform: Platform, app_name: &str) -> BuiltItem {
    let role_name = item.role.as_ref().map(|r| r.to_lowercase());
    let role = role_name
        .as_deref()
        .and_then(|r| role(r, platform, app_name));
    let submenu_template = item
        .submenu
        .clone()
        .or_else(|| role.as_ref().and_then(|r| r.submenu.clone()));
    let submenu = submenu_template.map(|items| build_from_template(&items, platform, app_name));
    let mut item_type = item.item_type;
    if item_type.is_none() && submenu.is_some() {
        item_type = Some(ItemType::Submenu);
    }
    assert!(
        item_type != Some(ItemType::Submenu) || submenu.is_some(),
        "Invalid submenu"
    );
    let default_type = if role.as_ref().is_some_and(|r| r.has_checked) {
        ItemType::Checkbox
    } else {
        ItemType::Normal
    };
    BuiltItem {
        label: item
            .label
            .clone()
            .unwrap_or_else(|| role.as_ref().map(|r| r.label.clone()).unwrap_or_default()),
        item_type: item_type.unwrap_or(default_type),
        accelerator: item.accelerator.clone(),
        default_role_accelerator: role.as_ref().and_then(|r| r.accelerator.map(str::to_owned)),
        checked: item.checked.unwrap_or(false),
        enabled: item.enabled.unwrap_or(true),
        visible: item.visible.unwrap_or(true),
        register_accelerator: role
            .as_ref()
            .and_then(|r| r.register_accelerator)
            .unwrap_or(true),
        accelerator_works_when_hidden: true,
        icon: item.icon,
        command: item.command,
        role: role_name,
        submenu,
    }
}
