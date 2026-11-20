use std::cell::RefCell;

use asrs_js::string::JsError;
use asrs_js::value::JsValue;
use asrs_log::msgpack::{self, KeyCache};
use asrs_parity::{Response, Utf16String, Value};

use crate::{functions, js_json, js_time};

thread_local! {
    static KEYS: RefCell<KeyCache> = RefCell::new(KeyCache::new());
}

fn respond(result: Result<JsValue, JsError>) -> Response {
    match result {
        Ok(v) => Response::Ok(js_json::from_js(&v)),
        Err(e) => throw(e),
    }
}

fn throw(e: JsError) -> Response {
    Response::Throw(Value::Error(
        Utf16String::from(e.name),
        Utf16String::from(e.message),
    ))
}

fn bytes_arg(args: &[Value]) -> Result<Vec<u8>, Response> {
    match functions::arg(args, 0) {
        Value::Bytes(b) | Value::ArrayBuffer(b) | Value::DataView(b) => Ok(b),
        Value::TypedArray(kind, items) => js_json::typed_array_bytes(&kind, &items)
            .ok_or_else(|| Response::Error("bad typed array kind".to_owned())),
        _ => Err(Response::Error("expected bytes".to_owned())),
    }
}

fn decode(args: &[Value]) -> Response {
    match bytes_arg(args) {
        Ok(bytes) => respond(KEYS.with(|keys| {
            js_time::with_ambient_rng(|rng| msgpack::decode(&bytes, &mut keys.borrow_mut(), rng))
        })),
        Err(r) => r,
    }
}

fn encode(args: &[Value]) -> Response {
    match js_json::to_js_encodable(&functions::arg(args, 0)) {
        Ok(v) => match msgpack::encode(&v) {
            Ok(bytes) => Response::Ok(Value::Bytes(bytes)),
            Err(e) => throw(e),
        },
        Err(e) => Response::Error(e),
    }
}

fn decode_multi(args: &[Value]) -> Response {
    match bytes_arg(args) {
        Ok(bytes) => {
            let (values, err) = KEYS.with(|keys| {
                js_time::with_ambient_rng(|rng| {
                    msgpack::decode_multi(&bytes, &mut keys.borrow_mut(), rng)
                })
            });
            match err {
                Some(e) => throw(e),
                None => Response::Ok(Value::Array(values.iter().map(js_json::from_js).collect())),
            }
        }
        Err(r) => r,
    }
}

pub const FUNCTIONS: &[(&str, functions::Function)] = &[
    ("npm:@msgpack/msgpack:decode", decode),
    ("npm:@msgpack/msgpack:decodeMulti", decode_multi),
    ("npm:@msgpack/msgpack:encode", encode),
];
