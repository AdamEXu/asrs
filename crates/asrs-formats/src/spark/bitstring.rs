// Ported from REV Robotics CAN spec utilities (MIT, Copyright 2024 REV Robotics). See THIRD_PARTY_NOTICES.md.

//! Port of `bitstring.ts`. A "bit string" is the LSB-first character list upstream
//! builds; here it is a `u64` value plus a bit count, where bit `i` is character `i`
//! of the upstream string. Only the decode helpers are ported (see `can_spec`).

use super::SparkError;

/// An LSB-first bit string: `bits` significant bits of `value`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BitString {
    pub value: u64,
    pub bits: u32,
}

impl BitString {
    fn bits(&self) -> u32 {
        self.bits
    }
}

/// `bufferToBitString`: little-endian u64 conversion; rejects buffers over 8 bytes.
pub fn buffer_to_bit_string(buffer: &[u8]) -> Result<BitString, SparkError> {
    if buffer.len() > 8 {
        return Err(SparkError(format!(
            "bufferToBitString() called with {}-byte buffer (only buffers up to 8 bytes are currently supported)",
            buffer.len()
        )));
    }
    let mut bytes = [0u8; 8];
    bytes[..buffer.len()].copy_from_slice(buffer);
    Ok(BitString {
        value: u64::from_le_bytes(bytes),
        bits: (buffer.len() * 8) as u32,
    })
}

/// `bitStringToUint`.
pub fn bit_string_to_uint(bit_string: BitString) -> u64 {
    bit_string.value
}

/// `getSubBitString(bitString, startBit, lenBits)` = `slice(startBit, startBit+lenBits)`:
/// past-the-end slices come back shorter, exactly like `String.prototype.slice`.
pub fn get_sub_bit_string(bit_string: BitString, start_bit: u32, len_bits: u32) -> BitString {
    let available = bit_string.bits().saturating_sub(start_bit);
    let bits = len_bits.min(available);
    let mask = if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    BitString {
        value: (bit_string.value >> start_bit.min(64)) & mask,
        bits,
    }
}
