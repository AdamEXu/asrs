#![allow(clippy::disallowed_methods)]

//! The native properties of the app's own windows on macOS, never shown and in an app that stays in
//! the background, against those Electron gave upstream's in `parity/windows/macos-chrome.json`
//! (`cargo xtask app chrome`). `oracle/tools/windowChrome.m` reads both. With
//! `ASRS_CHROME_CAPTURES=<folder>` the windows are also ordered in off the screen and captured.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use asrs_app::views::hub_host::HubEntityHost;
use asrs_app::views::{download, info, prompts};
use asrs_app::windows::{self, KeepHidden, SatellitePlacement, WindowId, WindowKind, Windows};
use asrs_js::value::JsValue;
use asrs_parity::json::{self, Json};
use asrs_parity::vectors;
use asrs_ui::gpui::{
    App, AppContext, AsyncApp, Context, Entity, IntoElement, Render, Styled, Window, div,
};
use asrs_ui::platform::probe;
use asrs_ui::theme::{Appearance, ThemeSource};

const TIMEOUT: Duration = Duration::from_secs(60);
const THEMES: [(&str, ThemeSource); 3] = [
    ("light", ThemeSource::Light),
    ("dark", ThemeSource::Dark),
    ("system", ThemeSource::System),
];

/// What follows the height of AppKit's own title bar and the size of its buttons.
const GEOMETRY: [&str; 7] = [
    "close",
    "minimize",
    "zoom",
    "content",
    "minSize",
    "contentMinSize",
    "maxSize",
];

struct Blank;

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div().size_full()
    }
}

fn blank(window: &mut Window, cx: &mut App) -> Entity<Blank> {
    window.set_window_title("AdvantageScope");
    cx.new(|_| Blank)
}

fn open(name: &str, parent: WindowId, cx: &mut App) -> WindowId {
    let prompt = |kind, cx: &mut App| {
        let message = JsValue::from("tab");
        prompts::open(cx, kind, parent, &message, |_, _, _| {})
            .expect("a prompt")
            .0
    };
    match name {
        "Hub" => {
            windows::open_hub(cx, None, |window, cx| {
                window.set_window_title("AdvantageScope");
                let dark = Appearance::of(window, cx) == Appearance::Dark;
                HubEntityHost::open(window, dark, false, Vec::new(), cx)
            })
            .expect("a hub")
            .0
        }
        "Satellite" => {
            windows::open_satellite(cx, "s", SatellitePlacement::Unplaced, blank)
                .expect("a satellite")
                .0
        }
        "Preferences" => {
            prompts::open_preferences(cx, parent, |_, _| {})
                .expect("preferences")
                .expect("new")
                .0
        }
        "Download" => {
            download::open(cx, parent, "darwin")
                .expect("download")
                .expect("new")
                .0
        }
        "Licenses" => {
            info::open_licenses(cx, parent, Vec::new())
                .expect("licenses")
                .expect("new")
                .0
        }
        "SourceListHelp" => {
            windows::open_source_list_help(cx, parent, |window, cx| {
                window.set_window_title("Help \u{2014} AdvantageScope");
                cx.new(|_| Blank)
            })
            .expect("help")
            .0
        }
        "EditRange" => prompt(WindowKind::EditRange, cx),
        "UnitConversion" => prompt(WindowKind::UnitConversion, cx),
        "RenameTab" => prompt(WindowKind::RenameTab, cx),
        "EditFov" => prompt(WindowKind::EditFov, cx),
        "Export" => {
            prompts::open_export(cx, parent, false, |_, _, _| {})
                .expect("export")
                .0
        }
        "BetaWelcome" => {
            info::open_beta_welcome(cx, parent, false)
                .expect("welcome")
                .0
        }
        _ => panic!("{name}"),
    }
}

