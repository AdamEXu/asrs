//! Replays `revlog/dbc` and `revlog/convert` (the real `@rev-robotics/revlog-converter` 2.0.0)
//! through `asrs_formats::revlog`: shipped DBC texts, `Dbc` dumps and `CanDecoder` results as
//! CVE-1, and the exact WPILOG bytes or rejection of every REVLOG input. The open-failure cases
//! of `revlog/convert` go through `asrs_services::historical_start` in that crate's tests.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use asrs_formats::revlog::dbc::{CanDecoder, DataType, Dbc, Message, Signal, SignalValue};
use asrs_formats::revlog::{self, A301_DBC, ENCODER_DBC, SERVO_HUB_DBC, SPARK_DBC};
use asrs_parity::cve1::Value;
use asrs_parity::json::Json;
use asrs_parity::utf16::Utf16String;
use asrs_parity::vectors::{self, Record};

const SHOWN: usize = 8;

fn key(s: &str) -> Utf16String {
    let mut out = Utf16String::new();
    out.push_str(s);
    out
}

fn object(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (key(k), v)).collect())
}

fn signal_tree(s: &Signal) -> Value {
    let data_type = match s.data_type {
        DataType::Int => "int",
        DataType::Float => "float",
        DataType::Double => "double",
    };
    let mut pairs = vec![
        ("name", Value::string(&s.name)),
        ("startBit", Value::number(s.start_bit)),
        ("length", Value::number(s.length)),
        ("isLittleEndian", Value::Bool(s.is_little_endian)),
        ("isSigned", Value::Bool(s.is_signed)),
        ("factor", Value::number(s.factor)),
        ("offset", Value::number(s.offset)),
        ("min", Value::number(s.min)),
        ("max", Value::number(s.max)),
        ("unit", Value::string(&s.unit)),
        ("dataType", Value::string(data_type)),
    ];
    if let Some(description) = &s.description {
        pairs.push(("description", Value::string(description)));
    }
    object(pairs)
}

fn message_tree(m: &Message) -> Value {
    object(vec![
        ("id", Value::number(m.id)),
        ("name", Value::string(&m.name)),
        ("dlc", Value::number(m.dlc)),
        ("sender", Value::string(&m.sender)),
        (
            "signals",
            Value::Map(
                m.signals
                    .iter()
                    .map(|s| (Value::string(&s.name), signal_tree(s)))
                    .collect(),
            ),
        ),
    ])
}

fn dbc_tree(dbc: &Dbc) -> Value {
    Value::Instance(
        key("Dbc"),
        vec![
            (
                key("messages"),
                Value::Map(
                    dbc.messages()
                        .into_iter()
                        .map(|(name, m)| (Value::string(name), message_tree(m)))
                        .collect(),
                ),
            ),
            (
                key("messagesById"),
                Value::Map(
                    dbc.messages_by_id()
                        .into_iter()
                        .map(|(id, m)| (Value::number(id), message_tree(m)))
                        .collect(),
                ),
            ),
        ],
    )
}

fn decoded_tree(decoded: Option<Vec<(String, SignalValue)>>) -> Value {
    match decoded {
        None => Value::Null,
        Some(signals) => Value::Array(
            signals
                .into_iter()
                .map(|(name, value)| {
                    let value = match value {
                        SignalValue::Number(x) => Value::number(x),
                        SignalValue::BigInt(i) => Value::BigInt(format!("{i}")),
                    };
                    object(vec![("name", Value::string(&name)), ("value", value)])
                })
                .collect(),
        ),
    }
}

fn field<'a>(r: &'a Record, k: &str) -> Result<&'a Json, String> {
    r.field(k).ok_or_else(|| format!("no field {k:?}"))
}

fn number(r: &Record, k: &str) -> Result<f64, String> {
    field(r, k)?
        .as_f64()
        .ok_or_else(|| format!("{k:?} is not a number"))
}

fn string(r: &Record, k: &str) -> Result<String, String> {
    r.string(k).ok_or_else(|| format!("{k:?} is not a string"))
}

fn hex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
        return Err(format!("odd hex length {}", text.len()));
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|e| format!("{e}")))
        .collect()
}

