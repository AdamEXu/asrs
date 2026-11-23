//! Runs `main.ts`'s export flow on the app's windows: the messages a hub sends main, the export
//! prompt, its dialogs and the file.

use std::cell::RefCell;
use std::rc::Rc;

use asrs_js::ops;
use asrs_js::string::JsError;
use asrs_js::value::JsValue;
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_services::export_flow::{self, Effect, ExportPrompt};
use asrs_services::fs::RealFs;
use asrs_ui::gpui::{App, AppContext, Subscription};

use crate::main_process::MainProcess;
use crate::menus::dispatch;
use crate::views::prompts;
use crate::windows::{self, WindowId, Windows};

type Flow = Rc<RefCell<ExportPrompt>>;

/// `prompt-export` and `write-export` from a hub window.
pub(crate) fn hub_message(hub: WindowId, message: &NamedMessage<HubToMain>, cx: &mut App) {
    match message.name {
        HubToMain::PromptExport => {
            let effect = export_flow::prompt_export(&message.data, crate::IDENTITY);
            start(hub, &message.data, effect, cx);
        }
        HubToMain::WriteExport => write(hub, &message.data, cx),
        _ => {}
    }
}

/// The effects of a request, up to its prompt.
fn start(hub: WindowId, request: &JsValue, effect: Result<Effect, JsError>, cx: &mut App) {
    match effect {
        Ok(Effect::Warn(options)) => {
            let options = dispatch::message_box_options(&options);
            let request = request.clone();
            dispatch::show_message_box(Some(hub), options, cx, move |response, cx| {
                start(hub, &request, export_flow::warned(&request, response), cx);
            });
        }
        Ok(Effect::OpenPrompt {
            supports_akit,
            path,
        }) => open_prompt(hub, ops::to_boolean(&supports_akit), path, cx),
        Ok(Effect::Send { name, data }) => MainProcess::send_to_hub(cx, hub, name, data),
        Ok(other) => unreachable!("{other:?} needs a prompt"),
        Err(e) => eprintln!("asrs: prompt-export: {e:?}"),
    }
}

fn open_prompt(hub: WindowId, supports_akit: bool, path: JsValue, cx: &mut App) {
    let flow: Flow = Rc::new(RefCell::new(ExportPrompt::new(path)));
    let opened = prompts::open_export(cx, hub, supports_akit, {
        let flow = flow.clone();
        move |data, prompt, cx| {
            let effects = flow.borrow_mut().message(&data);
            match effects {
                Ok(effects) => perform(hub, prompt, &flow, effects, cx),
                Err(e) => eprintln!("asrs: export prompt: {e:?}"),
            }
        }
    });
    match opened {
        Ok((prompt, _)) => on_closed(hub, prompt, flow, cx),
        Err(e) => {
            eprintln!("asrs: opening the export prompt: {e}");
            let effect = flow.borrow_mut().closed();
            perform(hub, hub, &flow, effect, cx);
        }
    }
}

fn on_closed(hub: WindowId, prompt: WindowId, flow: Flow, cx: &mut App) {
    let Some(window) = Windows::handle(cx, prompt).map(|handle| handle.window_id()) else {
        return;
    };
    let subscription: Rc<RefCell<Option<Subscription>>> = Rc::default();
    let held = subscription.clone();
    *subscription.borrow_mut() = Some(cx.on_window_closed(move |cx, closed| {
        if closed == window {
            held.borrow_mut().take();
            let effect = flow.borrow_mut().closed();
            perform(hub, prompt, &flow, effect, cx);
        }
    }));
}

fn perform(
    hub: WindowId,
    prompt: WindowId,
    flow: &Flow,
    effects: impl IntoIterator<Item = Effect>,
    cx: &mut App,
) {
    for effect in effects {
        match effect {
            Effect::Send { name, data } => MainProcess::send_to_hub(cx, hub, name, data),
            Effect::DestroyPrompt => windows::close(cx, prompt),
            Effect::OpenExternal(url) => cx.open_url(&url.to_string_lossy()),
            Effect::Save { options, export } => {
                let options = dispatch::save_dialog_options(&options);
                let flow = flow.clone();
                dispatch::with_window(Some(prompt), cx, |window, cx| {
                    windows::save_dialog(window, cx, options, move |path, cx| {
                        let path = path.map(|p| p.to_string_lossy().into_owned());
                        let effects = flow.borrow_mut().saved(export, path.as_deref());
                        perform(hub, prompt, &flow, effects, cx);
                    });
                });
            }
            Effect::Warn(_) | Effect::OpenPrompt { .. } => {
                unreachable!("{effect:?} comes before the prompt")
            }
        }
    }
}

fn write(hub: WindowId, data: &JsValue, cx: &mut App) {
    let (path, content) = match export_flow::export_file(data) {
        Ok(file) => file,
        Err(e) => return eprintln!("asrs: write-export: {e:?}"),
    };
    let written = cx.background_spawn(async move {
        export_flow::write_export(&RealFs, &path, &content).map_err(|e| (e.name, e.message))
    });
    cx.spawn(async move |cx| {
        let written = written.await;
        cx.update(|cx| match written {
            Ok(Effect::Send { name, data }) => MainProcess::send_to_hub(cx, hub, name, data),
            Ok(other) => unreachable!("{other:?}"),
            Err((name, message)) => {
                eprintln!("asrs: write-export: {name}: {message}");
                dispatch::show_error("The export failed", &format!("{name}: {message}"), cx);
            }
        });
    })
    .detach();
}

/// The hub's `close` event: whether it may close now. A hub in the middle of an export asks
/// first, and closes itself on "Close".
pub(crate) fn may_close(hub: WindowId, cx: &mut App) -> bool {
    if !MainProcess::is_exporting(cx, hub) {
        return true;
    }
    let options = dispatch::message_box_options(&export_flow::closing_warning());
    dispatch::show_message_box(Some(hub), options, cx, move |response, cx| {
        if response != 0 {
            windows::close(cx, hub);
        }
    });
    false
}
