//! `fn` names for `main/assetUtil.ts`, backed by `asrs_model::assets`. Adapter only: CVE-1
//! arg to `JsValue`, call the parser, encode the result.

use asrs_js::value::JsValue;
use asrs_model::assets::{self, Parsed, TypeError};
use asrs_parity::{Response, Utf16String, Value};

use crate::js_json::{from_js, to_js};

pub type Function = fn(&[Value]) -> Response;

pub const FUNCTIONS: &[(&str, Function)] = &[
    ("main/assetUtil:parseField2d", parse_field_2d),
    ("main/assetUtil:parseField3d", parse_field_3d),
    ("main/assetUtil:parseRobot", parse_robot),
    ("main/assetUtil:parseJoystick", parse_joystick),
];

fn call<T>(
    args: &[Value],
    parse: fn(&JsValue) -> assets::ParseResult<T>,
    to_value: fn(&T) -> JsValue,
) -> Response {
    let raw = match args.first().map(to_js).unwrap_or(Ok(JsValue::Undefined)) {
        Ok(v) => v,
        Err(e) => return Response::Error(e),
    };
    match parse(&raw) {
        Ok(Parsed::Config(config)) => Response::Ok(from_js(&to_value(&config))),
        Ok(Parsed::Invalid) => Response::Ok(Value::string("invalid")),
        Ok(Parsed::Skip) => Response::Ok(Value::string("skip")),
        Err(e) => type_error(&e),
    }
}

fn type_error(e: &TypeError) -> Response {
    Response::Throw(Value::Error(
        "TypeError".into(),
        Utf16String::from_units(e.message.units().to_vec()),
    ))
}

fn parse_field_2d(args: &[Value]) -> Response {
    call(args, assets::parse_field_2d, assets::Config2d::to_js)
}

fn parse_field_3d(args: &[Value]) -> Response {
    call(args, assets::parse_field_3d, assets::Config3dField::to_js)
}

fn parse_robot(args: &[Value]) -> Response {
    call(args, assets::parse_robot, assets::Config3dRobot::to_js)
}

fn parse_joystick(args: &[Value]) -> Response {
    call(args, assets::parse_joystick, assets::ConfigJoystick::to_js)
}