fn compare_values(what: &str, expected: &Value, actual: &Value) -> Result<(), String> {
    if expected.encode() == actual.encode() {
        return Ok(());
    }
    let (e, a) = (expected.encode(), actual.encode());
    let at = e
        .bytes()
        .zip(a.bytes())
        .position(|(x, y)| x != y)
        .unwrap_or(e.len().min(a.len()));
    let start = at.saturating_sub(120);
    let window = |s: &str| {
        let mut lo = start.min(s.len());
        while !s.is_char_boundary(lo) {
            lo -= 1;
        }
        let mut hi = (at + 200).min(s.len());
        while !s.is_char_boundary(hi) {
            hi += 1;
        }
        s[lo..hi].to_owned()
    };
    Err(format!(
        "{what} differs at CVE-1 byte {at}\n  expected ...{}...\n  actual   ...{}...",
        window(&e),
        window(&a)
    ))
}

struct Tally {
    recorded: BTreeMap<&'static str, usize>,
    compared: BTreeMap<&'static str, usize>,
    failures: Vec<String>,
}

impl Tally {
    fn new() -> Tally {
        Tally {
            recorded: BTreeMap::new(),
            compared: BTreeMap::new(),
            failures: Vec::new(),
        }
    }

    fn record(&mut self, kind: &'static str, line: usize, result: Result<(), String>) {
        *self.recorded.entry(kind).or_default() += 1;
        match result {
            Ok(()) => *self.compared.entry(kind).or_default() += 1,
            Err(e) => self
                .failures
                .push(format!("line {line} ({kind}): {}", e.replace('\n', "\n  "))),
        }
    }

    fn finish(self, id: &str, expect_kinds: &[&str]) {
        if !self.failures.is_empty() {
            let mut message = format!("vectors {id}: {} records differ", self.failures.len());
            for f in self.failures.iter().take(SHOWN) {
                message.push_str("\n\n");
                message.push_str(f);
            }
            panic!("{message}");
        }
        for kind in expect_kinds {
            let recorded = self.recorded.get(kind).copied().unwrap_or(0);
            assert!(recorded > 0, "vectors {id}: no {kind} records");
            assert_eq!(
                self.compared.get(kind).copied().unwrap_or(0),
                recorded,
                "vectors {id}: compared {kind} records != recorded"
            );
        }
        assert_eq!(
            self.recorded.keys().copied().collect::<BTreeSet<_>>(),
            expect_kinds.iter().copied().collect::<BTreeSet<_>>(),
            "vectors {id}: record kinds"
        );
    }
}

fn resource(name: &str) -> Option<&'static str> {
    match name {
        "spark.public.dbc" => Some(SPARK_DBC),
        "servo_hub.public.dbc" => Some(SERVO_HUB_DBC),
        "encoder.public.dbc" => Some(ENCODER_DBC),
        "a301.public.dbc" => Some(A301_DBC),
        _ => None,
    }
}

#[test]
fn revlog_dbc() {
    let id = "revlog/dbc";
    let records = vectors::load(id).unwrap_or_else(|e| panic!("{e}"));
    let mut loads: HashMap<u64, Dbc> = HashMap::new();
    let mut tally = Tally::new();
    for r in &records {
        match string(r, "kind").as_deref() {
            Ok("resource") => tally.record(
                "resource",
                r.line,
                (|| {
                    let name = string(r, "name")?;
                    let text = string(r, "text")?;
                    let shipped =
                        resource(&name).ok_or_else(|| format!("no constant for {name}"))?;
                    if shipped != text {
                        return Err(format!("{name} differs from the package's file"));
                    }
                    Ok(())
                })(),
            ),
            Ok("load") => tally.record(
                "load",
                r.line,
                (|| {
                    let index = number(r, "load")? as u64;
                    let text = match r.field("source") {
                        Some(_) => {
                            let name = string(r, "source")?;
                            resource(&name)
                                .ok_or_else(|| format!("no constant for {name}"))?
                                .to_owned()
                        }
                        None => string(r, "text")?,
                    };
                    let dbc = Dbc::load(&text);
                    let expected = r.value("dump").map_err(|e| format!("{e}"))?;
                    let result = compare_values("Dbc", &expected, &dbc_tree(&dbc));
                    loads.insert(index, dbc);
                    result
                })(),
            ),
            Ok("decode") => tally.record(
                "decode",
                r.line,
                (|| {
                    let index = number(r, "load")? as u64;
                    let dbc = loads
                        .get(&index)
                        .ok_or_else(|| format!("no Dbc for load {index}"))?;
                    let frame_id = match r.value("id").map_err(|e| format!("{e}"))? {
                        Value::Number(bits) => f64::from_bits(bits),
                        other => return Err(format!("id is {}", other.kind())),
                    };
                    let data = hex(&string(r, "data")?)?;
                    let expected = r.value("result").map_err(|e| format!("{e}"))?;
                    let actual = decoded_tree(CanDecoder::new(dbc).decode(frame_id, &data));
                    compare_values(
                        &format!("decode(0x{:x}, {})", frame_id as u64, string(r, "data")?),
                        &expected,
                        &actual,
                    )
                })(),
            ),
            other => panic!("line {}: unknown record kind {other:?}", r.line),
        }
    }
    tally.finish(id, &["decode", "load", "resource"]);
}

