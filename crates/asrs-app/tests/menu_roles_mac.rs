#![allow(clippy::disallowed_methods)]

//! The roles of upstream's darwin menu that Electron leaves to AppKit's responder chain, sent
//! through it to windows that are never shown: every edit role must be taken by the view, as
//! `RenderWidgetHostViewCocoa` takes it, and act on the field or page in it once; Toggle Full
//! Screen must reach `toggleFullScreen:`, which is intercepted; full screen notifications must
//! reach the window's observer; and quitting must ask when it may not, and quit when it may.
//!
//! The general pasteboard is the user's: the clipboard actions are caught in the page, and
//! `edit_roles` checks what they do.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use asrs_app::menus::{app_menu, bridge, dispatch};
use asrs_model::preferences::Preferences;
use asrs_services::window_manager::Platform;
use asrs_ui::gpui::{
    AnyWindowHandle, App, AppContext, AsyncApp, Context, Entity, FocusHandle, InteractiveElement,
    IntoElement, ParentElement, Render, Styled, Window, WindowHandle, WindowOptions, div,
};
use asrs_ui::input::{self, FieldKind, FieldStyle, PlainText, TextField};
use asrs_ui::platform::{self, probe};
use asrs_ui::selection;

const EDIT_ROLES: [&str; 8] = [
    "undo",
    "redo",
    "cut",
    "copy",
    "paste",
    "pasteandmatchstyle",
    "delete",
    "selectall",
];

const COMMAND: u32 = 1 << 20;

const TIMEOUT: Duration = Duration::from_secs(60);

type Log = Rc<RefCell<Vec<&'static str>>>;

struct Page {
    field: Entity<TextField<PlainText>>,
    body: FocusHandle,
    log: Log,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .id("page")
            .track_focus(&self.body)
            .size_full()
            .capture_action(cx.listener(|page, _: &input::Cut, _, cx| page.caught("cut", cx)))
            .capture_action(cx.listener(|page, _: &input::Paste, _, cx| page.caught("paste", cx)))
            .capture_action(cx.listener(|page, _: &selection::Copy, _, cx| {
                page.caught("copy", cx);
            }))
            .on_action(cx.listener(|page, _: &selection::SelectAll, _, cx| {
                page.caught("select the page", cx);
            }))
            .child(self.field.clone())
    }
}

impl Page {
    fn caught(&mut self, action: &'static str, cx: &mut Context<'_, Self>) {
        self.log.borrow_mut().push(action);
        cx.stop_propagation();
    }
}

async fn settle(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(150))
        .await;
}

struct Check {
    window: WindowHandle<Page>,
    log: Log,
}

impl Check {
    fn open(cx: &mut App) -> Check {
        let options = WindowOptions {
            show: false,
            focus: false,
            ..Default::default()
        };
        let log = Log::default();
        let window = cx
            .open_window(options, |window, cx| {
                cx.new(|cx| Page {
                    field: cx.new(|cx| {
                        let model = PlainText::new("one two");
                        TextField::new(model, FieldKind::Text, FieldStyle::NATIVE, window, cx)
                    }),
                    body: cx.focus_handle(),
                    log: log.clone(),
                })
            })
            .expect("a window");
        Check { window, log }
    }

