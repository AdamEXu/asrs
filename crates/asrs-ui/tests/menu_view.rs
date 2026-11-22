//! The drawn popup menu in its `PopUp` window form: size, keyboard navigation that skips
//! separators, disabled and hidden entries, submenus opened from the keyboard into a window of
//! their own, and the chosen path or dismissal reaching the close handler once.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use asrs_ui::gpui::{AnyWindowHandle, App, AsyncApp, point, px};
use asrs_ui::menu::{
    CloseHandler, MIN_WIDTH, MenuView, PADDING, ROW_HEIGHT, SEPARATOR_HEIGHT, menu_size,
};
use asrs_ui::platform::{MenuPath, PopupEntry, PopupMenu};
use gpui_kit::Keystroke;

fn menu() -> PopupMenu {
    PopupMenu::new()
        .entry(PopupEntry::new("Disabled").enabled(false))
        .entry(PopupEntry::new("Hidden").visible(false))
        .separator()
        .entry(PopupEntry::new("First"))
        .entry(
            PopupEntry::new("More").submenu(
                PopupMenu::new()
                    .entry(PopupEntry::new("Inner"))
                    .entry(PopupEntry::new("Inner two").checked(true)),
            ),
        )
}

fn press(cx: &mut AsyncApp, window: AnyWindowHandle, key: &'static str) {
    window
        .update(cx, |_, window, cx| {
            window.dispatch_keystroke(Keystroke::parse(key).expect("keystroke"), cx);
        })
        .expect("menu window open");
}

async fn settle(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(100))
        .await;
}

fn open(cx: &mut App, closed: &Rc<RefCell<Vec<Option<MenuPath>>>>) -> AnyWindowHandle {
    let sink = closed.clone();
    let on_close: CloseHandler = Rc::new(move |path, _, _| sink.borrow_mut().push(path));
    MenuView::open_window(menu(), point(px(200.0), px(200.0)), 2.0, cx, on_close)
        .expect("popup window")
}

fn main() {
    if asrs_parity::ci::skip_in_ci("menu view", "a display") {
        return;
    }
    if !cfg!(target_os = "macos") {
        println!("menu view: skipped on this platform");
        return;
    }
    asrs_ui::gpui::application().run(|cx: &mut App| {
        asrs_ui::init(cx);
        let (width, height) = menu_size(cx, &menu(), 2.0);
        assert_eq!(height, 2.0 * PADDING + 3.0 * ROW_HEIGHT + SEPARATOR_HEIGHT);
        assert!(width >= MIN_WIDTH);
        let closed = Rc::new(RefCell::new(Vec::new()));
        let first = open(cx, &closed);
        cx.spawn(async move |cx| {
            settle(cx).await;
            press(cx, first, "down");
            press(cx, first, "down");
            press(cx, first, "right");
            settle(cx).await;
            let submenu = cx
                .update(|cx| cx.windows())
                .into_iter()
                .find(|w| *w != first)
                .expect("a submenu window");
            press(cx, submenu, "down");
            press(cx, submenu, "down");
            press(cx, submenu, "enter");
            settle(cx).await;
            let second = cx.update(|cx| open(cx, &closed));
            settle(cx).await;
            press(cx, second, "up");
            press(cx, second, "escape");
            settle(cx).await;
            let got = closed.borrow().clone();
            let windows = cx.update(|cx| cx.windows().len());
            println!("menu view: closed with {got:?}, {windows} windows left");
            let ok = got == vec![Some(vec![4, 1]), None] && windows == 0;
            cx.update(|cx| cx.quit());
            std::process::exit(i32::from(!ok));
        })
        .detach();
    });
}
