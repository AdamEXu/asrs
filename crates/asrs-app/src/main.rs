#![allow(clippy::disallowed_methods)]

use asrs_app::{main_process, windows};
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
    let argv: Vec<String> = std::env::args().collect();
    let (on_open, open_events) = main_process::open_file_events();
    let app = application();
    app.on_open_urls(on_open).on_reopen(main_process::reopen);
    app.run(move |cx: &mut App| {
        asrs_ui::init(cx);
        windows::init(cx);
        main_process::start(cx, &argv, open_events);
    });
}
