//! Runs the application menu's clicks in the app: [`click`]'s effects against the main process,
//! the windows and the platform's dialogs, and the roles Electron executes itself.
//!
//! [`open_window`], [`check_for_update`], [`open_satellite`] and
//! [`MainProcess::open_files`] are where the windows and services of other units plug in. A
//! command without one yet says so in a message box.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use asrs_js::string::JsError;
use asrs_js::value::JsValue;
use asrs_services::dialog::{self as services, MessageBox};
use asrs_services::window_manager::{Platform, WindowId, WindowKind};
use asrs_ui::gpui::{App, ClipboardItem, Window};

use super::bridge::{self, MenuAction};
use super::click::{self, ClickEnv, Effect, Failure, Reply, Then};
use super::context;
use super::model::Command;
use crate::export_flow;
use crate::main_process::MainProcess;
use crate::windows::{
    self, FileFilter, MessageBoxOptions, MessageBoxType, OpenDialogOptions, SaveDialogOptions,
    Windows,
};

/// What choosing an application menu entry does.
pub fn dispatch(action: MenuAction, cx: &mut App) {
    match action {
        MenuAction::Command { command, checked } => {
            let window = Windows::manager(cx).focused();
            run(cx, command_label(command), |env, out| {
                click::click(command, checked, window, env, out)
            });
        }
        MenuAction::Role(role) => execute_role(&role, cx),
    }
}

fn command_label(command: Command) -> String {
    format!("{command:?}")
}

