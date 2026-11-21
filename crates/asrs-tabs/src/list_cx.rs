//! What a controller that owns `SourceList`s reads besides [`TabCx`]. A list runs with the log,
//! the shared type memory and the theme borrowed while its preview callback reads the selection,
//! so all four come out of the hub in one borrow.

use std::cell::Ref;

use asrs_js::string::JsError;
use asrs_log::log::Log;
use asrs_model::selection::Selection;
use asrs_model::source_list::{Env, SourceListError};
use asrs_model::source_list_config::SourceListTypeMemory;

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
