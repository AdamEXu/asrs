#![allow(clippy::disallowed_methods)]

//! Installs upstream's darwin application menu through the bridge and reads the menu bar back
//! from AppKit: every item's title, key equivalent, modifier mask, state, enabled and hidden
//! flags, and its role wiring (selector or system menu) must be what Electron's
//! `ElectronMenuController` makes of the same built menu. Then chooses entries through AppKit
//! and checks that the command hooks receive them, including the hidden "New Tab (Shortcut)"
//! through its key equivalent.

use std::cell::RefCell;
use std::rc::Rc;

use asrs_app::menus::accelerator::Accelerator;
use asrs_app::menus::app_menu;
use asrs_app::menus::bridge::{APP_NAME, MenuAction, install_application_menu};
use asrs_app::menus::model::{Command, ItemType};
use asrs_app::menus::roles::{BuiltItem, build_from_template};
use asrs_model::preferences::{Preferences, SystemcoreStaticAddress};
use asrs_services::window_manager::Platform;
use asrs_ui::gpui::{App, AsyncApp};
use asrs_ui::platform::probe::{
    perform_application_menu_item, perform_key_equivalent, read_application_menu,
};
use asrs_ui::platform::{PopupItem, PopupMenu, native_menu_label};

/// `kRolesMap` in Electron 39's `electron_menu_controller.mm`, the roles that become AppKit
/// selectors.
const SELECTOR_ROLES: &[&str] = &[
    "about",
    "hide",
    "hideothers",
    "unhide",
    "front",
    "undo",
    "redo",
    "cut",
    "copy",
    "paste",
    "delete",
    "pasteandmatchstyle",
    "selectall",
    "showsubstitutions",
    "togglesmartquotes",
    "togglesmartdashes",
    "toggletextreplacement",
    "startspeaking",
    "stopspeaking",
    "minimize",
    "close",
    "zoom",
    "quit",
    "togglefullscreen",
    "toggletabbar",
    "selectnexttab",
    "showalltabs",
    "selectprevioustab",
    "mergeallwindows",
    "movetabtonewwindow",
    "clearrecentdocuments",
];

fn expected_role(item: &BuiltItem) -> Option<String> {
    let role = item.role.as_deref()?;
    let wired = match (role, &item.submenu) {
        ("services", _) => true,
        ("window" | "windowmenu", Some(submenu)) => !submenu.is_empty(),
        ("help", Some(_)) => true,
        (_, Some(_)) => false,
        (role, None) => SELECTOR_ROLES.contains(&role),
    };
    wired.then(|| if role == "window" { "windowmenu" } else { role }.to_owned())
}

const MAC_FUNCTION: u32 = 1 << 23;

/// AppKit's globe-key full screen shortcut: it moves an item that toggles full screen to Fn+F
/// and keeps the item's own shortcut on a hidden alternate right after it. It does this to any
/// menu, Electron's included, so the read-back folds the pair back into the item it came from.
fn fold_full_screen(read: &PopupMenu) -> PopupMenu {
    let mut items: Vec<PopupItem> = Vec::new();
    for item in &read.items {
        if let (Some(PopupItem::Entry(previous)), PopupItem::Entry(alternate)) =
            (items.last_mut(), item)
            && previous.role.as_deref() == Some("togglefullscreen")
            && previous.key_equivalent == Some(("f".to_owned(), MAC_FUNCTION))
            && alternate.role == previous.role
            && alternate.label == previous.label
            && !alternate.visible
        {
            previous.key_equivalent = alternate.key_equivalent.clone();
            continue;
        }
        items.push(item.clone());
    }
    PopupMenu { items }
}

