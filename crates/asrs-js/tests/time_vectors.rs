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
mod js_msgpack {
    pub const FUNCTIONS: &[(&str, crate::functions::Function)] = &[];
}
mod photon {
    pub const FUNCTIONS: &[(&str, crate::functions::Function)] = &[];
}

mod log_field {
    pub const FUNCTIONS: &[(&str, crate::functions::Function)] = &[];
}

mod log_run {
    pub const FUNCTIONS: &[(&str, crate::functions::Function)] = &[];
}

mod tabs_console {
    pub const FUNCTIONS: &[(&str, crate::functions::Function)] = &[];
    pub const CALLS: &[(&str, crate::functions::Function)] = &[];
}

mod log_struct {
    pub const FUNCTIONS: &[(&str, crate::functions::Function)] = &[];
}

#[path = "../../asrs-cli/src/js_time.rs"]
mod js_time;

use asrs_js::time::NamedZone;
use asrs_parity::vectors::check_fn_vectors;
use asrs_parity::{FnCase, Response};

fn dispatch(case: &FnCase<'_>) -> Response {
    match functions::lookup(case.name()) {
        Some(f) => f(&case.request.args),
        None => Response::Error(format!("no port of {}", case.name())),
    }
}

fn check_zoned(zone: &str, id: &str) {
    let named = NamedZone::get(zone).expect("zone resolves");
    js_time::with_zone(named, || check_fn_vectors(id, dispatch));
}

#[test]
fn time_utc() {
    check_zoned("UTC", "js/time");
}

#[test]
fn color_utc() {
    check_zoned("UTC", "js/color");
}

#[test]
fn time_zones_utc() {
    check_zoned("UTC", "js/time-zones/UTC");
}

#[test]
fn time_zones_los_angeles() {
    check_zoned("America/Los_Angeles", "js/time-zones/America/Los_Angeles");
}

#[test]
fn time_zones_kolkata() {
    check_zoned("Asia/Kolkata", "js/time-zones/Asia/Kolkata");
}

#[test]
fn time_zones_auckland() {
    check_zoned("Pacific/Auckland", "js/time-zones/Pacific/Auckland");
}
