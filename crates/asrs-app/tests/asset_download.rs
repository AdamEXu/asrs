#![allow(clippy::disallowed_methods)]

//! The asset download loop in the app, headless: `main_process::start` in a fresh home directory,
//! then the production host against a loopback server in place of GitHub. What the loop decides is
//! `asset_downloader`'s business; this checks that its requests, files, timers and reloads are real,
//! and that the app neither waits for it nor outlives it.

#[path = "support/asset_server.rs"]
mod asset_server;
#[path = "support/child.rs"]
mod child;

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use asrs_app::asset_download::{self, Config};
use asrs_app::main_process::{self, BetaTime, MainProcess};
use asrs_app::menus::bridge::MenuAction;
use asrs_app::menus::dispatch::dispatch;
use asrs_app::menus::model::Command;
use asrs_app::windows::{self, WindowId, Windows};
use asrs_hub::hub::HubModel;
use asrs_js::ops;
use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_model::messages::MainToHub;
use asrs_services::asset_downloader::{Distribution, INITIAL_STATUS};
use asrs_services::asset_loader::AssetFolders;
use asrs_services::lifecycle;
use asrs_services::path::PathStyle;
use asrs_services::window_manager::Platform;
use asrs_tabs::field_2d::{Field2dController, Field2dRenderer};
use asrs_ui::gpui::{HeadlessAppContext, NoopTextSystem};
use asrs_ui::platform::dialog::{Reply as Answer, Script, Shown};

use asset_server::{Gate, PATIENCE, RELEASES, Reply, Server, zip};

const HOME: &str = "ASRS_ASSET_DOWNLOAD_HOME";
const DEFAULT: &str = "default-assets-v2";
const FRC6328: &str = "frc-6328-assets-v2";
const DISK: &str = "Not enough disk space. Trying again soon.";
const FAILED: &str = "Cannot connect to GitHub. Trying again soon.";
const DONE: &str = "All assets downloaded. AdvantageScope will check for updates periodically.";
const FAILURE_MS: f64 = 30_000.0;
const SUCCESS_MS: f64 = 6.0 * 60.0 * 60.0 * 1000.0;
const FILE: u32 = 0o100_644;
const TAB: usize = 1;

static FREE: AtomicU64 = AtomicU64::new(1_000_000_000_000);

