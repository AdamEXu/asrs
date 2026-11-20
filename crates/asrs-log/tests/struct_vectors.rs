#![allow(clippy::disallowed_methods)]

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::collections::{HashMap, VecDeque};

use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue};
use asrs_log::log::struct_decoder::{Decoded, StructDecoder};
use asrs_parity::json::Json;
use asrs_parity::vectors::check_records;
use asrs_parity::{Record, Utf16String, Value};

type Item = (JsString, Vec<u8>, bool);

#[derive(Default)]
struct State {
    decoder: StructDecoder,
    structs: VecDeque<Item>,
    arrays: VecDeque<Item>,
    drain: VecDeque<Item>,
}

impl State {
    fn queue(&mut self, item: Item) {
        if item.2 {
            self.arrays.push_back(item);
        } else {
            self.structs.push_back(item);
        }
    }

    fn drain(&mut self) {
        self.drain = self
            .structs
            .iter()
            .chain(self.arrays.iter())
            .cloned()
            .collect();
        self.structs.clear();
        self.arrays.clear();
    }
}

fn js_string(json: &Json) -> Result<JsString, String> {
    json.as_str()
        .map(|s| JsString::from_units(s.units().to_vec()))
        .ok_or_else(|| format!("expected a string, got {json:?}"))
}

fn hex_bytes(json: &Json) -> Result<Vec<u8>, String> {
    let s = json
        .as_str()
        .ok_or_else(|| format!("expected hex, got {json:?}"))?
        .to_string_lossy();
    if s.len() % 2 != 0 {
        return Err(format!("odd hex {s:?}"));
    }
    (0..s.len() / 2)
        .map(|i| {
            u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|e| format!("hex {s:?}: {e}"))
        })
        .collect()
}

fn input_item(json: &Json) -> Result<Item, String> {
    let name = js_string(field_of(json, "n")?)?;
    let bytes = hex_bytes(field_of(json, "b")?)?;
    let is_array = matches!(json.get("a"), Some(Json::Bool(true)));
    Ok((name, bytes, is_array))
}

fn field_of<'a>(json: &'a Json, key: &str) -> Result<&'a Json, String> {
    json.get(key)
        .ok_or_else(|| format!("input missing {key:?}: {json:?}"))
}

fn decoded_value(decoded: &Decoded) -> Value {
    let mut schema_types = JsObject::new();
    for (key, ty) in decoded.schema_types.iter() {
        schema_types.insert(key.clone(), JsValue::String(ty.clone()));
    }
    js_json::from_js(&JsValue::Object(JsObject::from_iter([
        (JsString::from("data"), decoded.data.clone()),
        (JsString::from("schemaTypes"), JsValue::Object(schema_types)),
    ])))
}

fn compare(record: &Record, actual: Value) -> Result<(), String> {
    let (key, expected) = if let Some(ok) = record.field("ok") {
        ("ok", ok)
    } else if let Some(throw) = record.field("throw") {
        ("throw", throw)
    } else {
        return Err("record has neither ok nor throw".to_owned());
    };
    let expected = Value::from_json(expected).map_err(|e| e.to_string())?;
    let expected = Value::object([(key, expected)]);
    if actual != expected {
        return Err(format!("expected {expected:?}\ngot {actual:?}"));
    }
    Ok(())
}

fn replay(record: &Record, state: &mut State) -> Result<(), String> {
    if let Some(s) = record.field("s") {
        let name = js_string(field_of(s, "n")?)?;
        let bytes = hex_bytes(field_of(s, "b")?)?;
        let actual = match state.decoder.add_schema(&name, &bytes) {
            Ok(()) => Value::object([("ok", js_json::from_js(&state.decoder.to_serialized()))]),
            Err(e) => Value::object([(
                "throw",
                Value::Error(Utf16String::from(e.name), Utf16String::from(e.message)),
            )]),
        };
        compare(record, actual)?;
        state.drain();
        return Ok(());
    }
    if let Some(r) = record.field("r") {
        let input = Value::from_json(r).map_err(|e| e.to_string())?;
        let js = js_json::to_js(&input)?;
        let actual = match StructDecoder::from_serialized(&js) {
            Ok(decoder) => {
                state.decoder = decoder;
                Value::object([("ok", js_json::from_js(&state.decoder.to_serialized()))])
            }
            Err(e) => Value::object([(
                "throw",
                Value::Error(Utf16String::from(e.name), Utf16String::from(e.message)),
            )]),
        };
        return compare(record, actual);
    }
    if record.field("k").is_some() {
        state.drain();
        return Ok(());
    }
    let (input, is_array) = if let Some(d) = record.field("d") {
        (d, false)
    } else if let Some(a) = record.field("a") {
        (a, true)
    } else {
        return Err("record has no op".to_owned());
    };
    let (name, bytes, _) = input_item(input)?;
    let queued = matches!(input.get("q"), Some(Json::Bool(true)));
    if queued {
        let item = state
            .drain
            .pop_front()
            .ok_or("queued decode but the drain is empty")?;
        if item != (name.clone(), bytes.clone(), is_array) {
            return Err(format!(
                "queued decode mismatch: record has ({:?}, {} bytes, {is_array}), drain has ({:?}, {} bytes, {})",
                name,
                bytes.len(),
                item.0,
                item.1.len(),
                item.2
            ));
        }
    }
    if record.field("skip").is_some() {
        return Ok(());
    }
    let length = match input.get("l") {
        None => None,
        Some(l) => match Value::from_json(l).map_err(|e| e.to_string())? {
            Value::Number(bits) => Some(f64::from_bits(bits)),
            other => return Err(format!("bad length {other:?}")),
        },
    };
    let result = if is_array {
        state.decoder.decode_array(&name, &bytes, length)
    } else {
        state.decoder.decode(&name, &bytes)
    };
    match result {
        Ok(decoded) => compare(record, Value::object([("ok", decoded_value(&decoded))])),
        Err(e) => {
            state.queue((name, bytes, is_array));
            compare(
                record,
                Value::object([(
                    "throw",
                    Value::Error(Utf16String::from(e.name), Utf16String::from(e.message)),
                )]),
            )
        }
    }
}

fn check(id: &str) {
    let mut sessions: HashMap<String, State> = HashMap::new();
    check_records(id, |record| {
        let case = record
            .string("case")
            .ok_or_else(|| "missing case".to_owned())?;
        replay(record, sessions.entry(case).or_default())
    });
}

#[test]
fn replay_vectors() {
    check("log/struct-replay");
}

#[test]
fn fuzz_vectors() {
    check("log/struct-fuzz");
}
