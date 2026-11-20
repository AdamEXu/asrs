use std::collections::HashSet;

use crate::cve1::{TYPED_ARRAYS, Value, js_property_order};
use crate::utf16::Utf16String;

/// SplitMix64: small, seedable and identical on every platform.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }

    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + self.below((hi - lo + 1) as u64) as i64
    }

    fn one_in(&mut self, n: u64) -> bool {
        self.below(n) == 0
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

const SPECIAL_NUMBERS: [f64; 14] = [
    0.0,
    -0.0,
    1.0,
    -1.0,
    0.5,
    0.1,
    f64::MIN_POSITIVE,
    5e-324,
    f64::MAX,
    f64::INFINITY,
    f64::NEG_INFINITY,
    f64::NAN,
    9007199254740993.0,
    1e21,
];

const ERROR_NAMES: [&str; 7] = [
    "Error",
    "TypeError",
    "RangeError",
    "SyntaxError",
    "ReferenceError",
    "EvalError",
    "URIError",
];

/// A random value that both CVE-1 decoders keep exactly: canonical key order, no duplicate
/// or `-0` map keys, no signaling NaNs, typed array elements already in range, and no `c` or
/// `fn` tags, which the TS decoder rejects.
pub fn random_value(rng: &mut Rng, depth: u32) -> Value {
    let kinds = if depth == 0 { 14 } else { 18 };
    match rng.below(kinds) {
        0 => Value::Undefined,
        1 => Value::Null,
        2 => Value::Bool(rng.one_in(2)),
        3 | 4 => Value::Number(random_number(rng)),
        5 | 6 => Value::String(random_string(rng)),
        7 => Value::BigInt(random_bigint(rng)),
        8 => match rng.below(3) {
            0 => Value::Bytes(random_bytes(rng)),
            1 => Value::ArrayBuffer(random_bytes(rng)),
            _ => Value::DataView(random_bytes(rng)),
        },
        9 => Value::Date(if rng.one_in(8) {
            f64::NAN.to_bits()
        } else {
            (rng.range(-8_640_000_000_000_000, 8_640_000_000_000_000) as f64).to_bits()
        }),
        10 => {
            let name = if rng.one_in(3) {
                random_identifier(rng)
            } else {
                Utf16String::from(*rng.pick(&ERROR_NAMES))
            };
            Value::Error(name, random_string(rng))
        }
        11 => Value::Symbol((!rng.one_in(3)).then(|| random_string(rng))),
        12 => Value::Boxed(Box::new(match rng.below(3) {
            0 => Value::Number(random_number(rng)),
            1 => Value::String(random_string(rng)),
            _ => Value::Bool(rng.one_in(2)),
        })),
        13 => random_typed_array(rng),
        14 => Value::Array(
            (0..rng.below(6))
                .map(|_| {
                    if rng.one_in(6) {
                        Value::Hole
                    } else {
                        random_value(rng, depth - 1)
                    }
                })
                .collect(),
        ),
        15 => {
            let mut seen = HashSet::new();
            let pairs = (0..rng.below(6))
                .map(|_| (random_key(rng), random_value(rng, depth - 1)))
                .filter(|(k, _)| seen.insert(k.clone()))
                .collect();
            Value::Object(js_property_order(pairs))
        }
        16 => {
            let mut seen = HashSet::new();
            Value::Map(
                (0..rng.below(5))
                    .map(|_| (random_value(rng, depth - 1), random_value(rng, depth - 1)))
                    .map(|(k, v)| (zero_sign(k), v))
                    .filter(|(k, _)| primitive_identity(k).is_none_or(|id| seen.insert(id)))
                    .collect(),
            )
        }
        _ => {
            let mut seen = HashSet::new();
            Value::Set(
                (0..rng.below(5))
                    .map(|_| zero_sign(random_value(rng, depth - 1)))
                    .filter(|v| primitive_identity(v).is_none_or(|id| seen.insert(id)))
                    .collect(),
            )
        }
    }
}

fn zero_sign(value: Value) -> Value {
    match value {
        Value::Number(0x8000_0000_0000_0000) => Value::Number(0),
        other => other,
    }
}

fn primitive_identity(value: &Value) -> Option<String> {
    match value {
        Value::Number(bits) if f64::from_bits(*bits).is_nan() => Some("NaN".to_owned()),
        Value::Undefined
        | Value::Null
        | Value::Bool(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::BigInt(_) => Some(value.encode()),
        _ => None,
    }
}

fn quiet(bits: u64) -> u64 {
    if f64::from_bits(bits).is_nan() {
        bits | 0x0008_0000_0000_0000
    } else {
        bits
    }
}

fn random_number(rng: &mut Rng) -> u64 {
    match rng.below(4) {
        0 => rng.pick(&SPECIAL_NUMBERS).to_bits(),
        1 => quiet(rng.next_u64()),
        2 => (rng.range(-1000, 1000) as f64).to_bits(),
        _ => ((rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64 * 2e6 - 1e6).to_bits(),
    }
}

fn random_string(rng: &mut Rng) -> Utf16String {
    let mut units = Vec::new();
    for _ in 0..rng.below(12) {
        match rng.below(9) {
            0..=2 => units.push(rng.range(0x20, 0x7e) as u16),
            3 => units.push(rng.range(0, 0x1f) as u16),
            4 => units.push(*rng.pick(&[0x22u16, 0x5c, 0x7f, 0x2028, 0x2029, 0xfeff, 0xfffd])),
            5 => units.push(rng.range(0x80, 0xd7ff) as u16),
            6 => units.push(rng.range(0xd800, 0xdfff) as u16),
            7 => {
                let c = char::from_u32(rng.range(0x10000, 0x10ffff) as u32).expect("astral scalar");
                let mut buf = [0u16; 2];
                units.extend_from_slice(c.encode_utf16(&mut buf));
            }
            _ => units.push(rng.range(0xe000, 0xffff) as u16),
        }
    }
    Utf16String::from_units(units)
}

fn random_identifier(rng: &mut Rng) -> Utf16String {
    let len = 1 + rng.below(8);
    let s: String = (0..len)
        .map(|_| (b'a' + rng.below(26) as u8) as char)
        .collect();
    Utf16String::from(s)
}

fn random_key(rng: &mut Rng) -> Utf16String {
    match rng.below(4) {
        0 => Utf16String::from(rng.below(20).to_string()),
        1 => Utf16String::from(
            rng.pick(&[
                "4294967294",
                "4294967295",
                "01",
                "-0",
                "1.5",
                "",
                "__proto__",
                "length",
            ])
            .to_owned(),
        ),
        _ => random_string(rng),
    }
}

fn random_bigint(rng: &mut Rng) -> String {
    if rng.one_in(5) {
        return "0".to_owned();
    }
    let len = 1 + rng.below(40);
    let mut digits = String::new();
    digits.push((b'1' + rng.below(9) as u8) as char);
    for _ in 1..len {
        digits.push((b'0' + rng.below(10) as u8) as char);
    }
    if rng.one_in(2) {
        digits.insert(0, '-');
    }
    digits
}

fn random_bytes(rng: &mut Rng) -> Vec<u8> {
    (0..rng.below(16)).map(|_| rng.next_u64() as u8).collect()
}

fn random_typed_array(rng: &mut Rng) -> Value {
    let name = *rng.pick(&TYPED_ARRAYS);
    let items = (0..rng.below(6))
        .map(|_| match name {
            "Int8Array" => int(rng, -128, 127),
            "Uint8ClampedArray" => int(rng, 0, 255),
            "Int16Array" => int(rng, -32768, 32767),
            "Uint16Array" => int(rng, 0, 65535),
            "Int32Array" => int(rng, i64::from(i32::MIN), i64::from(i32::MAX)),
            "Uint32Array" => int(rng, 0, i64::from(u32::MAX)),
            "Float32Array" => {
                let narrow = f32::from_bits(rng.next_u64() as u32);
                Value::Number(quiet(f64::from(narrow).to_bits()))
            }
            "Float64Array" => Value::Number(random_number(rng)),
            "BigInt64Array" => Value::BigInt((rng.next_u64() as i64).to_string()),
            _ => Value::BigInt(rng.next_u64().to_string()),
        })
        .collect();
    Value::TypedArray(name.to_owned(), items)
}

fn int(rng: &mut Rng, lo: i64, hi: i64) -> Value {
    Value::number(rng.range(lo, hi) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_values_are_fixed_points_of_the_rust_decoder() {
        let mut rng = Rng::new(1);
        for i in 0..20_000 {
            let value = random_value(&mut rng, 4);
            let text = value.encode();
            let again = Value::decode(&text).unwrap_or_else(|e| panic!("case {i}: {e}\n{text}"));
            assert_eq!(again.encode(), text, "case {i}");
        }
    }

    #[test]
    fn same_seed_same_values() {
        let values = |seed| {
            let mut rng = Rng::new(seed);
            (0..50)
                .map(|_| random_value(&mut rng, 3).encode())
                .collect::<Vec<_>>()
        };
        assert_eq!(values(7), values(7));
        assert_ne!(values(7), values(8));
    }
}
