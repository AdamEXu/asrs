#![allow(clippy::disallowed_methods)]

#[path = "../../asrs-cli/src/functions.rs"]
#[allow(dead_code)]
mod functions;
#[path = "../../asrs-cli/src/js_color.rs"]
#[allow(dead_code)]
mod js_color;
#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;
#[path = "../../asrs-cli/src/js_math.rs"]
#[allow(dead_code)]
mod js_math;
#[path = "../../asrs-cli/src/js_msgpack.rs"]
#[allow(dead_code)]
mod js_msgpack;
#[path = "../../asrs-cli/src/js_regex.rs"]
#[allow(dead_code)]
mod js_regex;
#[path = "../../asrs-cli/src/js_string.rs"]
#[allow(dead_code)]
mod js_string;
#[path = "../../asrs-cli/src/js_time.rs"]
#[allow(dead_code)]
mod js_time;
#[path = "../../asrs-cli/src/log_field.rs"]
#[allow(dead_code)]
mod log_field;
#[path = "../../asrs-cli/src/log_keys.rs"]
#[allow(dead_code)]
mod log_keys;
#[path = "../../asrs-cli/src/log_run.rs"]
#[allow(dead_code)]
mod log_run;
#[path = "../../asrs-cli/src/log_struct.rs"]
#[allow(dead_code)]
mod log_struct;
#[path = "../../asrs-cli/src/photon.rs"]
#[allow(dead_code)]
mod photon;
#[path = "../../asrs-cli/src/shared.rs"]
#[allow(dead_code)]
mod shared;

use asrs_parity::vectors::check_fn_vectors;
use asrs_parity::{FnCase, Response};

fn dispatch(case: &FnCase<'_>) -> Response {
    match functions::lookup(case.name()).or_else(|| {
        shared::FUNCTIONS
            .iter()
            .find(|(n, _)| *n == case.name())
            .map(|(_, f)| *f)
    }) {
        Some(f) => f(&case.request.args),
        None => Response::Error(format!("no port of {}", case.name())),
    }
}

#[test]
fn photon() {
    check_fn_vectors("log/photon", dispatch);
}