struct WpilogRecord {
    offset: usize,
    entry: u64,
    timestamp: u64,
    payload: Vec<u8>,
}

fn wpilog_records(bytes: &[u8]) -> Vec<WpilogRecord> {
    let mut out = Vec::new();
    if bytes.len() < 12 {
        return out;
    }
    let extra = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    let mut pos = 12 + extra;
    let read = |at: usize, n: usize| -> Option<u64> {
        let slice = bytes.get(at..at + n)?;
        Some(
            slice
                .iter()
                .rev()
                .fold(0u64, |acc, &b| (acc << 8) | b as u64),
        )
    };
    while pos < bytes.len() {
        let bitfield = bytes[pos];
        let id_len = (bitfield & 3) as usize + 1;
        let size_len = ((bitfield >> 2) & 3) as usize + 1;
        let ts_len = ((bitfield >> 4) & 7) as usize + 1;
        let (Some(entry), Some(size), Some(timestamp)) = (
            read(pos + 1, id_len),
            read(pos + 1 + id_len, size_len),
            read(pos + 1 + id_len + size_len, ts_len),
        ) else {
            break;
        };
        let start = pos + 1 + id_len + size_len + ts_len;
        let end = (start + size as usize).min(bytes.len());
        out.push(WpilogRecord {
            offset: pos,
            entry,
            timestamp,
            payload: bytes[start.min(end)..end].to_vec(),
        });
        pos = end;
    }
    out
}

fn describe_record(r: &WpilogRecord) -> String {
    let payload = if r.entry == 0 && r.payload.first() == Some(&0) && r.payload.len() >= 9 {
        let name_len =
            u32::from_le_bytes([r.payload[5], r.payload[6], r.payload[7], r.payload[8]]) as usize;
        let name = r.payload.get(9..9 + name_len).unwrap_or(&[]);
        format!(
            "start entry {} {:?}",
            u32::from_le_bytes([r.payload[1], r.payload[2], r.payload[3], r.payload[4]]),
            String::from_utf8_lossy(name)
        )
    } else {
        r.payload.iter().map(|b| format!("{b:02x}")).collect()
    };
    format!(
        "@{} entry {} t={} [{}]",
        r.offset, r.entry, r.timestamp, payload
    )
}

fn compare_wpilog(expected: &[u8], actual: &[u8]) -> Result<(), String> {
    if expected == actual {
        return Ok(());
    }
    let at = expected
        .iter()
        .zip(actual)
        .position(|(a, b)| a != b)
        .unwrap_or(expected.len().min(actual.len()));
    let e = wpilog_records(expected);
    let a = wpilog_records(actual);
    let index = e
        .iter()
        .zip(&a)
        .position(|(x, y)| {
            x.offset != y.offset
                || x.entry != y.entry
                || x.timestamp != y.timestamp
                || x.payload != y.payload
        })
        .unwrap_or(e.len().min(a.len()));
    let show = |records: &[WpilogRecord]| {
        records
            .get(index)
            .map(describe_record)
            .unwrap_or_else(|| "(none)".to_owned())
    };
    Err(format!(
        "WPILOG output differs at byte {at} (expected {} bytes, got {}); record {index}:\n  expected {}\n  actual   {}",
        expected.len(),
        actual.len(),
        show(&e),
        show(&a)
    ))
}