fn run(
    cx: &mut App,
    what: String,
    handler: impl FnOnce(&ClickEnv<'_>, &mut Vec<Effect>) -> Result<(), Failure>,
) {
    let mut effects = Vec::new();
    let Some(result) = MainProcess::with_click_env(cx, |env| handler(env, &mut effects)) else {
        return;
    };
    let mut missing = Vec::new();
    for effect in effects {
        if let Err(Unavailable(name)) = perform(effect, &what, cx)
            && !missing.contains(&name)
        {
            missing.push(name);
        }
    }
    for name in missing {
        show_unavailable(name, cx);
    }
    match result {
        Ok(()) => {}
        Err(Failure::Throw(e)) => {
            eprintln!("asrs: {what}: {e:?}");
            show_failure(&e, cx);
        }
        Err(Failure::Rejection(e)) => eprintln!("asrs: {what}: {e:?}"),
    }
}

/// A command whose window or service this build does not have.
struct Unavailable(&'static str);

fn perform(effect: Effect, what: &str, cx: &mut App) -> Result<(), Unavailable> {
    match effect {
        Effect::Send { window, name, data } => MainProcess::send_to_hub(cx, window, name, data),
        Effect::SendAllPreferences => MainProcess::send_all_preferences(cx),
        Effect::ReloadAssets => MainProcess::reload_assets(cx),
        Effect::MessageBox {
            window,
            sync: _,
            options,
            then,
        } => {
            let what = what.to_owned();
            show_message_box(
                window,
                message_box_options(&options),
                cx,
                move |reply, cx| {
                    resume(cx, what, then, Reply::Box(reply));
                },
            );
        }
        Effect::OpenDialog {
            window,
            options,
            then,
        } => {
            let what = what.to_owned();
            let options = open_dialog_options(&options);
            let done = move |paths: Vec<PathBuf>, cx: &mut App| {
                let paths = paths.iter().map(|p| path_text(p)).collect();
                resume(cx, what, then, Reply::Open(paths));
            };
            with_window(window, cx, |window, cx| {
                windows::open_dialog(window, cx, options, done);
            });
        }
        Effect::SaveDialog {
            window,
            options,
            then,
        } => {
            let what = what.to_owned();
            let options = save_dialog_options(&options);
            let done = move |path: Option<PathBuf>, cx: &mut App| {
                resume(cx, what, then, Reply::Save(path.map(|p| path_text(&p))));
            };
            with_window(window, cx, |window, cx| {
                windows::save_dialog(window, cx, options, done);
            });
        }
        Effect::OpenFilesSmart { files, window } => {
            MainProcess::open_files(cx, files, Some(window));
        }
        Effect::CheckForUpdate => return check_for_update(),
        Effect::OpenHub(state) => {
            MainProcess::open_hub(cx, state.as_ref(), Vec::new());
        }
        Effect::OpenSatellite(_) => return open_satellite(),
        Effect::OpenWindow { kind, parent } => return open_window(kind, parent, cx),
        Effect::NewTabPopup { window, rect } => new_tab_popup(window, &rect, cx),
        Effect::Close(window) => {
            if export_flow::may_close(window, cx) {
                windows::close(cx, window);
            }
        }
        Effect::Destroy(window) => windows::close(cx, window),
        Effect::MoveTop(window) => {
            if let Some(handle) = Windows::handle(cx, window) {
                let _ = handle.update(cx, |_, window, _| window.activate_window());
            }
        }
        Effect::OpenPath(path) => match path.as_str() {
            Some(path) => cx.open_with_system(Path::new(&path.to_string_lossy())),
            None => eprintln!("asrs: shell.openPath: not a string: {path:?}"),
        },
        Effect::OpenExternal(url) => cx.open_url(&url),
        Effect::WriteClipboard(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
    }
    Ok(())
}

fn resume(cx: &mut App, what: String, then: Then, reply: Reply) {
    run(cx, what, |env, out| click::resume(then, &reply, env, out));
}

/// `openPreferences`, `openDownload` and `openLicenses`.
fn open_window(kind: WindowKind, parent: WindowId, cx: &mut App) -> Result<(), Unavailable> {
    Err(Unavailable(match kind {
        WindowKind::Preferences => {
            if let Err(e) = MainProcess::open_preferences(cx, parent) {
                show_failure(&e, cx);
            }
            return Ok(());
        }
        WindowKind::Download => "Downloading logs",
        WindowKind::Licenses => "The licenses window",
        other => panic!("{other:?} does not open from the application menu"),
    }))
}

/// `checkForUpdate(true)`.
fn check_for_update() -> Result<(), Unavailable> {
    Err(Unavailable("Checking for updates"))
}

/// `createSatellite({ state })`.
fn open_satellite() -> Result<(), Unavailable> {
    Err(Unavailable("Opening pop-out windows"))
}

/// `newTabPopup(window, rect)`.
fn new_tab_popup(window: WindowId, rect: &JsValue, cx: &mut App) {
    let menu = match context::new_tab(rect) {
        Ok(menu) => menu,
        Err(e) => {
            eprintln!("asrs: new tab menu: {e:?}");
            return;
        }
    };
    let Some(handle) = Windows::handle(cx, window) else {
        return;
    };
    let on_effects: bridge::EffectHandler = Rc::new(move |effects, cx| {
        for message in effects.iter().filter_map(context::Effect::to_hub) {
            MainProcess::send_to_hub(cx, window, message.name, message.data);
        }
    });
    let _ = handle.update(cx, |_, window, cx| {
        bridge::popup_context(window, cx, Platform::current(), &menu, on_effects)
    });
}

/// `roles.execute` for the roles that reach the app. On macOS that leaves out every role with an
/// AppKit selector.
fn execute_role(role: &str, cx: &mut App) {
    let focused = Windows::focused(cx);
    let on_window = |cx: &mut App, f: fn(&Window)| {
        if let Some(handle) = focused {
            let _ = handle.update(cx, |_, window, _| f(window));
        }
    };
    match role {
        "minimize" => on_window(cx, Window::minimize_window),
        "togglefullscreen" => on_window(cx, Window::toggle_fullscreen),
        "quit" => cx.quit(),
        // A focused text field takes these keystrokes before the accelerator table sees them,
        // and with none focused Chromium's editing commands do nothing.
        "undo" | "redo" | "cut" | "copy" | "paste" | "pasteandmatchstyle" | "delete"
        | "selectall" => {}
        "reload" | "forcereload" => show_unavailable("Reloading the window", cx),
        "toggledevtools" => show_unavailable("Developer tools", cx),
        "resetzoom" | "zoomin" | "zoomout" => show_unavailable("Zooming the window", cx),
        other => show_unavailable_role(other, cx),
    }
}

fn show_unavailable_role(role: &str, cx: &mut App) {
    eprintln!("asrs: menu role {role} is not handled");
    show_unavailable("This command", cx);
}

fn show_unavailable(what: &'static str, cx: &mut App) {
    eprintln!("asrs: {what}: not available yet");
    show_error(
        "Not available yet",
        &format!(
            "{what} is not available in this version of {}.",
            crate::IDENTITY.name
        ),
        cx,
    );
}

fn show_failure(e: &JsError, cx: &mut App) {
    show_error(
        "The command failed",
        &format!("{}: {}", e.name, e.message),
        cx,
    );
}

pub(crate) fn show_error(message: &str, detail: &str, cx: &mut App) {
    let options = MessageBoxOptions {
        kind: MessageBoxType::Error,
        title: "Error".to_owned(),
        message: message.to_owned(),
        detail: detail.to_owned(),
        ..MessageBoxOptions::default()
    };
    let window = Windows::manager(cx).focused();
    show_message_box(window, options, cx, |_, _| {});
}

/// Off macOS the platform dialogs answer for the user when they have no window or no button, so
/// a box without them goes on the focused or most recent hub window with Electron's default "OK".
pub(crate) fn show_message_box(
    window: Option<WindowId>,
    mut options: MessageBoxOptions,
    cx: &mut App,
    on_done: impl FnOnce(usize, &mut App) + 'static,
) {
    let mut window = window;
    if !cfg!(target_os = "macos") {
        let manager = Windows::manager(cx);
        window = window
            .or_else(|| manager.focused())
            .or_else(|| manager.hubs().first().copied());
        if options.buttons.is_empty() {
            options.buttons.push("OK".to_owned());
        }
    }
    with_window(window, cx, |window, cx| {
        windows::message_box(window, cx, options, move |result, cx| {
            on_done(result.response, cx);
        });
    });
}

pub(crate) fn with_window(
    window: Option<WindowId>,
    cx: &mut App,
    show: impl FnOnce(Option<&mut Window>, &mut App),
) {
    match window.and_then(|id| Windows::handle(cx, id)) {
        Some(handle) => {
            if let Err(e) = handle.update(cx, |_, window, cx| show(Some(window), cx)) {
                eprintln!("asrs: dialog: {e}");
            }
        }
        None => show(None, cx),
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(crate) fn message_box_options(options: &MessageBox) -> MessageBoxOptions {
    let index = |id: Option<f64>| id.map(|id| id as usize);
    MessageBoxOptions {
        kind: match options.kind {
            "info" => MessageBoxType::Info,
            "warning" => MessageBoxType::Warning,
            "error" => MessageBoxType::Error,
            "question" => MessageBoxType::Question,
            _ => MessageBoxType::None,
        },
        title: options.title.to_owned(),
        message: options.message.to_string_lossy(),
        detail: options.detail.to_string_lossy(),
        buttons: options
            .buttons
            .iter()
            .flatten()
            .map(|b| (*b).to_owned())
            .collect(),
        default_id: index(options.default_id),
        cancel_id: index(options.cancel_id),
        ..MessageBoxOptions::default()
    }
}

fn file_filter(filter: &services::FileFilter) -> FileFilter {
    FileFilter {
        name: filter.name.to_owned(),
        extensions: filter.extensions.iter().map(|e| (*e).to_owned()).collect(),
    }
}

fn open_dialog_options(options: &services::OpenDialog) -> OpenDialogOptions {
    let has = |property| options.properties.contains(&property);
    OpenDialogOptions {
        title: options.title.clone(),
        message: options.message.unwrap_or("").to_owned(),
        button_label: options.button_label.unwrap_or("").to_owned(),
        default_path: None,
        filters: options.filters.iter().map(file_filter).collect(),
        open_file: has("openFile"),
        open_directory: has("openDirectory"),
        multi_selections: has("multiSelections"),
        create_directory: has("createDirectory"),
    }
}

pub(crate) fn save_dialog_options(options: &services::SaveDialog) -> SaveDialogOptions {
    SaveDialogOptions {
        title: options.title.to_owned(),
        message: String::new(),
        button_label: String::new(),
        default_path: options.default_path.as_ref().map(PathBuf::from),
        filters: vec![file_filter(&options.filters)],
        create_directory: options.properties.contains(&"createDirectory"),
    }
}
