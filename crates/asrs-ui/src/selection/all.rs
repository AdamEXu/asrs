//! A selection of a window's whole document, which also shows in its form controls.

use std::collections::HashSet;

use gpui_kit::{App, Global, Window, WindowId};

use super::colors::{SelectionColors, page_active};

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

pub(crate) fn forget(cx: &mut App, window: WindowId) {
    if cx.has_global::<Windows>() {
        cx.global_mut::<Windows>().0.remove(&window);
    }
}

/// What a form control's text is selected with.
pub fn colors(window: &Window, cx: &App) -> Option<SelectionColors> {
    let id = window.window_handle().window_id();
    cx.try_global::<Windows>()?.0.get(&id)?;
    Some(SelectionColors::new(page_active(window, cx)))
}
