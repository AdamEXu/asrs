#![allow(clippy::disallowed_methods)]

#[path = "../../asrs-cli/src/tabs_console.rs"]
#[allow(dead_code)]
mod tabs_console;

use asrs_parity::{FnCase, Response, Value, fn_vectors};

fn dispatch(case: &FnCase<'_>) -> Response {
    let args = &case.request.args;
    if case.name() == "oracle:call" {
        let source = args
            .first()
            .and_then(Value::as_str)
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        return match tabs_console::CALLS.iter().find(|(s, _)| *s == source) {
            Some((_, f)) => f(&args[1..]),
            None => Response::Error(format!("no port of {source:?}")),
        };
    }
    match tabs_console::FUNCTIONS.iter().find(|(n, _)| *n == case.name()) {
        Some((_, f)) => f(args),
        None => Response::Error(format!("no port of {}", case.name())),
    }
}

fn_vectors!(ansi_up, "tabs/ansi", dispatch);
fn_vectors!(ansi_up_append, "tabs/ansi-append", dispatch);
fn_vectors!(console_format, "tabs/console-format", dispatch);
fn_vectors!(console_filter, "tabs/console-filter", dispatch);
fn_vectors!(console_export, "tabs/console-export", dispatch);
