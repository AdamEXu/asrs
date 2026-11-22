//! `main.ts`'s handlers for the sidebar's context-menu requests: menus whose entries copy text
//! to the clipboard, popped up at the rounded position the hub sent.

use std::rc::Rc;

use asrs_js::math::round;
use asrs_js::value::JsValue;
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_ui::gpui::{App, ClipboardItem, Window, point, px};
use asrs_ui::menu::{CloseHandler, MenuView};
use asrs_ui::platform::{self, MenuPath, PopupEntry, PopupMenu};

fn text(data: &JsValue, key: &str) -> String {
    match data.get(key) {
        Some(JsValue::String(s)) => s.to_string_lossy(),
        Some(other) => asrs_js::ops::to_string(other).to_string_lossy(),
        None => "undefined".to_owned(),
    }
}

fn position(data: &JsValue) -> (i32, i32) {
    let at = |i: usize| match data.get("position") {
        Some(JsValue::Array(items)) => match items.get(i) {
            Some(JsValue::Number(n)) => round(*n) as i32,
            _ => 0,
        },
        _ => 0,
    };
    (at(0), at(1))
}

/// Pops up `entries` (label, enabled, text to copy) at `(x, y)` in window content coordinates.
fn popup(window: &mut Window, cx: &mut App, entries: Vec<(String, bool, String)>, x: i32, y: i32) {
    let mut menu = PopupMenu::new();
    for (label, enabled, _) in &entries {
        menu = menu.entry(PopupEntry::new(label.clone()).enabled(*enabled));
    }
    let copies: Rc<Vec<String>> = Rc::new(entries.into_iter().map(|(_, _, t)| t).collect());
    let choose = move |path: Option<MenuPath>, cx: &mut App| {
        if let Some(text) = path.and_then(|p| p.first().and_then(|&i| copies.get(i)).cloned()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    };
    if cfg!(target_os = "macos") {
        platform::popup_menu(window, cx, menu, x, y, choose);
    } else {
        let origin = window.bounds().origin + point(px(x as f32), px(y as f32));
        let on_close: CloseHandler = Rc::new(move |path, _, cx| choose(path, cx));
        MenuView::open_window(menu, origin, window.scale_factor(), cx, on_close);
    }
}

/// Handles `message` if it is one of the sidebar's context-menu requests.
pub fn handle(message: &NamedMessage<HubToMain>, window: &mut Window, cx: &mut App) {
    let data = &message.data;
    match message.name {
        HubToMain::AskOpenSidebarContextMenu => {
            let (title, full_title) = (text(data, "title"), text(data, "fullTitle"));
            let (x, y) = position(data);
            let entries = vec![
                (format!("Copy \"{title}\" to clipboard"), true, title),
                (
                    format!("Copy \"{full_title}\" to clipboard"),
                    true,
                    full_title,
                ),
            ];
            popup(window, cx, entries, x, y);
        }
        HubToMain::AskOpenTuningContextMenu => {
            let enabled = matches!(data.get("isTuningMode"), Some(JsValue::Bool(true)));
            let (x, y) = position(data);
            let entries = vec![(
                "Copy Tuned Values".to_owned(),
                enabled,
                text(data, "values"),
            )];
            popup(window, cx, entries, x, y);
        }
        _ => {}
    }
}
