#![allow(clippy::disallowed_methods)]

//! The hub inside the root `asrs_ui::open_window` gives every window, against the hub alone, which
//! is what scenario mode captures: the root paints nothing, so the sidebar's vibrancy or acrylic
//! shows through both alike.

use std::path::Path;
use std::sync::Arc;

use asrs_app::main_process;
use asrs_app::views::hub_host::{HubEntityHost, boot_messages};
use asrs_app::windows::{self, Rect};
use asrs_model::messages::{MainToHub, NamedMessage};
use asrs_parity::vectors;
use asrs_services::fs::RealFs;
use asrs_services::lifecycle::{AppPaths, MainState};
use asrs_services::path::PathStyle;
use asrs_services::windows::WindowSystem;
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::gpui::{Bounds, WindowBounds, WindowOptions, point, px, size};
use asrs_ui::selection::colors::ForcedActive;
use asrs_ui::theme::ThemeSource;

struct NoScreen;

impl WindowSystem for NoScreen {
    fn window_bounds(&self, _: f64) -> Option<Rect> {
        None
    }

    fn displays(&self) -> Vec<Rect> {
        vec![self.primary_display()]
    }

    fn primary_display(&self) -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            width: 1280.0,
            height: 800.0,
        }
    }
}

fn boot(user_data: &Path, theme: &str) -> Vec<NamedMessage<MainToHub>> {
    std::fs::create_dir_all(user_data).expect("userData");
    let prefs = format!(r#"{{"theme":"{theme}"}}"#);
    std::fs::write(user_data.join("prefs.json"), prefs).expect("prefs.json");
    let path = |path: &Path| path.to_str().expect("path").to_owned();
    let paths = AppPaths {
        style: PathStyle::host(),
        user_data: path(user_data),
        temp: path(&user_data.join("temp")),
        bundled_assets: path(
            &vectors::workspace_root().join("reference/AdvantageScope/bundledAssets"),
        ),
        packaged: false,
    };
    let started = MainState::start(&RealFs, &NoScreen, paths, main_process::process_platform());
    let (main, _) = started.unwrap_or_else(|e| panic!("{e:?}"));
    let preferences = main
        .preferences(&RealFs)
        .unwrap_or_else(|e| panic!("{e:?}"));
    boot_messages(main.assets.set_assets_data(), true, preferences, Vec::new())
}

fn main() {
    if cfg!(target_os = "linux") {
        println!("window root: skipped, Linux windows are opaque");
        return;
    }
    if cfg!(windows) && asrs_parity::ci::skip_in_ci("window root", "a desktop") {
        return;
    }
    let dir = std::env::temp_dir().join(format!("asrs-window-root-{}", std::process::id()));
    let content = size(px(1100.0), px(650.0));
    for (theme, source) in [("light", ThemeSource::Light), ("dark", ThemeSource::Dark)] {
        for scale in [1.0, 2.0] {
            let mut capture = HeadlessCapture::new(Arc::new(())).expect("a headless renderer");
            capture.set_transparent_windows(true);
            capture.context().update(|cx| {
                windows::init(cx);
                cx.set_global(source);
                cx.set_global(ForcedActive(false));
            });
            let messages = boot(&dir.join(format!("{theme}-alone")), theme);
            let alone = capture
                .open_window(content, scale, |window, cx| {
                    HubEntityHost::open_with(window, false, messages, None, cx)
                })
                .expect("window");
            let alone = capture.capture(alone.into()).expect("capture");
            let messages = boot(&dir.join(format!("{theme}-rooted")), theme);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.0), px(0.0)),
                    content,
                ))),
                ..WindowOptions::default()
            };
            let (rooted, _) = capture
                .context()
                .update(|cx| {
                    asrs_ui::open_window(options, cx, |window, cx| {
                        HubEntityHost::open_with(window, false, messages, None, cx)
                    })
                })
                .expect("window");
            capture.set_scale_factor(rooted, scale).expect("scale");
            let rooted = capture.capture(rooted).expect("capture");
            assert_eq!(
                (rooted.width, rooted.height),
                (alone.width, alone.height),
                "{theme} {scale}x"
            );
            let clear = |rgba: &[u8]| rgba.chunks(4).filter(|p| p[3] < 255).count();
            let differing = (alone.rgba.chunks(4).zip(rooted.rgba.chunks(4)))
                .filter(|(a, b)| a != b)
                .count();
            println!(
                "{theme} {scale}x: {differing} px differ, {} not opaque",
                clear(&rooted.rgba)
            );
            assert!(clear(&alone.rgba) > 0, "{theme} {scale}x: no vibrancy");
            assert_eq!(differing, 0, "{theme} {scale}x");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
