//! Port of `can-spec-util.ts`: `getCanSignalValue`, `parseCanFrame` and
//! `buildCanFramePayload`. Bit positions are LSB-first. `isBigEndian` is kept in
//! the spec structs but ignored on decode, matching upstream (it has a TODO and
//! never applies it). `console.warn`/`console.error` calls upstream emits on
//! clamped or failed values are dropped; they do not change return values.

use super::BigNum;
use super::bitstring::{
    BitString, bit_string_to_buffer, bit_string_to_uint, buffer_to_bit_string, concat,
    double_to_bit_string, float_to_bit_string, get_sub_bit_string, int_to_bit_string,
    uint_to_bit_string,
};
use super::SparkError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalType {
    Boolean,
    Uint,
    Int,
    Float,
    Double,
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
pub fn get_can_signal_value(signal: &Signal, bit_string: BitString) -> Result<SignalValue, SparkError> {
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
        SignalType::Double => {
            if signal_bits.bits != 64 {
                return Err(SparkError("Double signals must be 32 bits".to_owned()));
            }
            // `writeBigUInt64LE(Number(uint))` rounds the bits through f64.
            let rounded = signal_as_uint as f64;
            BigNum::from_f64(f64::from_bits(rounded as u64))
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

/// `buildCanFramePayload`. `signalValues` must contain every signal in the spec.
pub fn build_can_frame_payload(
    frame_spec: &Frame,
    signal_values: &ParsedCanFrame,
) -> Result<Vec<u8>, SparkError> {
    let mut frame_bit_string = BitString { value: 0, bits: 0 };
    for (signal_key, signal_spec) in frame_spec.signals {
        if signal_spec.signal_type == SignalType::Boolean {
            let value = match signal_values.get(signal_key) {
                Some(SignalValue::Bool(b)) => *b,
                _ => panic!("missing boolean value for signal {signal_key}"),
            };
            frame_bit_string = concat(
                frame_bit_string,
                BitString {
                    value: value as u64,
                    bits: 1,
                },
            );
            continue;
        }
        let mut decoded_value = match signal_values.get(signal_key) {
            Some(SignalValue::Num(v)) => v.clone(),
            _ => panic!("missing numeric value for signal {signal_key}"),
        };
        if let Some(min) = signal_spec.decoded_min {
            let min = decimal(min);
            if decoded_value.lt(&min) {
                decoded_value = min;
            }
        }
        if let Some(max) = signal_spec.decoded_max {
            let max = decimal(max);
            if decoded_value.gt(&max) {
                decoded_value = max;
            }
        }
        let adjusted_value = decoded_value
            .minus(&decimal(signal_spec.offset))
            .div(&decimal(signal_spec.decode_scale_factor));
        let mut encoded_value = adjusted_value.clone();
        if matches!(
            signal_spec.signal_type,
            SignalType::Uint | SignalType::Int
        ) {
            encoded_value = adjusted_value.integer_value(super::bignum::ROUND_HALF_DOWN);
            let encoded_min = decimal(
                signal_spec.encoded_min.expect("integer signals have encodedMin"),
            );
            let encoded_max = decimal(
                signal_spec.encoded_max.expect("integer signals have encodedMax"),
            );
            if encoded_value.lt(&encoded_min) || encoded_value.gt(&encoded_max) {
                encoded_value = adjusted_value.integer_value(super::bignum::ROUND_DOWN);
                if encoded_value.lt(&encoded_min) {
                    encoded_value = encoded_min;
                }
                if encoded_value.gt(&encoded_max) {
                    encoded_value = encoded_max;
                }
            }
        }
        let signal_bit_string = match encode_signal(signal_spec, &encoded_value) {
            Ok(bs) => bs,
            Err(_) => {
                return Err(SparkError(format!(
                    "Failed to encode signal \"{}\" on frame \"{}\"",
                    signal_spec.name, frame_spec.name
                )));
            }
        };
        frame_bit_string = concat(frame_bit_string, signal_bit_string);
    }
    let buffer = bit_string_to_buffer(frame_bit_string)?;
    if buffer.len() != frame_spec.length_bytes as usize {
        return Err(SparkError(format!(
            "The assembled CAN frame payload was {} bytes when it should have been {} bytes",
            buffer.len(),
            frame_spec.length_bytes
        )));
    }
    Ok(buffer)
}

fn encode_signal(signal_spec: &Signal, encoded_value: &BigNum) -> Result<BitString, SparkError> {
    match signal_spec.signal_type {
        SignalType::Uint => {
            let int = parse_bigint(&encoded_value.to_fixed())?;
            uint_to_bit_string(int, signal_spec.length_bits)
        }
        SignalType::Float => float_to_bit_string(encoded_value.to_number()),
        SignalType::Int => {
            let int = parse_bigint(&encoded_value.to_fixed())?;
            int_to_bit_string(int, signal_spec.length_bits)
        }
        SignalType::Double => double_to_bit_string(encoded_value.to_number()),
        SignalType::Boolean => unreachable!("boolean handled in buildCanFramePayload"),
    }
}

/// `BigInt(str)`: a decimal integer string, nothing else.
fn parse_bigint(s: &str) -> Result<i128, SparkError> {
    let (neg, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(SparkError(format!("Cannot convert {s} to a BigInt")));
    }
    digits
        .parse::<i128>()
        .map(|v| if neg { -v } else { v })
        .map_err(|_| SparkError(format!("Cannot convert {s} to a BigInt")))
}
