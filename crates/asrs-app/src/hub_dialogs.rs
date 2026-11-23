//! Shows the dialogs and opens the windows main answers hub messages with, for the window that
//! sent the message.

use asrs_js::time::{Clock, NamedZone, SystemClock};
use asrs_js::value::JsValue;
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_services::fs::RealFs;
use asrs_services::hub_dialogs;
use asrs_services::update::feedback_url;
use asrs_ui::gpui::{AnyWindowHandle, App, Window};

use crate::main_process::{MainProcess, RELEASE_PROFILE};
use crate::menus::dispatch::{message_box_options, save_dialog_options};
use crate::windows::{self, Windows};

/// `handleHubMessage`'s `error`, `export-console`, `source-list-help`, `prompt-update` and
/// `open-feedback`; other messages show nothing here.
pub fn show(message: &NamedMessage<HubToMain>, window: AnyWindowHandle, cx: &mut App) {
    match message.name {
        HubToMain::Error => {
            let options = match hub_dialogs::error(&message.data) {
                Ok(options) => options,
                Err(e) => return eprintln!("asrs: error: {}: {}", e.name, e.message),
            };
            let options = message_box_options(&options);
            on(window, cx, move |window, cx| {
                windows::message_box(Some(window), cx, options, |_, _| {});
            });
        }
        HubToMain::ExportConsole => {
            let text = match &message.data {
                JsValue::String(text) => text.clone(),
                other => return eprintln!("asrs: export-console: not a string: {other:?}"),
            };
            let options = hub_dialogs::console_export(SystemClock.now(), &NamedZone::system());
            on(window, cx, move |window, cx| {
                windows::save_dialog(
                    Some(window),
                    cx,
                    save_dialog_options(&options),
                    move |path, _| {
                        let Some(path) = path else { return };
                        let path = path.to_string_lossy();
                        if let Err(e) = hub_dialogs::write_console(&RealFs, &path, &text) {
                            eprintln!("asrs: {path}: {}", e.message);
                        }
                    },
                );
            });
        }
        HubToMain::SourceListHelp => {
            let config = message.data.clone();
            cx.defer(move |cx| {
                if let Some(parent) = Windows::id_of(cx, window) {
                    MainProcess::open_source_list_help(cx, parent, &config);
                }
            });
        }
        HubToMain::PromptUpdate => cx.defer(MainProcess::prompt_update),
        HubToMain::OpenFeedback => cx.open_url(&feedback_url(RELEASE_PROFILE.repository)),
        _ => {}
    }
}

fn on(window: AnyWindowHandle, cx: &mut App, show: impl FnOnce(&mut Window, &mut App) + 'static) {
    // The message may come from inside an update of the window.
    cx.defer(move |cx| {
        if let Err(e) = window.update(cx, |_, window, cx| show(window, cx)) {
            eprintln!("asrs: dialog: {e}");
        }
    });
}
