#![allow(clippy::disallowed_methods)]

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use asrs_js::string::JsError;
use asrs_js::time::Rng;
use asrs_log::msgpack::{self, KeyCache};
use asrs_parity::vectors::check_records;
use asrs_parity::{Record, Utf16String, Value};

/// Replays a record's recorded `Math.random` values and counts how many were drawn.
struct Replay {
    values: Vec<f64>,
    drawn: usize,
}

impl Rng for Replay {
    fn next_f64(&mut self) -> f64 {
        let v = self.values.get(self.drawn).copied().unwrap_or(f64::NAN);
        self.drawn += 1;
        v
    }
}

impl Replay {
    fn of(record: &Record) -> Result<Self, String> {
        let values = match record.field("r") {
            None => Vec::new(),
            Some(_) => match record.value("r").map_err(|e| e.to_string())? {
                Value::Array(items) => items
                    .iter()
                    .map(|v| match v {
                        Value::Number(bits) => Ok(f64::from_bits(*bits)),
                        other => Err(format!("r holds {other:?}")),
                    })
                    .collect::<Result<_, _>>()?,
                other => return Err(format!("r is not an array: {other:?}")),
            },
        };
        Ok(Replay { values, drawn: 0 })
    }

    fn check(&self) -> Result<(), String> {
        if self.drawn != self.values.len() {
            return Err(format!(
                "drew {} Math.random values, upstream drew {}",
                self.drawn,
                self.values.len()
            ));
        }
        Ok(())
    }
}

fn thrown(e: JsError) -> Value {
    Value::Error(Utf16String::from(e.name), Utf16String::from(e.message))
}

fn hex_bytes(record: &Record, key: &str) -> Result<Vec<u8>, String> {
    match record.value(key).map_err(|e| e.to_string())? {
        Value::Bytes(b) => Ok(b),
        other => Err(format!("{key} is not bytes: {other:?}")),
    }
}

fn outcome(record: &Record) -> Result<Result<Value, Value>, String> {
    if let Some(ok) = record.field("ok") {
        Ok(Ok(Value::from_json(ok).map_err(|e| e.to_string())?))
    } else if let Some(e) = record.field("throw") {
        Ok(Err(Value::from_json(e).map_err(|e| e.to_string())?))
    } else {
        Err("record has neither ok nor throw".to_owned())
    }
}

#[test]
fn decode_vectors() {
    let mut keys = KeyCache::new();
    check_records("msgpack/decode", |record| {
        let bytes = hex_bytes(record, "d")?;
        let mut rng = Replay::of(record)?;
        let actual = msgpack::decode(&bytes, &mut keys, &mut rng)
            .map(|v| js_json::from_js(&v))
            .map_err(thrown);
        let expected = outcome(record)?;
        if actual != expected {
            return Err(format!("expected {expected:?}\ngot {actual:?}"));
        }
        rng.check()
    });
}

#[test]
fn multi_vectors() {
    let mut keys = KeyCache::new();
    check_records("msgpack/multi", |record| {
        let bytes = hex_bytes(record, "m")?;
        let mut rng = Replay::of(record)?;
        let (values, err) = msgpack::decode_multi(&bytes, &mut keys, &mut rng);
        let actual = match err {
            Some(e) => Err(thrown(e)),
            None => Ok(Value::Array(values.iter().map(js_json::from_js).collect())),
        };
        let expected = outcome(record)?;
        if actual != expected {
            return Err(format!("expected {expected:?}\ngot {actual:?}"));
        }
        rng.check()
    });
}

#[test]
fn encode_vectors() {
    check_records("msgpack/encode", |record| {
        let input = record.value("e").map_err(|e| e.to_string())?;
        let value = js_json::to_js_encodable(&input)?;
        let actual = msgpack::encode(&value).map(Value::Bytes).map_err(thrown);
        let expected = outcome(record)?;
        if actual != expected {
            return Err(format!(
                "input {input:?}\nexpected {expected:?}\ngot {actual:?}"
            ));
        }
        Ok(())
    });
}
