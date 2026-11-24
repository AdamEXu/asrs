#![allow(clippy::disallowed_methods)]

use std::io::IsTerminal;

use asrs_app::{main_process, self_test, windows};
use asrs_model::app_identity::APP_USER_MODEL_ID;
use asrs_ui::gpui::{App, application};
use asrs_ui::platform;

/// Reports an error that keeps the app from starting, in a box when no terminal shows stderr.
fn cannot_start(error: &str) -> ! {
    let name = asrs_app::IDENTITY.name;
    eprintln!("{name}: {error}");
    let terminal = std::io::stderr().is_terminal()
        || std::io::stdout().is_terminal()
        || std::io::stdin().is_terminal();
    if !terminal {
        platform::show_startup_error(&format!("{name} cannot start"), error);
    }
    std::process::exit(1);
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    if let Some(code) = self_test::run_from_args(&argv) {
        std::process::exit(code);
    }
    if let Err(e) = asrs_js::math::libm::load() {
        cannot_start(e);
    }
    #[cfg(feature = "parity")]
    if let Some(code) = asrs_app::scenario::run_from_args() {
        std::process::exit(code);
    }
    let (on_open, open_events) = main_process::open_file_events();
    let app = application();
    app.on_open_urls(on_open).on_reopen(main_process::reopen);
    app.run(move |cx: &mut App| {
        if cfg!(windows) {
            cx.set_app_identity(APP_USER_MODEL_ID, asrs_app::IDENTITY.name);
        }
        asrs_ui::init(cx);
        windows::init(cx);
        main_process::start(cx, &argv, open_events);
    });
}
