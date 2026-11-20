//! Replays the `genSpark` oracle vectors: explicit payload records compare the whole
//! CVE-1 result object, and the seeded sweep compares an fnv1a-64 digest of it. The
//! sweep inputs come from a port of oracle/src/corpus/rng.ts, so both sides see the
//! same frames and payloads.

use asrs_formats::spark::{
    Frame, ParsedCanFrame, SignalValue, SparkError, parse_can_frame, tables,
};
use asrs_parity::cve1::Value;
use asrs_parity::json::Json;
use asrs_parity::vectors::{self, Record};

const STATUS_KEYS: [&str; 8] = [
    "STATUS_0", "STATUS_1", "STATUS_2", "STATUS_3", "STATUS_4", "STATUS_5", "STATUS_6", "STATUS_7",
];

fn fnv1a32(text: &str) -> u32 {
    let mut hash = 0x811c9dc5u32;
    for unit in text.encode_utf16() {
        hash ^= unit as u32;
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

/// Port of oracle `corpus/rng.ts` Rng (mulberry32).
struct Rng {
    state: u32,
}

impl Rng {
    fn new(seed: u32, stream: &str) -> Rng {
        Rng {
            state: seed ^ fnv1a32(stream),
        }
    }

    fn u32(&mut self) -> u32 {
        self.state = self.state.wrapping_add(0x6d2b79f5);
        let mut t = self.state;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        t ^ (t >> 14)
    }

    fn below(&mut self, n: u32) -> u32 {
        self.u32() % n
    }

    fn bool(&mut self) -> bool {
        self.u32() & 1 == 1
    }

    fn bytes(&mut self, out: &mut [u8]) {
        for b in out {
            *b = (self.u32() & 0xff) as u8;
        }
    }
}

/// The year's frame list: `STATUS_0..7` from the generated tables, plus
/// `GET_FIRMWARE_VERSION` for 2026.
fn frames_for(year: u64) -> Vec<(&'static str, &'static Frame)> {
    let status: &'static [&'static Frame] = match year {
        2025 => tables::STATUS_2025,
        2026 => tables::STATUS_2026,
        _ => panic!("unknown spec year {year}"),
    };
    let mut frames: Vec<(&'static str, &'static Frame)> = status
        .iter()
        .enumerate()
        .map(|(i, frame)| (STATUS_KEYS[i], *frame))
        .collect();
    if year == 2026 {
        frames.push(("GET_FIRMWARE_VERSION", &tables::GET_FIRMWARE_VERSION_2026));
    }
    frames
}

/// The `ok` record shape genSpark emits: signal key -> number-or-bool in declaration
/// order.
fn result_value(result: &ParsedCanFrame) -> Value {
    Value::Object(
        result
            .entries
            .iter()
            .map(|(key, value)| {
                let v = match value {
                    SignalValue::Bool(b) => Value::Bool(*b),
                    SignalValue::Num(v) => Value::Number(v.to_number().to_bits()),
                };
                ((*key).into(), v)
            })
            .collect(),
    )
}

fn feed(h: &mut u64, byte: u8) {
    *h = (*h ^ byte as u64).wrapping_mul(0x100000001b3);
}

/// The sweep digest: fnv1a-64 over each result entry (or `0x03` + the error message).
fn result_digest(result: &Result<ParsedCanFrame, SparkError>) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    match result {
        Ok(parsed) => {
            for (key, value) in &parsed.entries {
                for b in key.bytes() {
                    feed(&mut h, b);
                }
                feed(&mut h, 0);
                match value {
                    SignalValue::Bool(v) => {
                        feed(&mut h, 1);
                        feed(&mut h, *v as u8);
                    }
                    SignalValue::Num(v) => {
                        feed(&mut h, 2);
                        for b in v.to_number().to_bits().to_le_bytes() {
                            feed(&mut h, b);
                        }
                    }
                }
            }
        }
        Err(e) => {
            feed(&mut h, 3);
            for b in e.0.bytes() {
                feed(&mut h, b);
            }
        }
    }
    h
}

/// The seeded frame pick/mode/payload draw, mirroring the genSpark sweep loop.
fn next_sweep(rng: &mut Rng, frame_count: u32) -> (usize, [u8; 8]) {
    let index = rng.below(frame_count) as usize;
    let mode = rng.below(10);
    let mut data = [0u8; 8];
    match mode {
        6 => {}
        7 => data.fill(0xff),
        8 => {
            for b in &mut data {
                *b = if rng.bool() { 0xff } else { 0 };
            }
        }
        9 => {
            for b in &mut data {
                *b = [0x00, 0x7f, 0x80, 0xff][rng.below(4) as usize];
            }
        }
        _ => rng.bytes(&mut data),
    }
    (index, data)
}

