use std::collections::HashMap;
use std::fmt::{self, Write};

use crate::json::{self, Json, write_string};
use crate::utf16::Utf16String;

pub const TYPED_ARRAYS: [&str; 10] = [
    "Int8Array",
    "Uint8ClampedArray",
    "Int16Array",
    "Uint16Array",
    "Int32Array",
    "Uint32Array",
    "Float32Array",
    "Float64Array",
    "BigInt64Array",
    "BigUint64Array",
];

const CANONICAL_NAN: u64 = 0x7ff8_0000_0000_0000;

/// A JS value in CVE-1 form. Numbers are IEEE-754 bit patterns so equality is bit-exact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Undefined,
    Null,
    Bool(bool),
    Number(u64),
    String(Utf16String),
    /// Canonical decimal digits, with a leading `-` when negative.
    BigInt(String),
    Array(Vec<Value>),
    /// An array hole; only meaningful as an `Array` element.
    Hole,
    Object(Vec<(Utf16String, Value)>),
    Instance(Utf16String, Vec<(Utf16String, Value)>),
    Bytes(Vec<u8>),
    TypedArray(String, Vec<Value>),
    ArrayBuffer(Vec<u8>),
    DataView(Vec<u8>),
    Date(u64),
    Map(Vec<(Value, Value)>),
    Set(Vec<Value>),
    Error(Utf16String, Utf16String),
    Boxed(Box<Value>),
    Function(Utf16String),
    Symbol(Option<Utf16String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeError(pub String);

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CVE-1: {}", self.0)
    }
}

impl std::error::Error for DecodeError {}

impl From<json::ParseError> for DecodeError {
    fn from(e: json::ParseError) -> Self {
        DecodeError(e.to_string())
    }
}

type Decoded = Result<Value, DecodeError>;

fn fail<T>(message: impl Into<String>) -> Result<T, DecodeError> {
    Err(DecodeError(message.into()))
}

impl Value {
    pub fn number(x: f64) -> Self {
        Value::Number(x.to_bits())
    }

    pub fn string(s: &str) -> Self {
        Value::String(Utf16String::from(s))
    }

    pub fn object<K: Into<Utf16String>>(pairs: impl IntoIterator<Item = (K, Value)>) -> Self {
        Value::Object(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(bits) => Some(f64::from_bits(*bits)),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&Utf16String> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The value of `key` in an `Object` or `Instance`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(pairs) | Value::Instance(_, pairs) => {
                pairs.iter().find(|(k, _)| k.eq_str(key)).map(|(_, v)| v)
            }
            _ => None,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Value::Undefined => "undefined",
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::BigInt(_) => "bigint",
            Value::Array(_) => "array",
            Value::Hole => "hole",
            Value::Object(_) => "object",
            Value::Instance(..) => "instance",
            Value::Bytes(_) => "Uint8Array",
            Value::TypedArray(..) => "typed array",
            Value::ArrayBuffer(_) => "ArrayBuffer",
            Value::DataView(_) => "DataView",
            Value::Date(_) => "Date",
            Value::Map(_) => "Map",
            Value::Set(_) => "Set",
            Value::Error(..) => "Error",
            Value::Boxed(_) => "boxed primitive",
            Value::Function(_) => "function",
            Value::Symbol(_) => "symbol",
        }
    }

    pub fn encode(&self) -> String {
        let mut out = String::new();
        self.encode_into(&mut out);
        out
    }