fn input(r: &Record, blobs: &HashMap<u64, Vec<u8>>) -> Result<Vec<u8>, String> {
    let json = field(r, "input")?;
    if let Some(b) = json.get("bytes") {
        return hex(&b.as_str().ok_or("bytes")?.to_string_lossy());
    }
    let id = json.get("blob").and_then(Json::as_f64).ok_or("blob id")? as u64;
    let blob = blobs.get(&id).ok_or_else(|| format!("no blob {id}"))?;
    let len = match json.get("len") {
        Some(len) => len.as_f64().ok_or("len")? as usize,
        None => blob.len(),
    };
    let mut out = blob.get(..len).ok_or("len past blob")?.to_vec();
    if let Some(flips) = json.get("flips") {
        for flip in flips.as_array().ok_or("flips")? {
            let pair = flip.as_array().ok_or("flip")?;
            let offset = pair[0].as_f64().ok_or("flip offset")? as usize;
            let xor = pair[1].as_f64().ok_or("flip xor")? as u8;
            out[offset] ^= xor;
        }
    }
    Ok(out)
}

fn expected_error(r: &Record) -> Result<Option<(String, String)>, String> {
    match field(r, "error")? {
        Json::Null => Ok(None),
        json => {
            let pair = json.as_array().ok_or("error is not an array")?;
            let text = |i: usize| {
                pair.get(i)
                    .and_then(Json::as_str)
                    .map(|s| s.to_string_lossy())
                    .ok_or("error entry")
            };
            Ok(Some((text(0)?, text(1)?)))
        }
    }
}

#[test]
fn revlog_convert() {
    let id = "revlog/convert";
    let records = vectors::load(id).unwrap_or_else(|e| panic!("{e}"));
    let mut blobs: HashMap<u64, Vec<u8>> = HashMap::new();
    let mut tally = Tally::new();
    for r in &records {
        match string(r, "kind").as_deref() {
            Ok("blob") => tally.record(
                "blob",
                r.line,
                (|| {
                    blobs.insert(number(r, "blob")? as u64, hex(&string(r, "bytes")?)?);
                    Ok(())
                })(),
            ),
            Ok("case") => {
                let setup = string(r, "setup").unwrap_or_default();
                let kind = match setup.as_str() {
                    "file" => "file case",
                    "missing" | "directory" | "unreadable" | "none" => {
                        tally.record("open case (historical_start tests)", r.line, Ok(()));
                        continue;
                    }
                    other => panic!("line {}: unknown setup {other:?}", r.line),
                };
                tally.record(
                    kind,
                    r.line,
                    (|| {
                        let name = string(r, "name")?;
                        if number(r, "uncaught")? != 0.0 {
                            return Err(format!("{name}: uncaught exceptions recorded"));
                        }
                        let bytes = input(r, &blobs)?;
                        let expected_output = match field(r, "output")? {
                            Json::Null => None,
                            json => Some(hex(&json.as_str().ok_or("output")?.to_string_lossy())?),
                        };
                        let expected_error = expected_error(r)?;
                        let context = |e: String| format!("{name}: {e}");
                        match (revlog::convert(&bytes), expected_output, expected_error) {
                            (Ok(actual), Some(expected), None) => {
                                compare_wpilog(&expected, &actual).map_err(context)
                            }
                            (Err(actual), None, Some((name, message))) => {
                                if actual.name == name && actual.message == message {
                                    Ok(())
                                } else {
                                    Err(context(format!(
                                        "rejection {}: {:?}, expected {name}: {message:?}",
                                        actual.name, actual.message
                                    )))
                                }
                            }
                            (Ok(actual), None, Some((name, message))) => Err(context(format!(
                                "resolved with {} bytes, expected rejection {name}: {message:?}",
                                actual.len()
                            ))),
                            (Err(actual), Some(expected), None) => Err(context(format!(
                                "rejected with {}: {:?}, expected {} bytes",
                                actual.name,
                                actual.message,
                                expected.len()
                            ))),
                            _ => Err(context(
                                "record has both or neither output and error".into(),
                            )),
                        }
                    })(),
                );
            }
            other => panic!("line {}: unknown record kind {other:?}", r.line),
        }
    }
    tally.finish(
        id,
        &["blob", "file case", "open case (historical_start tests)"],
    );
}
