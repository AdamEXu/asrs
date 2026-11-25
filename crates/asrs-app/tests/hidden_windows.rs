#![allow(clippy::disallowed_methods)]

//! The app's own windows on macOS, never shown and in an app that stays in the background, against
//! what Electron's did in `parity/windows/macos.json`: where a first launch's hub and a satellite
//! without saved bounds open, and which of a window's notifications dim a prompt and the Download
//! window and order the hubs.

use std::time::Duration;

use asrs_app::views::{download, prompts};
use asrs_app::windows::{self, KeepHidden, SatellitePlacement, WindowId, WindowKind, Windows};
use asrs_js::value::JsValue;
use asrs_parity::json::{self, Json};
use asrs_parity::vectors;
use asrs_ui::gpui::{
    App, AppContext, AsyncApp, Context, Entity, IntoElement, Render, Styled, Window, div,
};
use asrs_ui::platform::{self, probe};

const TIMEOUT: Duration = Duration::from_secs(60);

struct Blank;

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div().size_full()
    }
}

fn blank(_: &mut Window, cx: &mut App) -> Entity<Blank> {
    cx.new(|_| Blank)
}

fn recording() -> Json {
    let path = vectors::workspace_root().join("parity/windows/macos.json");
    let file = json::parse(&std::fs::read_to_string(path).expect("recording")).expect("JSON");
    let records = file.get("records").and_then(Json::as_array);
    records.expect("records")[0].clone()
}

fn numbers(value: &Json, key: &str) -> Vec<f64> {
    let items = value.get(key).and_then(Json::as_array).expect("numbers");
    items
        .iter()
        .map(|v| v.as_f64().expect("a number"))
        .collect()
}

async fn settle(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(150))
        .await;
}

fn with_window<R>(
    cx: &mut AsyncApp,
    id: WindowId,
    f: impl FnOnce(&mut Window, &mut App) -> R,
) -> R {
    cx.update(|cx| {
        let handle = Windows::handle(cx, id).expect("open");
        handle.update(cx, |_, window, cx| f(window, cx))
    })
    .expect("open")
}

async fn placement(recording: &Json, cx: &mut AsyncApp) -> WindowId {
    let display = &recording.get("displays").and_then(Json::as_array).unwrap()[0];
    let here = cx.update(|cx| {
        let display = cx.primary_display().expect("a display");
        [display.bounds(), display.visible_bounds()].map(|b| {
            [b.origin.x, b.origin.y, b.size.width, b.size.height]
                .map(|v| f64::from(f32::from(v)))
                .to_vec()
        })
    });
    let recorded = [numbers(display, "bounds"), numbers(display, "workArea")];
    let hub = cx
        .update(|cx| windows::open_hub(cx, None, blank))
        .expect("a hub")
        .0;
    let satellite = cx
        .update(|cx| windows::open_satellite(cx, "s", SatellitePlacement::Unplaced, blank))
        .expect("a satellite")
        .0;
    settle(cx).await;
    if here != recorded {
        println!("hidden windows: placement skipped, the screen is {here:?}, not {recorded:?}");
        return hub;
    }
    let cases = recording.get("placement").and_then(Json::as_array);
    for (id, width, style) in [(hub, 1100.0, true), (satellite, 900.0, false)] {
        let case = cases.unwrap().iter().find(|case| {
            let options = case.get("options").unwrap();
            options.get("width").and_then(Json::as_f64) == Some(width)
                && options.get("vibrancy").is_some() == style
        });
        let (x, y, w, h) =
            with_window(cx, id, |window, _| platform::window_bounds(window)).expect("bounds");
        assert_eq!(vec![x, y, w, h], numbers(case.unwrap(), "bounds"));
    }
    println!("hidden windows: placement compared");
    hub
}

async fn focus(recording: &Json, hub: WindowId, cx: &mut AsyncApp) {
    let (prompt, prompt_view) = cx
        .update(|cx| {
            let kind = WindowKind::RenameTab;
            prompts::open(cx, kind, hub, &JsValue::from("tab"), |_, _, _| {})
        })
        .expect("a prompt");
    let (download, download_view) = cx
        .update(|cx| download::open(cx, hub, "darwin"))
        .expect("download")
        .expect("new");
    let other = cx
        .update(|cx| windows::open_hub(cx, None, blank))
        .expect("a hub")
        .0;
    settle(cx).await;
    let mut steps = 0;
    for record in recording.get("focus").and_then(Json::as_array).unwrap() {
        let window = record.get("window").and_then(Json::as_str).unwrap();
        let id = match window.to_string_lossy().as_str() {
            "hub" => hub,
            "prompt" => prompt,
            _ => download,
        };
        let mut blurred = None;
        for step in record.get("steps").and_then(Json::as_array).unwrap() {
            if id == hub {
                cx.update(|cx| Windows::raise(cx, other));
            }
            let post = step.get("post").and_then(Json::as_str).unwrap();
            let post = post.to_string_lossy();
            with_window(cx, id, |window, _| {
                probe::post_window_notification(window, &post);
            });
            settle(cx).await;
            let events = step.get("events").and_then(Json::as_array).unwrap();
            let event = events.last().and_then(Json::as_str);
            let focused = event.map(|event| event.to_string_lossy() == "focus");
            if id == hub {
                let first = cx.update(|cx| Windows::manager(cx).hubs()[0]);
                assert_eq!(first == hub, focused == Some(true), "{window:?} {post}");
            } else {
                blurred = focused.map(|focused| !focused).or(blurred);
                let dims = cx.update(|cx| {
                    if id == prompt {
                        prompt_view.read(cx).page().blurred()
                    } else {
                        download_view.read(cx).page().blurred()
                    }
                });
                assert_eq!(dims, blurred.unwrap_or(false), "{window:?} {post}");
            }
            steps += 1;
        }
    }
    assert_eq!(steps, 42);
}

fn main() {
    if asrs_parity::ci::skip_in_ci("hidden windows", "a display") {
        return;
    }
    if !cfg!(target_os = "macos") {
        println!("hidden windows: skipped on this platform");
        return;
    }
    std::thread::spawn(|| {
        std::thread::sleep(TIMEOUT);
        eprintln!("hidden windows: stuck");
        std::process::exit(1);
    });
    std::panic::set_hook(Box::new(|info| {
        eprintln!("{info}");
        std::process::exit(1);
    }));
    asrs_ui::gpui::application().run(|cx: &mut App| {
        probe::stay_in_background();
        asrs_ui::init(cx);
        windows::init(cx);
        cx.set_global(KeepHidden);
        cx.spawn(async move |cx| {
            let recording = recording();
            let hub = placement(&recording, cx).await;
            focus(&recording, hub, cx).await;
            for id in cx.update(|cx| Windows::ids(cx)) {
                let seen = with_window(cx, id, |window, _| {
                    platform::native_window(window).map(|native| native.is_visible())
                });
                assert_eq!(seen, Some(false));
            }
            assert_eq!(probe::app_is_active(), Some(false));
            println!("hidden windows: passed");
            std::process::exit(0);
        })
        .detach();
    });
}
