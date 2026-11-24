//! `app.quit()`: `Browser::Quit` closes every window, and a hub that is exporting asks first.

use std::cell::Cell;
use std::rc::Rc;

use asrs_ui::gpui::{App, Global};
use asrs_ui::platform;

use crate::export_flow;
use crate::main_process::MainProcess;
use crate::windows::{self, WindowId, Windows};

#[derive(Default)]
struct Quit {
    /// No open hub is exporting, so that quitting has nothing to ask.
    unasked: Rc<Cell<bool>>,
    asking: bool,
    /// `is_quitting_`.
    quitting: bool,
}

impl Global for Quit {}

pub(crate) fn init(cx: &mut App) {
    let unasked = Rc::new(Cell::new(true));
    cx.set_global(Quit {
        unasked: unasked.clone(),
        ..Quit::default()
    });
    platform::on_quit_request(cx, move || unasked.get(), quit);
}

/// After a hub's export started or ended, or the hub closed.
pub(crate) fn exports_changed(cx: &mut App) {
    let exporting = Windows::ids(cx)
        .into_iter()
        .any(|window| MainProcess::is_exporting(cx, window));
    if let Some(quit) = cx.try_global::<Quit>() {
        quit.unasked.set(!exporting);
    }
}

pub fn quit(cx: &mut App) {
    let Some(quit) = cx.try_global::<Quit>() else {
        return cx.quit();
    };
    if quit.unasked.get() {
        return cx.quit();
    }
    if quit.asking {
        return;
    }
    cx.global_mut::<Quit>().quitting = true;
    let mut windows = Windows::ids(cx);
    // `WindowList::CloseAllWindows` goes by age, and macOS reverses it.
    if !cfg!(target_os = "macos") {
        windows.reverse();
    }
    close_all(windows, cx);
}

/// Closes the windows, the last first.
fn close_all(mut windows: Vec<WindowId>, cx: &mut App) {
    while let Some(window) = windows.pop() {
        if Windows::handle(cx, window).is_none() {
            continue;
        }
        if !MainProcess::is_exporting(cx, window) {
            MainProcess::before_unload(cx, window);
            windows::close(cx, window);
            continue;
        }
        cx.global_mut::<Quit>().asking = true;
        export_flow::ask_to_close(window, cx, move |close, cx| {
            let quit = cx.global_mut::<Quit>();
            quit.asking = false;
            if close {
                windows::close(cx, window);
            } else {
                quit.quitting = false;
            }
            close_all(windows, cx);
        });
        return;
    }
}

/// `Browser::OnWindowAllClosed`, with `main.ts`'s `window-all-closed` listener.
pub(crate) fn windows_all_closed(cx: &mut App) {
    let quitting = cx.try_global::<Quit>().is_some_and(|quit| quit.quitting);
    if quitting || !cfg!(target_os = "macos") {
        cx.quit();
    }
}