    fn update<R>(
        &self,
        cx: &mut AsyncApp,
        f: impl FnOnce(&mut Page, &mut Window, &mut Context<'_, Page>) -> R,
    ) -> R {
        self.window.update(cx, f).expect("open")
    }

    async fn focus(&self, cx: &mut AsyncApp, field: bool) {
        self.update(cx, |page, window, cx| {
            if field {
                page.field.update(cx, |field, cx| field.focus(window, cx));
            } else {
                window.focus(&page.body, cx);
            }
            window.refresh();
        });
        self.draw(cx).await;
    }

    async fn draw(&self, cx: &mut AsyncApp) {
        settle(cx).await;
        AnyWindowHandle::from(self.window)
            .update(cx, |_, window, cx| window.draw(cx).clear(cx))
            .expect("open");
        settle(cx).await;
    }

    /// Returns the field's value after the role, and the actions the page caught.
    async fn role(&self, cx: &mut AsyncApp, role: &str) -> (String, Vec<&'static str>) {
        self.log.borrow_mut().clear();
        let sent = self.update(cx, |_, window, _| probe::perform_menu_role(window, role));
        assert!(sent, "nothing takes {role}");
        self.draw(cx).await;
        let value = self.update(cx, |page, _, cx| page.field.read(cx).value());
        (value, self.log.borrow().clone())
    }
}

async fn edit_roles(check: &Check, cx: &mut AsyncApp) {
    check.focus(cx, true).await;
    for role in EDIT_ROLES {
        let wired = check.update(cx, |_, window, _| probe::menu_role(window, role));
        let wired = wired.expect(role);
        println!("{role}: {wired:?}");
        assert!(wired.enabled, "{role}");
        let (_, implementor) = wired.target.expect(role);
        assert_eq!(implementor, "GPUIView", "{role}");
    }

    for role in ["delete", "undo"] {
        assert_eq!(check.role(cx, role).await, ("one two".into(), vec![]));
    }
    assert_eq!(
        check.role(cx, "selectall").await,
        ("one two".into(), vec![])
    );
    assert_eq!(check.role(cx, "delete").await, (String::new(), vec![]));
    assert_eq!(check.role(cx, "undo").await, ("one two".into(), vec![]));
    assert_eq!(check.role(cx, "redo").await, (String::new(), vec![]));
    assert_eq!(check.role(cx, "undo").await, ("one two".into(), vec![]));
    for (role, action) in [
        ("cut", "cut"),
        ("copy", "copy"),
        ("paste", "paste"),
        ("pasteandmatchstyle", "paste"),
    ] {
        assert_eq!(check.role(cx, role).await, ("one two".into(), vec![action]));
    }

    check.focus(cx, false).await;
    assert_eq!(check.role(cx, "copy").await.1, ["copy"]);
    assert_eq!(check.role(cx, "selectall").await.1, ["select the page"]);
    assert_eq!(check.role(cx, "delete").await, ("one two".into(), vec![]));

    for role in EDIT_ROLES {
        probe::perform_menu_role_without_window(role).expect("a delegate");
    }
    let copy = probe::validate_menu_role_without_window("copy");
    assert_eq!(copy, Some(true), "the delegate's validation returns");
    for key in ["x", "c", "v", "a"] {
        assert!(probe::perform_key_equivalent(key, COMMAND), "{key}");
    }
}

async fn full_screen(check: &Check, cx: &mut AsyncApp) {
    assert!(probe::intercept_full_screen());
    let wired = check.update(cx, |_, window, _| {
        probe::menu_role(window, "togglefullscreen")
    });
    let wired = wired.expect("Toggle Full Screen");
    println!("togglefullscreen: {wired:?}");
    assert!(wired.enabled);
    assert_eq!(wired.target.expect("a target").1, "GPUIWindow");
    check.role(cx, "togglefullscreen").await;
    assert_eq!(probe::full_screen_toggles(), 1);

    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let observer = check.update(cx, |_, window, cx| {
        let native = platform::native_window(window).expect("a native window");
        native.hidden_inset_titlebar();
        platform::observe_full_screen(window, cx, move |full_screen, _| {
            sink.borrow_mut().push(full_screen);
        })
    });
    let observer = observer.expect("an observer");
    let chrome = |toolbar, title, buttons| probe::WindowChrome {
        toolbar,
        title,
        buttons,
    };
    let windowed = check.update(cx, |_, window, _| probe::window_chrome(window));
    assert!(windowed.expect("a window").toolbar);
    let steps: [(&str, probe::WindowChrome, &[bool]); 4] = [
        (
            "NSWindowWillEnterFullScreenNotification",
            chrome(false, true, true),
            &[],
        ),
        (
            "NSWindowDidEnterFullScreenNotification",
            chrome(false, true, true),
            &[true],
        ),
        (
            "NSWindowWillExitFullScreenNotification",
            chrome(false, false, false),
            &[true],
        ),
        (
            "NSWindowDidExitFullScreenNotification",
            chrome(true, false, true),
            &[true, false],
        ),
    ];
    for (name, expected, changes) in steps {
        check.update(cx, |_, window, _| {
            probe::post_window_notification(window, name);
        });
        settle(cx).await;
        let read = check.update(cx, |_, window, _| probe::window_chrome(window));
        assert_eq!(read, Some(expected), "{name}");
        assert_eq!(*seen.borrow(), changes, "{name}");
    }
    observer.stop();
    check.update(cx, |_, window, _| {
        probe::post_window_notification(window, steps[1].0);
    });
    settle(cx).await;
    assert_eq!(*seen.borrow(), [true, false], "after it is stopped");
}

async fn quitting(check: &Check, cx: &mut AsyncApp) {
    let may_quit = Rc::new(Cell::new(false));
    let asked = Rc::new(Cell::new(0));
    cx.update(|cx| {
        let (may_quit, asked) = (may_quit.clone(), asked.clone());
        platform::on_quit_request(
            cx,
            move || may_quit.get(),
            move |_| asked.set(asked.get() + 1),
        );
        cx.quit();
    });
    settle(cx).await;
    assert_eq!(asked.get(), 1, "the app quitting itself");
    check.role(cx, "quit").await;
    assert_eq!(asked.get(), 2, "the menu item's and the Dock's terminate:");
    may_quit.set(true);
    println!("menu roles mac: passed, quitting");
    check.role(cx, "quit").await;
}

fn main() {
    if asrs_parity::ci::skip_in_ci("menu roles mac", "a display") {
        return;
    }
    if !cfg!(target_os = "macos") {
        println!("menu roles mac: skipped on this platform");
        return;
    }
    std::thread::spawn(|| {
        std::thread::sleep(TIMEOUT);
        eprintln!("menu roles mac: stuck");
        std::process::exit(1);
    });
    asrs_ui::gpui::application().run(|cx: &mut App| {
        probe::stay_in_background();
        asrs_ui::init(cx);
        asrs_app::windows::init(cx);
        let template =
            app_menu::template(Platform::Mac, &Preferences::default(), asrs_app::IDENTITY);
        bridge::install_application_menu(cx, Platform::Mac, &template, Rc::new(dispatch::dispatch));
        platform::install_edit_commands(cx, dispatch::run_edit);
        let check = Check::open(cx);
        cx.spawn(async move |cx| {
            edit_roles(&check, cx).await;
            full_screen(&check, cx).await;
            assert_eq!(probe::app_is_active(), Some(false));
            quitting(&check, cx).await;
            settle(cx).await;
            panic!("the app did not quit");
        })
        .detach();
    });
}