    pub fn encode_into(&self, out: &mut String) {
        match self {
            Value::Undefined => out.push_str(r#"{"u":null}"#),
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Number(bits) => {
                let _ = write!(out, r#"{{"n":"{bits:016x}"}}"#);
            }
            Value::String(s) => write_string(s.units(), out),
            Value::BigInt(digits) => {
                out.push_str(r#"{"bi":"#);
                write_string(&digits.encode_utf16().collect::<Vec<_>>(), out);
                out.push('}');
            }
            Value::Array(items) => encode_list(items, out),
            Value::Hole => out.push_str(r#"{"hole":null}"#),
            Value::Object(pairs) => {
                out.push_str(r#"{"o":"#);
                encode_pairs(pairs, out);
                out.push('}');
            }
            Value::Instance(name, pairs) => {
                out.push_str(r#"{"c":["#);
                write_string(name.units(), out);
                out.push(',');
                encode_pairs(pairs, out);
                out.push_str("]}");
            }
            Value::Bytes(bytes) => encode_hex("b", bytes, out),
            Value::TypedArray(name, items) => {
                out.push_str(r#"{"ta":["#);
                write_string(&name.encode_utf16().collect::<Vec<_>>(), out);
                out.push(',');
                encode_list(items, out);
                out.push_str("]}");
            }
            Value::ArrayBuffer(bytes) => encode_hex("ab", bytes, out),
            Value::DataView(bytes) => encode_hex("dv", bytes, out),
            Value::Date(bits) => {
                let _ = write!(out, r#"{{"d":"{bits:016x}"}}"#);
            }
            Value::Map(entries) => {
                out.push_str(r#"{"map":["#);
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push('[');
                    k.encode_into(out);
                    out.push(',');
                    v.encode_into(out);
                    out.push(']');
                }
                out.push_str("]}");
            }
            Value::Set(items) => {
                out.push_str(r#"{"set":"#);
                encode_list(items, out);
                out.push('}');
            }
            Value::Error(name, message) => {
                out.push_str(r#"{"err":["#);
                write_string(name.units(), out);
                out.push(',');
                write_string(message.units(), out);
                out.push_str("]}");
            }
            Value::Boxed(inner) => {
                out.push_str(r#"{"box":"#);
                inner.encode_into(out);
                out.push('}');
            }
            Value::Function(name) => {
                out.push_str(r#"{"fn":"#);
                write_string(name.units(), out);
                out.push('}');
            }
            Value::Symbol(description) => {
                out.push_str(r#"{"sym":"#);
                match description {
                    Some(d) => write_string(d.units(), out),
                    None => out.push_str("null"),
                }
                out.push('}');
            }
        }
    }

    /// Decodes one CVE-1 text into the value the TS decoder would build, so re-encoding
    /// yields the bytes JS would write. Unlike the TS decoder this also accepts `c` and `fn`.
    pub fn decode(text: &str) -> Decoded {
        Value::from_json(&json::parse(text)?)
    }

    pub fn from_json(json: &Json) -> Decoded {
        decode_value(json, false)
    }

    /// Best-effort reading of arbitrary JSON for display and diffing: CVE-1 tags are decoded
    /// where they are valid and anything else becomes a plain object, array or number.
    pub fn from_json_lenient(json: &Json) -> Value {
        match json {
            Json::Null => Value::Null,
            Json::Bool(b) => Value::Bool(*b),
            Json::Number(n) => Value::number(*n),
            Json::String(s) => Value::String(s.clone()),
            Json::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| match item {
                        Json::Object(pairs) if is_hole(pairs) => Value::Hole,
                        _ => Value::from_json_lenient(item),
                    })
                    .collect(),
            ),
            Json::Object(pairs) => {
                if let [(key, _)] = pairs.as_slice()
                    && TAGS.iter().any(|t| key.eq_str(t))
                    && let Ok(value) = Value::from_json(json)
                {
                    return value;
                }
                Value::Object(
                    pairs
                        .iter()
                        .map(|(k, v)| (k.clone(), Value::from_json_lenient(v)))
                        .collect(),
                )
            }
        }
    }
}

impl From<f64> for Value {
    fn from(x: f64) -> Self {
        Value::number(x)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::string(s)
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::string(&s)
    }
}

impl From<Utf16String> for Value {
    fn from(s: Utf16String) -> Self {
        Value::String(s)
    }
}

impl From<Vec<Value>> for Value {
    fn from(items: Vec<Value>) -> Self {
        Value::Array(items)
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode())
    }
}

fn encode_list(items: &[Value], out: &mut String) {
    out.push('[');
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        item.encode_into(out);
    }
    out.push(']');
}

fn encode_pairs(pairs: &[(Utf16String, Value)], out: &mut String) {
    out.push('[');
    for (i, (key, value)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('[');
        write_string(key.units(), out);
        out.push(',');
        value.encode_into(out);
        out.push(']');
    }
    out.push(']');
}

fn encode_hex(tag: &str, bytes: &[u8], out: &mut String) {
    let _ = write!(out, r#"{{"{tag}":""#);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out.push_str("\"}");
}

const TAGS: [&str; 17] = [
    "u", "n", "bi", "hole", "o", "c", "b", "ta", "ab", "dv", "d", "map", "set", "err", "box", "fn",
    "sym",
];

fn is_hole(pairs: &[(Utf16String, Json)]) -> bool {
    pairs.iter().any(|(k, _)| k.eq_str("hole"))
}

fn decode_value(json: &Json, in_array: bool) -> Decoded {
    match json {
        Json::Null => Ok(Value::Null),
        Json::Bool(b) => Ok(Value::Bool(*b)),
        Json::Number(n) => Ok(Value::number(*n)),
        Json::String(s) => Ok(Value::String(s.clone())),
        Json::Array(items) => items
            .iter()
            .map(|item| decode_value(item, true))
            .collect::<Result<_, _>>()
            .map(Value::Array),
        Json::Object(pairs) if in_array && is_hole(pairs) => Ok(Value::Hole),
        Json::Object(pairs) => {
            let [(tag, body)] = pairs.as_slice() else {
                let keys: Vec<String> = pairs.iter().map(|(k, _)| k.quoted()).collect();
                return fail(format!(
                    "tag object must have exactly one key, got [{}]",
                    keys.join(",")
                ));
            };
            let tag = tag.as_string().unwrap_or_default();
            decode_tag(&tag, body)
        }
    }
}

fn decode_tag(tag: &str, body: &Json) -> Decoded {
    match tag {
        "u" => Ok(Value::Undefined),
        "n" => Ok(Value::Number(parse_bits(expect_ascii(body, "n")?)?)),
        "bi" => Ok(Value::BigInt(canonical_bigint(expect_ascii(body, "bi")?)?)),
        "o" => Ok(Value::Object(decode_pairs(body, "o")?)),
        "c" => {
            let [name, pairs] = expect_tuple(body, "c")?;
            Ok(Value::Instance(
                expect_string(name, "c name")?.clone(),
                decode_pairs(pairs, "c")?,
            ))
        }
        "b" => Ok(Value::Bytes(parse_hex(expect_ascii(body, "b")?)?)),
        "ab" => Ok(Value::ArrayBuffer(parse_hex(expect_ascii(body, "ab")?)?)),
        "dv" => Ok(Value::DataView(parse_hex(expect_ascii(body, "dv")?)?)),
        "ta" => {
            let [name, items] = expect_tuple(body, "ta")?;
            let name = expect_ascii(name, "ta name")?;
            if !TYPED_ARRAYS.contains(&name.as_str()) {
                return fail(format!("unknown typed array {name}"));
            }
            let items = expect_array(items, "ta items")?
                .iter()
                .map(|item| typed_array_element(&name, decode_value(item, false)?))
                .collect::<Result<_, _>>()?;
            Ok(Value::TypedArray(name, items))
        }
        "d" => Ok(Value::Date(time_clip(parse_bits(expect_ascii(
            body, "d",
        )?)?))),
        "map" => {
            let mut entries: Vec<(Value, Value)> = Vec::new();
            let mut index: HashMap<PrimitiveKey, usize> = HashMap::new();
            for entry in expect_array(body, "map")? {
                let [k, v] = expect_tuple(entry, "map entry")?;
                let key = negative_zero_to_zero(decode_value(k, false)?);
                let value = decode_value(v, false)?;
                let primitive = PrimitiveKey::of(&key);
                if let Some(&i) = primitive.as_ref().and_then(|p| index.get(p)) {
                    entries[i].1 = value;
                    continue;
                }
                if let Some(p) = primitive {
                    index.insert(p, entries.len());
                }
                entries.push((key, value));
            }
            Ok(Value::Map(entries))
        }
        "set" => {
            let mut items = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for item in expect_array(body, "set")? {
                let value = negative_zero_to_zero(decode_value(item, false)?);
                if let Some(p) = PrimitiveKey::of(&value)
                    && !seen.insert(p)
                {
                    continue;
                }
                items.push(value);
            }
            Ok(Value::Set(items))
        }
        "err" => {
            let [name, message] = expect_tuple(body, "err")?;
            let name = expect_string(name, "err name")?;
            let mut message = expect_string(message, "err message")?.clone();
            if name.eq_str("DOMException") {
                return fail("DOMException cannot be decoded");
            }
            if name.eq_str("AggregateError") || name.eq_str("SuppressedError") {
                message = Utf16String::new();
            }
            Ok(Value::Error(name.clone(), message))
        }
        "box" => Ok(match decode_value(body, false)? {
            primitive @ (Value::Number(_) | Value::String(_) | Value::Bool(_)) => {
                Value::Boxed(Box::new(primitive))
            }
            Value::BigInt(_) => Value::Instance(Utf16String::from("BigInt"), Vec::new()),
            Value::Symbol(_) => Value::Instance(Utf16String::from("Symbol"), Vec::new()),
            Value::Null | Value::Undefined => Value::Object(Vec::new()),
            object => object,
        }),
        "fn" => Ok(Value::Function(expect_string(body, "fn")?.clone())),
        "sym" => match body {
            Json::Null => Ok(Value::Symbol(None)),
            Json::String(s) => Ok(Value::Symbol(Some(s.clone()))),
            _ => fail("sym must be a string or null"),
        },
        "hole" => fail("hole outside an array"),
        _ => fail(format!("unknown tag {}", Utf16String::from(tag).quoted())),
    }
}

fn expect_array<'a>(json: &'a Json, what: &str) -> Result<&'a [Json], DecodeError> {
    json.as_array()
        .ok_or_else(|| DecodeError(format!("{what} must be an array")))
}

fn expect_tuple<'a, const N: usize>(
    json: &'a Json,
    what: &str,
) -> Result<&'a [Json; N], DecodeError> {
    expect_array(json, what)?
        .try_into()
        .map_err(|_| DecodeError(format!("{what} must be an array of {N}")))
}

fn expect_string<'a>(json: &'a Json, what: &str) -> Result<&'a Utf16String, DecodeError> {
    json.as_str()
        .ok_or_else(|| DecodeError(format!("{what} must be a string")))
}

fn expect_ascii(json: &Json, what: &str) -> Result<String, DecodeError> {
    expect_string(json, what)?
        .as_string()
        .ok_or_else(|| DecodeError(format!("{what} must be ASCII")))
}

fn parse_bits(text: String) -> Result<u64, DecodeError> {
    if text.len() != 16 || !text.bytes().all(is_lower_hex) {
        return fail(format!(
            "bad number bits {}",
            Utf16String::from(text).quoted()
        ));
    }
    Ok(u64::from_str_radix(&text, 16).expect("validated hex"))
}

fn is_lower_hex(b: u8) -> bool {
    b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
}

fn parse_hex(text: String) -> Result<Vec<u8>, DecodeError> {
    if !text.len().is_multiple_of(2) || !text.bytes().all(is_lower_hex) {
        return fail("bad hex bytes");
    }
    Ok((0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("validated hex"))
        .collect())
}

fn canonical_bigint(text: String) -> Result<String, DecodeError> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.as_str()),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return fail(format!(
            "bad bigint {}",
            Utf16String::from(text.as_str()).quoted()
        ));
    }
    let trimmed = digits.trim_start_matches('0');
    Ok(match (trimmed.is_empty(), negative) {
        (true, _) => "0".to_owned(),
        (false, true) => format!("-{trimmed}"),
        (false, false) => trimmed.to_owned(),
    })
}

fn decode_pairs(json: &Json, what: &str) -> Result<Vec<(Utf16String, Value)>, DecodeError> {
    let mut pairs: Vec<(Utf16String, Value)> = Vec::new();
    let mut index: HashMap<Utf16String, usize> = HashMap::new();
    for pair in expect_array(json, what)? {
        let [key, value] = expect_tuple(pair, &format!("{what} entry"))?;
        let key = expect_string(key, &format!("{what} key"))?;
        let value = decode_value(value, false)?;
        match index.get(key) {
            Some(&i) => pairs[i].1 = value,
            None => {
                index.insert(key.clone(), pairs.len());
                pairs.push((key.clone(), value));
            }
        }
    }
    Ok(js_property_order(pairs))
}

/// Integer-like keys (array indices up to 2^32 - 2) ascending, then the rest in insertion order.
pub fn js_property_order(pairs: Vec<(Utf16String, Value)>) -> Vec<(Utf16String, Value)> {
    let (mut indexed, named): (Vec<_>, Vec<_>) = pairs
        .into_iter()
        .map(|(k, v)| (array_index(&k), k, v))
        .partition(|(i, _, _)| i.is_some());
    indexed.sort_by_key(|(i, _, _)| *i);
    indexed
        .into_iter()
        .chain(named)
        .map(|(_, k, v)| (k, v))
        .collect()
}

pub fn array_index(key: &Utf16String) -> Option<u32> {
    let units = key.units();
    if units.is_empty() || units.len() > 10 || (units.len() > 1 && units[0] == u16::from(b'0')) {
        return None;
    }
    let mut value: u64 = 0;
    for &u in units {
        if !(u16::from(b'0')..=u16::from(b'9')).contains(&u) {
            return None;
        }
        value = value * 10 + u64::from(u - u16::from(b'0'));
    }
    (value <= 4_294_967_294).then_some(value as u32)
}

fn time_clip(bits: u64) -> u64 {
    let t = f64::from_bits(bits);
    if !t.is_finite() || t.abs() > 8.64e15 {
        CANONICAL_NAN
    } else {
        (t.trunc() + 0.0).to_bits()
    }
}

fn negative_zero_to_zero(value: Value) -> Value {
    match value {
        Value::Number(0x8000_0000_0000_0000) => Value::Number(0),
        other => other,
    }
}

#[derive(Hash, PartialEq, Eq)]
enum PrimitiveKey {
    Undefined,
    Null,
    Bool(bool),
    Number(u64),
    String(Utf16String),
    BigInt(String),
}

impl PrimitiveKey {
    /// The SameValueZero identity of a primitive; `None` for values that are never equal to
    /// another decoded value (objects and symbols).
    fn of(value: &Value) -> Option<Self> {
        Some(match value {
            Value::Undefined => PrimitiveKey::Undefined,
            Value::Null => PrimitiveKey::Null,
            Value::Bool(b) => PrimitiveKey::Bool(*b),
            Value::Number(bits) => {
                let x = f64::from_bits(*bits);
                PrimitiveKey::Number(if x.is_nan() {
                    CANONICAL_NAN
                } else {
                    (x + 0.0).to_bits()
                })
            }
            Value::String(s) => PrimitiveKey::String(s.clone()),
            Value::BigInt(d) => PrimitiveKey::BigInt(d.clone()),
            _ => return None,
        })
    }
}

fn typed_array_element(name: &str, value: Value) -> Decoded {
    let big = name.starts_with("Big");
    match (value, big) {
        (Value::BigInt(digits), true) => {
            let wrapped = bigint_mod_2_64(&digits);
            Ok(Value::BigInt(if name == "BigInt64Array" {
                (wrapped as i64).to_string()
            } else {
                wrapped.to_string()
            }))
        }
        (Value::Number(bits), true) => fail(format!(
            "Cannot convert {} to a BigInt",
            Value::Number(bits)
        )),
        (Value::Number(bits), false) => Ok(Value::Number(store_number(name, f64::from_bits(bits)))),
        (Value::BigInt(_), false) => fail("Cannot convert a BigInt value to a number"),
        (other, _) => fail(format!(
            "typed array elements must be numbers or bigints, got {}",
            other.kind()
        )),
    }
}

fn bigint_mod_2_64(digits: &str) -> u64 {
    let (negative, digits) = match digits.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, digits),
    };
    let magnitude = digits.bytes().fold(0u64, |acc, d| {
        acc.wrapping_mul(10).wrapping_add(u64::from(d - b'0'))
    });
    if negative {
        magnitude.wrapping_neg()
    } else {
        magnitude
    }
}

