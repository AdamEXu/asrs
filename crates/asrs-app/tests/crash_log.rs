#![allow(clippy::disallowed_methods)]

//! The crash log of a process that panics: this test binary, run again for each case.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use asrs_app::crash_log::{self, CrashLog, FILE, OLD_FILE};

const DIR: &str = "ASRS_CRASH_LOG_DIR";
const CASE: &str = "ASRS_CRASH_LOG_CASE";
const VERSION: &str = "asrs 0.0.0-test";
const TIMEOUT: Duration = Duration::from_secs(60);

struct Home(PathBuf);

impl Home {
    fn new(case: &str) -> Home {
        let home = std::env::temp_dir().join(format!("asrs-crash-{case}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).expect("home");
        Home(home)
    }

    fn data(&self) -> PathBuf {
        self.0.join("data")
    }

    fn start(&self, case: &str, stderr: Stdio) -> Child {
        Command::new(std::env::current_exe().expect("the test binary"))
            .args(["--exact", "crashes", "--ignored", "--nocapture"])
            .env(DIR, &self.0)
            .env(CASE, case)
            .stdout(Stdio::null())
            .stderr(stderr)
            .spawn()
            .expect("the test binary")
    }

    fn log(&self) -> String {
        fs::read_to_string(self.data().join(FILE)).expect("the crash log")
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn wait(mut child: Child) -> ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("the child") {
            return status;
        }
        if start.elapsed() > TIMEOUT {
            let _ = child.kill();
            panic!("the child hangs");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn entries(log: &str) -> usize {
    log.matches(VERSION).count()
}

extern "C" fn cannot_unwind() {
    panic!("in a window procedure");
}

#[test]
#[ignore = "run by the other tests"]
fn crashes() {
    let Some(home) = std::env::var_os(DIR).map(PathBuf::from) else {
        return;
    };
    let case = std::env::var(CASE).expect("a case");
    let named = home.to_string_lossy().into_owned();
    let log = CrashLog::new(home.join("data"), VERSION, Some(named));
    let told = home.join("told");
    crash_log::install(log, move |logged, aborts| {
        let logged = if logged { "logged" } else { "lost" };
        let aborts = if aborts { ", aborts" } else { "" };
        let _ = fs::write(&told, format!("{logged}{aborts}"));
    });
    match case.as_str() {
        "panic" => panic!("cannot read {}", home.join("robot.wpilog").display()),
        "abort" => cannot_unwind(),
        "abort-thread" => {
            let _ = std::thread::spawn(|| cannot_unwind()).join();
        }
        "thread" => {
            let thread = std::thread::Builder::new().name("decoder".to_owned());
            let thread = thread.spawn(|| panic!("in a thread")).expect("a thread");
            assert!(thread.join().is_err());
        }
        "threads" => {
            let threads: Vec<_> = (0..8)
                .map(|n| std::thread::spawn(move || panic!("thread {n}")))
                .collect();
            for thread in threads {
                assert!(thread.join().is_err());
            }
        }
        "repeated" => {
            for n in 0..30 {
                let thread = std::thread::spawn(move || panic!("number {n}."));
                assert!(thread.join().is_err());
            }
        }
        "symbols" => {
            let thread = std::thread::spawn(|| panic!("in a thread"));
            assert!(thread.join().is_err());
            let log = home.join("data").join(FILE);
            let start = Instant::now();
            while !fs::read_to_string(&log).is_ok_and(|log| log.contains("\nsymbols:\n")) {
                if start.elapsed() > TIMEOUT / 2 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        "contents" => {
            let thread = std::thread::spawn(|| {
                let text = std::hint::black_box("confidential \u{e9}".to_owned());
                let at = std::hint::black_box(14);
                println!("{}", &text[at..]);
            });
            assert!(thread.join().is_err());
        }
        other => panic!("{other}"),
    }
}

#[test]
fn a_panic_is_logged() {
    let home = Home::new("panic");
    let status = wait(home.start("panic", Stdio::null()));
    assert!(!status.success());
    let log = home.log();
    let lines: Vec<&str> = log.lines().collect();
    assert!(
        lines[0].starts_with("--- 2") && lines[0].ends_with("Z ---"),
        "{log}"
    );
    let platform = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
    assert_eq!(lines[1], format!("{VERSION}, {platform}"));
    assert!(
        lines[2].starts_with("thread 'crashes' panicked at ") && lines[2].contains("crash_log.rs:"),
        "{log}"
    );
    let file = Path::new("~").join("robot.wpilog");
    assert_eq!(lines[3], format!("cannot read {}", file.display()));
    assert!(lines[4].starts_with("CrashLog::new at 0x"), "{log}");
    let frame = lines[5].trim_start();
    assert!(frame.starts_with("0: ") && frame.contains(" 0x"), "{log}");
    assert!(
        !log.contains(&*home.0.to_string_lossy()),
        "the home directory: {log}"
    );
    let told = fs::read_to_string(home.0.join("told")).expect("told");
    assert_eq!(told, "logged");
}

#[test]
fn a_backtrace_is_followed_by_its_symbols() {
    let home = Home::new("symbols");
    assert!(wait(home.start("symbols", Stdio::null())).success());
    let log = home.log();
    let (entry, symbols) = log.split_once("\nsymbols:\n").expect(&log);
    let mut frames = entry
        .lines()
        .skip_while(|line| !line.starts_with("CrashLog::new at "));
    let first = frames.nth(1).unwrap_or_default().trim_start();
    assert!(first.starts_with("0: 0x"), "{log}");
    let address = first.split(' ').nth(1).unwrap_or_default();
    assert!(symbols.contains(address), "{log}");
    assert!(symbols.contains("crash_log::crashes"), "{log}");
}

#[test]
fn a_panic_that_aborts_is_logged() {
    for case in ["abort", "abort-thread"] {
        let home = Home::new(case);
        assert!(!wait(home.start(case, Stdio::null())).success());
        assert!(home.log().contains("in a window procedure"));
        let told = fs::read_to_string(home.0.join("told")).expect("told");
        assert_eq!(told, "logged, aborts");
    }
}

#[test]
fn a_panic_is_logged_when_stderr_is_closed() {
    let home = Home::new("closed");
    let mut child = home.start("panic", Stdio::piped());
    drop(child.stderr.take());
    assert!(!wait(child).success());
    assert!(home.log().contains("cannot read"));
}

#[test]
fn a_thread_s_panic_is_logged_and_the_process_goes_on() {
    let home = Home::new("thread");
    assert!(wait(home.start("thread", Stdio::null())).success());
    let log = home.log();
    assert!(log.contains("thread 'decoder' panicked at "), "{log}");
    assert!(log.contains("in a thread"), "{log}");
}

#[test]
fn panics_at_once_are_all_logged() {
    let home = Home::new("threads");
    assert!(wait(home.start("threads", Stdio::null())).success());
    let log = home.log();
    assert_eq!(entries(&log), 8, "{log}");
    for n in 0..8 {
        assert!(log.contains(&format!("\nthread {n}\n")), "{log}");
    }
}

#[test]
fn a_thread_that_keeps_panicking_does_not_fill_the_log() {
    let home = Home::new("repeated");
    assert!(wait(home.start("repeated", Stdio::null())).success());
    let log = home.log();
    assert_eq!(entries(&log), 10);
    assert!(log.contains("number 9."));
    assert!(log.len() < 256 << 10);
}

#[test]
fn what_a_string_holds_is_not_logged() {
    let home = Home::new("contents");
    assert!(wait(home.start("contents", Stdio::null())).success());
    let log = home.log();
    assert!(log.contains("char boundary"), "{log}");
    assert!(!log.contains("confidential"), "{log}");
}

#[test]
fn a_full_log_is_rotated() {
    let home = Home::new("rotated");
    fs::create_dir_all(home.data()).expect("data");
    let full = "x".repeat(256 << 10);
    fs::write(home.data().join(FILE), &full).expect("full");
    fs::write(home.data().join(OLD_FILE), "older").expect("older");
    assert!(wait(home.start("thread", Stdio::null())).success());
    assert_eq!(entries(&home.log()), 1);
    let old = fs::read_to_string(home.data().join(OLD_FILE)).expect("old");
    assert!(old == full);
}

#[test]
fn a_log_that_cannot_be_written_stops_nothing() {
    let home = Home::new("unwritable");
    fs::write(home.data(), "a file").expect("a file");
    assert!(wait(home.start("thread", Stdio::null())).success());
    let told = fs::read_to_string(home.0.join("told")).expect("told");
    assert_eq!(told, "lost");
}
