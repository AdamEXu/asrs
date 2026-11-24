//! What a controller that owns `SourceList`s reads besides [`TabCx`]. A list runs with the log,
//! the shared type memory and the theme borrowed while its preview callback reads the selection,
//! so all four come out of the hub in one borrow.

use std::cell::Ref;

use asrs_js::ops;
use asrs_js::string::JsError;
use asrs_js::value::{JsValue, ToJs};
use asrs_log::log::Log;
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::selection::Selection;
use asrs_model::source_list::{
    Env, ListInput, ListOutput, ListPointer, Rect, SourceList, SourceListDom, SourceListError,
};
use asrs_model::source_list_config::SourceListItemState;

use crate::cx::TabCx;

pub struct ListParts<'a> {
    pub log: Ref<'a, Log>,
    /// `window.typeMemory`.
    pub type_memory: &'a mut JsValue,
    pub selection: &'a mut dyn Selection,
    /// `prefers-color-scheme: dark`.
    pub dark: bool,
    /// The lists' layout.
    pub dom: &'a mut dyn SourceListDom,
}

pub trait SourceListCx: TabCx {
    /// `window.matchMedia("(prefers-color-scheme: dark)").matches`.
    fn tab_dark(&self) -> bool;

    fn list_parts(&mut self) -> ListParts<'_>;
}

/// Runs `f` with the `Env` a `SourceList` call needs.
pub fn with_env<C: SourceListCx + ?Sized, R>(cx: &mut C, f: impl FnOnce(&mut Env<'_>) -> R) -> R {
    let parts = cx.list_parts();
    let mut env = Env {
        log: &parts.log,
        type_memory: parts.type_memory,
        dark: parts.dark,
        dom: parts.dom,
    };
    f(&mut env)
}

fn rect_js(rect: Rect) -> JsValue {
    ops::object([
        ("x", JsValue::Number(rect.x)),
        ("y", JsValue::Number(rect.y)),
        ("width", JsValue::Number(rect.width)),
        ("height", JsValue::Number(rect.height)),
    ])
}

/// `getState()` of every list but `index`, which upstream controllers pass each list as its
/// supplemental state suppliers.
fn others(lists: &[&mut SourceList], index: usize) -> Vec<Vec<SourceListItemState>> {
    lists
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != index)
        .map(|(_, list)| list.state().to_vec())
        .collect()
}

/// `input` through a controller's `lists`, in construction order. An exception in one list's
/// listener is reported and the next listener still runs, so all of them come back together.
pub fn route_lists<C: SourceListCx + ?Sized>(
    lists: &mut [&mut SourceList],
    cx: &mut C,
    input: ListInput<'_>,
    errors: &mut Vec<JsError>,
) -> ListOutput {
    let mut out = ListOutput::default();
    let mut report = |result: Result<(), SourceListError>| {
        if let Err(e) = result {
            errors.push(js_error(e));
        }
    };
    match input {
        ListInput::FieldDrag {
            fields,
            children,
            x,
            y,
            end,
        } => {
            for i in 0..lists.len() {
                let supplemental = others(lists, i);
                let supplemental: Vec<&[SourceListItemState]> =
                    supplemental.iter().map(Vec::as_slice).collect();
                let list = &mut *lists[i];
                report(with_env(cx, |env| {
                    list.handle_field_drag(env, &supplemental, fields, children, x, y, end)
                }));
            }
        }
        ListInput::ItemDrag { payload, x, y, end } => {
            let source = lists.iter().position(|l| l.uuid() == payload.uuid);
            for i in 0..lists.len() {
                let result = match source {
                    Some(j) if j != i => {
                        let (target, source) = pair(lists, i, j);
                        with_env(cx, |env| {
                            target.handle_item_drag(env, payload, Some(source), x, y, end)
                        })
                    }
                    _ => with_env(cx, |env| {
                        lists[i].handle_item_drag(env, payload, None, x, y, end)
                    }),
                };
                report(result);
            }
        }
        ListInput::TypeResponse { uuid, state } => {
            if let Some(list) = lists.iter_mut().find(|l| l.uuid() == uuid) {
                report(with_env(cx, |env| list.type_response(env, state.clone())));
            }
        }
        ListInput::ClearResponse { uuid } => {
            if let Some(list) = lists.iter_mut().find(|l| l.uuid() == uuid) {
                list.clear_response();
            }
        }
        ListInput::Frame => {
            for list in lists.iter_mut() {
                let time = if list.state().is_empty() {
                    None
                } else {
                    cx.tab_selection().get_render_time()
                };
                report(with_env(cx, |env| list.frame(env, time, None)));
            }
        }
        ListInput::Pointer { uuid, event } => {
            if let Some(list) = lists.iter_mut().find(|l| l.uuid() == uuid) {
                pointer(list, cx, event, &mut out, &mut report);
            }
        }
    }
    out
}

