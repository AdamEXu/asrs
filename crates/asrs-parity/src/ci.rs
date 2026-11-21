//! Skipping tests under `ASRS_CI=1`, which CI sets: tests that need what a CI runner lacks
//! (the oracle, generated corpora, downloaded public logs, a GPU or a display) skip with a
//! notice instead of failing.

use std::io::Write as _;

pub const CI_VARIABLE: &str = "ASRS_CI";

pub fn is_ci() -> bool {
    std::env::var_os(CI_VARIABLE).is_some_and(|v| v == "1")
}

/// Reports a skipped test on stderr directly, since libtest captures `eprintln!` output of
/// passing tests.
pub fn skip_notice(what: &str, reason: &str) {
    let _ = writeln!(std::io::stderr(), "note: skipping {what}: {reason}");
}

/// Returns true, after printing a notice, when running under `ASRS_CI=1`, so a test that needs
/// `needs` returns early there.
pub fn skip_in_ci(what: &str, needs: &str) -> bool {
    let skip = is_ci();
    if skip {
        skip_notice(
            what,
            &format!("needs {needs}, which {CI_VARIABLE}=1 runs lack"),
        );
    }
    skip
}
