#![allow(clippy::disallowed_methods)]

use asrs_app::views::hub_host::HubEntityHost;
use asrs_app::windows;
use asrs_ui::gpui::{App, application};

fn main() {
    if let Err(e) = asrs_js::math::libm::load() {
        eprintln!("asrs: {e}");
        std::process::exit(1);
    }
    #[cfg(feature = "parity")]
    if let Some(code) = asrs_app::scenario::run_from_args() {
        std::process::exit(code);
    }
    let files: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    application().run(|cx: &mut App| {
        asrs_ui::init(cx);
        windows::init(cx);
        windows::open_hub(cx, None, |window, cx| {
            let dark =
                asrs_ui::theme::Appearance::of(window, cx) == asrs_ui::theme::Appearance::Dark;
            HubEntityHost::open(window, dark, cfg!(feature = "parity"), files, cx)
        })
        .expect("failed to open the main window");
        cx.activate(true);
    });
}
