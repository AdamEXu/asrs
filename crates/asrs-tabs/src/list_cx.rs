//! What a controller that owns `SourceList`s reads besides [`TabCx`]. A list runs with the log,
//! the shared type memory and the theme borrowed while its preview callback reads the selection,
//! so all four come out of the hub in one borrow.

use std::cell::Ref;

use asrs_js::ops;
use asrs_js::string::JsError;
use asrs_js::value::JsValue;
use asrs_log::log::Log;
use asrs_model::selection::Selection;
use asrs_model::source_list::{Env, SourceList, SourceListError};
use asrs_model::source_list_config::{SourceListItemState, SourceListTypeMemory};

use crate::cx::TabCx;

pub struct ListParts<'a> {
    pub log: Ref<'a, Log>,
    /// `window.typeMemory`.
    pub type_memory: &'a mut SourceListTypeMemory,
    pub selection: &'a mut dyn Selection,
    /// `prefers-color-scheme: dark`.
    pub dark: bool,
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
    };
    f(&mut env)
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

/// A saved `SourceListState` for `setState`. A value of another shape makes upstream throw
/// midway through `setState`, which is not modelled.
pub fn source_state(value: &JsValue) -> Result<Vec<SourceListItemState>, JsError> {
    let unmodeled = |what: &str| JsError::new("Unmodeled", format!("source list: {what}"));
    value
        .as_array()
        .ok_or_else(|| unmodeled("restored sources that are not an array"))?
        .iter()
        .map(|item| {
            SourceListItemState::from_js(item)
                .ok_or_else(|| unmodeled("a restored source item of another shape"))
        })
        .collect()
}

/// `sourceList.setState(value)`: `clear()`, then `value.forEach(addListItem)`. An item
/// `SourceListItemState` cannot hold stays in upstream's state after `addListItem`
/// pushes it, so the port reports it as unmodeled instead of guessing.
pub fn set_source_state<C: SourceListCx + ?Sized>(
    list: &mut SourceList,
    cx: &mut C,
    value: &JsValue,
) -> Result<(), JsError> {
    let JsValue::Array(items) = value else {
        list.clear();
        if let JsValue::Bytes(_) = value {
            return Err(JsError::new("Unmodeled", "a typed array of sources"));
        }
        ops::get(value, "forEach")?;
        return Err(JsError::type_error("state.forEach is not a function"));
    };
    let items = items
        .iter()
        .map(|item| {
            SourceListItemState::from_js(item)
                .ok_or_else(|| JsError::new("Unmodeled", "a source item of another shape"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    with_env(cx, |env| list.set_state(env, items)).map_err(js_error)
}
