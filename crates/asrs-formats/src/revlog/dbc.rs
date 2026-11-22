//! `@rev-robotics/revlog-converter` 2.0.0 `dist/lib/revDBC.js`: the `Dbc` line parser and the
//! `CanDecoder` that turns a CAN frame into signal values. Numbers stay `f64` where upstream
//! keeps a JS number (ids, bit positions and lengths come from `parseInt`).

use std::sync::OnceLock;

use asrs_js::math::BigInt;
use asrs_js::number::{parse_float, parse_int};
use asrs_js::regex::{Match, RegExp};
use asrs_js::string::JsString;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataType {
    Int,
    Float,
    Double,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Signal {
    pub name: String,
    pub start_bit: f64,
    pub length: f64,
    pub is_little_endian: bool,
    pub is_signed: bool,
    pub factor: f64,
    pub offset: f64,
    pub min: f64,
    pub max: f64,
    pub unit: String,
    pub data_type: DataType,
    /// Set by a `CM_ SG_` line; upstream adds the `description` key only then.
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub id: f64,
    pub name: String,
    pub dlc: f64,
    pub sender: String,
    /// `signals`, a `Map` keyed by name, in its iteration order.
    pub signals: Vec<Signal>,
}

impl Message {
    pub fn signal(&self, name: &str) -> Option<&Signal> {
        self.signals.iter().find(|s| s.name == name)
    }
}

/// `new Dbc()`: `messages` and `messagesById` share message objects, so lookups by either key
/// see the same mutations. Both maps keep first-insertion order and replace values in place;
/// the arena holds every object ever created.
#[derive(Clone, Debug, Default)]
pub struct Dbc {
    messages: Vec<(String, usize)>,
    messages_by_id: Vec<(f64, usize)>,
    arena: Vec<Message>,
}

fn pattern(name: &str) -> &'static RegExp {
    static RE_BO: OnceLock<RegExp> = OnceLock::new();
    static RE_SG: OnceLock<RegExp> = OnceLock::new();
    static RE_VAL_TYPE: OnceLock<RegExp> = OnceLock::new();
    static RE_COMMENT: OnceLock<RegExp> = OnceLock::new();
    static RE_SPLIT: OnceLock<RegExp> = OnceLock::new();
    let (cell, source) = match name {
        "bo" => (&RE_BO, r"^BO_\s+(\d+)\s+(\w+):\s*(\d+)\s+(\w+)"),
        "sg" => (
            &RE_SG,
            r#"^\s*SG_\s+(\w+)\s*:\s*(\d+)\|(\d+)@([01])([+-])\s*\(\s*([\d.-]+)\s*,\s*([\d.-]+)\s*\)\s*\[\s*([\d.-]+)\s*\|\s*([\d.-]+)\s*\]\s*"(.*?)""#,
        ),
        "val_type" => (
            &RE_VAL_TYPE,
            r"^\s*SIG_VALTYPE_\s+(\d+)\s+(\w+)\s*:\s*(\d+)\s*;",
        ),
        "comment" => (&RE_COMMENT, r#"^\s*CM_\s+SG_\s+(\d+)\s+(\w+)\s*"(.*)";"#),
        "split" => (&RE_SPLIT, r"\r?\n"),
        _ => unreachable!(),
    };
    cell.get_or_init(|| RegExp::new(source, "").expect("upstream pattern"))
}

fn group(line: &[u16], m: &Match, i: usize) -> Vec<u16> {
    m.group(line, i).map_or_else(Vec::new, |g| g.into_units())
}

fn text(line: &[u16], m: &Match, i: usize) -> String {
    m.group(line, i)
        .map(|g| g.to_string_lossy())
        .unwrap_or_default()
}

/// `Map.set` on an insertion-ordered map: replace in place or append.
fn map_set<K: PartialEq, V>(map: &mut Vec<(K, V)>, key: K, value: V) {
    match map.iter_mut().find(|(k, _)| *k == key) {
        Some((_, slot)) => *slot = value,
        None => map.push((key, value)),
    }
}

fn map_get<'a, K: PartialEq, V>(map: &'a [(K, V)], key: &K) -> Option<&'a V> {
    map.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

