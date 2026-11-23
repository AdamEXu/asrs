#![allow(clippy::disallowed_methods)]

//! The application menu in the app, headless: `main_process::start` in a fresh home directory,
//! then clicks through `menus::dispatch` and keystrokes through the accelerator table, with the
//! dialogs answered by a script. What the handlers do is `menu_click_vectors`' business; this
//! checks that their effects reach the hubs, the windows, the files and the dialogs.

use std::io::{ErrorKind, Read};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::Arc;
use std::time::Duration;

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
use asrs_model::source_list::configs;
use asrs_model::tab_type::TabType;
use asrs_services::download::client::Target;
use asrs_services::download::{DownloadConfig, DownloadService};
use asrs_services::historical_start::HOOT_UNSUPPORTED;
use asrs_services::lifecycle;
use asrs_services::path::PathStyle;
use asrs_services::window_manager::Platform;
use asrs_ui::gpui::{HeadlessAppContext, KeyUpEvent, Keystroke, NoopTextSystem, PlatformInput};
use asrs_ui::platform::dialog::{Reply, Script, Shown};
use asrs_ui::theme::ThemeSource;

const HOME: &str = "ASRS_MENU_DISPATCH_HOME";

#[test]
fn the_menu_in_the_app() {
    let home = std::env::temp_dir().join(format!("asrs-menu-dispatch-{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("home");
    let mut child = Process::new(std::env::current_exe().expect("the test binary"));
    child.args(["--exact", "in_a_fresh_home", "--ignored", "--nocapture"]);
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
    let status = child.status().expect("the test binary");
    let _ = std::fs::remove_dir_all(&home);
    assert!(status.success(), "{status}");
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
        self.cx.run_until_parked();
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
            let window = Windows::of_kind(cx, kind)?;
            Windows::id_of(cx, window)
        })
    }

    fn preferences(&mut self) -> Option<WindowId> {
        self.window(WindowKind::Preferences)
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

    messages_reach_the_focused_hub(&mut app, hub);
    preferences_reach_the_file_the_hubs_and_the_menu(&mut app, hub);
    dialogs_show_and_resume(&mut app, hub);
    the_preferences_window_writes_the_file(&mut app, hub);
    the_licenses_window_opens_once(&mut app, hub);
    help_windows_open_beside_the_hub(&mut app, hub);
    closing_licenses_or_help_stops_the_download(&mut app, hub);
    accelerators_reach_the_window_they_were_pressed_in(&mut app, hub);
    let hub = windows_open_and_close(&mut app, hub);
    layouts_round_trip(&mut app, hub);
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

fn closing_licenses_or_help_stops_the_download(app: &mut AppUnderTest, hub: WindowId) {
    for kind in [WindowKind::Licenses, WindowKind::SourceListHelp] {
        let robot = TcpListener::bind("127.0.0.1:0").expect("a port");
        let port = robot.local_addr().expect("an address").port();
        let mut config = DownloadConfig::new(PathStyle::host());
        let target = config.target;
        config.target = Box::new(move |options| Target {
            port,
            ready_timeout_ms: 600_000.0,
            ..target(options)
        });
        let (service, _events) = DownloadService::spawn(config).expect("the service");
        service.window_opened();
        service.start("127.0.0.1", "/");
        let (mut connection, _) = robot.accept().expect("a connection");
        app.cx.update(|cx| MainProcess::set_download(cx, service));
        if kind == WindowKind::Licenses {
            assert!(app.click(Command::OpenLicenses, &[]).is_empty());
        } else {
            let data = configs::mechanism().to_js();
            app.cx
                .update(|cx| MainProcess::open_source_list_help(cx, hub, &data))
                .expect("a help window");
        }
        app.settle();
        let timeout = Some(Duration::from_millis(300));
        connection.set_read_timeout(timeout).expect("a timeout");
        let mut ident = Vec::new();
        let open = connection.read_to_end(&mut ident).expect_err("no end yet");
        assert!(
            matches!(open.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
            "{open}"
        );
        let window = app.window(kind).expect("the window");
        app.close(window);
        let timeout = Some(Duration::from_secs(60));
        connection.set_read_timeout(timeout).expect("a timeout");
        connection
            .read_to_end(&mut ident)
            .unwrap_or_else(|e| panic!("{kind:?}: {e}"));
        app.focus(hub);
    }
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

    let shown = app.click(Command::DownloadLogs, &[Reply::Button(0)]);
    let (window, kind, text, _) = message(&shown);
    assert_eq!(
        (window, kind, text),
        (Some(hub), MessageBoxType::Error, "Not available yet")
    );

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
