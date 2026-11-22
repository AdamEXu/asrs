#![allow(clippy::disallowed_methods)]

use asrs_app::hub::EmptyHub;
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
        windows::open_hub(cx, None, |_, cx| cx.new(|_| EmptyHub))
            .expect("failed to open the main window");
        cx.activate(true);
    });
}
