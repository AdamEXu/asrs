//! From menu templates to what each platform shows: the macOS menu bar, the accelerator table
//! that replaces it on Windows and Linux, and context menus.

use std::rc::Rc;

use asrs_services::window_manager::Platform;
use asrs_ui::gpui::{App, Global, Window, point, px};
use asrs_ui::menu::{CloseHandler, MenuView};
use asrs_ui::platform::{self, MenuPath, PopupEntry, PopupMenu};

use super::accelerator::Accelerator;
use super::context::{ContextMenu, Effect};
use super::model::{Command, ItemType, MenuItem};
use super::roles::{BuiltItem, build_appended, build_from_template};

/// `app.name`, which Electron puts into role labels such as "Hide AdvantageScope".
pub const APP_NAME: &str = crate::IDENTITY.name;

/// What choosing a menu entry asks for: a template's command, or a role Electron executes
/// itself (reload, zoom and the like) that has no native selector.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    Command {
        command: Command,
        /// `item.checked` as the click handler sees it.
        checked: bool,
    },
    Role(String),
}

pub type ActionHandler = Rc<dyn Fn(MenuAction, &mut App)>;

pub type EffectHandler = Rc<dyn Fn(Vec<Effect>, &mut App)>;

/// The native menu for built items. Key equivalents (macOS) and shortcut texts (elsewhere) come
/// from the item's accelerator or, with `use_default_accelerator`, its role's default.
pub fn to_popup(
    items: &[BuiltItem],
    platform: Platform,
    use_default_accelerator: bool,
) -> PopupMenu {
    let mut menu = PopupMenu::new();
    for item in items {
        if item.item_type == ItemType::Separator {
            menu = menu.separator();
            continue;
        }
        let mut entry = PopupEntry::new(item.label.clone())
            .enabled(item.enabled)
            .visible(item.visible)
            .checked(item.checked)
            .works_when_hidden(item.accelerator_works_when_hidden);
        if let Some(role) = &item.role {
            entry = entry.role(role.clone());
        }
        if let Some(rgb) = item.icon {
            entry = entry.swatch(rgb);
        }
        let accelerator = item
            .accelerator_for(use_default_accelerator)
            .filter(|_| item.submenu.is_none())
            .and_then(|a| Accelerator::parse(a, platform));
        if let Some(accelerator) = accelerator {
            if platform == Platform::Mac {
                if let Some((key, modifiers)) = accelerator.mac_key_equivalent() {
                    entry = entry.key_equivalent(key, modifiers);
                }
            } else if let Some(shortcut) = accelerator.shortcut_text() {
                entry = entry.shortcut(shortcut);
            }
        }
        if let Some(submenu) = &item.submenu {
            entry = entry.submenu(to_popup(submenu, platform, use_default_accelerator));
        }
        menu = menu.entry(entry);
    }
    menu
}

pub fn item_at<'a>(items: &'a [BuiltItem], path: &[usize]) -> Option<&'a BuiltItem> {
    let (&first, rest) = path.split_first()?;
    let item = items.get(first)?;
    if rest.is_empty() {
        Some(item)
    } else {
        item_at(item.submenu.as_deref()?, rest)
    }
}

pub fn action_at(items: &[BuiltItem], path: &[usize]) -> Option<MenuAction> {
    let item = item_at(items, path)?;
    match (&item.command, &item.role) {
        (Some(command), _) => Some(MenuAction::Command {
            command: *command,
            checked: item.checked,
        }),
        (None, Some(role)) => Some(MenuAction::Role(role.clone())),
        (None, None) => None,
    }
}

/// The `MenuItem` constructor's click wrapper: a checkbox flips before its handler runs. Returns
/// whether it did.
fn flip_checkbox(items: &mut [BuiltItem], path: &[usize]) -> bool {
    let Some((&first, rest)) = path.split_first() else {
        return false;
    };
    let Some(item) = items.get_mut(first) else {
        return false;
    };
    match (&mut item.submenu, rest.is_empty()) {
        (Some(submenu), false) => flip_checkbox(submenu, rest),
        (_, true) if item.item_type == ItemType::Checkbox => {
            item.checked = !item.checked;
            true
        }
        _ => false,
    }
}