fn store_number(name: &str, x: f64) -> u64 {
    let int = |bits: u32, signed: bool| -> u64 {
        if !x.is_finite() {
            return 0;
        }
        let modulus = 2f64.powi(bits as i32);
        let r = x.trunc() % modulus;
        let r = if r < 0.0 { r + modulus } else { r };
        let r = if signed && r >= modulus / 2.0 {
            r - modulus
        } else {
            r
        };
        (r + 0.0).to_bits()
    };
    match name {
        "Int8Array" => int(8, true),
        "Int16Array" => int(16, true),
        "Int32Array" => int(32, true),
        "Uint16Array" => int(16, false),
        "Uint32Array" => int(32, false),
        "Uint8ClampedArray" => {
            if x.is_nan() || x <= 0.0 {
                0
            } else if x >= 255.0 {
                255f64.to_bits()
            } else {
                x.round_ties_even().to_bits()
            }
        }
        "Float32Array" => f64::from(x as f32).to_bits(),
        _ => x.to_bits(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(text: &str) -> String {
        Value::decode(text).unwrap().encode()
    }

    #[test]
    fn encodes_like_the_ts_selftest() {
        let cases: [(Value, &str); 12] = [
            (Value::Undefined, r#"{"u":null}"#),
            (Value::number(-0.0), r#"{"n":"8000000000000000"}"#),
            (Value::number(0.1), r#"{"n":"3fb999999999999a"}"#),
            (Value::number(f64::NAN), r#"{"n":"7ff8000000000000"}"#),
            (
                Value::String(Utf16String::from_units(vec![0xd800, 0x78, 0xdc00])),
                r#""\ud800x\udc00""#,
            ),
            (
                Value::string("a\"\\\n\u{1}\u{7f} "),
                "\"a\\\"\\\\\\n\\u0001\u{7f} \"",
            ),
            (
                Value::Array(vec![Value::number(1.0), Value::Hole]),
                r#"[{"n":"3ff0000000000000"},{"hole":null}]"#,
            ),
            (Value::Bytes(vec![0, 15, 255]), r#"{"b":"000fff"}"#),
            (Value::Symbol(None), r#"{"sym":null}"#),
            (
                Value::Error("RangeError".into(), "bad".into()),
                r#"{"err":["RangeError","bad"]}"#,
            ),
            (
                Value::Instance("Point".into(), vec![("x".into(), Value::Null)]),
                r#"{"c":["Point",[["x",null]]]}"#,
            ),
            (Value::BigInt("-12".into()), r#"{"bi":"-12"}"#),
        ];
        for (value, expected) in cases {
            assert_eq!(value.encode(), expected);
            assert_eq!(roundtrip(expected), expected);
        }
    }

    #[test]
    fn normalizes_like_js() {
        let cases = [
            (
                r#"{"o":[["b",1],["1",2],["b",3],["4294967295",4],["4294967294",5],["01",6]]}"#,
                r#"{"o":[["1",{"n":"4000000000000000"}],["4294967294",{"n":"4014000000000000"}],["b",{"n":"4008000000000000"}],["4294967295",{"n":"4010000000000000"}],["01",{"n":"4018000000000000"}]]}"#,
            ),
            (
                r#"{"map":[[{"n":"8000000000000000"},"a"],[{"n":"0000000000000000"},"b"]]}"#,
                r#"{"map":[[{"n":"0000000000000000"},"b"]]}"#,
            ),
            (
                r#"{"set":[{"n":"8000000000000000"},{"n":"7ff8000000000000"},{"n":"7ff8000000000001"}]}"#,
                r#"{"set":[{"n":"0000000000000000"},{"n":"7ff8000000000000"}]}"#,
            ),
            (
                r#"{"ta":["Float32Array",[{"n":"7ff8000000000001"},{"n":"3fb999999999999a"}]]}"#,
                r#"{"ta":["Float32Array",[{"n":"7ff8000000000000"},{"n":"3fb99999a0000000"}]]}"#,
            ),
            (
                r#"{"ta":["Uint8ClampedArray",[2.5,3.5,-2]]}"#,
                r#"{"ta":["Uint8ClampedArray",[{"n":"4000000000000000"},{"n":"4010000000000000"},{"n":"0000000000000000"}]]}"#,
            ),
            (
                r#"{"ta":["Int8Array",[128,-0]]}"#,
                r#"{"ta":["Int8Array",[{"n":"c060000000000000"},{"n":"0000000000000000"}]]}"#,
            ),
            (
                r#"{"ta":["BigInt64Array",[{"bi":"18446744073709551615"}]]}"#,
                r#"{"ta":["BigInt64Array",[{"bi":"-1"}]]}"#,
            ),
            (r#"{"d":"bff8000000000000"}"#, r#"{"d":"bff0000000000000"}"#),
            (r#"{"d":"8000000000000000"}"#, r#"{"d":"0000000000000000"}"#),
            (r#"{"bi":"-000"}"#, r#"{"bi":"0"}"#),
            (r#"{"box":{"bi":"1"}}"#, r#"{"c":["BigInt",[]]}"#),
            (r#"{"box":null}"#, r#"{"o":[]}"#),
            (r#"{"box":[1]}"#, r#"[{"n":"3ff0000000000000"}]"#),
            (
                r#"{"err":["AggregateError","x"]}"#,
                r#"{"err":["AggregateError",""]}"#,
            ),
            (
                r#"{"n":"3ff0000000000000","n":"4000000000000000"}"#,
                r#"{"n":"4000000000000000"}"#,
            ),
            (r#"{"u":5}"#, r#"{"u":null}"#),
            (r#"[{"hole":null,"x":1}]"#, r#"[{"hole":null}]"#),
        ];
        for (input, expected) in cases {
            assert_eq!(roundtrip(input), expected, "{input}");
        }
    }

    #[test]
    fn rejects_what_ts_rejects() {
        for bad in [
            r#"{"n":"3FF0000000000000"}"#,
            r#"{"a":1,"b":2}"#,
            r#"{"nope":1}"#,
            r#"{"hole":null}"#,
            r#"{"ta":["Float16Array",[]]}"#,
            r#"{"ta":["BigInt64Array",[1]]}"#,
            r#"{"err":["DOMException","x"]}"#,
            r#"{"bi":"1.5"}"#,
            r#"{"o":[[1,2]]}"#,
        ] {
            assert!(Value::decode(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn accepts_tags_only_rust_can_decode() {
        for text in [r#"{"c":["X",[]]}"#, r#"{"fn":"f"}"#] {
            assert_eq!(roundtrip(text), text);
        }
    }
}
