//! Shows the dialogs main answers hub messages with, on the window that sent the message.

use asrs_js::time::{Clock, NamedZone, SystemClock};
use asrs_js::value::JsValue;
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_services::fs::RealFs;
use asrs_services::hub_dialogs;
use asrs_ui::gpui::{AnyWindowHandle, App, Window};

use crate::menus::dispatch::{message_box_options, save_dialog_options};
use crate::windows;

/// `handleHubMessage`'s `error` and `export-console`; other messages show no dialog here.
pub fn show(message: &NamedMessage<HubToMain>, window: AnyWindowHandle, cx: &mut App) {
    match message.name {
        HubToMain::Error => {
            let options = match hub_dialogs::error(&message.data) {
                Ok(options) => options,
                Err(e) => return eprintln!("asrs: error: {}: {}", e.name, e.message),
            };
            let mut options = message_box_options(&options);
            // Off macOS the platform dialogs answer for the user when they have no button.
            if !cfg!(target_os = "macos") {
                options.buttons.push("OK".to_owned());
            }
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