/// `accelerator_util::GenerateAcceleratorTable`: every non-submenu item that registers its
/// accelerator (its own or its role's default), as a GPUI keystroke. Later items win.
pub fn accelerator_table(items: &[BuiltItem], platform: Platform) -> Vec<(String, MenuPath)> {
    let mut table: Vec<(String, MenuPath)> = Vec::new();
    collect_accelerators(items, platform, &mut Vec::new(), &mut table);
    table
}

fn collect_accelerators(
    items: &[BuiltItem],
    platform: Platform,
    prefix: &mut MenuPath,
    table: &mut Vec<(String, MenuPath)>,
) {
    for (index, item) in items.iter().enumerate() {
        prefix.push(index);
        if let Some(submenu) = &item.submenu {
            collect_accelerators(submenu, platform, prefix, table);
        } else if item.register_accelerator
            && let Some(keystroke) = item
                .accelerator_for(true)
                .and_then(|a| Accelerator::parse(a, platform))
                .and_then(|a| a.gpui_keystroke())
        {
            table.retain(|(k, _)| *k != keystroke);
            table.push((keystroke, prefix.clone()));
        }
        prefix.pop();
    }
}

type PathHandler = Rc<dyn Fn(MenuPath, &mut App)>;

/// `Menu.getApplicationMenu()` and what its clicks run.
struct ApplicationMenu {
    platform: Platform,
    items: Vec<BuiltItem>,
    dispatch: PathHandler,
}

impl Global for ApplicationMenu {}

/// `Menu.getApplicationMenu().items[index].submenu`, for `open-app-menu`.
fn application_submenu(cx: &App, index: usize) -> Option<PopupMenu> {
    let menu = cx.try_global::<ApplicationMenu>()?;
    let submenu = menu.items.get(index)?.submenu.as_deref()?;
    Some(to_popup(submenu, menu.platform, true))
}

/// `open-app-menu`'s `submenu.popup({ window, x, y })`.
pub fn popup_application_submenu(window: &mut Window, cx: &mut App, index: usize, x: i32, y: i32) {
    let Some(menu) = application_submenu(cx, index) else {
        return;
    };
    show_popup(window, cx, menu, x, y, move |path, cx| {
        if let Some(path) = path {
            let mut full = vec![index];
            full.extend(path);
            let dispatch = cx.global::<ApplicationMenu>().dispatch.clone();
            dispatch(full, cx);
        }
    });
}

/// Installs `template` as the application menu: the menu bar on macOS, the accelerator table
/// elsewhere. Call again with the rewritten template whenever upstream rebuilds its menu.
pub fn install_application_menu(
    cx: &mut App,
    platform: Platform,
    template: &[MenuItem],
    on_action: ActionHandler,
) {
    show_application_menu(
        cx,
        platform,
        build_from_template(template, platform, APP_NAME),
        on_action,
    );
}

fn show_application_menu(
    cx: &mut App,
    platform: Platform,
    built: Vec<BuiltItem>,
    on_action: ActionHandler,
) {
    let menu = (platform == Platform::Mac).then(|| to_popup(&built, platform, true));
    let table = accelerator_table(&built, platform);
    let shown = built.clone();
    let dispatch: PathHandler = Rc::new(move |path, cx| {
        if !item_at(&built, &path).is_some_and(|item| item.enabled) {
            return;
        }
        let mut clicked = built.clone();
        let flipped = flip_checkbox(&mut clicked, &path);
        let action = action_at(&clicked, &path);
        if flipped {
            show_application_menu(cx, platform, clicked, on_action.clone());
        }
        if let Some(action) = action {
            on_action(action, cx);
        }
    });
    cx.set_global(ApplicationMenu {
        platform,
        items: shown,
        dispatch: dispatch.clone(),
    });
    match menu {
        Some(menu) => {
            platform::set_application_menu(cx, &menu, move |path, cx| dispatch(path, cx));
        }
        None => {
            // The keystroke's window is borrowed until its event ends, and the handlers reach
            // windows by handle.
            asrs_ui::menu::install_accelerators(cx, table, move |path, _, cx| {
                let dispatch = dispatch.clone();
                cx.defer(move |cx| dispatch(path, cx));
            });
        }
    }
}

