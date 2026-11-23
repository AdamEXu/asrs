#![allow(clippy::disallowed_methods)]

//! The application menu in the app, headless: `main_process::start` in a fresh home directory,
//! then clicks through `menus::dispatch` and keystrokes through the accelerator table, with the
//! dialogs answered by a script. What the handlers do is `menu_click_vectors`' business; this
//! checks that their effects reach the hubs, the windows, the files and the dialogs.

#[path = "support/child.rs"]
mod child;

#[path = "support/settle.rs"]
mod settle;

use std::io::{ErrorKind, Read};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::Arc;
use std::time::Duration;

use asrs_app::download_flow::{self, RobotPort, RobotTimeout};
use asrs_app::licenses::{self, LicensesFile};
use asrs_app::main_process::{self, BetaTime, MainProcess};
use asrs_app::menus::app_menu;
use asrs_app::menus::bridge::{self, MenuAction};
use asrs_app::menus::dispatch::dispatch;
use asrs_app::menus::model::{Command, HelpLink, MenuItem};
use asrs_app::windows::{self, MessageBoxType, Rect, WindowId, WindowKind, Windows};
use asrs_hub::hub::HubModel;
use asrs_js::value::{JsValue, ToJs};
use asrs_model::json_file;
use asrs_model::preferences::{LiveMode, Preferences, SystemcoreStaticAddress, Theme, Timestamps};
use asrs_model::selection::{Selection, SelectionMode};
use asrs_model::source_list::configs;
use asrs_model::tab_type::TabType;
use asrs_services::dialog::OpenDialog;
use asrs_services::download::DownloadEvent;
use asrs_services::download::downloader::{ListItem, OpenLogBox, SaveLogDialog, WindowMessage};
use asrs_services::download::page::PageEvent;
use asrs_services::historical_start::HOOT_UNSUPPORTED;
use asrs_services::lifecycle;
use asrs_services::window_manager::Platform;
use asrs_tabs::list_cx::SourceListCx;
use asrs_ui::gpui::{
    HeadlessAppContext, KeyUpEvent, Keystroke, Modifiers, MouseButton, MouseDownEvent,
    MouseUpEvent, NoopTextSystem, PlatformInput, point, px,
};
use asrs_ui::platform::dialog::{Reply, Script, Shown};
use asrs_ui::theme::ThemeSource;

const HOME: &str = "ASRS_MENU_DISPATCH_HOME";

#[test]
fn the_menu_in_the_app() {
    run_in_a_fresh_home("in_a_fresh_home");
}

#[test]
fn saved_windows_open_at_launch() {
    run_in_a_fresh_home("with_saved_windows");
}