async fn wait(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

fn say(cx: &mut AsyncApp, id: WindowId, name: &str) {
    cx.update(|cx| {
        let handle = Windows::handle(cx, id).expect("open");
        handle.update(cx, |_, window, _| {
            probe::set_represented_filename(window, name);
        })
    })
    .expect("open");
}

fn chrome(cx: &mut AsyncApp, id: WindowId, file: &Path) -> Json {
    let _ = std::fs::remove_file(file);
    say(cx, id, &format!("asrs-chrome {}", file.display()));
    json::parse(&std::fs::read_to_string(file).expect("the window's properties")).expect("JSON")
}

async fn capture(cx: &mut AsyncApp, id: WindowId, file: PathBuf) {
    let _ = std::fs::remove_file(&file);
    say(cx, id, &format!("asrs-capture {}", file.display()));
    for _ in 0..100 {
        if file.exists() {
            break;
        }
        wait(cx, 100).await;
    }
    wait(cx, 300).await;
}

fn fields(value: &Json) -> Vec<(String, String)> {
    let Json::Object(pairs) = value else {
        panic!("{}", value.to_compact());
    };
    pairs
        .iter()
        .map(|(key, value)| (key.to_string_lossy(), value.to_compact()))
        .collect()
}

async fn compare(scratch: &Path, cx: &mut AsyncApp) {
    let path = vectors::workspace_root().join("parity/windows/macos-chrome.json");
    let file = json::parse(&std::fs::read_to_string(path).expect("recording")).expect("JSON");
    let record = file.get("record").expect("a record");
    let windows = record
        .get("windows")
        .and_then(Json::as_array)
        .expect("windows");
    let captures = std::env::var_os("ASRS_CHROME_CAPTURES").map(PathBuf::from);
    let out = scratch.join("window.json");
    let text = |value: &Json, key: &str| {
        let text = value.get(key).and_then(Json::as_str).expect("text");
        text.to_string_lossy()
    };
    let version = Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .expect("sw_vers");
    let version = String::from_utf8_lossy(&version.stdout).into_owned();
    let major = |version: &str| version.trim().split('.').next().map(str::to_owned);
    let same_macos = major(&version) == major(&text(record, "macos"));
    let mut differences = Vec::new();
    let mut compared = 0;
    for (theme, source) in THEMES {
        cx.update(|cx| windows::set_theme(cx, source));
        let parent = cx
            .update(|cx| windows::open_hub(cx, None, blank))
            .expect("a hub")
            .0;
        for expected in windows.iter().filter(|w| text(w, "theme") == theme) {
            let name = text(expected, "name");
            let id = cx.update(|cx| open(&name, parent, cx));
            wait(cx, 300).await;
            let ours = chrome(cx, id, &out);
            let theirs = fields(expected.get("chrome").expect("chrome"));
            for (key, theirs) in theirs {
                if GEOMETRY.contains(&key.as_str()) && !same_macos
                    || key == "effectiveAppearance" && theme == "system"
                {
                    continue;
                }
                let ours = ours.get(&key).map(Json::to_compact);
                compared += 1;
                if ours.as_deref() != Some(&theirs) {
                    differences.push(format!("{name} {theme} {key}: {ours:?}, not {theirs}"));
                }
            }
            if let Some(dir) = &captures {
                let active = std::env::var_os("ASRS_CHROME_ACTIVE").is_some();
                let state = if active { "active" } else { "inactive" };
                capture(cx, id, dir.join(format!("{name}-{theme}-{state}.png"))).await;
            }
            cx.update(|cx| windows::close(cx, id));
            wait(cx, 100).await;
        }
        cx.update(|cx| windows::close(cx, parent));
        wait(cx, 100).await;
    }
    let window = cx
        .update(|cx| windows::open_satellite(cx, "t", SatellitePlacement::Unplaced, blank))
        .expect("a satellite")
        .0;
    let changes = record.get("changes").and_then(Json::as_array);
    for change in changes.expect("changes") {
        let theme = text(change, "theme");
        let (_, source) = THEMES.into_iter().find(|(name, _)| *name == theme).unwrap();
        cx.update(|cx| windows::set_theme(cx, source));
        wait(cx, 300).await;
        let ours = chrome(cx, window, &out);
        for (key, theirs) in fields(change) {
            if key == "theme" || key == "effectiveAppearance" && theme == "system" {
                continue;
            }
            let ours = ours.get(&key).map(Json::to_compact);
            compared += 1;
            if ours.as_deref() != Some(&theirs) {
                differences.push(format!("change to {theme} {key}: {ours:?}, not {theirs}"));
            }
        }
    }
    for difference in &differences {
        println!("{difference}");
    }
    assert!(
        differences.is_empty(),
        "{} of {compared} differ",
        differences.len()
    );
}

/// Where the library is built, and the process started again with it loaded.
fn restart(scratch: &Path) -> ! {
    let library = scratch.join("windowChrome.dylib");
    let source = vectors::workspace_root().join("oracle/tools/windowChrome.m");
    let built = Command::new("cc")
        .args(["-dynamiclib", "-framework", "AppKit", "-o"])
        .args([&library, &source])
        .status();
    assert!(built.expect("cc").success(), "cc");
    let status = Command::new(std::env::current_exe().expect("this test"))
        .args(std::env::args_os().skip(1))
        .env("DYLD_INSERT_LIBRARIES", &library)
        .env("ASRS_WINDOW_CHROME", "1")
        .status();
    std::process::exit(status.expect("started").code().unwrap_or(1));
}

fn main() {
    if asrs_parity::ci::skip_in_ci("window chrome", "a display") {
        return;
    }
    if !cfg!(target_os = "macos") {
        println!("window chrome: skipped on this platform");
        return;
    }
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    if std::env::var_os("ASRS_WINDOW_CHROME").is_none() {
        restart(&scratch);
    }
    if std::env::var_os("ASRS_CHROME_CAPTURES").is_none() {
        std::thread::spawn(|| {
            std::thread::sleep(TIMEOUT);
            eprintln!("window chrome: stuck");
            std::process::exit(1);
        });
    }
    std::panic::set_hook(Box::new(|info| {
        eprintln!("{info}");
        std::process::exit(1);
    }));
    asrs_ui::gpui::application().run(move |cx: &mut App| {
        probe::stay_in_background();
        asrs_ui::init(cx);
        windows::init(cx);
        cx.set_global(KeepHidden);
        cx.spawn(async move |cx| {
            compare(&scratch, cx).await;
            if std::env::var_os("ASRS_CHROME_ACTIVE").is_none() {
                assert_eq!(probe::app_is_active(), Some(false));
            }
            println!("window chrome: passed");
            std::process::exit(0);
        })
        .detach();
    });
}
