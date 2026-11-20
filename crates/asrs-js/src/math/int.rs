/// ECMAScript ToInt32: truncate toward zero, then wrap modulo 2^32. NaN and infinities give 0.
pub fn to_int32(x: f64) -> i32 {
    to_uint32(x) as i32
}

/// ECMAScript ToUint32.
pub fn to_uint32(x: f64) -> u32 {
    let bits = x.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    if exponent == 0x7ff || exponent < 1023 {
        return 0;
    }
    let mantissa = (bits & ((1 << 52) - 1)) | (1 << 52);
    let shift = exponent - 1075;
    let magnitude = if shift >= 32 {
        0
    } else if shift >= 0 {
        (mantissa << shift) as u32
    } else {
        (mantissa >> -shift) as u32
    };
    if bits >> 63 == 0 {
        magnitude
    } else {
        magnitude.wrapping_neg()
    }
}

/// ToInt16, as an `Int16Array` store does.
pub fn to_int16(x: f64) -> i16 {
    to_uint32(x) as i16
}

/// ToUint16, as a `Uint16Array` store does.
pub fn to_uint16(x: f64) -> u16 {
    to_uint32(x) as u16
}

/// ToInt8, as an `Int8Array` store does.
pub fn to_int8(x: f64) -> i8 {
    to_uint32(x) as i8
}

/// ToUint8, as a `Uint8Array` or `Buffer` store does.
pub fn to_uint8(x: f64) -> u8 {
    to_uint32(x) as u8
}

/// ToUint8Clamp, as a `Uint8ClampedArray` store does: clamp to [0, 255], ties to even.
pub fn to_uint8_clamp(x: f64) -> u8 {
    if x.is_nan() || x <= 0.0 {
        return 0;
    }
    if x >= 255.0 {
        return 255;
    }
    let f = x.floor();
    let diff = x - f;
    let f = f as u8;
    if diff < 0.5 || (diff == 0.5 && f.is_multiple_of(2)) {
        f
    } else {
        f + 1
    }
}

/// `a << b`.
pub fn shift_left(a: f64, b: f64) -> i32 {
    to_int32(a).wrapping_shl(to_uint32(b) & 31)
}

/// `a >> b`.
pub fn shift_right(a: f64, b: f64) -> i32 {
    to_int32(a) >> (to_uint32(b) & 31)
}

/// `a >>> b`.
pub fn shift_right_unsigned(a: f64, b: f64) -> u32 {
    to_uint32(a) >> (to_uint32(b) & 31)
}

/// `Math.imul`.
pub fn imul(a: f64, b: f64) -> i32 {
    to_int32(a).wrapping_mul(to_int32(b))
}

/// `Math.clz32`.
pub fn clz32(x: f64) -> u32 {
    to_uint32(x).leading_zeros()
}
