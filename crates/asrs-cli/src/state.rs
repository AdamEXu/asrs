//! `oracle:stateRun`, backed by `asrs_log::joystick_state` and
//! `asrs_log::mechanism_state`.

use asrs_js::string::{JsError, JsString};
use asrs_log::log::Log;
use asrs_log::log::joystick_state::{JoystickState, decode_pov_bitfield, get_joystick_state};
use asrs_log::log::mechanism_state::{MechanismState, get_mechanism_state, merge_mechanism_states};
use asrs_parity::{Response, Utf16String, Value};

use crate::functions::{arg, to_number, to_units};
use crate::log_field::{boolean, js_string};
use crate::{log_run, shared::Function};

pub const FUNCTIONS: &[(&str, Function)] = &[("oracle:stateRun", state_run)];

fn quiet(x: f64) -> f64 {
    if x.is_nan() {
        f64::from_bits(x.to_bits() | (1 << 51))
    } else {
        x
    }
}

fn n(x: f64) -> Value {
    Value::number(quiet(x))
}

fn ok(v: Value) -> Result<Value, Response> {
    Ok(Value::object([("ok", v)]))
}

fn throw(e: JsError) -> Result<Value, Response> {
    Ok(Value::object([(
        "throw",
        Value::Error(e.name.into(), Utf16String::from(e.message)),
    )]))
}

fn joystick_value(s: &JoystickState) -> Value {
    Value::object([
        ("mapping", Value::string(s.mapping.name())),
        (
            "buttons",
            Value::Array(
                s.buttons
                    .iter()
                    .map(|b| {
                        Value::object([("id", n(f64::from(b.id))), ("state", Value::Bool(b.state))])
                    })
                    .collect(),
            ),
        ),
        (
            "axes",
            Value::Array(
                s.axes
                    .iter()
                    .map(|a| Value::object([("id", n(f64::from(a.id))), ("value", n(a.value))]))
                    .collect(),
            ),
        ),
        (
            "povs",
            Value::Array(
                s.povs
                    .iter()
                    .map(|p| Value::object([("id", n(f64::from(p.id))), ("angle", n(p.angle))]))
                    .collect(),
            ),
        ),
        (
            "touchpads",
            Value::Array(
                s.touchpads
                    .iter()
                    .map(|pad| {
                        Value::Array(
                            pad.iter()
                                .map(|f| {
                                    Value::object([
                                        ("x", n(f.x)),
                                        ("y", n(f.y)),
                                        ("down", Value::Bool(f.down)),
                                    ])
                                })
                                .collect(),
                        )
                    })
                    .collect(),
            ),
        ),
    ])
}

fn coords(values: &[Option<f64>]) -> Value {
    Value::Array(values.iter().map(|x| x.map_or(Value::Null, n)).collect())
}

fn mechanism_value(s: &MechanismState) -> Value {
    Value::object([
        (
            "backgroundColor",
            Value::String(Utf16String::from_units(s.background_color.units().to_vec())),
        ),
        (
            "dimensions",
            Value::Array(s.dimensions.iter().map(|&x| n(x)).collect()),
        ),
        (
            "lines",
            Value::Array(
                s.lines
                    .iter()
                    .map(|line| {
                        Value::object([
                            ("start", coords(&line.start)),
                            ("end", coords(&line.end)),
                            (
                                "color",
                                line.color.clone().map_or(n(0.0), |c| {
                                    Value::String(Utf16String::from_units(c.units().to_vec()))
                                }),
                            ),
                            ("weight", line.weight.map_or(Value::Null, n)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

fn mechanism_or_null(state: Option<MechanismState>) -> Result<Value, Response> {
    ok(state.as_ref().map_or(Value::Null, mechanism_value))
}

fn state_call(log: &Log, call: &[Value]) -> Result<Value, Response> {
    let name = js_string(&arg(call, 0))
        .map(|s| s.to_string_lossy())
        .map_err(Response::Error)?;
    let num = |i: usize| to_number(&arg(call, i));
    match name.as_str() {
        "getJoystickState" => match get_joystick_state(log, num(1)?, num(2)?) {
            Ok(s) => ok(joystick_value(&s)),
            Err(e) => throw(e),
        },
        "getMechanismState" => {
            let key = JsString::from_units(to_units(&arg(call, 1))?);
            mechanism_or_null(get_mechanism_state(log, &key, num(2)?))
        }
        "mergeMechanismStates" => {
            let Some(keys) = arg(call, 1).as_array().map(<[Value]>::to_vec) else {
                return Err(Response::Error(
                    "stateRun: merge keys must be an array".to_owned(),
                ));
            };
            let time = num(2)?;
            let mut states = Vec::new();
            for key in &keys {
                let key = JsString::from_units(to_units(key)?);
                states.extend(get_mechanism_state(log, &key, time));
            }
            mechanism_or_null(merge_mechanism_states(&states))
        }
        "decodePovBitfield" => ok(n(decode_pov_bitfield(num(1)?))),
        other => Ok(Value::object([(
            "throw",
            Value::Error(
                "Error".into(),
                format!("stateRun: unknown call {other:?}").into(),
            ),
        )])),
    }
}

fn state_run(args: &[Value]) -> Response {
    let cache = match boolean(&arg(args, 0)) {
        Ok(c) => c,
        Err(e) => return Response::Error(e),
    };
    let ops = match arg(args, 1).as_array() {
        Some(ops) => ops.to_vec(),
        None => return Response::Error("stateRun: ops must be an array".to_owned()),
    };
    let calls = match arg(args, 2).as_array() {
        Some(calls) => calls.to_vec(),
        None => return Response::Error("stateRun: calls must be an array".to_owned()),
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
                Value::Error("TypeError".into(), "call is not iterable".into()),
            )])),
        };
        out.push(match result {
            Ok(v) => v,
            Err(r) => return r,
        });
    }
    Response::Ok(Value::Array(out))
}
