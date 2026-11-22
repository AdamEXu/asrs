#![allow(clippy::disallowed_methods)]

use asrs_app::views::frame::HubFrame;
use asrs_app::views::standalone::StandaloneHub;
use asrs_app::windows;
use asrs_ui::gpui::{App, AppContext, application};

fn main() {
    if let Err(e) = asrs_js::math::libm::load() {
        eprintln!("asrs: {e}");
        std::process::exit(1);
    }
    #[cfg(feature = "parity")]
    if let Some(code) = asrs_app::scenario::run_from_args() {
        std::process::exit(code);
    }
    application().run(|cx: &mut App| {
        asrs_ui::init(cx);
        windows::init(cx);
        windows::open_hub(cx, None, |window, cx| {
            let dark =
                asrs_ui::theme::Appearance::of(window, cx) == asrs_ui::theme::Appearance::Dark;
            cx.new(|_| HubFrame::new(StandaloneHub::new(dark, cfg!(feature = "parity"))))
        })
        .expect("failed to open the main window");
        cx.activate(true);
    });
}