fn compare(read: &PopupMenu, built: &[BuiltItem], at: &str, compared: &mut usize) {
    let read = fold_full_screen(read);
    assert_eq!(read.items.len(), built.len(), "{at}: item count");
    for (i, (read, item)) in read.items.iter().zip(built).enumerate() {
        let at = format!("{at}/{i} {:?}", item.label);
        *compared += 1;
        let entry = match read {
            PopupItem::Separator => {
                assert_eq!(item.item_type, ItemType::Separator, "{at}: separator");
                continue;
            }
            PopupItem::Entry(entry) => entry,
        };
        assert_ne!(item.item_type, ItemType::Separator, "{at}");
        assert_eq!(entry.label, native_menu_label(&item.label), "{at}: title");
        assert_eq!(entry.role, expected_role(item), "{at}: role wiring");
        assert_eq!(entry.swatch, None, "{at}: image");
        if item.role.as_deref() == Some("services") {
            assert_eq!(entry.submenu, Some(PopupMenu::new()), "{at}: services menu");
            continue;
        }
        assert_eq!(entry.enabled, item.enabled, "{at}: enabled");
        assert_eq!(entry.visible, item.visible, "{at}: hidden");
        assert_eq!(entry.checked, item.checked, "{at}: state");
        match &item.submenu {
            Some(submenu) => {
                assert_eq!(entry.key_equivalent, None, "{at}: key equivalent");
                let read = entry.submenu.as_ref().expect("a submenu");
                compare(read, submenu, &at, compared);
            }
            None => {
                let want = item
                    .accelerator_for(true)
                    .map(|a| Accelerator::parse(a, Platform::Mac).expect("a valid accelerator"))
                    .and_then(|a| a.mac_key_equivalent());
                assert_eq!(entry.key_equivalent, want, "{at}: key equivalent");
                assert_eq!(
                    entry.works_when_hidden, item.accelerator_works_when_hidden,
                    "{at}: works when hidden"
                );
                assert_eq!(entry.submenu, None, "{at}");
            }
        }
    }
}

fn install(
    cx: &mut App,
    prefs: &Preferences,
    received: &Rc<RefCell<Vec<MenuAction>>>,
) -> Vec<BuiltItem> {
    let mut template = app_menu::template(Platform::Mac, prefs);
    app_menu::apply_preferences(&mut template, prefs);
    let sink = received.clone();
    install_application_menu(
        cx,
        Platform::Mac,
        &template,
        Rc::new(move |action, _| sink.borrow_mut().push(action)),
    );
    build_from_template(&template, Platform::Mac, APP_NAME)
}

fn verify(built: &[BuiltItem]) -> usize {
    let (read, added) = read_application_menu().expect("a main menu");
    println!("app menu mac: AppKit added {added:?}");
    let mut compared = 0;
    compare(&read, built, "menu", &mut compared);
    compared
}

async fn settle(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(std::time::Duration::from_millis(100))
        .await;
}

fn main() {
    if asrs_parity::ci::skip_in_ci("app menu mac", "a display") {
        return;
    }
    if !cfg!(target_os = "macos") {
        println!("app menu mac: skipped on this platform");
        return;
    }
    asrs_ui::gpui::application().run(|cx: &mut App| {
        asrs_ui::init(cx);
        let received = Rc::new(RefCell::new(Vec::new()));
        cx.spawn(async move |cx| {
            let built = cx.update(|cx| install(cx, &Preferences::default(), &received));
            settle(cx).await;
            let mut compared = verify(&built);
            let prefs = Preferences {
                systemcore_static_address: SystemcoreStaticAddress::Wifi,
                user_assets_folder: Some("/custom".into()),
                ..Preferences::default()
            };
            let built = cx.update(|cx| install(cx, &prefs, &received));
            settle(cx).await;
            compared += verify(&built);
            assert!(compared > 2 * 120, "{compared} items compared");

            assert!(
                perform_application_menu_item(&[1, 0]),
                "File > Open Log(s)..."
            );
            assert!(
                perform_application_menu_item(&[1, 2, 4]),
                "Connect to Robot > Systemcore"
            );
            assert!(
                perform_key_equivalent("t", 1 << 20),
                "Cmd+T reaches the hidden item"
            );
            assert!(
                perform_key_equivalent("+", 1 << 20),
                "Cmd+Plus reaches Zoom In"
            );
            settle(cx).await;
            let got = received.borrow().clone();
            let want = vec![
                MenuAction::Command(Command::OpenLogs),
                MenuAction::Command(Command::StartLive {
                    simulator: false,
                    mode: Some(asrs_model::preferences::LiveMode::Nt4Systemcore),
                }),
                MenuAction::Command(Command::NewTabPopup),
                MenuAction::Role("zoomin".to_owned()),
            ];
            let ok = got == want;
            println!("app menu mac: {compared} items equal, dispatch {got:?}");
            cx.update(|cx| cx.quit());
            std::process::exit(i32::from(!ok));
        })
        .detach();
    });
}