/// `menu.popup({ window, x, y })` for a context menu template: native on macOS, drawn in windows
/// of its own on Linux, and on Windows until its native backend exists. `x` and `y` are window content
/// coordinates.
pub fn popup_context_menu(
    window: &mut Window,
    cx: &mut App,
    platform: Platform,
    template: &[MenuItem],
    x: i32,
    y: i32,
    on_action: ActionHandler,
) {
    let built = Rc::new(build_from_template(template, platform, APP_NAME));
    let menu = to_popup(&built, platform, false);
    let choose = move |path: Option<MenuPath>, cx: &mut App| {
        if let Some(action) = path.and_then(|path| action_at(&built, &path)) {
            on_action(action, cx);
        }
    };
    show_popup(window, cx, menu, x, y, choose);
}

/// The context menu that popped up last, in parity scenarios, which click its items themselves
/// instead of showing it.
#[cfg(feature = "parity")]
#[derive(Default)]
pub struct ScriptedMenu(Option<(ContextMenu, EffectHandler)>);

#[cfg(feature = "parity")]
impl Global for ScriptedMenu {}

#[cfg(feature = "parity")]
impl ScriptedMenu {
    /// Clicks the item at `path` of the menu that popped up.
    pub fn click(cx: &mut App, path: &[String]) -> Result<(), String> {
        let (menu, on_effects) = cx
            .global_mut::<ScriptedMenu>()
            .0
            .take()
            .ok_or("no menu popped up")?;
        let mut items = menu.items.as_slice();
        let mut command = None;
        for label in path {
            let item = items
                .iter()
                .find(|i| i.label.as_deref() == Some(label))
                .ok_or_else(|| format!("no menu item {label:?}"))?;
            if item.enabled == Some(false) {
                return Err(format!("menu item {label:?} is disabled"));
            }
            command = item.command;
            items = item.submenu.as_deref().unwrap_or_default();
        }
        let effects = command
            .and_then(|command| menu.effects(command))
            .ok_or_else(|| format!("{path:?} is not a clickable item"))?;
        on_effects(effects.to_vec(), cx);
        Ok(())
    }
}

/// Pops up a menu `main.ts` built with `new Menu()` and `append`, at its position or, without
/// one, at the mouse. The chosen item's effects go to `on_effects`.
pub fn popup_context(
    window: &mut Window,
    cx: &mut App,
    platform: Platform,
    menu: &ContextMenu,
    on_effects: EffectHandler,
) {
    #[cfg(feature = "parity")]
    if cx.has_global::<ScriptedMenu>() {
        cx.global_mut::<ScriptedMenu>().0 = Some((menu.clone(), on_effects));
        return;
    }
    let built = build_appended(&menu.items, platform, APP_NAME);
    let popup = to_popup(&built, platform, false);
    let (x, y) = menu
        .position
        .map_or((-1, -1), |(x, y)| (x as i32, y as i32));
    let menu = menu.clone();
    let choose = move |path: Option<MenuPath>, cx: &mut App| {
        if let Some(command) = path
            .and_then(|path| item_at(&built, &path))
            .and_then(|item| item.command)
            && let Some(effects) = menu.effects(command)
        {
            on_effects(effects.to_vec(), cx);
        }
    };
    show_popup(window, cx, popup, x, y, choose);
}

/// A menu of plain items popped up at window content coordinates. `on_close` gets the chosen
/// item's index.
pub fn popup_labels(
    window: &mut Window,
    cx: &mut App,
    platform: Platform,
    labels: Vec<String>,
    x: i32,
    y: i32,
    on_close: impl FnOnce(Option<usize>, &mut App) + 'static,
) {
    let items: Vec<MenuItem> = labels.into_iter().map(MenuItem::new).collect();
    let popup = to_popup(&build_appended(&items, platform, APP_NAME), platform, false);
    show_popup(window, cx, popup, x, y, move |path, cx| {
        on_close(path.and_then(|path| path.first().copied()), cx)
    });
}

fn show_popup(
    window: &mut Window,
    cx: &mut App,
    menu: PopupMenu,
    x: i32,
    y: i32,
    choose: impl FnOnce(Option<MenuPath>, &mut App) + 'static,
) {
    if !cfg!(windows) {
        platform::popup_menu(window, cx, menu, x, y, choose);
    } else {
        let origin = window.bounds().origin + point(px(x as f32), px(y as f32));
        let choose = std::cell::Cell::new(Some(choose));
        let on_close: CloseHandler = Rc::new(move |path, _, cx| {
            if let Some(choose) = choose.take() {
                choose(path, cx);
            }
        });
        MenuView::open_window(menu, origin, window.scale_factor(), cx, on_close);
    }
}
