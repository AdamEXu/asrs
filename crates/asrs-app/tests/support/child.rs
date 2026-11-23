use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(300);

/// Runs a test in its own process, which is killed and reported if it hangs: GPUI's headless
/// scheduler can deadlock (#319).
pub fn run(command: &mut Command, test: &str) -> ExitStatus {
    let mut child = command.spawn().expect("the test binary");
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("the test binary") {
            return status;
        }
        if start.elapsed() > TIMEOUT {
            child.kill().expect("the test binary");
            child.wait().expect("the test binary");
            panic!("{test}: still running after {TIMEOUT:?}, so it was killed");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
