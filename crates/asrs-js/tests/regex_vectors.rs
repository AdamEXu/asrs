#![allow(clippy::disallowed_methods)]

#[path = "../../asrs-cli/src/js_regex.rs"]
#[allow(dead_code)]
mod js_regex;
#[path = "../../asrs-cli/src/js_string.rs"]
#[allow(dead_code)]
mod js_string;

use asrs_parity::{FnCase, Response, Value, fn_vectors};

fn dispatch(case: &FnCase<'_>) -> Response {
    let args = &case.request.args;
    if case.name() == "oracle:call" {
        let source = args
            .first()
            .and_then(Value::as_str)
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        return match js_regex::CALLS.iter().find(|(s, _)| *s == source) {
            Some((_, f)) => f(&args[1..]),
            None => Response::Error(format!("no port of {source:?}")),
        };
    }
    match js_regex::FUNCTIONS
        .iter()
        .chain(js_string::FUNCTIONS)
        .find(|(n, _)| *n == case.name())
    {
        Some((_, f)) => f(args),
        None => Response::Error(format!("no port of {}", case.name())),
    }
}

fn_vectors!(regex_sites, "regex/sites", dispatch);
fn_vectors!(regex_engine, "regex/engine", dispatch);
