#![allow(clippy::disallowed_methods)]

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use asrs_app::crash_log::{self, CrashLog};
use asrs_app::{main_process, self_test, windows};
use asrs_model::app_identity::{APP_USER_MODEL_ID, AppIdentity};
use asrs_services::lifecycle;
use asrs_services::window_manager::Platform;
use asrs_ui::gpui::{App, application};
use asrs_ui::platform;

fn is_release() -> bool {
    asrs_app::IDENTITY == AppIdentity::RELEASE
}

const CRASH_BOX_WAIT: Duration = Duration::from_secs(60);

/// Set once the process is the app, rather than one of its command-line checks.
static APP: AtomicBool = AtomicBool::new(false);

fn has_terminal() -> bool {
    std::io::stderr().is_terminal()
        || std::io::stdout().is_terminal()
        || std::io::stdin().is_terminal()
}

/// Reports an error that keeps the app from starting, in a box when no terminal shows stderr.
fn cannot_start(error: &str) -> ! {
    let name = asrs_app::IDENTITY.name;
    let mut detail = error.to_owned();
    if is_release() {
        detail += &format!("\n\n{}", asrs_app::IDENTITY.version_line());
    }
    let _ = writeln!(std::io::stderr(), "{name}: {detail}");
    if !has_terminal() {
        platform::show_startup_error(&format!("{name} cannot start"), &detail);
    }
    std::process::exit(1);
}

fn name_version_in_panics() {
    let version = asrs_app::IDENTITY.version_line();
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // `eprintln!` panics when stderr is closed, and a panic in the hook aborts.
        let _ = writeln!(std::io::stderr(), "{version}");
        hook(info);
    }));
}

fn log_panics() {
    let env = |name: &str| std::env::var_os(name).map(|v| v.to_string_lossy().into_owned());
    let Some(dir) = lifecycle::user_data_dir(Platform::current(), env) else {
        return;
    };
    let home = env(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    let version = asrs_app::IDENTITY.version_line();
    let log = CrashLog::new(PathBuf::from(dir), &version, home);
    let path = log.path();
    let main = thread::current().id();
    crash_log::install(log, move |logged| {
        // Nothing catches a panic on the main thread. Other threads' leave the app running.
        if cfg!(windows) && thread::current().id() == main && APP.swap(false, Ordering::SeqCst) {
            show_crash(&version, logged.then(|| path.display().to_string()));
        }
    });
}

/// Its own thread runs the box: one on the main thread would run the windows' procedures, which
/// panicked. Nobody may be there to close it.
fn show_crash(version: &str, log: Option<String>) {
    if has_terminal() {
        return;
    }
    let name = asrs_app::IDENTITY.name;
    let mut detail = "An unexpected error closed the app.".to_owned();
    if let Some(log) = log {
        detail += &format!(" Its details are in {log}.");
    }
    detail += &format!("\n\n{version}");
    let (closed, wait) = mpsc::channel();
    let shown = thread::Builder::new().spawn(move || {
        platform::show_startup_error(&format!("{name} has to close"), &detail);
        let _ = closed.send(());
    });
    if shown.is_ok() {
        let _ = wait.recv_timeout(CRASH_BOX_WAIT);
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    if is_release() {
        if argv.get(1).is_some_and(|arg| arg == "--version") {
            platform::attach_console();
            println!("{}", asrs_app::IDENTITY.version_line());
            return;
        }
        name_version_in_panics();
        log_panics();
    }
    if let Some(code) = self_test::run_from_args(&argv) {
        std::process::exit(code);
    }
    APP.store(true, Ordering::SeqCst);
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
        asrs_ui::scroll::use_system_style(cx);
        windows::init(cx);
        main_process::start(cx, &argv, open_events);
    });
}