impl Dbc {
    /// `new Dbc().load(content)`.
    pub fn load(content: &str) -> Dbc {
        let content = JsString::from(content);
        let mut dbc = Dbc::default();
        let mut current: Option<usize> = None;
        for line in pattern("split").split(content.units(), None) {
            let Some(line) = line else { continue };
            let line = line.units();
            if let Some(m) = pattern("bo").exec(line, &mut 0) {
                let id = parse_int(&group(line, &m, 1), 10.0);
                let index = dbc.arena.len();
                let name = text(line, &m, 2);
                dbc.arena.push(Message {
                    id,
                    name: name.clone(),
                    dlc: parse_int(&group(line, &m, 3), 10.0),
                    sender: text(line, &m, 4),
                    signals: Vec::new(),
                });
                map_set(&mut dbc.messages, name, index);
                map_set(&mut dbc.messages_by_id, id, index);
                current = Some(index);
                continue;
            }
            if let Some(m) = pattern("sg").exec(line, &mut 0) {
                if let Some(index) = current {
                    let signal = Signal {
                        name: text(line, &m, 1),
                        start_bit: parse_int(&group(line, &m, 2), 10.0),
                        length: parse_int(&group(line, &m, 3), 10.0),
                        is_little_endian: text(line, &m, 4) == "1",
                        is_signed: text(line, &m, 5) == "-",
                        factor: parse_float(&group(line, &m, 6)),
                        offset: parse_float(&group(line, &m, 7)),
                        min: parse_float(&group(line, &m, 8)),
                        max: parse_float(&group(line, &m, 9)),
                        unit: text(line, &m, 10),
                        data_type: DataType::Int,
                        description: None,
                    };
                    let signals = &mut dbc.arena[index].signals;
                    match signals.iter_mut().find(|s| s.name == signal.name) {
                        Some(slot) => *slot = signal,
                        None => signals.push(signal),
                    }
                }
                continue;
            }
            if let Some(m) = pattern("val_type").exec(line, &mut 0) {
                let id = parse_int(&group(line, &m, 1), 10.0);
                if let Some(index) = map_get(&dbc.messages_by_id, &id) {
                    let name = text(line, &m, 2);
                    let kind = text(line, &m, 3);
                    if let Some(signal) = dbc.arena[*index]
                        .signals
                        .iter_mut()
                        .find(|s| s.name == name)
                    {
                        match kind.as_str() {
                            "1" => signal.data_type = DataType::Float,
                            "2" => signal.data_type = DataType::Double,
                            _ => {}
                        }
                    }
                }
                continue;
            }
            if let Some(m) = pattern("comment").exec(line, &mut 0) {
                let id = parse_int(&group(line, &m, 1), 10.0);
                if let Some(index) = map_get(&dbc.messages_by_id, &id) {
                    let name = text(line, &m, 2);
                    if let Some(signal) = dbc.arena[*index]
                        .signals
                        .iter_mut()
                        .find(|s| s.name == name)
                    {
                        signal.description = Some(text(line, &m, 3));
                    }
                }
            }
        }
        dbc
    }

    /// `dbc.messages`: name to message, in `Map` iteration order.
    pub fn messages(&self) -> Vec<(&str, &Message)> {
        self.messages
            .iter()
            .map(|(name, index)| (name.as_str(), &self.arena[*index]))
            .collect()
    }

    /// `dbc.messagesById`: id to message, in `Map` iteration order.
    pub fn messages_by_id(&self) -> Vec<(f64, &Message)> {
        self.messages_by_id
            .iter()
            .map(|(id, index)| (*id, &self.arena[*index]))
            .collect()
    }

    /// `messagesById.get(id)`; NaN and unknown ids miss.
    pub fn message_by_id(&self, id: f64) -> Option<&Message> {
        map_get(&self.messages_by_id, &id).map(|index| &self.arena[*index])
    }
}

/// A decoded signal value: a JS number, or a BigInt where upstream keeps one (an unscaled
/// integer outside the safe-integer range).
#[derive(Clone, Debug, PartialEq)]
pub enum SignalValue {
    Number(f64),
    BigInt(BigInt),
}

const MAX_SAFE_INTEGER: i128 = 9007199254740991;

/// A raw integer signal: `i128` holds every little-endian value and every big-endian one under
/// 128 bits; longer big-endian signals grow without bound like upstream's BigInt.
enum Raw {
    Small(i128),
    Big(BigInt),
}

impl Raw {
    fn big(&self) -> BigInt {
        match self {
            Raw::Big(b) => b.clone(),
            Raw::Small(i) => {
                let magnitude = BigInt::from_u64((i.unsigned_abs() >> 64) as u64)
                    .shl(64)
                    .bitor(&BigInt::from_u64(i.unsigned_abs() as u64));
                if *i < 0 { magnitude.neg() } else { magnitude }
            }
        }
    }

    fn number(&self) -> f64 {
        match self {
            Raw::Small(i) => *i as f64,
            Raw::Big(b) => b.to_f64(),
        }
    }

    fn is_safe(&self) -> bool {
        match self {
            Raw::Small(i) => (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(i),
            Raw::Big(b) => {
                let max = BigInt::from_i64(MAX_SAFE_INTEGER as i64);
                *b <= max && *b >= max.neg()
            }
        }
    }
}

/// `CanDecoder` with `database` set.
pub struct CanDecoder<'a> {
    database: &'a Dbc,
}