fn in_fresh_home(test: &str) {
    let home = std::env::temp_dir().join(format!("asrs-assets-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("home");
    let mut command = Process::new(std::env::current_exe().expect("the test binary"));
    command.args(["--exact", test, "--ignored", "--nocapture"]);
    for variable in [
        "HOME",
        "APPDATA",
        "XDG_CONFIG_HOME",
        "TMPDIR",
        "TEMP",
        "TMP",
        HOME,
    ] {
        command.env(variable, &home);
    }
    let status = child::run(&mut command, test);
    let _ = std::fs::remove_dir_all(&home);
    assert!(status.success(), "{test}: {status}");
}

macro_rules! in_a_fresh_home {
    ($($name:ident => $body:ident;)*) => {$(
        #[test]
        fn $name() {
            in_fresh_home(stringify!($body));
        }
    )*};
}

in_a_fresh_home! {
    a_parity_build_downloads_nothing => parity_in_a_fresh_home;
    new_assets_reach_open_tabs => tabs_in_a_fresh_home;
    failures_are_tried_again => failures_in_a_fresh_home;
    a_slow_server_holds_nothing_up => slow_in_a_fresh_home;
    a_connection_cut_inside_a_body_is_waited_for => cut_in_a_fresh_home;
    hostile_downloads_stay_in_the_folder => hostile_in_a_fresh_home;
    little_disk_space_waits => disk_in_a_fresh_home;
    quitting_ends_the_downloads => quit_in_a_fresh_home;
    stopping_ends_the_loop_wherever_it_waits => stop_in_a_fresh_home;
}

struct AppUnderTest {
    cx: HeadlessAppContext,
    hub: WindowId,
    server: Server,
    folders: AssetFolders,
    auto: PathBuf,
    /// When the last timer waited for was due.
    due: f64,
}

impl AppUnderTest {
    /// The app with one hub on a 2D field tab; `None` unless run by [`in_fresh_home`].
    fn start() -> Option<AppUnderTest> {
        std::env::var_os(HOME)?;
        let user_data =
            lifecycle::user_data_dir(Platform::current(), |name| std::env::var(name).ok());
        let user_data = user_data.expect("a user-data directory");
        std::fs::create_dir_all(&user_data).expect("user data");
        let beta = Path::new(&user_data).join("beta-2027.json");
        std::fs::write(beta, r#"{"welcomeComplete":true}"#).expect("beta-2027.json");
        let bundled = main_process::bundled_assets_dir();
        let folders = AssetFolders::new(PathStyle::host(), &user_data, &bundled.to_string_lossy());
        let mut cx = HeadlessAppContext::new(Arc::new(NoopTextSystem::new()));
        let (_, received) = async_channel::unbounded();
        cx.update(|cx| {
            asrs_ui::init(cx);
            windows::init(cx);
            cx.set_global(BetaTime(0.0));
            cx.set_global(Script::default());
            main_process::start(cx, &[], received);
        });
        cx.run_until_parked();
        let hub = cx.update(|cx| Windows::manager(cx).hubs()[0]);
        let tab = |kind: f64, uuid: &str, height: f64| {
            ops::object([
                ("type", JsValue::Number(kind)),
                ("title", JsValue::string("")),
                ("controller", JsValue::Null),
                ("controllerUUID", JsValue::string(uuid)),
                ("renderer", JsValue::Null),
                ("controlsHeight", JsValue::Number(height)),
            ])
        };
        let tabs = JsValue::Array(vec![
            tab(0.0, "documentation", 0.0),
            tab(2.0, "field200000000000000000000000000", 200.0),
        ]);
        let sidebar = ops::object([
            ("width", JsValue::Number(300.0)),
            ("expanded", JsValue::Array(Vec::new())),
        ]);
        let state = ops::object([
            ("sidebar", sidebar),
            (
                "tabs",
                ops::object([("selected", JsValue::Number(TAB as f64)), ("tabs", tabs)]),
            ),
        ]);
        cx.update(|cx| MainProcess::send_to_hub(cx, hub, MainToHub::RestoreState, state));
        cx.run_until_parked();
        Some(AppUnderTest {
            cx,
            hub,
            server: Server::start(),
            auto: PathBuf::from(folders.auto.to_string_lossy()),
            folders,
            due: 0.0,
        })
    }

    /// Starts the loop against the test's server.
    fn download(&mut self) {
        let config = Config {
            distribution: Distribution::Frc6328,
            api: self.server.url(""),
            free_space: |_| Some(FREE.load(Ordering::SeqCst) as f64),
            ..Config::new(self.folders.clone())
        };
        self.cx.update(|cx| asset_download::start(config, cx));
    }

    fn status(&mut self) -> String {
        self.cx.update(|cx| asset_download::status(cx))
    }

    fn skip(&mut self, ms: f64) {
        self.cx.update(|cx| asset_download::skip(ms, cx));
    }

    fn until(&mut self, what: &str, mut done: impl FnMut(&mut AppUnderTest) -> bool) {
        let start = Instant::now();
        while !done(self) {
            let status = self.status();
            assert!(start.elapsed() < PATIENCE, "waiting for {what}: {status}");
            self.cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Waits for the loop to set a timer of one of these delays.
    fn until_timer(&mut self, delays: std::ops::RangeInclusive<f64>) -> f64 {
        let mut found = None;
        let due = self.due;
        self.until("a timer", |app| {
            let timers = app.cx.update(|cx| asset_download::timers(cx));
            found = timers
                .into_iter()
                .find(|timer| delays.contains(&timer.delay_ms) && timer.due_ms > due);
            found.is_some()
        });
        let timer = found.expect("a timer");
        self.due = timer.due_ms;
        timer.delay_ms
    }

    /// Waits for a check to end, and gives the delay to the next.
    fn until_checked(&mut self) -> f64 {
        self.until_timer(FAILURE_MS..=SUCCESS_MS)
    }

    fn until_status(&mut self, expected: &str) {
        self.until(expected, |app| app.status() == expected);
    }

    fn frame(&mut self) {
        let window = self.cx.update(|cx| Windows::handle(cx, self.hub));
        self.cx
            .update_window(window.expect("open"), |_, window, cx| {
                window.simulate_next_frame(cx);
            })
            .expect("open");
        self.cx.run_until_parked();
    }

    fn model<R>(&mut self, f: impl FnOnce(&mut HubModel) -> R) -> R {
        self.cx.update(|cx| {
            let hubs = MainProcess::hub_windows(cx);
            let hub = hubs.iter().find(|(id, _)| *id == self.hub).expect("a hub");
            let hub = hub.1.upgrade().expect("a live hub");
            hub.update(cx, |hub, cx| hub.update(cx, |model, _| f(model)))
        })
    }

    /// The FRC fields the tab's select offers, as `(value, label)`.
    fn fields(&mut self) -> Vec<(String, String)> {
        self.model(|hub| {
            let tab = hub.tabs().tab(TAB).expect("the 2D field tab");
            let controller = tab.controller().downcast_ref::<Field2dController>();
            let controls = &controller.expect("a 2D field controller").controls;
            controls
                .frc
                .iter()
                .map(|(value, label)| (value.to_string_lossy(), label.to_string_lossy()))
                .collect()
        })
    }

    fn select(&mut self, field: &str) {
        self.model(|hub| {
            let (tabs, page) = hub.tabs_mut();
            let tab = tabs.tab_mut(TAB).expect("the 2D field tab");
            let controller = tab.controller.downcast_mut::<Field2dController>();
            let controller = controller.expect("a 2D field controller");
            controller.select_field(page, JsString::from(field));
        });
    }

    /// The natural size of the field image the tab shows from the folder.
    fn image_size(&mut self, folder: &str) -> Option<[f64; 2]> {
        self.model(|hub| {
            let tab = hub.tabs().tab(TAB).expect("the 2D field tab");
            let renderer = tab.renderer().downcast_ref::<Field2dRenderer>();
            let dom = renderer.expect("a 2D field renderer").dom().borrow();
            let mut images = dom.images.iter();
            images
                .find(|(path, _)| path.to_string_lossy().contains(folder))
                .map(|(_, size)| *size)
        })
    }

    /// What `Asset Download Status...` shows.
    fn shown_status(&mut self) -> String {
        self.cx.update(|cx| {
            cx.global_mut::<Script>().replies = vec![Answer::Button(0)].into();
            dispatch(
                MenuAction::Command {
                    command: Command::AssetDownloadStatus,
                    checked: false,
                },
                cx,
            );
        });
        self.cx.run_until_parked();
        let shown = self
            .cx
            .update(|cx| std::mem::take(&mut cx.global_mut::<Script>().shown));
        let [(_, Shown::Message(options))] = &shown[..] else {
            panic!("not one message box: {shown:?}");
        };
        assert_eq!(options.message, "Asset Download Status");
        options.detail.clone()
    }

    fn listing(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.auto)
            .expect("autoAssets")
            .map(|e| e.expect("autoAssets").file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

fn field_zip(name: &str) -> Vec<u8> {
    let bundled = main_process::bundled_assets_dir();
    let folder = bundled.join("Field2d_FRCEvergreen");
    let config = std::fs::read_to_string(folder.join("config.json")).expect("config.json");
    let config = config.replace("\"Evergreen\"", &format!("\"{name}\""));
    let image = std::fs::read(folder.join("image.png")).expect("image.png");
    zip(&[
        ("config.json", FILE, config.as_bytes()),
        ("image.png", FILE, &image),
    ])
}

/// An asset the loader skips.
fn small_zip(name: &str) -> Vec<u8> {
    let config = format!(r#"{{"name":"{name}"}}"#);
    zip(&[("config.json", FILE, config.as_bytes())])
}

#[test]
#[ignore = "run by a_parity_build_downloads_nothing with its own home directory"]
fn parity_in_a_fresh_home() {
    let Some(mut app) = AppUnderTest::start() else {
        return;
    };
    const { assert!(!asset_download::ENABLED) };
    app.cx.advance_clock(Duration::from_secs(20));
    app.cx.run_until_parked();
    assert_eq!(app.status(), INITIAL_STATUS);
    assert_eq!(app.shown_status(), INITIAL_STATUS);
    assert!(app.cx.update(asset_download::stop).is_none());
    assert_eq!(app.listing(), [""; 0]);
}

#[test]
#[ignore = "run by new_assets_reach_open_tabs with its own home directory"]
fn tabs_in_a_fresh_home() {
    let Some(mut app) = AppUnderTest::start() else {
        return;
    };
    let before = app.fields();
    let [(evergreen, _)] = &before[..] else {
        panic!("the bundled field: {before:?}");
    };
    std::fs::create_dir_all(app.auto.join("Robot_Stale/deep")).expect("a stale asset");
    std::fs::write(app.auto.join("stray.txt"), "stray").expect("a stray file");

    let gate = Gate::default();
    app.server.releases(DEFAULT, &["Field2d_2099Test.zip"]);
    let held = Reply::redirect(app.server.url("/object/1?sig=a%20b"));
    app.server.reply(
        "/Field2d_2099Test.zip",
        Reply::Hold(gate.clone(), Box::new(held)),
    );
    let data = field_zip("2099 Test");
    app.server.reply("/object/1?sig=a%20b", Reply::body(data));
    app.download();
    app.until("the asset's request", |app| {
        app.server.count("/Field2d_2099Test.zip") == 1
    });
    let started = app.shown_status();
    let time = started
        .strip_prefix("Download started at ")
        .and_then(|rest| {
            rest.strip_suffix(". Please wait a few minutes for the download to complete.")
        })
        .unwrap_or_else(|| panic!("{started}"));
    assert!(time.ends_with("AM") || time.ends_with("PM"), "{time}");
    assert_eq!(app.fields(), before);
    gate.open();
    app.until_status(DONE);
    assert_eq!(app.shown_status(), DONE);
    assert_eq!(app.listing(), ["Field2d_2099Test"]);
    let bundled = main_process::bundled_assets_dir();
    let image = "image.png";
    assert_eq!(
        std::fs::read(app.auto.join("Field2d_2099Test").join(image)).expect(image),
        std::fs::read(bundled.join("Field2d_FRCEvergreen").join(image)).expect(image)
    );

    app.until("the field in the tab", |app| app.fields().len() == 2);
    let added = app.fields();
    let (value, label) = added
        .iter()
        .find(|field| !before.contains(field))
        .expect("the new field");
    assert_eq!(label, "2099 Test");
    app.select(value);
    app.until("the field's image", |app| {
        app.frame();
        app.image_size("Field2d_2099Test").is_some()
    });
    let size = app.image_size("Field2d_2099Test").expect("the image");
    assert!(size[0] > 0.0 && size[1] > 0.0, "{size:?}");

    for (path, head) in app.server.requests() {
        let mut names: Vec<String> = head
            .lines()
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .map(|(name, _)| name.to_lowercase())
            .collect();
        names.sort();
        let expected: &[&str] = if path == RELEASES {
            assert!(head.contains("cache-control: no-cache\r\n"), "{head}");
            assert!(head.contains("pragma: no-cache\r\n"), "{head}");
            &[
                "accept",
                "accept-encoding",
                "cache-control",
                "host",
                "pragma",
                "user-agent",
            ]
        } else {
            &["accept-encoding", "host", "user-agent"]
        };
        assert_eq!(names, expected, "{head}");
        let agent = head
            .lines()
            .find_map(|line| line.strip_prefix("user-agent: "));
        let agent = agent.unwrap_or_else(|| panic!("no agent: {head}"));
        assert!(
            agent.starts_with("ureq/") || agent.starts_with("got/8.3.2 "),
            "{agent}"
        );
    }

    app.server.releases(FRC6328, &["Field2d_Next.zip"]);
    let next = Reply::body(field_zip("Next"));
    app.server.reply("/Field2d_Next.zip", next);
    assert_eq!(app.until_checked(), SUCCESS_MS);
    assert_eq!(app.server.count(RELEASES), 1);
    app.skip(SUCCESS_MS);
    assert_eq!(app.until_checked(), SUCCESS_MS);
    assert_eq!(app.server.count(RELEASES), 2);
    assert_eq!(app.status(), DONE);
    assert_eq!(app.listing(), ["Field2d_Next"]);
    app.until("the updated field in the tab", |app| {
        let fields = app.fields();
        fields.len() == 2 && fields.iter().any(|(_, label)| label == "Next")
    });
    assert!(app.fields().iter().any(|(value, _)| value == evergreen));
    assert_eq!(app.server.count("/Field2d_2099Test.zip"), 1);
}

#[test]
#[ignore = "run by failures_are_tried_again with its own home directory"]
fn failures_in_a_fresh_home() {
    let Some(mut app) = AppUnderTest::start() else {
        return;
    };
    let path = "/Field2d_A.zip";
    app.server.reply(RELEASES, Reply::status(500));
    app.download();
    let mut failed = 0;
    let mut fails = |app: &mut AppUnderTest| {
        failed += 1;
        assert_eq!(app.until_checked(), FAILURE_MS);
        assert_eq!(app.status(), FAILED);
        assert_eq!(app.server.count(RELEASES), failed);
    };
    fails(&mut app);
    for reply in [
        Reply::Drop,
        Reply::body("not json"),
        Reply::Cut(b"[{\"tag_name\":\"default-assets-v2\"}]".to_vec()),
    ] {
        app.server.reply(RELEASES, reply);
        app.skip(FAILURE_MS);
        fails(&mut app);
    }
    app.server.reply(RELEASES, Reply::Stall(b"[]".to_vec()));
    app.skip(FAILURE_MS);
    app.until("the stalled request", |app| app.server.count(RELEASES) == 5);
    app.until_timer(5_000.0..=5_000.0);
    app.skip(5_000.0);
    fails(&mut app);

    app.server.releases(DEFAULT, &["Field2d_A.zip"]);
    app.skip(FAILURE_MS);
    fails(&mut app);
    assert_eq!(app.server.count(path), 1);

    app.server.releases(DEFAULT, &["Field2d_A.zip"]);
    let corrupt = &b"PK\x03\x04 and no more"[..];
    app.server.reply(path, Reply::body(corrupt));
    app.skip(FAILURE_MS);
    fails(&mut app);
    assert_eq!(app.server.count(path), 2);
    assert!(!app.auto.join("Field2d_A").exists());

    app.server.releases(DEFAULT, &["Field2d_A.zip"]);
    for _ in 0..2 {
        app.server.reply(path, Reply::Drop);
    }
    app.server.reply(path, Reply::body(small_zip("A")));
    app.skip(FAILURE_MS);
    for backoff in [2_000.0, 4_000.0] {
        let delay = app.until_timer(backoff..=backoff + 100.0);
        app.skip(delay);
    }
    assert_eq!(app.until_checked(), SUCCESS_MS);
    assert_eq!(app.server.count(path), 5);
    app.until_status(DONE);
    assert_eq!(app.listing(), ["Field2d_A"]);
    assert_eq!(
        std::fs::read(app.auto.join("Field2d_A/config.json")).expect("config.json"),
        br#"{"name":"A"}"#
    );
}

#[test]
#[ignore = "run by a_slow_server_holds_nothing_up with its own home directory"]
fn slow_in_a_fresh_home() {
    let Some(mut app) = AppUnderTest::start() else {
        return;
    };
    let (api, slow) = (Gate::default(), Gate::default());
    let names = ["Field2d_Slow.zip", "Field2d_Fast.zip"];
    let list = Reply::body(app.server.release_list(DEFAULT, &names));
    app.server
        .reply(RELEASES, Reply::Hold(api.clone(), Box::new(list)));
    let body = Box::new(Reply::body(small_zip("slow")));
    app.server
        .reply("/Field2d_Slow.zip", Reply::Hold(slow.clone(), body));
    app.server
        .reply("/Field2d_Fast.zip", Reply::body(small_zip("fast")));
    app.download();
    app.until("the held API request", |app| {
        app.server.count(RELEASES) == 1
    });
    for _ in 0..10 {
        app.frame();
    }
    assert_eq!(app.shown_status(), INITIAL_STATUS);
    api.open();
    app.until("the fast asset", |app| {
        app.auto.join("Field2d_Fast").exists()
    });
    for _ in 0..10 {
        app.frame();
    }
    assert!(app.shown_status().starts_with("Download started at "));
    assert!(!app.auto.join("Field2d_Slow").exists());
    slow.open();
    app.until_status(DONE);
    assert_eq!(app.listing(), ["Field2d_Fast", "Field2d_Slow"]);
}

#[test]
#[ignore = "run by a_connection_cut_inside_a_body_is_waited_for with its own home directory"]
fn cut_in_a_fresh_home() {
    let Some(mut app) = AppUnderTest::start() else {
        return;
    };
    app.server
        .releases(DEFAULT, &["Field2d_Cut.zip", "Field2d_Whole.zip"]);
    app.server
        .reply("/Field2d_Cut.zip", Reply::Cut(small_zip("cut")));
    app.server
        .reply("/Field2d_Whole.zip", Reply::body(small_zip("whole")));
    app.download();
    app.until("the whole asset", |app| {
        app.auto.join("Field2d_Whole").exists() && app.server.count("/Field2d_Cut.zip") == 1
    });
    for _ in 0..50 {
        app.skip(SUCCESS_MS);
        app.frame();
    }
    assert!(app.status().starts_with("Download started at "));
    assert_eq!(app.listing(), ["Field2d_Whole"]);
    assert_eq!(app.server.count(RELEASES), 1);
}

#[test]
#[ignore = "run by hostile_downloads_stay_in_the_folder with its own home directory"]
fn hostile_in_a_fresh_home() {
    let Some(mut app) = AppUnderTest::start() else {
        return;
    };
    let user_data = app.auto.parent().expect("user data").to_owned();
    let outside = user_data.join("outside");
    std::fs::create_dir_all(&outside).expect("outside");
    let snapshot = || {
        let mut names: Vec<_> = std::fs::read_dir(&user_data)
            .expect("user data")
            .map(|e| e.expect("user data").file_name())
            .filter(|name| name != "state.json")
            .collect();
        names.sort();
        (
            names,
            std::fs::read_dir(&outside).expect("outside").count(),
            std::fs::read(user_data.join("prefs.json")).expect("prefs.json"),
        )
    };
    let expected = snapshot();

    let slip = [
        "../prefs.json",
        "a/../../../prefs.json",
        "/prefs.json",
        "..\\prefs.json",
    ];
    let cases: Vec<(&str, Vec<u8>)> = slip
        .iter()
        .map(|name| ("Field2d_Slip.zip", zip(&[(name, FILE, b"hostile")])))
        .chain([
            ("../Escaped.zip", small_zip("hostile")),
            ("../outside/Escaped.zip", small_zip("hostile")),
            ("../../../Escaped.zip", small_zip("hostile")),
            ("Field2d_A/../../prefs.json.zip", b"hostile".to_vec()),
            ("../prefs.json.zip", b"hostile".to_vec()),
            (
                "Field2d_Link.zip",
                zip(&[
                    ("link", 0o120_777, b"../../outside"),
                    ("link/file", FILE, b"hostile"),
                ]),
            ),
        ])
        .collect();
    for (checks, (name, body)) in cases.into_iter().enumerate() {
        let file = name.rsplit('/').next().expect("a name");
        let path = format!("/{file}");
        let served = app.server.count(&path);
        app.server.releases(DEFAULT, &[name]);
        app.server.reply(&path, Reply::body(body));
        if checks == 0 {
            app.download();
        } else {
            app.skip(SUCCESS_MS);
        }
        let next = app.until_checked();
        assert_eq!(app.server.count(RELEASES), checks + 1);
        // An asset whose target exists is not downloaded.
        let exists = name.ends_with("prefs.json.zip");
        assert_eq!(app.server.count(&path), served + usize::from(!exists));
        assert_eq!(next, if exists { SUCCESS_MS } else { FAILURE_MS }, "{name}");
        assert_eq!(snapshot(), expected, "{name}");
    }

    #[cfg(unix)]
    for body in [b"not an archive".to_vec(), small_zip("hostile")] {
        let link = app.auto.join("Field2d_Linked_download");
        std::os::unix::fs::symlink(&outside, &link).expect("symlink");
        let served = app.server.count("/Field2d_Linked.zip");
        let checks = app.server.count(RELEASES);
        app.server.releases(DEFAULT, &["Field2d_Linked.zip"]);
        app.server.reply("/Field2d_Linked.zip", Reply::body(body));
        app.skip(SUCCESS_MS);
        assert_eq!(app.until_checked(), FAILURE_MS);
        assert_eq!(app.server.count(RELEASES), checks + 1);
        assert_eq!(app.server.count("/Field2d_Linked.zip"), served + 1);
        assert_eq!(snapshot(), expected, "through a symlink");
        std::fs::remove_file(link).expect("the link");
    }
}

#[test]
#[ignore = "run by little_disk_space_waits with its own home directory"]
fn disk_in_a_fresh_home() {
    let Some(mut app) = AppUnderTest::start() else {
        return;
    };
    let real = asset_download::free_space(&app.auto.to_string_lossy());
    assert!(real.is_some_and(|free| free > 0.0), "{real:?}");
    let missing = app.auto.join("missing/deeper");
    let above = asset_download::free_space(&missing.to_string_lossy());
    assert!(above.is_some_and(|free| free > 0.0), "{above:?}");

    FREE.store(7_999_999_999, Ordering::SeqCst);
    app.download();
    assert_eq!(app.until_checked(), FAILURE_MS);
    assert_eq!(app.shown_status(), DISK);
    app.server.releases(DEFAULT, &["Field2d_A.zip"]);
    app.server
        .reply("/Field2d_A.zip", Reply::body(small_zip("A")));
    assert_eq!(app.server.requests().len(), 0);
    FREE.store(8_000_000_000, Ordering::SeqCst);
    app.skip(FAILURE_MS);
    app.until_status(DONE);
    assert_eq!(app.listing(), ["Field2d_A"]);
}

#[test]
#[ignore = "run by quitting_ends_the_downloads with its own home directory"]
fn quit_in_a_fresh_home() {
    let Some(mut app) = AppUnderTest::start() else {
        return;
    };
    let names = ["Field2d_Stalled.zip", "Field2d_Held.zip"];
    app.server.releases(DEFAULT, &names);
    app.server
        .reply("/Field2d_Stalled.zip", Reply::Stall(small_zip("stalled")));
    let gate = Gate::default();
    let held = Box::new(Reply::body(small_zip("held")));
    app.server
        .reply("/Field2d_Held.zip", Reply::Hold(gate.clone(), held));
    app.download();
    app.until("both requests", |app| {
        names
            .iter()
            .all(|name| app.server.count(&format!("/{name}")) == 1)
    });
    let AppUnderTest {
        cx, server, auto, ..
    } = app;
    drop(cx);
    let start = Instant::now();
    while server.left() == 0 {
        assert!(start.elapsed() < PATIENCE, "the stalled connection is open");
        std::thread::sleep(Duration::from_millis(1));
    }
    gate.open();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(server.requests().len(), 3);
    assert_eq!(std::fs::read_dir(auto).expect("autoAssets").count(), 0);
}

#[test]
#[ignore = "run by stopping_ends_the_loop_wherever_it_waits with its own home directory"]
fn stop_in_a_fresh_home() {
    let Some(_) = std::env::var_os(HOME) else {
        return;
    };
    let cases: [fn(&mut AppUnderTest); 5] = [
        |app| {
            app.server.reply(RELEASES, Reply::Stall(b"[]".to_vec()));
            app.download();
            app.until("the request", |app| app.server.count(RELEASES) == 1);
        },
        |app| {
            app.server.reply(RELEASES, Reply::status(500));
            app.download();
            app.until_status(FAILED);
        },
        |app| {
            app.server.releases(DEFAULT, &["Field2d_A.zip"]);
            app.server
                .reply("/Field2d_A.zip", Reply::Stall(small_zip("A")));
            app.download();
            app.until("the request", |app| app.server.count("/Field2d_A.zip") == 1);
        },
        |app| {
            app.server.releases(DEFAULT, &[]);
            app.download();
            app.until_status(DONE);
        },
        |app| {
            app.server.releases(DEFAULT, &["Field2d_A.zip"]);
            app.server
                .reply("/Field2d_A.zip", Reply::body(small_zip("A")));
            app.download();
            let start = Instant::now();
            while !app.auto.join("Field2d_A").exists() {
                assert!(start.elapsed() < PATIENCE);
                std::thread::sleep(Duration::from_millis(1));
            }
        },
    ];
    for case in cases {
        let mut app = AppUnderTest::start().expect("an app");
        case(&mut app);
        let worker = app.cx.update(asset_download::stop);
        let AppUnderTest {
            cx, server, auto, ..
        } = app;
        drop(cx);
        let worker = worker.expect("the loop");
        let start = Instant::now();
        while !worker.is_finished() {
            assert!(start.elapsed() < PATIENCE, "the loop is running");
            std::thread::sleep(Duration::from_millis(1));
        }
        worker.join().expect("the loop");
        let requests = server.requests().len();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(server.requests().len(), requests);
        std::fs::remove_dir_all(auto).expect("autoAssets");
    }
}
