//! `oracle:robotStateRun`, backed by `asrs_log::log::robot_state`.

use asrs_js::string::JsString;
use asrs_js::time::SetClock;
use asrs_log::log::robot_state::{self, RobotMode, RobotStateRange};
use asrs_log::log::{Log, LogValueSetBoolean};
use asrs_log::match_info::MatchInfo;
use asrs_parity::{Response, Utf16String, Value};

use crate::functions::{Function, arg, quiet_nan, to_number};
use crate::log_field::{boolean, js_string, set_value, string_value};
use crate::log_run;

pub const FUNCTIONS: &[(&str, Function)] = &[("oracle:robotStateRun", robot_state_run)];

fn num(v: &Value) -> Result<f64, Response> {
    to_number(v).map(quiet_nan)
}

fn n(x: f64) -> Value {
    Value::number(quiet_nan(x))
}

fn bool_set(set: &Option<LogValueSetBoolean>) -> Value {
    set.as_ref().map_or(Value::Null, set_value)
}

fn mode_name(mode: RobotMode) -> Utf16String {
    match mode {
        RobotMode::Disabled => "disabled".into(),
        RobotMode::Auto => "auto".into(),
        RobotMode::Teleop => "teleop".into(),
        RobotMode::Utility => "utility".into(),
    }
}

fn range_value(range: &RobotStateRange) -> Value {
    Value::object([
        ("start", n(range.start)),
        ("end", range.end.map_or(Value::Undefined, n)),
        ("mode", Value::String(mode_name(range.mode))),
    ])
}

fn match_info_value(info: &MatchInfo) -> Value {
    Value::object([
        ("year", n(info.year)),
        ("event", string_value(&info.event)),
        ("matchType", n(info.match_type.as_number())),
        ("matchNumber", n(info.match_number)),
    ])
}

fn key_value(key: Option<JsString>) -> Value {
    key.map_or(Value::Undefined, |key| string_value(&key))
}

fn state_call(log: &Log, call: &[Value]) -> Result<Value, Response> {
    let name = js_string(&arg(call, 0))
        .map(|s| s.to_string_lossy())
        .map_err(Response::Error)?;
    let time = |i: usize| num(&arg(call, i));
    fn ok(v: Value) -> Result<Value, Response> {
        Ok(Value::object([("ok", v)]))
    }
    match name.as_str() {
        "getEnabledKey" => ok(key_value(robot_state::get_enabled_key(log))),
        "getAutonomousKey" => ok(key_value(robot_state::get_autonomous_key(log))),
        "getUtilityKey" => ok(key_value(robot_state::get_utility_key(log))),
        "getEnabledData" => ok(bool_set(&robot_state::get_enabled_data(log))),
        "getAutonomousData" => ok(bool_set(&robot_state::get_autonomous_data(log))),
        "getUtilityData" => ok(bool_set(&robot_state::get_utility_data(log))),
        "getRobotStateRanges" => ok(Value::Array(
            robot_state::get_robot_state_ranges(log)
                .iter()
                .map(range_value)
                .collect(),
        )),
        "getProgramStartTime" => {
            ok(robot_state::get_program_start_time(log).map_or(Value::Null, n))
        }
        "getIsRedAlliance" => ok(Value::Bool(robot_state::get_is_red_alliance(log, time(1)?))),
        "getDriverStation" => ok(n(robot_state::get_driver_station(log, time(1)?))),
        "getMatchInfo" => ok(robot_state::get_match_info(log, &SetClock(time(1)?))
            .map_or(Value::Null, |info| match_info_value(&info))),
        "getElimMatchString" => Ok(
            match asrs_log::match_info::get_elim_match_string(time(1)?, time(2)?) {
                Some(text) => Value::object([("ok", string_value(&text))]),
                None => Value::object([(
                    "throw",
                    Value::Error(
                        "TypeError".into(),
                        "Cannot read properties of undefined (reading '0')".into(),
                    ),
                )]),
            },
        ),
        other => Ok(Value::object([(
            "throw",
            Value::Error(
                "Error".into(),
                format!("robotStateRun: unknown call {other:?}").into(),
            ),
        )])),
    }
}

fn robot_state_run(args: &[Value]) -> Response {
    let cache = match boolean(&arg(args, 0)) {
        Ok(c) => c,
        Err(e) => return Response::Error(e),
    };
    let ops = match arg(args, 1).as_array() {
        Some(ops) => ops.to_vec(),
        None => return Response::Error("robotStateRun: ops must be an array".to_owned()),
    };
    let calls = match arg(args, 2).as_array() {
        Some(calls) => calls.to_vec(),
        None => return Response::Error("robotStateRun: calls must be an array".to_owned()),
    };
    let (log, _) = match log_run::build_log(cache, &ops) {
        Ok(built) => built,
        Err(e) => return Response::Error(e),
    };
    let mut out = Vec::new();
    for call in &calls {
        let result = match call.as_array() {
            Some(call) => state_call(&log, call),
            None => Ok(Value::object([(
                "throw",
                Value::Error(
                    "Error".into(),
                    "robotStateRun: a call must be an array".into(),
                ),
            )])),
        };
        out.push(match result {
            Ok(v) => v,
            Err(r) => return r,
        });
    }
    Response::Ok(Value::Array(out))
}