fn hex_to_bytes(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).expect("hex byte"))
        .collect()
}

fn json_u64(record: &Record, key: &str) -> u64 {
    match record.field(key) {
        Some(Json::Number(n)) => *n as u64,
        _ => panic!("record missing {key}"),
    }
}

fn check_vector(id: &str) {
    let records = vectors::load(id).expect("load vector");
    let mut frames: Vec<(&'static str, &'static Frame)> = Vec::new();
    let mut frame_keys: Vec<String> = Vec::new();
    let mut rng: Option<Rng> = None;
    let mut expected_sweep = 0u64;
    let mut sweep = 0u64;
    let mut edge = 0u64;
    let mut mismatches = Vec::new();
    for record in &records {
        if let Some(Json::String(h)) = record.field("h") {
            let expected = u64::from_str_radix(&h.to_string_lossy(), 16).expect("h is hex");
            let rng = rng.as_mut().expect("meta record precedes sweep");
            let (index, data) = next_sweep(rng, frames.len() as u32);
            let got = result_digest(&parse_can_frame(frames[index].1, &data, None));
            if got != expected {
                let hex: String = data.iter().map(|b| format!("{b:02x}")).collect();
                mismatches.push(format!(
                    "line {}: sweep {} {}/{} digest {got:016x} != {expected:016x}",
                    record.line,
                    sweep + 1,
                    frame_keys[index],
                    hex
                ));
            }
            sweep += 1;
            continue;
        }
        if record.field("t").is_some() {
            let year = json_u64(record, "year");
            let seed = json_u64(record, "seed") as u32;
            expected_sweep = json_u64(record, "sweep");
            let table_frames = frames_for(year);
            frame_keys = match record.field("frames") {
                Some(Json::Array(items)) => items
                    .iter()
                    .map(|j| j.as_str().expect("frame key").to_string_lossy())
                    .collect(),
                _ => panic!("meta record missing frames"),
            };
            frames = frame_keys
                .iter()
                .map(|key| {
                    let (name, frame) = table_frames
                        .iter()
                        .find(|(k, _)| k == key)
                        .unwrap_or_else(|| panic!("no frame for key {key}"));
                    (*name, *frame)
                })
                .collect();
            rng = Some(Rng::new(seed, &format!("spark/{year}")));
            continue;
        }
        let key = record.string("f").expect("edge record has f");
        let data = hex_to_bytes(&record.string("d").expect("edge record has d"));
        let frame = frames
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, f)| *f)
            .unwrap_or_else(|| panic!("no frame for key {key}"));
        match parse_can_frame(frame, &data, None) {
            Ok(parsed) => match record.value("ok") {
                Ok(expected) => {
                    let got = result_value(&parsed);
                    if got != expected {
                        mismatches.push(format!(
                            "line {}: {key}/{}: {got:?} != {expected:?}",
                            record.line,
                            record.string("d").unwrap_or_default()
                        ));
                    }
                }
                _ => mismatches.push(format!(
                    "line {}: {key}/{}: oracle threw but the port parsed",
                    record.line,
                    record.string("d").unwrap_or_default()
                )),
            },
            Err(e) => match record.value("throw") {
                Ok(Value::Error(name, message)) => {
                    if name.to_string_lossy() != "Error" || message.to_string_lossy() != e.0 {
                        mismatches.push(format!(
                            "line {}: {key}/{}: port error {:?} != oracle error {:?}",
                            record.line,
                            record.string("d").unwrap_or_default(),
                            e.0,
                            message.to_string_lossy()
                        ));
                    }
                }
                _ => mismatches.push(format!(
                    "line {}: {key}/{}: port threw {:?} but the oracle parsed",
                    record.line,
                    record.string("d").unwrap_or_default(),
                    e.0
                )),
            },
        }
        edge += 1;
    }
    assert!(
        mismatches.is_empty(),
        "{id}: {} mismatches\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
    assert_eq!(sweep, expected_sweep, "{id}: sweep count");
    eprintln!("{id}: {edge} explicit records, {sweep} sweep frames, 0 mismatches");
}

#[test]
fn spark_2025() {
    check_vector("spark/2025");
}

#[test]
fn spark_2026() {
    check_vector("spark/2026");
}
