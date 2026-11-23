//! `main.ts`'s prompt windows: `createEditRangeWindow`, `createUnitConversionWindow`,
//! `createRenameTabWindow`, `createEditFovWindow` and the window of `createExportWindow`.

mod layout;
pub mod window;

pub use window::PromptWindow;

use std::cell::Cell;

use asrs_js::ops;
use asrs_js::value::JsValue;
use asrs_tabs::prompts::{
    EditFov, EditRange, Export, PromptEvent, PromptPage, RenameTab, UnitConversion,
};
use asrs_ui::gpui::{App, AppContext};

use crate::menus::context::Prompt;
use crate::windows::{self, MessageBoxOptions, Opening, WindowId, WindowKind, Windows};

/// The page a prompt window loads.
pub fn page(kind: WindowKind) -> Box<dyn PromptPage> {
    match kind {
        WindowKind::EditRange => Box::<EditRange>::default(),
        WindowKind::UnitConversion => Box::<UnitConversion>::default(),
        WindowKind::RenameTab => Box::<RenameTab>::default(),
        WindowKind::EditFov => Box::<EditFov>::default(),
        WindowKind::Export => Box::<Export>::default(),
        other => panic!("{other:?} is not a prompt"),
    }
}

/// Opens a modal prompt on `parent` and posts `message` to its page. Each value the page posts
/// back goes to `on_message`, and its alerts show as message boxes on the prompt, as Chromium's
/// do.
pub fn open(
    cx: &mut App,
    kind: WindowKind,
    parent: WindowId,
    message: &JsValue,
    on_message: impl Fn(JsValue, WindowId, &mut App) + 'static,
) -> Opening<PromptWindow> {
    let page = page(kind);
    let title = format!("{} \u{2014} {}", page.title(), crate::IDENTITY.name);
    let (id, view) = windows::open_modal(cx, kind, parent, |window, cx| {
        window.set_window_title(&title);
        cx.new(|cx| PromptWindow::new(page, window, cx))
    })?;
    cx.subscribe(&view, move |_, event: &PromptEvent, cx| match event {
        PromptEvent::Reply(value) => on_message(value.clone(), id, cx),
        PromptEvent::Alert(message) => alert(message.to_string_lossy(), id, cx),
        PromptEvent::Error(error) => eprintln!("asrs: {title}: {error:?}"),
        PromptEvent::Select(_) => {}
    })
    .detach();
    if let Some(handle) = Windows::handle(cx, id) {
        handle.update(cx, |_, window, cx| {
            view.update(cx, |view, cx| view.message(message, window, cx));
        })?;
    }
    Ok((id, view))
}

/// `window.alert(message)` in Electron: a message box with only an OK button.
fn alert(message: String, id: WindowId, cx: &mut App) {
    if let Some(handle) = Windows::handle(cx, id) {
        let options = MessageBoxOptions {
            message,
            buttons: vec!["OK".to_owned()],
            default_id: Some(0),
            ..MessageBoxOptions::default()
        };
        let _ = handle.update(cx, |_, window, cx| {
            windows::message_box(Some(window), cx, options, |_, _| {});
        });
    }
}

/// Opens the prompt a context menu item asks for. Its first reply closes it and goes to
/// `on_reply`.
pub fn ask(
    cx: &mut App,
    parent: WindowId,
    prompt: &Prompt,
    on_reply: impl FnOnce(JsValue, &mut App) + 'static,
) -> Opening<PromptWindow> {
    let on_reply = Cell::new(Some(on_reply));
    open(
        cx,
        prompt.window(),
        parent,
        &prompt.initial(),
        move |value, id, cx| {
            if let Some(on_reply) = on_reply.take() {
                windows::close(cx, id);
                on_reply(value, cx);
            }
        },
    )
}

/// The window of `createExportWindow`. `on_message` gets `null` from the exit button, the help
/// URL, and the `ExportOptions` confirming posts; closing the window is the caller's.
pub fn open_export(
    cx: &mut App,
    parent: WindowId,
    supports_akit: bool,
    on_message: impl Fn(JsValue, WindowId, &mut App) + 'static,
) -> Opening<PromptWindow> {
    let message = ops::object([("supportsAkit", JsValue::Bool(supports_akit))]);
    open(cx, WindowKind::Export, parent, &message, on_message)
}
