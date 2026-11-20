//! Port of `can-spec-util.ts`: `getCanSignalValue` and `parseCanFrame`. Bit positions
//! are LSB-first. `isBigEndian` is kept in the spec structs but ignored on decode,
//! matching upstream (it has a TODO and never applies it). `console.warn` calls on
//! clamped values are dropped; they do not change return values.
//!
//! `buildCanFramePayload` and the double-signal decode are not ported: both reach
//! `writeBigUInt64LE(Number(..))` in the `buffer` package, which always throws, upstream
//! never calls the former, and no frame URCL decodes has a double signal (the table
//! generator rejects one).

use super::BigNum;
use super::SparkError;
use super::bitstring::{BitString, bit_string_to_uint, buffer_to_bit_string, get_sub_bit_string};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalType {
    Boolean,
    Uint,
    Int,
    Float,
}

/// Decimal fields are stored as their `BigNumber("...")` literal and parsed by
/// the same `BigNumber(v)` path upstream uses.
#[derive(Clone, Copy, Debug)]
pub struct Signal {
    pub signal_type: SignalType,
    pub name: &'static str,
    pub bit_position: u32,
    pub length_bits: u32,
    pub is_big_endian: bool,
    pub description: Option<&'static str>,
    pub unit: Option<&'static str>,
    pub decode_scale_factor: &'static str,
    pub offset: &'static str,
    pub decoded_min: Option<&'static str>,
    pub decoded_max: Option<&'static str>,
    pub encoded_min: Option<&'static str>,
    pub encoded_max: Option<&'static str>,
}

#[derive(Clone, Copy, Debug)]
pub enum FrameKind {
    Periodic {
        default_period_ms: u32,
        enabled_by_default: bool,
    },
    NonPeriodic {
        rtr: bool,
    },
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub name: &'static str,
    pub description: Option<&'static str>,
    pub api_class: u32,
    pub api_index: u32,
    pub arb_id: u32,
    pub length_bytes: u32,
    /// Signals in declaration order; `parseCanFrame` emits results in this order.
    pub signals: &'static [(&'static str, Signal)],
    pub broadcast: bool,
    pub version_implemented: &'static str,
    pub version_deprecated: Option<&'static str>,
    pub version_removed: Option<&'static str>,
    pub frame_range_name: Option<&'static str>,
    pub kind: FrameKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SignalValue {
    Bool(bool),
    Num(BigNum),
}

/// `ParsedCanFrame`: signal keys in `signalsToDecode` (or declaration) order.
#[derive(Clone, Debug)]
pub struct ParsedCanFrame {
    pub entries: Vec<(&'static str, SignalValue)>,
}

impl ParsedCanFrame {
    pub fn get(&self, key: &str) -> Option<&SignalValue> {
        self.entries.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
    }
}

fn decimal(literal: &str) -> BigNum {
    BigNum::from_decimal(literal)
}

/// `getCanSignalValue`: extract the signal's bits, coerce per type, then apply
/// `scale` and `offset` in exact decimal arithmetic and clamp to the decoded range.
pub fn get_can_signal_value(
    signal: &Signal,
    bit_string: BitString,
) -> Result<SignalValue, SparkError> {
    let signal_bits = get_sub_bit_string(bit_string, signal.bit_position, signal.length_bits);
    let signal_as_uint = bit_string_to_uint(signal_bits);
    if signal.signal_type == SignalType::Boolean {
        if signal_bits.bits != 1 {
            return Err(SparkError(format!(
                "boolean signal {} does not have a length of 1 bit",
                signal.name
            )));
        }
        return Ok(SignalValue::Bool(signal_as_uint == 1));
    }
    let unadjusted = match signal.signal_type {
        SignalType::Uint => BigNum::from_int(signal_as_uint as i128),
        SignalType::Int => {
            let mut v = signal_as_uint as i128;
            if signal_bits.bits > 0 && signal_as_uint >> (signal_bits.bits - 1) & 1 == 1 {
                v -= 1i128 << signal_bits.bits;
            }
            BigNum::from_int(v)
        }
        SignalType::Float => {
            if signal_bits.bits != 32 {
                return Err(SparkError("Float signals must be 32 bits".to_owned()));
            }
            BigNum::from_f64(f32::from_bits(signal_as_uint as u32) as f64)
        }
        SignalType::Boolean => unreachable!("boolean handled above"),
    };
    let mut result = unadjusted
        .times(&decimal(signal.decode_scale_factor))
        .plus(&decimal(signal.offset));
    if let Some(min) = signal.decoded_min {
        let min = decimal(min);
        if result.lt(&min) {
            result = min;
        }
    }
    if let Some(max) = signal.decoded_max {
        let max = decimal(max);
        if result.gt(&max) {
            result = max;
        }
    }
    Ok(SignalValue::Num(result))
}

/// `parseCanFrame`.
pub fn parse_can_frame(
    frame_spec: &Frame,
    data: &[u8],
    signals_to_decode: Option<&[&'static str]>,
) -> Result<ParsedCanFrame, SparkError> {
    let keys: Vec<&'static str> = match signals_to_decode {
        Some(keys) => keys.to_vec(),
        None => frame_spec.signals.iter().map(|(k, _)| *k).collect(),
    };
    if data.len() < frame_spec.length_bytes as usize {
        return Err(SparkError(format!(
            "Tried to decode a {}-byte frame as a \"{}\" frame",
            data.len(),
            frame_spec.name
        )));
    }
    let bit_string = buffer_to_bit_string(data)?;
    let mut entries = Vec::with_capacity(keys.len());
    for key in keys {
        let signal = &frame_spec
            .signals
            .iter()
            .find(|(k, _)| *k == key)
            .expect("signalsToDecode key must exist in the frame spec")
            .1;
        entries.push((key, get_can_signal_value(signal, bit_string)?));
    }
    Ok(ParsedCanFrame { entries })
}
