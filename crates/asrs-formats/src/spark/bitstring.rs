//! Port of `bitstring.ts`. A "bit string" is the LSB-first character list upstream
//! builds; here it is a `u64` value plus a bit count, where bit `i` is character `i`
//! of the upstream string. Buffer conversion is little-endian, matching the `for` loop
//! that ORs each byte at `index * 8`.

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

/// `bitStringToBuffer`: errors when the bit string is over 64 bits or not a whole byte.
/// The upstream write goes through `writeBigUInt64LE(Number(uint))`, so values that
/// lose integer precision in f64 are written back at their rounded value.
pub fn bit_string_to_buffer(bit_string: BitString) -> Result<Vec<u8>, SparkError> {
    if bit_string.bits() > 64 {
        return Err(SparkError(format!(
            "bitStringToBuffer() called with {}-bit BitString (only BitStrings up to 64 bits are currently supported)",
            bit_string.bits()
        )));
    }
    if bit_string.bits() % 8 != 0 {
        return Err(SparkError(format!(
            "bitStringToBuffer() called with {}-bit BitString (only BitStrings with a length divisible by 8 are currently supported)",
            bit_string.bits()
        )));
    }
    let rounded = bit_string.value as f64;
    if !(rounded >= 0.0 && rounded < 18446744073709551616.0) {
        return Err(SparkError(format!(
            "bitStringToBuffer() value {rounded} is out of range for a 64-bit unsigned integer"
        )));
    }
    Ok((rounded as u64).to_le_bytes()[..(bit_string.bits() / 8) as usize].to_vec())
}

/// `uintToBitString`: `val + 2^lengthBits` as binary minus its first character; any
/// result length other than `lengthBits` throws. For negative `val + 2^lengthBits`
/// the first character is `-`, so the magnitude's binary digits become the field.
pub fn uint_to_bit_string(value: i128, length_bits: u32) -> Result<BitString, SparkError> {
    let t = value + (1i128 << length_bits);
    let (neg, mag) = if t < 0 {
        (true, (-t) as u128)
    } else {
        (false, t as u128)
    };
    let bin_digits = mag.checked_ilog2().map_or(1, |l| l + 1);
    let out_len = if neg { bin_digits } else { bin_digits - 1 };
    if out_len != length_bits {
        return Err(SparkError(format!(
            "uintToBitString() was called with a value ({value}) too large for the specified number of bits ({length_bits})"
        )));
    }
    let field = if neg { mag } else { mag - (1u128 << length_bits) };
    Ok(BitString {
        value: field as u64,
        bits: length_bits,
    })
}

/// `intToBitString`: positive modulo into the unsigned `length_bits` space.
pub fn int_to_bit_string(value: i128, length_bits: u32) -> Result<BitString, SparkError> {
    let modulus = 1i128 << length_bits;
    let wrapped = value.rem_euclid(modulus);
    uint_to_bit_string(wrapped, length_bits)
}

/// `floatToBitString`: IEEE-754 binary32 bits, little-endian.
pub fn float_to_bit_string(value: f64) -> Result<BitString, SparkError> {
    let bits = (value as f32).to_bits();
    uint_to_bit_string(bits as i128, 32)
}

/// `doubleToBitString`: IEEE-754 binary64 bits, little-endian.
pub fn double_to_bit_string(value: f64) -> Result<BitString, SparkError> {
    uint_to_bit_string(value.to_bits() as i128, 64)
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

/// `+` on bit strings: appends `next` after `base`.
pub fn concat(base: BitString, next: BitString) -> BitString {
    BitString {
        value: base.value | (next.value << base.bits.min(64)),
        bits: base.bits + next.bits,
    }
}
