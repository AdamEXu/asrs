//! `main.ts`'s prompt windows: `createEditRangeWindow`, `createUnitConversionWindow`,
//! `createRenameTabWindow`, `createEditFovWindow`, the window of `createExportWindow` and
//! `openPreferences`.

mod layout;
pub mod window;

pub use window::PromptWindow;

use asrs_js::ops;
use asrs_js::value::JsValue;
use asrs_tabs::prompts::{
    EditFov, EditRange, Export, Preferences, PromptEvent, PromptPage, RenameTab, UnitConversion,
};
use asrs_ui::gpui::{App, AppContext, Entity, Window};

use crate::menus::context::Prompt;
use crate::windows::{
    self, MessageBoxOptions, Opening, SingletonOpening, WindowId, WindowKind, Windows,
};

/// The page a prompt window loads.
pub fn page(kind: WindowKind) -> Box<dyn PromptPage> {
    match kind {
        WindowKind::EditRange => Box::<EditRange>::default(),
        WindowKind::UnitConversion => Box::<UnitConversion>::default(),
        WindowKind::RenameTab => Box::<RenameTab>::default(),
        WindowKind::EditFov => Box::<EditFov>::default(),
        WindowKind::Export => Box::<Export>::default(),
        WindowKind::Preferences => Box::<Preferences>::default(),
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
    let (id, view) = windows::open_modal(cx, kind, parent, |window, cx| load(kind, window, cx))?;
    connect(cx, id, &view, on_message);
    post(cx, id, &view, message)?;
    Ok((id, view))
}

fn load(kind: WindowKind, window: &mut Window, cx: &mut App) -> Entity<PromptWindow> {
    let page = page(kind);
    window.set_window_title(&format!(
        "{} \u{2014} {}",
        page.title(),
        crate::IDENTITY.name
    ));
    cx.new(|cx| PromptWindow::new(page, window, cx))
}

fn connect(
    cx: &mut App,
    id: WindowId,
    view: &Entity<PromptWindow>,
    on_message: impl Fn(JsValue, WindowId, &mut App) + 'static,
) {
    cx.subscribe(view, move |view, event: &PromptEvent, cx| match event {
        PromptEvent::Reply(value) => on_message(value.clone(), id, cx),
        PromptEvent::Alert(message) => alert(message.to_string_lossy(), id, cx),
        PromptEvent::Error(error) => {
            eprintln!("asrs: {}: {error:?}", view.read(cx).page().title());
        }
        PromptEvent::Select(_) => {}
    })
    .detach();
}

/// `port2.postMessage(message)`.
pub fn post(
    cx: &mut App,
    id: WindowId,
    view: &Entity<PromptWindow>,
    message: &JsValue,
) -> asrs_ui::gpui::Result<()> {
    if let Some(handle) = Windows::handle(cx, id) {
        handle.update(cx, |_, window, cx| {
            view.update(cx, |view, cx| view.message(message, window, cx));
        })?;
    }
    Ok(())
}

/// The window of `openPreferences`, or `None` when it was open and now has the focus. Its replies
/// close it and go to `on_reply`.
pub fn open_preferences(
    cx: &mut App,
    parent: WindowId,
    on_reply: impl Fn(JsValue, &mut App) + 'static,
) -> SingletonOpening<PromptWindow> {
    let kind = WindowKind::Preferences;
    let opened = windows::open_preferences(cx, parent, |window, cx| load(kind, window, cx))?;
    if let Some((id, view)) = &opened {
        connect(cx, *id, view, close_and(on_reply));
    }
    Ok(opened)
}

/// Closes the prompt and hands its reply to `on_reply`. Enter on a button posts two replies, the
/// window's `keydown` listener's and the button's, and `main.ts` handles both.
fn close_and(
    on_reply: impl Fn(JsValue, &mut App) + 'static,
) -> impl Fn(JsValue, WindowId, &mut App) + 'static {
    move |value, id, cx| {
        windows::close(cx, id);
        on_reply(value, cx);
    }
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

/// Opens the prompt a context menu item asks for. Its replies close it and go to `on_reply`.
pub fn ask(
    cx: &mut App,
    parent: WindowId,
    prompt: &Prompt,
    on_reply: impl Fn(JsValue, &mut App) + 'static,
) -> Opening<PromptWindow> {
    open(
        cx,
        prompt.window(),
        parent,
        &prompt.initial(),
        close_and(on_reply),
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