fn pair<'a>(
    lists: &'a mut [&mut SourceList],
    a: usize,
    b: usize,
) -> (&'a mut SourceList, &'a mut SourceList) {
    if a < b {
        let (lo, hi) = lists.split_at_mut(b);
        (&mut *lo[a], &mut *hi[0])
    } else {
        let (lo, hi) = lists.split_at_mut(a);
        (&mut *hi[0], &mut *lo[b])
    }
}

fn pointer<C: SourceListCx + ?Sized>(
    list: &mut SourceList,
    cx: &mut C,
    event: ListPointer,
    out: &mut ListOutput,
    report: &mut impl FnMut(Result<(), SourceListError>),
) {
    let prompt = match event {
        ListPointer::Down { x, y } => {
            list.mouse_down(x, y);
            None
        }
        ListPointer::Up => {
            list.mouse_up();
            None
        }
        ListPointer::Move { x, y } => {
            match list.mouse_move(cx.list_parts().dom, x, y) {
                Ok(drag) => out.drag = drag,
                Err(e) => report(Err(e)),
            }
            None
        }
        ListPointer::ContextMenu { index, x, y } => list.context_menu(index, x, y),
        ListPointer::TypeClick(index) => list.prompt_type(cx.list_parts().dom, index),
        ListPointer::HideClick(index) => {
            report(with_env(cx, |env| list.toggle_hidden(env, index)));
            None
        }
        ListPointer::NameClick { index, now } => {
            report(with_env(cx, |env| list.name_click(env, index, now)));
            None
        }
        ListPointer::RemoveClick(index) => {
            report(list.remove(index));
            None
        }
        ListPointer::ClearClick => {
            let prompt = list.request_clear(cx.list_parts().dom);
            let data = ops::object([
                ("uuid", JsValue::string(&prompt.uuid)),
                ("rect", rect_js(prompt.rect)),
            ]);
            cx.send_main_message(NamedMessage::new(HubToMain::SourceListClearPrompt, data));
            None
        }
        ListPointer::HelpClick => {
            let config = list.config().to_js();
            cx.send_main_message(NamedMessage::new(HubToMain::SourceListHelp, config));
            None
        }
        ListPointer::EditClick => {
            out.edit = Some(list.edit_click(cx.list_parts().dom));
            None
        }
    };
    if let Some(prompt) = prompt {
        let data = ops::object([
            ("uuid", JsValue::string(&prompt.uuid)),
            ("config", prompt.config.to_js()),
            ("state", prompt.state.to_js()),
            ("rect", rect_js(prompt.rect)),
        ]);
        cx.send_main_message(NamedMessage::new(HubToMain::SourceListTypePrompt, data));
    }
}

/// A `SourceList` exception as the hub reports it: an `Error` keeps its name, a thrown string
/// (`'Unknown type "x"'`) has none.
pub fn js_error(error: SourceListError) -> JsError {
    match error {
        SourceListError::UnknownType(_) => JsError::new("", format!("{error}")),
        SourceListError::Thrown(text) => {
            const NAMES: [&str; 6] = [
                "TypeError",
                "RangeError",
                "SyntaxError",
                "ReferenceError",
                "Error",
                "Unmodeled",
            ];
            match text.split_once(": ").and_then(|(name, message)| {
                NAMES.iter().find(|n| **n == name).map(|n| (*n, message))
            }) {
                Some((name, message)) => JsError::new(name, message),
                None => JsError::new("", text),
            }
        }
    }
}

/// `sourceList.setState(value)`.
pub fn set_source_state<C: SourceListCx + ?Sized>(
    list: &mut SourceList,
    cx: &mut C,
    value: &JsValue,
) -> Result<(), JsError> {
    with_env(cx, |env| list.restore(env, value)).map_err(js_error)
}