impl<'a> CanDecoder<'a> {
    pub fn new(database: &'a Dbc) -> CanDecoder<'a> {
        CanDecoder { database }
    }

    /// `decode(createFrame(id, data))`: `None` for an id the database does not have, else
    /// each signal's `{ name, value }` in the message's signal order.
    pub fn decode(&self, id: f64, data: &[u8]) -> Option<Vec<(String, SignalValue)>> {
        let message = self.database.message_by_id(id)?;
        // Upstream zero-pads data to `max(dlc, 8)` when shorter; a longer frame is kept whole,
        // so the visible end is max(dlc, 8, len) and past-end reads are zero.
        let needed = if message.dlc > 8.0 { message.dlc } else { 8.0 };
        // Buffer.alloc for the padding fails on an absurd dlc upstream; cap the virtual end
        // where real frames never reach.
        let len = data.len().max(if needed > 4_294_967_296.0 {
            4_294_967_296
        } else {
            needed as usize
        });
        let byte = |i: usize| data.get(i).copied().unwrap_or(0);
        let mut out = Vec::with_capacity(message.signals.len());
        for signal in &message.signals {
            let mut value = SignalValue::Number(0.0);
            if matches!(signal.data_type, DataType::Float | DataType::Double) {
                let byte_offset = (signal.start_bit / 8.0).floor();
                let is_double =
                    matches!(signal.data_type, DataType::Double) || signal.length == 64.0;
                let width = if is_double { 8 } else { 4 };
                if byte_offset >= 0.0 && byte_offset + (width as f64) <= len as f64 {
                    let start = byte_offset as usize;
                    let mut bytes = [0u8; 8];
                    for (i, slot) in bytes.iter_mut().enumerate().take(width) {
                        *slot = byte(start + i);
                    }
                    value = SignalValue::Number(if is_double {
                        f64::from_le_bytes(bytes)
                    } else {
                        f64::from(f32::from_le_bytes(bytes[..4].try_into().unwrap()))
                    });
                }
            } else {
                let raw = if signal.is_little_endian {
                    let byte_start = (signal.start_bit / 8.0).floor();
                    let bit_start = (signal.start_bit % 8.0) as u32;
                    let mut raw = 0i128;
                    if byte_start >= 0.0 {
                        for i in 0..8usize {
                            if byte_start + (i as f64) >= len as f64 {
                                break;
                            }
                            raw |= (byte(byte_start as usize + i) as i128) << (i * 8);
                        }
                    }
                    let mut raw = if signal.length <= 0.0 {
                        0
                    } else if signal.length >= 128.0 {
                        raw >> bit_start
                    } else {
                        (raw >> bit_start) & ((1i128 << (signal.length as u32)) - 1)
                    };
                    if signal.is_signed
                        && signal.length >= 1.0
                        && signal.length <= 127.0
                        && (raw & (1i128 << (signal.length as u32 - 1))) != 0
                    {
                        raw -= 1i128 << (signal.length as u32);
                    }
                    Raw::Small(raw)
                } else {
                    let mut raw = 0i128;
                    let mut wide: Option<BigInt> = None;
                    let mut bit = signal.start_bit;
                    let bits = if signal.length > 0.0 {
                        signal.length as u64
                    } else {
                        0
                    };
                    for _ in 0..bits {
                        let byte_index = (bit / 8.0).floor();
                        let b = bit % 8.0;
                        if byte_index >= 0.0 && byte_index < len as f64 {
                            let bit_val = ((byte(byte_index as usize) as i128) >> (b as u32)) & 1;
                            if let Some(w) = &mut wide {
                                *w = w.shl(1).bitor(&BigInt::from_u64(bit_val as u64));
                            } else if raw >> 126 != 0 {
                                let w = Raw::Small(raw).big();
                                wide = Some(w.shl(1).bitor(&BigInt::from_u64(bit_val as u64)));
                            } else {
                                raw = (raw << 1) | bit_val;
                            }
                        }
                        bit += if b == 0.0 { 15.0 } else { -1.0 };
                    }
                    wide.map_or(Raw::Small(raw), Raw::Big)
                };
                value = if signal.factor == 1.0 && signal.offset == 0.0 {
                    if raw.is_safe() {
                        SignalValue::Number(raw.number())
                    } else {
                        SignalValue::BigInt(raw.big())
                    }
                } else {
                    SignalValue::Number(raw.number() * signal.factor + signal.offset)
                };
            }
            if let SignalValue::Number(physical) = &mut value
                && (signal.min != 0.0 || signal.max != 0.0)
            {
                if *physical < signal.min {
                    *physical = signal.min;
                }
                if *physical > signal.max {
                    *physical = signal.max;
                }
            }
            out.push((signal.name.clone(), value));
        }
        Some(out)
    }
}
