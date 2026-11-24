//! A selection of a window's whole document, which also shows in its form controls.

use std::collections::HashSet;

use gpui_kit::{App, Global, Window, WindowId};

use super::colors::{SelectionColors, page_active};
use crate::theme::Appearance;

#[derive(Default)]
struct Windows(HashSet<WindowId>);

impl Global for Windows {}

pub fn set(all: bool, window: &Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    let was = cx
        .try_global::<Windows>()
        .is_some_and(|windows| windows.0.contains(&id));
    if was == all {
        return;
    }
    let windows = &mut cx.default_global::<Windows>().0;
    if all {
        windows.insert(id);
    } else {
        windows.remove(&id);
    }
}

/// What a form control's text is selected with.
pub fn colors(window: &Window, cx: &App) -> Option<SelectionColors> {
    let id = window.window_handle().window_id();
    cx.try_global::<Windows>()?.0.get(&id)?;
    let appearance = Appearance::of(window, cx);
    Some(SelectionColors::of(appearance, page_active(window, cx)))
}
