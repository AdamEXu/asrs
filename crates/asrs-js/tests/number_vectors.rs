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
#[path = "../../asrs-cli/src/js_regex.rs"]
#[allow(dead_code)]
mod js_regex;
#[path = "../../asrs-cli/src/js_string.rs"]
#[allow(dead_code)]
mod js_string;
#[path = "../../asrs-cli/src/js_time.rs"]
#[allow(dead_code)]
mod js_time;

mod js_msgpack {
    pub const FUNCTIONS: &[(&str, crate::functions::Function)] = &[];
}

mod log_field {
    pub const FUNCTIONS: &[(&str, crate::functions::Function)] = &[];
}

use asrs_parity::{FnCase, Response, fn_vectors};

fn dispatch(case: &FnCase<'_>) -> Response {
    match functions::lookup(case.name()) {
        Some(f) => f(&case.request.args),
        None => Response::Error(format!("no port of {}", case.name())),
    }
}

fn_vectors!(number_tostring, "number/tostring", dispatch);
fn_vectors!(number_tofixed, "number/tofixed", dispatch);
fn_vectors!(number_toprecision, "number/toprecision", dispatch);
fn_vectors!(number_toexponential, "number/toexponential", dispatch);
fn_vectors!(number_parse, "number/parse", dispatch);
fn_vectors!(number_numberformat, "number/numberformat", dispatch);