fn run_in_a_fresh_home(test: &str) {
    let home = std::env::temp_dir().join(format!("asrs-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("home");
    let mut child = Process::new(std::env::current_exe().expect("the test binary"));
    child.args(["--exact", test, "--ignored", "--nocapture"]);
    for variable in [
        "HOME",
        "APPDATA",
        "XDG_CONFIG_HOME",
        "TMPDIR",
        "TEMP",
        "TMP",
        HOME,
    ] {
        child.env(variable, &home);
    }
    let status = child::run(&mut child, test);
    let _ = std::fs::remove_dir_all(&home);
    assert!(status.success(), "{test}: {status}");
}

struct AppUnderTest {
    cx: HeadlessAppContext,
    home: PathBuf,
}

impl AppUnderTest {
    fn start(home: PathBuf) -> AppUnderTest {
        let mut cx = HeadlessAppContext::new(Arc::new(NoopTextSystem::new()));
        let (_send, events) = async_channel::unbounded();
        cx.update(|cx| {
            asrs_ui::init(cx);
            windows::init(cx);
            cx.set_global(Script::default());
            cx.set_global(BetaTime(0.0));
            main_process::start(cx, &[], events);
        });
        let mut app = AppUnderTest { cx, home };
        app.settle();
        app
    }

    fn settle(&mut self) {
        settle::settle(&mut self.cx);
    }

    fn user_data(&self, file: &str) -> PathBuf {
        let user_data =
            lifecycle::user_data_dir(Platform::current(), |name| std::env::var(name).ok());
        Path::new(&user_data.expect("a user-data directory")).join(file)
    }

    fn prefs_path(&self) -> PathBuf {
        self.user_data("prefs.json")
    }

    fn pref(&self, key: &str) -> JsValue {
        let bytes = std::fs::read(self.prefs_path()).expect("prefs.json");
        let prefs = json_file::parse(&bytes).expect("prefs.json");
        prefs.get(key).cloned().expect(key)
    }

    fn hubs(&mut self) -> Vec<WindowId> {
        self.cx.update(|cx| Windows::manager(cx).hubs().to_vec())
    }

    fn bounds(&mut self, window: WindowId) -> Rect {
        let bounds = self.cx.update(|cx| Windows::manager(cx).bounds(window));
        bounds.expect("bounds")
    }

    fn focus(&mut self, window: WindowId) {
        let handle = self.cx.update(|cx| Windows::handle(cx, window));
        self.cx
            .update_window(handle.expect("open"), |_, window, _| {
                window.activate_window()
            })
            .expect("open");
        self.settle();
        let focused = self.cx.update(|cx| Windows::manager(cx).focused());
        assert_eq!(focused, Some(window));
    }

    fn window(&mut self, kind: WindowKind) -> Option<WindowId> {
        self.cx.update(|cx| {
            let window = Windows::of_kind(cx, kind, 0)?;
            Windows::id_of(cx, window)
        })
    }

    fn preferences(&mut self) -> Option<WindowId> {
        self.window(WindowKind::Preferences)
    }

    fn satellites(&mut self) -> Vec<WindowId> {
        let pages = self.cx.update(|cx| MainProcess::satellite_windows(cx));
        pages.into_iter().map(|(id, _)| id).collect()
    }

    fn active_satellites(&mut self, hub: WindowId) -> Vec<String> {
        self.hub(hub, |hub| {
            let active = hub.tabs().active_satellites().iter();
            active
                .map(|uuid| uuid.as_str().expect("a UUID").to_string_lossy())
                .collect()
        })
    }

    fn title(&mut self, satellite: WindowId) -> String {
        let pages = self.cx.update(|cx| MainProcess::satellite_windows(cx));
        let page = pages.into_iter().find(|(id, _)| *id == satellite);
        let (_, page) = page.expect("a satellite");
        self.cx
            .update(|cx| page.read(cx).model().title().to_string_lossy())
    }

    fn frames(&mut self, window: WindowId, count: usize) {
        let handle = self.cx.update(|cx| Windows::handle(cx, window));
        let handle = handle.expect("open");
        for _ in 0..count {
            self.cx
                .update_window(handle, |_, window, cx| window.simulate_next_frame(cx))
                .expect("open");
            self.settle();
        }
    }

    fn close(&mut self, window: WindowId) {
        self.cx.update(|cx| windows::close(cx, window));
        self.settle();
    }

    fn hub<R>(&mut self, window: WindowId, read: impl FnOnce(&HubModel) -> R) -> R {
        self.cx.update(|cx| {
            let hubs = MainProcess::hub_windows(cx);
            let hub = hubs.iter().find(|(id, _)| *id == window).expect("a hub");
            read(hub.1.upgrade().expect("a live hub").read(cx).model())
        })
    }

    fn tabs(&mut self, window: WindowId) -> Vec<Option<TabType>> {
        self.hub(window, |hub| {
            hub.tabs().tabs().map(|t| t.tab_type()).collect()
        })
    }

    fn historical_paths(&mut self, window: WindowId) -> Vec<String> {
        self.hub(window, |hub| {
            hub.historical_paths()
                .map(|p| p.to_string_lossy())
                .collect()
        })
    }

    fn run(
        &mut self,
        replies: &[Reply],
        act: impl FnOnce(&mut AppUnderTest),
    ) -> Vec<(Option<WindowId>, Shown)> {
        self.cx
            .update(|cx| cx.global_mut::<Script>().replies = replies.to_vec().into());
        act(self);
        self.settle();
        self.cx.update(|cx| {
            let script = cx.global_mut::<Script>();
            assert!(
                script.replies.is_empty(),
                "never asked for {:?}",
                script.replies
            );
            let shown = std::mem::take(&mut script.shown);
            shown
                .into_iter()
                .map(|(window, shown)| {
                    let id = window.map(|window| Windows::id_of(cx, window).expect("a window"));
                    (id, shown)
                })
                .collect()
        })
    }

    fn click_checked(
        &mut self,
        command: Command,
        checked: bool,
        replies: &[Reply],
    ) -> Vec<(Option<WindowId>, Shown)> {
        self.run(replies, |app| {
            app.cx
                .update(|cx| dispatch(MenuAction::Command { command, checked }, cx))
        })
    }

    fn click(&mut self, command: Command, replies: &[Reply]) -> Vec<(Option<WindowId>, Shown)> {
        self.click_checked(command, false, replies)
    }

    fn press(
        &mut self,
        window: WindowId,
        key: &'static str,
        replies: &[Reply],
    ) -> Vec<(Option<WindowId>, Shown)> {
        self.run(replies, |app| {
            let handle = app.cx.update(|cx| Windows::handle(cx, window));
            app.cx
                .update_window(handle.expect("open"), |_, window, cx| {
                    let keystroke = Keystroke::parse(key).expect("a keystroke");
                    window.dispatch_keystroke(keystroke.clone(), cx);
                    window.dispatch_event(PlatformInput::KeyUp(KeyUpEvent { keystroke }), cx);
                })
                .expect("open");
        })
    }

    /// Tab, or Shift+Tab, `n` times.
    fn tab(&mut self, window: WindowId, n: usize, back: bool) {
        let key = if back { "shift-tab" } else { "tab" };
        for _ in 0..n {
            assert!(self.press(window, key, &[]).is_empty());
        }
    }

    fn send(&mut self, window: WindowId, inputs: Vec<PlatformInput>) {
        let handle = self.cx.update(|cx| Windows::handle(cx, window));
        let handle = handle.expect("open");
        self.cx
            .update_window(handle, |_, window, _| window.refresh())
            .expect("open");
        self.settle();
        self.cx
            .update_window(handle, |_, window, cx| {
                for input in inputs {
                    window.dispatch_event(input, cx);
                }
            })
            .expect("open");
        self.settle();
    }

    fn click_at(&mut self, window: WindowId, x: f64, y: f64) {
        let position = point(px(x as f32), px(y as f32));
        let (button, modifiers) = (MouseButton::Left, Modifiers::default());
        self.send(
            window,
            vec![
                PlatformInput::MouseDown(MouseDownEvent {
                    button,
                    position,
                    modifiers,
                    click_count: 1,
                    first_mouse: false,
                }),
                PlatformInput::MouseUp(MouseUpEvent {
                    button,
                    position,
                    modifiers,
                    click_count: 1,
                }),
            ],
        );
    }

    fn key(&mut self, window: WindowId, key: &'static str) {
        assert!(self.press(window, key, &[]).is_empty());
    }

    fn mode(&mut self, hub: WindowId) -> SelectionMode {
        self.hub(hub, |hub| hub.page().selection().get_mode())
    }

    fn type_text(&mut self, window: WindowId, text: &str) {
        let handle = self.cx.update(|cx| Windows::handle(cx, window));
        let handle = handle.expect("open");
        for c in text.chars() {
            let keystroke = Keystroke {
                key: if c == ' ' {
                    "space".to_owned()
                } else {
                    c.to_string()
                },
                key_char: Some(c.to_string()),
                modifiers: Default::default(),
            };
            self.cx
                .update_window(handle, |_, window, cx| {
                    window.dispatch_keystroke(keystroke, cx);
                })
                .expect("open");
            self.settle();
        }
    }

    fn fullscreen(&mut self, window: WindowId) -> bool {
        let handle = self.cx.update(|cx| Windows::handle(cx, window));
        self.cx
            .update_window(handle.expect("open"), |_, window, _| window.is_fullscreen())
            .expect("open")
    }
}

/// The window a box without a parent gets: none on macOS, the focused one elsewhere.
fn unparented(focused: WindowId) -> Option<WindowId> {
    (!cfg!(target_os = "macos")).then_some(focused)
}

fn message(shown: &[(Option<WindowId>, Shown)]) -> (Option<WindowId>, MessageBoxType, &str, &str) {
    let [(window, Shown::Message(options))] = shown else {
        panic!("not one message box: {shown:?}");
    };
    (*window, options.kind, &options.message, &options.detail)
}

fn open(paths: &[&Path]) -> Reply {
    Reply::Open(paths.iter().map(|p| p.to_path_buf()).collect())
}

#[test]
#[ignore = "run by the_menu_in_the_app with its own home directory"]
fn in_a_fresh_home() {
    let Some(home) = std::env::var_os(HOME) else {
        return;
    };
    let mut app = AppUnderTest::start(PathBuf::from(home));
    let hubs = app.hubs();
    let [hub] = hubs[..] else {
        panic!("{hubs:?}");
    };
    the_beta_welcome_is_acknowledged(&mut app, hub);
    app.focus(hub);

    keys_reach_the_page_unless_a_text_field_has_focus(&mut app, hub);
    messages_reach_the_focused_hub(&mut app, hub);
    preferences_reach_the_file_the_hubs_and_the_menu(&mut app, hub);
    dialogs_show_and_resume(&mut app, hub);
    the_preferences_window_writes_the_file(&mut app, hub);
    the_licenses_window_opens_once(&mut app, hub);
    help_windows_open_beside_the_hub(&mut app, hub);
    let robot = TcpListener::bind("127.0.0.1:0").expect("a port");
    let port = robot.local_addr().expect("its address").port();
    app.cx.update(|cx| {
        cx.set_global(RobotPort(port));
        cx.set_global(RobotTimeout(600_000.0));
    });
    closing_licenses_or_help_stops_the_download(&mut app, hub, &robot);
    drop(robot);
    the_download_window_shows_what_the_service_sends(&mut app, hub, port);
    accelerators_reach_the_window_they_were_pressed_in(&mut app, hub);
    let hub = windows_open_and_close(&mut app, hub);
    layouts_round_trip(&mut app, hub);
    satellites_open_close_and_round_trip(&mut app);
}

const SAVED: &str = r##"{"hubs":[{"x":100,"y":80,"width":1000,"height":600,"state":{"sidebar":{"width":300,"expanded":[]},"tabs":{"selected":1,"tabs":[{"type":0,"title":"","controller":null,"controllerUUID":"documentation","renderer":"","controlsHeight":0},{"type":2,"title":"Field","controller":{"sources":[],"field":"FRC:Evergreen","orientation":0,"size":"large"},"controllerUUID":"fieldtab","renderer":null,"controlsHeight":200}]}}}],"satellites":[{"x":300,"y":200,"width":640,"height":480,"state":{"type":2,"visualizer":{"zoom":1.5,"pan":[0.25,-0.5],"trackingMode":0,"isFTC":false}},"uuid":"fieldtab"},{"x":320,"y":220,"width":500,"height":300,"state":{"type":4,"visualizer":null},"uuid":"gone"}]}"##;

#[test]
#[ignore = "run by saved_windows_open_at_launch with its own home directory"]
fn with_saved_windows() {
    let Some(home) = std::env::var_os(HOME) else {
        return;
    };
    let user_data = lifecycle::user_data_dir(Platform::current(), |name| std::env::var(name).ok())
        .expect("a user-data directory");
    std::fs::create_dir_all(&user_data).expect("user data");
    let file = Path::new(&user_data).join("state-dev.json");
    std::fs::write(&file, SAVED).expect("state");
    let mut app = AppUnderTest::start(PathBuf::from(home));
    let [hub] = app.hubs()[..] else {
        panic!("{:?}", app.hubs());
    };
    let satellites = app.satellites();
    let [field, table] = satellites[..] else {
        panic!("{satellites:?}");
    };
    let rect = |x, y, width, height| Rect {
        x,
        y,
        width,
        height,
    };
    assert_eq!(app.bounds(hub), rect(100.0, 80.0, 1000.0, 600.0));
    assert_eq!(app.bounds(field), rect(300.0, 200.0, 640.0, 480.0));
    assert_eq!(app.bounds(table), rect(320.0, 220.0, 500.0, 300.0));
    assert_eq!(app.active_satellites(hub), ["fieldtab", "gone"]);

    // A change elsewhere has the tracker write the windows again, and the satellites' part of the
    // file comes back byte for byte.
    app.click(Command::NewTab(TabType::Metadata), &[]);
    app.cx.advance_clock(Duration::from_secs(2));
    app.settle();
    let written = std::fs::read_to_string(&file).expect("state");
    assert_ne!(written, SAVED);
    let satellites =
        |text: &str| text[text.find("\"satellites\"").expect("satellites")..].to_owned();
    assert_eq!(satellites(written.trim_end()), satellites(SAVED));
}

fn satellites_open_close_and_round_trip(app: &mut AppUnderTest) {
    let [other, hub] = app.hubs()[..] else {
        panic!("{:?}", app.hubs());
    };
    app.close(other);
    app.focus(hub);
    app.click(Command::NewTab(TabType::Table), &[]);
    let uuid = app.hub(hub, |hub| {
        let tabs = hub.tabs();
        let selected = tabs.selected_index().expect("a selected tab");
        tabs.tab(selected).expect("it").uuid().clone()
    });
    let uuid = uuid.as_str().expect("a UUID").to_string_lossy();
    assert!(app.satellites().is_empty());
    for _ in 0..2 {
        assert!(app.click(Command::NewPopOut, &[]).is_empty());
    }
    let satellites = app.satellites();
    let [first, second] = satellites[..] else {
        panic!("{satellites:?}");
    };
    let (from, to) = (app.bounds(hub), app.bounds(first));
    assert_eq!((to.width, to.height), (900.0, 500.0));
    assert_eq!(to.x, (from.x + from.width / 2.0 - 450.0).floor());
    assert_eq!(app.active_satellites(hub), [uuid.as_str()]);

    app.frames(hub, 3);
    assert_eq!(app.title(first), "\u{1f522} Table \u{2014} AdvantageScope");
    assert_eq!(app.title(second), app.title(first));

    let file = app.home.join("satellites.json");
    app.cx.advance_clock(Duration::from_secs(2));
    app.settle();
    let export = |app: &mut AppUnderTest| {
        let _ = std::fs::remove_file(&file);
        app.click(Command::ExportLayout, &[Reply::Save(Some(file.clone()))]);
        std::fs::read(&file).expect("the exported layout")
    };
    let exported = export(app);
    let layout = json_file::parse(&exported).expect("JSON");
    let saved = layout.get("satellites").and_then(JsValue::as_array);
    let saved = saved.expect("satellites");
    assert_eq!(saved.len(), 2);
    for satellite in saved {
        assert_eq!(satellite.get("uuid"), Some(&JsValue::string(&uuid)));
        let state = satellite.get("state").expect("state");
        assert_eq!(state.get("type"), Some(&JsValue::Number(4.0)));
    }

    // "Close Tab" on a satellite destroys its window; the other still shows the tab.
    app.focus(second);
    app.click(Command::CloseTab, &[]);
    assert_eq!(app.satellites(), [first]);
    assert_eq!(app.active_satellites(hub), [uuid.as_str()]);
    app.focus(first);
    app.click(Command::CloseWindow, &[]);
    assert!(app.satellites().is_empty());
    assert!(app.active_satellites(hub).is_empty());

    let import = std::env::temp_dir().join(format!("asrs-layout-{}.json", std::process::id()));
    std::fs::write(&import, &exported).expect("a layout");
    app.focus(hub);
    let shown = app.click(Command::ImportLayout, &[open(&[&import])]);
    let _ = std::fs::remove_file(&import);
    assert_eq!(shown.len(), 1, "{shown:?}");
    let hubs = app.hubs();
    let [restored] = hubs[..] else {
        panic!("{hubs:?}");
    };
    assert_ne!(restored, hub);
    assert_eq!(app.satellites().len(), 2);
    assert_eq!(app.active_satellites(restored), [uuid.as_str()]);
    app.cx.advance_clock(Duration::from_secs(2));
    app.settle();
    app.focus(restored);
    assert_eq!(export(app), exported);

    app.close(restored);
    app.cx.advance_clock(Duration::from_secs(2));
    app.settle();
    assert!(app.hubs().is_empty());
    assert_eq!(app.satellites().len(), 2, "satellites outlive their hub");
}

fn keys_reach_the_page_unless_a_text_field_has_focus(app: &mut AppUnderTest, hub: WindowId) {
    use SelectionMode::{Idle, Playback, Static};
    let width = app.bounds(hub).width;
    // `body.fancy-title-bar-win` puts the buttons below the title bar.
    let y = if cfg!(windows) { 63.0 } else { 22.0 };
    let (play, popup) = ((width - 130.0, y), (width - 59.0, y));
    let (search, below) = ((100.0, 87.0), (100.0, 400.0));
    assert_eq!(app.mode(hub), Idle);
    app.key(hub, "space");
    assert_eq!(app.mode(hub), Playback);
    app.key(hub, "space");
    assert_eq!(app.mode(hub), Static);

    app.click_at(hub, play.0, play.1);
    assert_eq!(app.mode(hub), Playback);
    app.key(hub, "space");
    assert_eq!(app.mode(hub), Static, "with the pressed button hidden");

    app.click_at(hub, search.0, search.1);
    for key in ["space", "l", "left"] {
        app.key(hub, key);
        assert_eq!(app.mode(hub), Static, "{key} in the search field");
    }
    app.click_at(hub, below.0, below.1);
    app.key(hub, "space");
    assert_eq!(app.mode(hub), Playback, "with the search field blurred");

    app.click_at(hub, popup.0, popup.1);
    let [satellite] = app.satellites()[..] else {
        panic!("{:?}", app.satellites());
    };
    app.focus(hub);
    app.key(hub, "space");
    assert_eq!(app.mode(hub), Static, "with a button focused");
    assert_eq!(app.satellites(), [satellite], "the button is not clicked");
    app.focus(satellite);
    app.key(satellite, "space");
    assert_eq!(app.mode(hub), Playback, "from a satellite");
    app.key(satellite, "space");
    app.close(satellite);
    app.focus(hub);
}

fn the_beta_welcome_is_acknowledged(app: &mut AppUnderTest, hub: WindowId) {
    let file = app.user_data("beta-2027.json");
    assert!(!file.exists());
    let window = app.window(WindowKind::BetaWelcome).expect("the welcome");
    let bounds = app.bounds(window);
    let height = if cfg!(windows) { 530.0 } else { 490.0 };
    assert_eq!((bounds.width, bounds.height), (450.0, height));
    for _ in 0..3 {
        app.tab(window, 1, false);
        app.press(window, "enter", &[]);
        assert!(!file.exists());
        app.press(window, "space", &[]);
    }
    assert_eq!(app.window(WindowKind::BetaWelcome), Some(window));
    app.tab(window, 1, false);
    app.press(window, "enter", &[]);
    assert_eq!(app.window(WindowKind::BetaWelcome), None);
    let written = std::fs::read_to_string(&file).expect("beta-2027.json");
    assert_eq!(
        written,
        "{\"welcomeComplete\":true,\"surveyStatus\":false}\n"
    );
    assert_eq!(app.hubs(), [hub]);
}

fn the_licenses_window_opens_once(app: &mut AppUnderTest, hub: WindowId) {
    let file = app.home.join("licenses.json");
    app.cx
        .update(|cx| cx.set_global(LicensesFile(file.clone())));
    let shown = app.click(Command::OpenLicenses, &[Reply::Button(0)]);
    let (_, kind, text, _) = message(&shown);
    assert_eq!(
        (kind, text),
        (MessageBoxType::Error, "The licenses could not be read")
    );
    assert_eq!(app.window(WindowKind::Licenses), None);

    let list = r#"[{"module":"AdvantageScope","text":"a\n\nb"},{"module":"z","text":"- c"}]"#;
    std::fs::write(&file, list).expect("licenses.json");
    assert!(app.click(Command::OpenLicenses, &[]).is_empty());
    let window = app.window(WindowKind::Licenses).expect("the licenses");
    let (bounds, parent) = (app.bounds(window), app.bounds(hub));
    assert_eq!((bounds.width, bounds.height), (600.0, 625.0));
    let center = |start: f64, outer: f64, inner: f64| (start + outer / 2.0 - inner / 2.0).floor();
    assert_eq!(bounds.x, center(parent.x, parent.width, 600.0));
    assert_eq!(bounds.y, center(parent.y, parent.height, 625.0));
    app.focus(hub);
    assert!(app.click(Command::OpenLicenses, &[]).is_empty());
    assert_eq!(app.window(WindowKind::Licenses), Some(window));
    let modules = app.cx.update(|cx| {
        let licenses = licenses::load(cx).expect("the list");
        licenses
            .iter()
            .map(|l| (l.module.len(), l.parts.len()))
            .collect::<Vec<_>>()
    });
    assert_eq!(modules, [(1, 3), (1, 1)]);
    app.close(window);
    assert_eq!(app.window(WindowKind::Licenses), None);
    app.focus(hub);
}

fn help_windows_open_beside_the_hub(app: &mut AppUnderTest, hub: WindowId) {
    let parent = app.bounds(hub);
    let mut opened = Vec::new();
    let path = app.prefs_path();
    let saved = std::fs::read_to_string(&path).expect("prefs.json");
    let edited = saved.replace("\"liveDiscard\":1200", "\"liveDiscard\":1100");
    assert_ne!(edited, saved);
    for (config, prefs, discard) in [
        (configs::line_graph_numeric(), edited, 1100.0),
        (configs::mechanism(), saved, 1200.0),
    ] {
        std::fs::write(&path, prefs).expect("prefs.json");
        let data = config.to_js();
        let opened_help = app
            .cx
            .update(|cx| MainProcess::open_source_list_help(cx, hub, &data));
        let (window, page) = opened_help.expect("a help window");
        app.settle();
        let held = app.hub(hub, |hub| hub.page().preferences().map(|p| p.live_discard));
        assert_eq!(held, Some(discard));
        let bounds = app.bounds(window);
        assert_eq!((bounds.width, bounds.height), (350.0, 500.0));
        assert_eq!(bounds.x, parent.x + 30.0);
        assert_eq!(bounds.y, (parent.y + parent.height / 2.0 - 250.0).floor());
        let help = app.cx.update(|cx| page.read(cx).help().clone());
        assert_eq!(help.title(), format!("{} Help", config.title));
        let documented = config.types.iter().filter(|t| t.show_docs).count();
        assert_eq!(help.items().len(), documented);
        opened.push(window);
    }
    assert_ne!(opened[0], opened[1]);
    for window in opened {
        app.close(window);
    }
    app.focus(hub);
}

/// Points the preferences at 127.0.0.1 and returns the file as it was.
fn write_robot_address(app: &AppUnderTest) -> Vec<u8> {
    let path = app.prefs_path();
    let before = std::fs::read(&path).expect("prefs.json");
    let mut prefs = json_file::parse(&before).expect("prefs.json");
    let object = prefs.as_object_mut().expect("prefs.json is an object");
    object.insert("robotAddress", JsValue::string("127.000.0.1"));
    object.insert("systemcoreStaticAddress", JsValue::string(""));
    std::fs::write(&path, json_file::stringify(&prefs).expect("JSON")).expect("prefs.json");
    before
}

fn assert_connected(connection: &mut TcpStream) {
    let timeout = Some(Duration::from_millis(300));
    connection.set_read_timeout(timeout).expect("a timeout");
    let open = connection.read_to_end(&mut Vec::new());
    let open = open.expect_err("no end yet");
    assert!(
        matches!(open.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
        "{open}"
    );
}

fn assert_ends(connection: &mut TcpStream, what: &str) {
    let timeout = Some(Duration::from_secs(60));
    connection.set_read_timeout(timeout).expect("a timeout");
    connection
        .read_to_end(&mut Vec::new())
        .unwrap_or_else(|e| panic!("{what}: {e}"));
}

fn closing_licenses_or_help_stops_the_download(
    app: &mut AppUnderTest,
    hub: WindowId,
    robot: &TcpListener,
) {
    let before = write_robot_address(app);
    assert!(app.click(Command::NewPopOut, &[]).is_empty());
    let [satellite] = app.satellites()[..] else {
        panic!("{:?}", app.satellites());
    };
    app.focus(hub);
    assert!(app.click(Command::DownloadLogs, &[]).is_empty());
    let download = app.window(WindowKind::Download);
    let download = download.expect("the download window");
    let (mut connection, _) = robot.accept().expect("a connection");

    app.focus(hub);
    assert!(app.click(Command::OpenLicenses, &[]).is_empty());
    let licenses = app.window(WindowKind::Licenses).expect("the licenses");
    assert_connected(&mut connection);
    app.close(licenses);
    assert_ends(&mut connection, "closing the licenses");

    app.cx.update(MainProcess::send_all_preferences);
    let (mut connection, _) = robot.accept().expect("a connection");
    assert_connected(&mut connection);
    let told = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = [told.clone(), told.clone(), told.clone()];
    let (opened, held) = app.cx.update(|cx| {
        let pages = MainProcess::hub_windows(cx);
        let page = pages[0].1.upgrade().expect("a live hub");
        let (_, pop_out) = MainProcess::satellite_windows(cx).remove(0);
        let view = download_flow::view(cx).expect("the download window's view");
        let [hubs, satellites, downloads] = seen;
        let held = [
            cx.observe(&page, move |_, _| hubs.borrow_mut().push("hub")),
            cx.observe(&pop_out, move |_, _| {
                satellites.borrow_mut().push("satellite")
            }),
            cx.subscribe(&view, move |_, event: &PageEvent, _| {
                if matches!(event, PageEvent::Start { .. }) {
                    downloads.borrow_mut().push("download");
                }
            }),
        ];
        let data = configs::mechanism().to_js();
        (MainProcess::open_source_list_help(cx, hub, &data), held)
    });
    drop(held);
    let (help, _) = opened.expect("a help window");
    app.settle();
    assert_eq!(*told.borrow(), ["hub", "satellite", "download"]);
    assert_ends(&mut connection, "starting again");
    let (mut connection, _) = robot.accept().expect("a connection");
    assert_connected(&mut connection);
    app.close(help);
    assert_ends(&mut connection, "closing the help");

    app.close(download);
    app.close(satellite);
    std::fs::write(app.prefs_path(), before).expect("prefs.json");
    app.cx.update(MainProcess::send_all_preferences);
    app.focus(hub);
}

fn messages_reach_the_focused_hub(app: &mut AppUnderTest, hub: WindowId) {
    let before = app.tabs(hub);
    assert!(
        app.click(Command::NewTab(TabType::Statistics), &[])
            .is_empty()
    );
    let after = app.tabs(hub);
    assert_eq!(after.len(), before.len() + 1);
    assert!(after.contains(&Some(TabType::Statistics)));
    let selected = app.hub(hub, |hub| hub.tabs().selected_index());
    app.click(Command::MoveTab(-1), &[]);
    assert_ne!(app.hub(hub, |hub| hub.tabs().selected_index()), selected);
    app.click(Command::MoveTab(1), &[]);
    app.click(Command::CloseTab, &[]);
    assert_eq!(app.tabs(hub), before);
}

fn preferences_reach_the_file_the_hubs_and_the_menu(app: &mut AppUnderTest, hub: WindowId) {
    let address = Command::UseSystemcoreAddress(SystemcoreStaticAddress::Usb);
    for (checked, value) in [(true, "usb"), (false, ""), (true, "usb")] {
        assert!(app.click_checked(address, checked, &[]).is_empty());
        assert_eq!(app.pref("systemcoreStaticAddress"), JsValue::string(value));
        let held = app.hub(hub, |hub| {
            hub.page()
                .preferences()
                .map(|p| p.systemcore_static_address.as_str())
        });
        assert_eq!(held, Some(value));
    }
    let phoenix = Command::StartLive {
        simulator: true,
        mode: Some(LiveMode::Phoenix),
    };
    assert!(app.click(phoenix, &[]).is_empty());
    assert_eq!(app.pref("liveMode"), JsValue::string("phoenix"));
    let held = app.hub(hub, |hub| hub.page().preferences().map(|p| p.live_mode));
    assert_eq!(held, Some(LiveMode::Phoenix));
    let (label, usb) = app.cx.update(|cx| {
        let menu = MainProcess::menu(cx);
        let item = |path: &[usize]| MenuItem::at(menu, path).expect("an item");
        (item(&[1, 3, 0]).label.clone(), item(&[1, 7]).checked)
    });
    assert_eq!(label.as_deref(), Some("Default: Phoenix Diagnostics"));
    assert_eq!(usb, Some(true));

    let custom = app.home.join("custom assets");
    std::fs::create_dir_all(&custom).expect("custom assets");
    let command = Command::UseCustomAssetsFolder;
    let shown = app.click_checked(command, true, &[open(&[&custom])]);
    let [(None, Shown::Open(options))] = &shown[..] else {
        panic!("{shown:?}");
    };
    assert!(options.open_directory && options.create_directory && !options.open_file);
    assert_eq!(
        app.pref("userAssetsFolder"),
        JsValue::string(&custom.to_string_lossy())
    );
    assert!(app.click_checked(command, false, &[]).is_empty());
    assert_eq!(app.pref("userAssetsFolder"), JsValue::Null);

    let saved = std::fs::read(app.prefs_path()).expect("prefs.json");
    std::fs::write(app.prefs_path(), "{").expect("prefs.json");
    let shown = app.click_checked(address, true, &[Reply::Button(0)]);
    let (window, kind, text, _) = message(&shown);
    assert_eq!(
        (window, kind, text),
        (Some(hub), MessageBoxType::Error, "The command failed")
    );
    std::fs::write(app.prefs_path(), saved).expect("prefs.json");
}

fn the_preferences_window_writes_the_file(app: &mut AppUnderTest, hub: WindowId) {
    let path = app.prefs_path();
    let read = || std::fs::read_to_string(&path).expect("prefs.json");
    let held = |app: &mut AppUnderTest| {
        app.hub(hub, |hub| {
            let prefs = hub.page().preferences().expect("preferences");
            (prefs.theme, prefs.timestamps)
        })
    };
    let (theme, dark) = if cfg!(target_os = "linux") {
        ("light", "d")
    } else {
        ("system", "always d")
    };

    let before = read();
    assert!(app.click(Command::OpenPreferences, &[]).is_empty());
    let window = app.preferences().expect("the preferences window");
    let bounds = app.bounds(window);
    let parent = app.bounds(hub);
    assert_eq!((bounds.width, bounds.height), (400.0, 473.0));
    let center = |start: f64, outer: f64, inner: f64| (start + outer / 2.0 - inner / 2.0).floor();
    assert_eq!(bounds.x, center(parent.x, parent.width, 400.0));
    assert_eq!(bounds.y, center(parent.y, parent.height, 473.0));
    app.focus(window);
    assert!(app.click(Command::OpenPreferences, &[]).is_empty());
    assert_eq!(app.preferences(), Some(window));

    app.tab(window, 1, false);
    app.type_text(window, dark);
    app.tab(window, 2, false);
    app.type_text(window, "o");
    assert!(app.press(window, "enter", &[]).is_empty());
    assert_eq!(app.preferences(), None);
    let after = before
        .replace(&format!("\"theme\":\"{theme}\""), "\"theme\":\"dark\"")
        .replace(
            "\"timestamps\":\"start-zero\"",
            "\"timestamps\":\"original\"",
        );
    assert_ne!(after, before);
    assert_eq!(read(), after);
    assert_eq!(held(app), (Theme::Dark, Timestamps::Original));
    let source = app.cx.update(|cx| *cx.global::<ThemeSource>());
    assert_eq!(source, ThemeSource::Dark);
    app.send(hub, Vec::new());
    assert!(app.hub(hub, |hub| hub.page().tab_dark()));

    app.focus(hub);
    let spaced = after.replace(":", ": ");
    std::fs::write(&path, &spaced).expect("prefs.json");
    app.click(Command::OpenPreferences, &[]);
    let window = app.preferences().expect("the preferences window");
    app.tab(window, 1, false);
    app.type_text(window, "m");
    app.tab(window, 2, true);
    assert!(app.press(window, "enter", &[]).is_empty());
    assert_eq!(app.preferences(), None);
    assert_eq!(
        read(),
        after,
        "Enter on the exit button confirms, then cancels"
    );
    assert_eq!(held(app), (Theme::Dark, Timestamps::Original));

    app.focus(hub);
    std::fs::write(&path, "{").expect("prefs.json");
    let shown = app.click(Command::OpenPreferences, &[Reply::Button(0)]);
    assert_eq!(message(&shown).2, "The command failed");
    let window = app.preferences().expect("the preferences window");
    assert!(app.press(window, "enter", &[]).is_empty());
    assert_eq!(app.preferences(), Some(window));
    assert_eq!(read(), "{");
    app.close(window);
    assert_eq!(app.preferences(), None);
    std::fs::write(&path, before).expect("prefs.json");
    app.focus(hub);
}

fn dialogs_show_and_resume(app: &mut AppUnderTest, hub: WindowId) {
    let log = app.home.join("opened.csv");
    std::fs::write(&log, "Timestamp,/Value\n0,1\n").expect("a log");
    assert_eq!(app.click(Command::OpenLogs, &[open(&[])]).len(), 1);
    assert!(app.historical_paths(hub).is_empty());
    let shown = app.click(Command::OpenLogs, &[open(&[&log])]);
    let [(Some(parent), Shown::Open(options))] = &shown[..] else {
        panic!("{shown:?}");
    };
    assert_eq!(*parent, hub);
    assert_eq!(options.title, "Select the robot log file(s) to open");
    assert!(options.open_file && options.multi_selections && !options.open_directory);
    assert_eq!(options.filters[0].name, "Robot logs");
    assert_eq!(options.filters[0].extensions.len(), 9);
    assert_eq!(app.historical_paths(hub), [log.to_string_lossy()]);

    let empty = app.home.join("empty.csv");
    std::fs::write(&empty, "").expect("a log");
    let shown = app.click(Command::OpenLogs, &[open(&[&empty]), Reply::Button(0)]);
    let [(_, Shown::Open(_)), error] = &shown[..] else {
        panic!("{shown:?}");
    };
    let (window, kind, text, detail) = message(std::slice::from_ref(error));
    assert_eq!((window, kind), (Some(hub), MessageBoxType::Error));
    assert_eq!(text, "Failed to open log");
    assert!(detail.ends_with("the required format of CSV files."));

    let shown = app.click(Command::About, &[Reply::Button(1)]);
    let (window, kind, text, detail) = message(&shown);
    assert_eq!((window, kind), (unparented(hub), MessageBoxType::Info));
    assert_eq!(text, "AdvantageScope");
    let copied = app.cx.update(|cx| cx.read_from_clipboard());
    let copied = copied.and_then(|item| item.text()).expect("copied text");
    assert!(copied.starts_with("Version: ") && detail.ends_with(&copied));

    let shown = app.click(Command::OwletDownloadStatus, &[Reply::Button(0)]);
    assert_eq!(message(&shown).3, HOOT_UNSUPPORTED);

    let shown = app.click(Command::CheckForUpdates, &[Reply::Button(0)]);
    let (window, kind, text, detail) = message(&shown);
    assert_eq!((window, kind), (unparented(hub), MessageBoxType::Info));
    assert_eq!(text, "Cannot check for updates");
    assert!(detail.contains("development environment"));

    let before = app.tabs(hub);
    for (button, resets) in [(1, false), (0, true)] {
        let shown = app.click(Command::ResetToDsLayout, &[Reply::Button(button)]);
        let [(Some(parent), Shown::Message(options))] = &shown[..] else {
            panic!("{shown:?}");
        };
        assert_eq!(*parent, hub);
        assert_eq!(options.buttons, ["Reset", "Cancel"]);
        assert_eq!((options.default_id, options.cancel_id), (Some(0), Some(1)));
        assert_eq!(app.tabs(hub) != before, resets);
    }
    let help = Command::Help(HelpLink::GitHubRepository);
    assert!(app.click(help, &[]).is_empty());
}

/// Main's side of the download window, against a port of 127.0.0.1 nothing listens on.
fn the_download_window_shows_what_the_service_sends(
    app: &mut AppUnderTest,
    hub: WindowId,
    port: u16,
) {
    let before = write_robot_address(app);

    let download = |app: &mut AppUnderTest| app.window(WindowKind::Download);
    assert!(app.click(Command::DownloadLogs, &[]).is_empty());
    let window = download(app).expect("the download window");
    let view = app
        .cx
        .update(|cx| download_flow::view(cx))
        .expect("its view");
    let (bounds, parent) = (app.bounds(window), app.bounds(hub));
    assert_eq!((bounds.width, bounds.height), (500.0, 500.0));
    assert_eq!(
        (bounds.x, bounds.y),
        (
            (parent.x + parent.width / 2.0 - 250.0).floor(),
            (parent.y + parent.height / 2.0 - 250.0).floor()
        )
    );
    app.focus(hub);
    assert!(app.click(Command::DownloadLogs, &[]).is_empty());
    assert_eq!(download(app), Some(window), "it is not opened twice");

    let alert = |app: &mut AppUnderTest| {
        let html = app.cx.update(|cx| {
            let page = view.read(cx).page();
            page.alert().map(|html| html.to_string_lossy())
        });
        html.filter(|html| html.contains("ECONNREFUSED"))
    };
    let started = std::time::Instant::now();
    let refused = loop {
        app.settle();
        if let Some(html) = alert(app) {
            break html;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the service reported no refused connection"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(
        refused,
        format!("Unknown error: connect ECONNREFUSED 127.0.0.1:{port}")
    );

    let starts = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = starts.clone();
    app.cx.update(|cx| {
        cx.subscribe(&view, move |_, event: &PageEvent, _| {
            if let PageEvent::Start { address, path } = event {
                let event = (address.to_string_lossy(), path.to_string_lossy());
                seen.borrow_mut().push(event);
            }
        })
        .detach();
        MainProcess::send_all_preferences(cx);
    });
    app.settle();
    let remote = app.pref("remotePath");
    let remote = remote.as_str().expect("remotePath").to_string_lossy();
    assert_eq!(*starts.borrow(), [("127.0.0.1".to_owned(), remote)]);

    let deliver = |app: &mut AppUnderTest, event| {
        app.cx.update(|cx| download_flow::deliver(event, cx));
        app.settle();
    };
    let names = |app: &mut AppUnderTest| {
        app.cx.update(|cx| {
            let rows = view.read(cx).page().rows();
            let items = rows.iter().filter_map(|row| row.item.as_ref());
            items.map(|item| item.name.clone()).collect::<Vec<_>>()
        })
    };
    let list = |names: &[&str]| {
        let item = |name: &&str| ListItem {
            name: (*name).to_owned(),
            size: Some(1e6),
            is_folder: false,
        };
        let message = WindowMessage::SetList(names.iter().map(item).collect());
        DownloadEvent::Window(message)
    };
    app.cx.update(|cx| {
        download_flow::deliver(list(&["a.wpilog", "b.wpilog"]), cx);
    });
    assert_eq!(names(app), ["b.wpilog", "a.wpilog"]);

    let folder = app.home.join("chosen");
    let shown = app.run(&[open(&[&folder])], |app| {
        let dialog = OpenDialog {
            title: "Select save location for robot logs".to_owned(),
            message: None,
            button_label: Some("Save"),
            properties: &["openDirectory", "createDirectory", "dontAddToRecent"],
            filters: None,
        };
        deliver(app, DownloadEvent::ChooseFolder { token: 0, dialog });
    });
    let [(Some(parent), Shown::Open(options))] = &shown[..] else {
        panic!("{shown:?}");
    };
    assert_eq!((*parent, options.button_label.as_str()), (window, "Save"));
    assert!(options.open_directory && options.create_directory && !options.open_file);

    let shown = app.run(&[Reply::Save(None)], |app| {
        let dialog = SaveLogDialog {
            title: "Select save location for robot log",
            default_path: "a.wpilog".to_owned(),
            properties: &["createDirectory"],
            filter_name: "WPILib robot log",
            extension: "wpilog".to_owned(),
        };
        deliver(app, DownloadEvent::ChooseFile { token: 0, dialog });
    });
    let [(Some(parent), Shown::Save(options))] = &shown[..] else {
        panic!("{shown:?}");
    };
    assert_eq!(*parent, window);
    assert_eq!(options.default_path, Some(PathBuf::from("a.wpilog")));
    assert_eq!(options.filters[0].extensions, ["wpilog"]);
    assert!(options.create_directory);

    let shown = app.run(&[Reply::Button(1)], |app| {
        let options = OpenLogBox {
            kind: "question",
            message: "Open log?",
            detail: "Would you like to open the log file \"a.wpilog\"?".to_owned(),
            buttons: ["Open", "Skip"],
            default_id: 0,
        };
        deliver(app, DownloadEvent::AskOpenLog { token: 0, options });
    });
    let (parent, kind, text, _) = message(&shown);
    assert_eq!(
        (parent, kind, text),
        (Some(window), MessageBoxType::Question, "Open log?")
    );

    let log = app.home.join("downloaded.csv");
    std::fs::write(&log, "Timestamp,/Value\n0,1\n").expect("a log");
    let log = log.to_string_lossy().into_owned();
    deliver(app, DownloadEvent::FocusHub);
    let focused = app.cx.update(|cx| Windows::manager(cx).focused());
    assert_eq!(focused, Some(hub));
    deliver(app, DownloadEvent::OpenFiles(vec![log.clone()]));
    assert_eq!(app.historical_paths(hub), [log]);

    deliver(app, DownloadEvent::DestroyWindow);
    assert_eq!(download(app), None);
    deliver(app, list(&["c.wpilog"]));

    app.cx.update(|cx| {
        let command = MenuAction::Command {
            command: Command::DownloadLogs,
            checked: false,
        };
        dispatch(command, cx);
        let view = download_flow::view(cx).expect("the download window");
        download_flow::deliver(list(&["stale.wpilog"]), cx);
        assert!(view.read(cx).page().rows().is_empty());
    });
    let window = download(app).expect("the download window");
    app.close(window);
    assert!(app.cx.update(|cx| download_flow::view(cx)).is_none());

    std::fs::write(app.prefs_path(), before).expect("prefs.json");
    app.cx.update(MainProcess::send_all_preferences);
    app.focus(hub);
}

fn accelerators_reach_the_window_they_were_pressed_in(app: &mut AppUnderTest, hub: WindowId) {
    app.cx.update(|cx| {
        let template =
            app_menu::template(Platform::Linux, &Preferences::default(), asrs_app::IDENTITY);
        bridge::install_application_menu(
            cx,
            Platform::Linux,
            &template,
            std::rc::Rc::new(dispatch),
        );
    });
    let log = app.home.join("pressed.csv");
    std::fs::write(&log, "Timestamp,/Value\n0,1\n").expect("a log");
    let shown = app.press(hub, "ctrl-o", &[open(&[&log])]);
    let [(Some(parent), Shown::Open(_))] = &shown[..] else {
        panic!("ctrl-o: {shown:?}");
    };
    assert_eq!(*parent, hub);
    assert_eq!(app.historical_paths(hub), [log.to_string_lossy()]);

    assert!(app.press(hub, "ctrl-,", &[]).is_empty());
    let preferences = app.preferences().expect("the preferences window");
    app.close(preferences);
    app.focus(hub);
    let shown = app.press(hub, "ctrl-r", &[Reply::Button(0)]);
    assert_eq!(message(&shown).0, Some(hub));

    for on in [true, false] {
        assert!(app.press(hub, "f11", &[]).is_empty());
        assert_eq!(app.fullscreen(hub), on);
    }
}

fn windows_open_and_close(app: &mut AppUnderTest, hub: WindowId) -> WindowId {
    let from = app.bounds(hub);
    assert!(app.click(Command::NewWindow, &[]).is_empty());
    let hubs = app.hubs();
    let [_, new] = hubs[..] else {
        panic!("{hubs:?}");
    };
    let to = app.bounds(new);
    assert_eq!((to.x, to.y), (from.x + 30.0, from.y + 30.0));
    assert_eq!((to.width, to.height), (from.width, from.height));
    app.focus(new);
    assert_eq!(app.hubs(), [new, hub]);
    app.click(Command::BringAllToFront, &[]);
    app.focus(hub);
    app.click(Command::CloseWindow, &[]);
    assert_eq!(app.hubs(), [new]);
    app.focus(new);
    new
}

fn layouts_round_trip(app: &mut AppUnderTest, hub: WindowId) {
    let file = app.home.join("layout.json");
    let shown = app.click(Command::ExportLayout, &[Reply::Save(None)]);
    let [(None, Shown::Save(options))] = &shown[..] else {
        panic!("{shown:?}");
    };
    let suggested = options.default_path.as_ref().expect("a default path");
    let suggested = suggested.to_string_lossy();
    assert!(suggested.starts_with("AdvantageScope ") && suggested.ends_with(".json"));
    assert!(!file.exists());

    app.click(Command::NewTab(TabType::Metadata), &[]);
    let tabs = app.tabs(hub);
    app.cx.advance_clock(Duration::from_secs(2));
    app.settle();
    let state = app.hub(hub, HubModel::save_state);
    app.click(Command::ExportLayout, &[Reply::Save(Some(file.clone()))]);
    let bytes = std::fs::read(&file).expect("the exported layout");
    let layout = json_file::parse(&bytes).expect("JSON");
    assert_eq!(json_file::stringify_pretty(&layout).expect("JSON"), bytes);
    let version = app
        .cx
        .update(|cx| MainProcess::with_click_env(cx, |env| env.paths.app_version().to_owned()));
    let version = version.expect("the main process");
    assert_eq!(layout.get("version"), Some(&JsValue::string(&version)));
    let hubs = layout.get("hubs").and_then(JsValue::as_array);
    let [saved] = hubs.expect("hubs") else {
        panic!("{hubs:?}");
    };
    assert_eq!(saved.get("state"), Some(&state));
    let bounds = app.bounds(hub);
    let x = saved.get("x").and_then(JsValue::as_f64);
    assert_eq!(x, Some(bounds.x));

    let emptied = std::rc::Rc::new(std::cell::Cell::new(false));
    let seen = emptied.clone();
    app.cx.update(|cx| {
        cx.on_window_closed(move |cx, _| seen.set(seen.get() || cx.windows().is_empty()))
            .detach();
    });
    app.click(Command::CloseTab, &[]);
    assert_ne!(app.tabs(hub), tabs);
    let replies = [open(&[&file, &file])];
    let shown = app.click(Command::ImportLayout, &replies);
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert!(!emptied.get(), "closing the last window quits off macOS");
    let hubs = app.hubs();
    assert_eq!(hubs.len(), 2);
    assert!(!hubs.contains(&hub));
    for new in hubs {
        assert_eq!(app.tabs(new), tabs);
        assert_eq!(app.bounds(new), bounds);
    }

    let old = app.home.join("old.json");
    std::fs::write(&old, r#"{"layout":[]}"#).expect("a layout");
    let replies = [open(&[&old]), Reply::Button(0)];
    let shown = app.click(Command::ImportLayout, &replies);
    assert_eq!(shown.len(), 2);
    let (_, kind, text, _) = message(&shown[1..]);
    assert_eq!(
        (kind, text),
        (MessageBoxType::Error, "Failed to import layout")
    );
    assert_eq!(app.hubs().len(), 2);
}
