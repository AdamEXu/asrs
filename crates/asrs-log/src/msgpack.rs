//! `@msgpack/msgpack` 3.0.0-beta2 `Decoder` and `Encoder` with the default options every
//! upstream call site uses (`new Decoder()` / `new Encoder()`): numbers decode to f64
//! (64-bit ints truncate past 2^53, no BigInt), the default extension codec (timestamp
//! extension -1 to `Date`, every other ext to an `ExtData` instance), and no length caps,
//! so the `maxXxxLength` checks can never fire and are not ported.
//!
//! The default `keyDecoder` is a process-global `CachedKeyDecoder` keyed on the raw key
//! bytes: a malformed key's string depends on which earlier payload cached those bytes
//! first (and on `Math.random` evictions), so it is nondeterministic by design. The port
//! matches `keyDecoder: null`, which is what the vectors generate against.
//!
//! The bundled oracle mangles the `DecodeError` class name to `_DecodeError`; the thrown
//! `name` is part of parity output, so errors use the mangled spelling.
//!
//! UTF-8 handling switches paths on length exactly like the library: strings decode
//! through `utf8DecodeJs` at <= 200 bytes and through lossy `TextDecoder` (BOM stripped)
//! above; strings encode through `utf8EncodeJs` at <= 50 UTF-16 units (lone surrogates
//! emitted raw) and through `TextEncoder` (lone surrogates as U+FFFD) above.

use asrs_js::math::int::to_uint32;
use asrs_js::number;
use asrs_js::string::{JsError, JsString, TextDecoder, text_encode};
use asrs_js::time::time_clip;
use asrs_js::value::{JsObject, JsValue};

const DECODE_ERROR: &str = "_DecodeError";
/// `Encoder`'s `DEFAULT_MAX_DEPTH`.
const MAX_DEPTH: usize = 100;
/// `TEXT_DECODER_THRESHOLD` in utf8.ts.
const TEXT_DECODER_THRESHOLD: usize = 200;
/// `TEXT_ENCODER_THRESHOLD` in utf8.ts.
const TEXT_ENCODER_THRESHOLD: usize = 50;
/// `CachedKeyDecoder`'s `DEFAULT_MAX_KEY_LENGTH`.
const MAX_KEY_LENGTH: usize = 16;

fn decode_error(message: impl Into<String>) -> JsError {
    JsError::new(DECODE_ERROR, message)
}

fn error(message: impl Into<String>) -> JsError {
    JsError::new("Error", message)
}

/// `MORE_DATA`: every bounds check inside string and binary decoding.
fn more_data() -> JsError {
    JsError::range("Insufficient data")
}

/// A `DataView` read past the end, as thrown by V8.
fn out_of_bounds() -> JsError {
    JsError::range("Offset is outside the bounds of the DataView")
}

fn pretty_byte(byte: u8) -> String {
    format!("0x{byte:02x}")
}

/// `decode(buffer)`: one value; trailing bytes are an error.
pub fn decode(bytes: &[u8]) -> Result<JsValue, JsError> {
    let mut decoder = RawDecoder::new(bytes);
    let object = decoder.do_decode_sync()?;
    if decoder.has_remaining(1) {
        return Err(JsError::range(format!(
            "Extra {} of {} byte(s) found at buffer[{}]",
            bytes.len() - decoder.pos,
            bytes.len(),
            decoder.pos
        )));
    }
    Ok(object)
}

/// `decodeMulti(buffer)`: every value in the stream. The generator yields the values
/// decoded before a mid-stream error, so they come back alongside the error.
pub fn decode_multi(bytes: &[u8]) -> (Vec<JsValue>, Option<JsError>) {
    let mut decoder = RawDecoder::new(bytes);
    let mut values = Vec::new();
    while decoder.has_remaining(1) {
        match decoder.do_decode_sync() {
            Ok(value) => values.push(value),
            Err(e) => return (values, Some(e)),
        }
    }
    (values, None)
}

enum ArrayState {
    Array {
        array: Vec<JsValue>,
        size: u32,
    },
    Map {
        map: JsObject,
        size: u32,
        key: Option<JsString>,
        read_count: u32,
        want_key: bool,
    },
}

struct RawDecoder<'a> {
    bytes: &'a [u8],
    pos: usize,
    stack: Vec<ArrayState>,
}

impl<'a> RawDecoder<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        RawDecoder {
            bytes,
            pos: 0,
            stack: Vec::new(),
        }
    }

    fn has_remaining(&self, size: usize) -> bool {
        self.bytes.len() - self.pos >= size
    }

    fn do_decode_sync(&mut self) -> Result<JsValue, JsError> {
        'decode: loop {
            let head = self.read_u8()?;
            let mut object: JsValue = 'value: {
                if head >= 0xe0 {
                    break 'value JsValue::Number(f64::from(head) - 256.0);
                } else if head < 0xc0 {
                    if head < 0x80 {
                        break 'value JsValue::Number(f64::from(head));
                    } else if head < 0x90 {
                        let size = u32::from(head - 0x80);
                        if size != 0 {
                            self.push_map_state(size);
                            continue 'decode;
                        } else {
                            break 'value JsValue::Object(JsObject::new());
                        }
                    } else if head < 0xa0 {
                        let size = u32::from(head - 0x90);
                        if size != 0 {
                            self.push_array_state(size);
                            continue 'decode;
                        } else {
                            break 'value JsValue::Array(Vec::new());
                        }
                    } else {
                        let length = usize::from(head - 0xa0);
                        break 'value self.decode_utf8_string(length, 0)?;
                    }
                }
                match head {
                    0xc0 => break 'value JsValue::Null,
                    0xc2 => break 'value JsValue::Bool(false),
                    0xc3 => break 'value JsValue::Bool(true),
                    0xca => break 'value JsValue::Number(self.read_f32()?),
                    0xcb => break 'value JsValue::Number(self.read_f64()?),
                    0xcc => break 'value JsValue::Number(f64::from(self.read_u8()?)),
                    0xcd => {
                        break 'value JsValue::Number(f64::from(self.read_u16()?));
                    }
                    0xce => {
                        break 'value JsValue::Number(f64::from(self.read_u32()?));
                    }
                    0xcf => break 'value JsValue::Number(self.read_u64()?),
                    0xd0 => {
                        break 'value JsValue::Number(f64::from(self.read_i8()?));
                    }
                    0xd1 => {
                        break 'value JsValue::Number(f64::from(self.read_i16()?));
                    }
                    0xd2 => {
                        break 'value JsValue::Number(f64::from(self.read_i32()?));
                    }
                    0xd3 => break 'value JsValue::Number(self.read_i64()?),
                    0xd9 => {
                        let length = usize::from(self.look_u8()?);
                        break 'value self.decode_utf8_string(length, 1)?;
                    }
                    0xda => {
                        let length = usize::from(self.look_u16()?);
                        break 'value self.decode_utf8_string(length, 2)?;
                    }
                    0xdb => {
                        let length = usize::try_from(self.look_u32()?).unwrap_or(usize::MAX);
                        break 'value self.decode_utf8_string(length, 4)?;
                    }
                    0xdc => {
                        let size = u32::from(self.read_u16()?);
                        if size != 0 {
                            self.push_array_state(size);
                            continue 'decode;
                        } else {
                            break 'value JsValue::Array(Vec::new());
                        }
                    }
                    0xdd => {
                        let size = self.read_u32()?;
                        if size != 0 {
                            self.push_array_state(size);
                            continue 'decode;
                        } else {
                            break 'value JsValue::Array(Vec::new());
                        }
                    }
                    0xde => {
                        let size = u32::from(self.read_u16()?);
                        if size != 0 {
                            self.push_map_state(size);
                            continue 'decode;
                        } else {
                            break 'value JsValue::Object(JsObject::new());
                        }
                    }
                    0xdf => {
                        let size = self.read_u32()?;
                        if size != 0 {
                            self.push_map_state(size);
                            continue 'decode;
                        } else {
                            break 'value JsValue::Object(JsObject::new());
                        }
                    }
                    0xc4 => {
                        let size = usize::from(self.look_u8()?);
                        break 'value self.decode_binary(size, 1)?;
                    }
                    0xc5 => {
                        let size = usize::from(self.look_u16()?);
                        break 'value self.decode_binary(size, 2)?;
                    }
                    0xc6 => {
                        let size = usize::try_from(self.look_u32()?).unwrap_or(usize::MAX);
                        break 'value self.decode_binary(size, 4)?;
                    }
                    0xd4 => break 'value self.decode_extension(1, 0)?,
                    0xd5 => break 'value self.decode_extension(2, 0)?,
                    0xd6 => break 'value self.decode_extension(4, 0)?,
                    0xd7 => break 'value self.decode_extension(8, 0)?,
                    0xd8 => break 'value self.decode_extension(16, 0)?,
                    0xc7 => {
                        let size = usize::from(self.look_u8()?);
                        break 'value self.decode_extension(size, 1)?;
                    }
                    0xc8 => {
                        let size = usize::from(self.look_u16()?);
                        break 'value self.decode_extension(size, 2)?;
                    }
                    0xc9 => {
                        let size = usize::try_from(self.look_u32()?).unwrap_or(usize::MAX);
                        break 'value self.decode_extension(size, 4)?;
                    }
                    _ => {
                        return Err(decode_error(format!(
                            "Unrecognized type byte: {}",
                            pretty_byte(head)
                        )));
                    }
                }
            };

            while let Some(state) = self.stack.last_mut() {
                match state {
                    ArrayState::Array { array, size } => {
                        array.push(object);
                        if array.len() as u64 == u64::from(*size) {
                            let Some(ArrayState::Array { array, .. }) = self.stack.pop() else {
                                unreachable!()
                            };
                            object = JsValue::Array(array);
                        } else {
                            continue 'decode;
                        }
                    }
                    ArrayState::Map {
                        map,
                        size,
                        key,
                        read_count,
                        want_key,
                    } => {
                        if *want_key {
                            *key = Some(match object {
                                JsValue::String(s) => s,
                                JsValue::Number(x) => JsString::from(number::to_string(x)),
                                other => {
                                    return Err(decode_error(format!(
                                        "The type of key must be string or number but {}",
                                        other.type_of()
                                    )));
                                }
                            });
                            if key.as_ref().is_some_and(|k| {
                                k.units().iter().copied().eq("__proto__".encode_utf16())
                            }) {
                                return Err(decode_error("The key __proto__ is not allowed"));
                            }
                            *want_key = false;
                            continue 'decode;
                        }
                        map.insert(key.take().expect("map key set"), object);
                        *read_count += 1;
                        if *read_count == *size {
                            let Some(ArrayState::Map { map, .. }) = self.stack.pop() else {
                                unreachable!()
                            };
                            object = JsValue::Object(map);
                        } else {
                            *want_key = true;
                            continue 'decode;
                        }
                    }
                }
            }
            return Ok(object);
        }
    }

    fn push_map_state(&mut self, size: u32) {
        self.stack.push(ArrayState::Map {
            map: JsObject::new(),
            size,
            key: None,
            read_count: 0,
            want_key: true,
        });
    }

    fn push_array_state(&mut self, size: u32) {
        self.stack.push(ArrayState::Array {
            array: Vec::new(),
            size,
        });
    }

    fn state_is_map_key(&self) -> bool {
        matches!(
            self.stack.last(),
            Some(ArrayState::Map { want_key: true, .. })
        )
    }

    fn decode_utf8_string(
        &mut self,
        byte_length: usize,
        header_offset: usize,
    ) -> Result<JsValue, JsError> {
        if self.bytes.len() < self.pos + header_offset + byte_length {
            return Err(more_data());
        }
        let offset = self.pos + header_offset;
        // `keyDecoder.canBeCached` strings go through the same utf8DecodeJs; with the
        // cache itself not ported, only the length threshold matters here.
        let text = if self.state_is_map_key() && byte_length > 0 && byte_length <= MAX_KEY_LENGTH {
            utf8_decode_js(self.bytes, offset, byte_length)
        } else {
            utf8_decode(self.bytes, offset, byte_length)
        };
        self.pos += header_offset + byte_length;
        Ok(JsValue::String(text))
    }

    fn decode_binary(
        &mut self,
        byte_length: usize,
        head_offset: usize,
    ) -> Result<JsValue, JsError> {
        if !self.has_remaining(byte_length + head_offset) {
            return Err(more_data());
        }
        let offset = self.pos + head_offset;
        let data = self.bytes[offset..offset + byte_length].to_vec();
        self.pos += head_offset + byte_length;
        Ok(JsValue::Bytes(data))
    }

    fn decode_extension(&mut self, size: usize, head_offset: usize) -> Result<JsValue, JsError> {
        let ext_type = self.read_i8_at(self.pos + head_offset)?;
        let JsValue::Bytes(data) = self.decode_binary(size, head_offset + 1)? else {
            unreachable!()
        };
        if ext_type != -1 {
            return Ok(JsValue::ExtData(ext_type, data));
        }
        // `decodeTimestampExtension` on the default codec.
        let view = data.as_slice();
        let (sec, nsec) = match view.len() {
            4 => (f64::from(read_u32(view, 0)), 0.0),
            8 => {
                let high = read_u32(view, 0);
                let low = read_u32(view, 4);
                (
                    f64::from(high & 0x3) * 4_294_967_296.0 + f64::from(low),
                    f64::from(high >> 2),
                )
            }
            12 => (get_int64(view, 4), f64::from(read_u32(view, 0))),
            n => {
                return Err(decode_error(format!(
                    "Unrecognized data size for timestamp (expected 4, 8, or 12): {n}"
                )));
            }
        };
        Ok(JsValue::Date(time_clip(sec * 1e3 + nsec / 1e6)))
    }

    fn look_u8(&self) -> Result<u8, JsError> {
        self.bytes.get(self.pos).copied().ok_or_else(out_of_bounds)
    }

    fn look_u16(&self) -> Result<u16, JsError> {
        self.bytes
            .get(self.pos..self.pos + 2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
            .ok_or_else(out_of_bounds)
    }

    fn look_u32(&self) -> Result<u32, JsError> {
        self.bytes
            .get(self.pos..self.pos + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(out_of_bounds)
    }

    fn read_u8(&mut self) -> Result<u8, JsError> {
        let v = self.look_u8()?;
        self.pos += 1;
        Ok(v)
    }

    fn read_i8_at(&self, pos: usize) -> Result<i8, JsError> {
        self.bytes
            .get(pos)
            .map(|&b| b as i8)
            .ok_or_else(out_of_bounds)
    }

    fn read_i8(&mut self) -> Result<i8, JsError> {
        let v = self.read_i8_at(self.pos)?;
        self.pos += 1;
        Ok(v)
    }

    fn read_u16(&mut self) -> Result<u16, JsError> {
        let v = self.look_u16()?;
        self.pos += 2;
        Ok(v)
    }

    fn read_i16(&mut self) -> Result<i16, JsError> {
        Ok(self.read_u16()? as i16)
    }

    fn read_u32(&mut self) -> Result<u32, JsError> {
        let v = self.look_u32()?;
        self.pos += 4;
        Ok(v)
    }

    fn read_i32(&mut self) -> Result<i32, JsError> {
        Ok(self.read_u32()? as i32)
    }

    /// `getUint64`: high and low words combined as f64, so bits past 2^53 are lost.
    fn read_u64(&mut self) -> Result<f64, JsError> {
        let high = self.read_u32()?;
        let low = self.read_u32()?;
        Ok(f64::from(high) * 4_294_967_296.0 + f64::from(low))
    }

    /// `getInt64`: the high word is signed.
    fn read_i64(&mut self) -> Result<f64, JsError> {
        let high = self.read_i32()?;
        let low = self.read_u32()?;
        Ok(f64::from(high) * 4_294_967_296.0 + f64::from(low))
    }

    fn read_f32(&mut self) -> Result<f64, JsError> {
        // Widening keeps the NaN payload; V8 does not canonicalize on this path.
        Ok(f64::from(f32::from_bits(self.read_u32()?)))
    }

    fn read_f64(&mut self) -> Result<f64, JsError> {
        let bytes = self
            .bytes
            .get(self.pos..self.pos + 8)
            .ok_or_else(out_of_bounds)?;
        self.pos += 8;
        Ok(f64::from_be_bytes(bytes.try_into().expect("8 bytes")))
    }
}

fn read_u32(view: &[u8], pos: usize) -> u32 {
    u32::from_be_bytes(view[pos..pos + 4].try_into().expect("in range"))
}

/// `getInt64` on a standalone slice.
fn get_int64(view: &[u8], pos: usize) -> f64 {
    let high = read_u32(view, pos) as i32;
    let low = read_u32(view, pos + 4);
    f64::from(high) * 4_294_967_296.0 + f64::from(low)
}

/// `utf8Decode`: the JS decoder below 200 bytes, `TextDecoder` above.
fn utf8_decode(bytes: &[u8], offset: usize, byte_length: usize) -> JsString {
    if byte_length > TEXT_DECODER_THRESHOLD {
        let decoder = TextDecoder::new(None, false, false).expect("utf-8 exists");
        return decoder
            .decode(&bytes[offset..offset + byte_length])
            .expect("non-fatal utf-8 decode cannot fail");
    }
    utf8_decode_js(bytes, offset, byte_length)
}

/// `utf8DecodeJs`: a lenient decoder that never fails. Continuation bytes read past the
/// string are the next bytes of the buffer, and `undefined` past the end, which `& 0x3f`
/// turns into 0.
fn utf8_decode_js(bytes: &[u8], input_offset: usize, byte_length: usize) -> JsString {
    let mut offset = input_offset;
    let end = offset + byte_length;
    let mut units = Vec::new();
    let byte_at = |offset: &mut usize| -> u32 {
        let b = u32::from(bytes.get(*offset).copied().unwrap_or(0));
        *offset += 1;
        b
    };
    while offset < end {
        let byte1 = u32::from(bytes[offset]);
        offset += 1;
        if byte1 & 0x80 == 0 {
            units.push(byte1 as u16);
        } else if byte1 & 0xe0 == 0xc0 {
            let byte2 = byte_at(&mut offset) & 0x3f;
            units.push((((byte1 & 0x1f) << 6) | byte2) as u16);
        } else if byte1 & 0xf0 == 0xe0 {
            let byte2 = byte_at(&mut offset) & 0x3f;
            let byte3 = byte_at(&mut offset) & 0x3f;
            units.push((((byte1 & 0x0f) << 12) | (byte2 << 6) | byte3) as u16);
        } else if byte1 & 0xf8 == 0xf0 {
            let byte2 = byte_at(&mut offset) & 0x3f;
            let byte3 = byte_at(&mut offset) & 0x3f;
            let byte4 = byte_at(&mut offset) & 0x3f;
            let mut unit = ((byte1 & 0x07) << 0x12) | (byte2 << 0x0c) | (byte3 << 0x06) | byte4;
            if unit > 0xffff {
                unit -= 0x10000;
                units.push((((unit >> 10) & 0x3ff) | 0xd800) as u16);
                unit = 0xdc00 | (unit & 0x3ff);
            }
            units.push(unit as u16);
        } else {
            units.push(byte1 as u16);
        }
    }
    JsString::from_units(units)
}

/// `encode(object)` with the default options: safe integers go out as the smallest
/// msgpack int, every other number as float64; `undefined` is `nil` (it is never
/// ignored); `Date` is the timestamp extension; `ExtData` re-encodes as itself.
pub fn encode(object: &JsValue) -> Result<Vec<u8>, JsError> {
    let mut out = Vec::new();
    do_encode(object, 1, &mut out)?;
    Ok(out)
}

fn do_encode(object: &JsValue, depth: usize, out: &mut Vec<u8>) -> Result<(), JsError> {
    if depth > MAX_DEPTH {
        return Err(error(format!("Too deep objects in depth {depth}")));
    }
    match object {
        JsValue::Undefined | JsValue::Null => out.push(0xc0),
        JsValue::Bool(b) => out.push(if *b { 0xc3 } else { 0xc2 }),
        JsValue::Number(x) => encode_number(*x, out),
        JsValue::String(s) => encode_string(s, out)?,
        JsValue::BigInt(_) => {
            return encode_object_fallback(object);
        }
        JsValue::Date(_)
        | JsValue::ExtData(..)
        | JsValue::Array(_)
        | JsValue::Bytes(_)
        | JsValue::Object(_) => encode_object(object, depth, out)?,
    }
    Ok(())
}

fn encode_object_fallback(object: &JsValue) -> Result<(), JsError> {
    let tag = match object {
        JsValue::BigInt(_) => "[object BigInt]",
        _ => "[object Object]",
    };
    Err(error(format!("Unrecognized object: {tag}")))
}

/// `Number.isSafeInteger` picks the int forms; everything else is float64.
fn encode_number(object: f64, out: &mut Vec<u8>) {
    if object.is_finite() && object.trunc() == object && object.abs() <= 9_007_199_254_740_991.0 {
        if object >= 0.0 {
            if object < 128.0 {
                out.push(object as u8);
            } else if object < 256.0 {
                out.push(0xcc);
                out.push(object as u8);
            } else if object < 65536.0 {
                out.push(0xcd);
                out.extend_from_slice(&(object as u16).to_be_bytes());
            } else if object < 4_294_967_296.0 {
                out.push(0xce);
                out.extend_from_slice(&(object as u32).to_be_bytes());
            } else {
                out.push(0xcf);
                out.extend_from_slice(&(object as u64).to_be_bytes());
            }
        } else if object >= -32.0 {
            out.push(0xe0 | (object + 32.0) as u8);
        } else if object >= -128.0 {
            out.push(0xd0);
            out.push(object as i8 as u8);
        } else if object >= -32768.0 {
            out.push(0xd1);
            out.extend_from_slice(&(object as i16).to_be_bytes());
        } else if object >= -2_147_483_648.0 {
            out.push(0xd2);
            out.extend_from_slice(&(object as i32).to_be_bytes());
        } else {
            out.push(0xd3);
            out.extend_from_slice(&(object as i64).to_be_bytes());
        }
    } else {
        // `forceFloat32` is false: everything non-safe-integer is float64.
        out.push(0xcb);
        out.extend_from_slice(&object.to_be_bytes());
    }
}

/// `writeStringHeader`: widths from the UTF-8 byte length, not the unit count.
fn write_string_header(byte_length: usize, out: &mut Vec<u8>) -> Result<(), JsError> {
    if byte_length < 32 {
        out.push(0xa0 + byte_length as u8);
    } else if byte_length < 0x100 {
        out.push(0xd9);
        out.push(byte_length as u8);
    } else if byte_length < 0x10000 {
        out.push(0xda);
        out.extend_from_slice(&(byte_length as u16).to_be_bytes());
    } else if byte_length < 0x1_0000_0000 {
        out.push(0xdb);
        out.extend_from_slice(&(byte_length as u32).to_be_bytes());
    } else {
        return Err(error(format!(
            "Too long string: {byte_length} bytes in UTF-8"
        )));
    }
    Ok(())
}

fn encode_string(object: &JsString, out: &mut Vec<u8>) -> Result<(), JsError> {
    let byte_length = utf8_count(object);
    write_string_header(byte_length, out)?;
    if object.len() > TEXT_ENCODER_THRESHOLD {
        out.extend_from_slice(&text_encode(object));
    } else {
        utf8_encode_js(object, out);
    }
    Ok(())
}

/// `utf8Count`: the same surrogate handling as `utf8EncodeJs`, counting what a
/// `TextEncoder` would write too since both paths emit 3 bytes per lone surrogate.
fn utf8_count(s: &JsString) -> usize {
    let units = s.units();
    let mut byte_length = 0;
    let mut pos = 0;
    while pos < units.len() {
        let mut value = u32::from(units[pos]);
        pos += 1;
        if value & 0xffff_ff80 == 0 {
            byte_length += 1;
            continue;
        } else if value & 0xffff_f800 == 0 {
            byte_length += 2;
        } else {
            if (0xd800..0xdc00).contains(&value) && pos < units.len() {
                let extra = u32::from(units[pos]);
                if extra & 0xfc00 == 0xdc00 {
                    pos += 1;
                    value = ((value & 0x3ff) << 10) + (extra & 0x3ff) + 0x10000;
                }
            }
            byte_length += if value & 0xffff_0000 == 0 { 3 } else { 4 };
        }
    }
    byte_length
}

/// `utf8EncodeJs`: lone surrogates are written as their own 3-byte encoding, so the
/// output can be ill-formed UTF-8; only `TextEncoder` (>50 units) writes U+FFFD.
fn utf8_encode_js(s: &JsString, out: &mut Vec<u8>) {
    let units = s.units();
    let mut pos = 0;
    while pos < units.len() {
        let mut value = u32::from(units[pos]);
        pos += 1;
        if value & 0xffff_ff80 == 0 {
            out.push(value as u8);
            continue;
        } else if value & 0xffff_f800 != 0 {
            if (0xd800..0xdc00).contains(&value) && pos < units.len() {
                let extra = u32::from(units[pos]);
                if extra & 0xfc00 == 0xdc00 {
                    pos += 1;
                    value = ((value & 0x3ff) << 10) + (extra & 0x3ff) + 0x10000;
                }
            }
            if value & 0xffff_0000 == 0 {
                out.push(((value >> 12) & 0x0f) as u8 | 0xe0);
                out.push(((value >> 6) & 0x3f) as u8 | 0x80);
            } else {
                out.push(((value >> 18) & 0x07) as u8 | 0xf0);
                out.push(((value >> 12) & 0x3f) as u8 | 0x80);
                out.push(((value >> 6) & 0x3f) as u8 | 0x80);
            }
        } else {
            out.push(((value >> 6) & 0x1f) as u8 | 0xc0);
        }
        out.push((value & 0x3f) as u8 | 0x80);
    }
}

fn encode_object(object: &JsValue, depth: usize, out: &mut Vec<u8>) -> Result<(), JsError> {
    // `extensionCodec.tryToEncode`: Date -> timestamp extension, ExtData -> itself.
    match object {
        JsValue::Date(t) => return encode_timestamp(*t, out),
        JsValue::ExtData(kind, data) => return encode_extension(*kind, data, out),
        _ => {}
    }
    match object {
        JsValue::Array(items) => {
            let size = items.len();
            if size < 16 {
                out.push(0x90 + size as u8);
            } else if size < 0x10000 {
                out.push(0xdc);
                out.extend_from_slice(&(size as u16).to_be_bytes());
            } else if size < 0x1_0000_0000 {
                out.push(0xdd);
                out.extend_from_slice(&(size as u32).to_be_bytes());
            } else {
                return Err(error(format!("Too large array: {size}")));
            }
            for item in items {
                do_encode(item, depth + 1, out)?;
            }
            Ok(())
        }
        JsValue::Bytes(bytes) => {
            let size = bytes.len();
            if size < 0x100 {
                out.push(0xc4);
                out.push(size as u8);
            } else if size < 0x10000 {
                out.push(0xc5);
                out.extend_from_slice(&(size as u16).to_be_bytes());
            } else if size < 0x1_0000_0000 {
                out.push(0xc6);
                out.extend_from_slice(&(size as u32).to_be_bytes());
            } else {
                return Err(error(format!("Too large binary: {size}")));
            }
            out.extend_from_slice(bytes);
            Ok(())
        }
        JsValue::Object(map) => {
            let size = map.len();
            if size < 16 {
                out.push(0x80 + size as u8);
            } else if size < 0x10000 {
                out.push(0xde);
                out.extend_from_slice(&(size as u16).to_be_bytes());
            } else if size < 0x1_0000_0000 {
                out.push(0xdf);
                out.extend_from_slice(&(size as u32).to_be_bytes());
            } else {
                return Err(error(format!("Too large map object: {size}")));
            }
            for (key, value) in map.iter() {
                encode_string(key, out)?;
                do_encode(value, depth + 1, out)?;
            }
            Ok(())
        }
        other => encode_object_fallback(other),
    }
}

/// `encodeDateToTimeSpec` + `encodeTimeSpecToTimestamp`, then `encodeExtension`.
fn encode_timestamp(msec: f64, out: &mut Vec<u8>) -> Result<(), JsError> {
    let mut sec = (msec / 1e3).floor();
    let mut nsec = (msec - sec * 1e3) * 1e6;
    // Normalizes { sec, nsec } to ensure nsec is unsigned.
    let nsec_in_sec = (nsec / 1e9).floor();
    sec += nsec_in_sec;
    nsec -= nsec_in_sec * 1e9;
    let data: Vec<u8> = if sec >= 0.0 && nsec >= 0.0 && sec <= 17_179_869_183.0 {
        if nsec == 0.0 && sec < 4_294_967_296.0 {
            to_uint32(sec).to_be_bytes().to_vec()
        } else {
            let sec_high = sec / 4_294_967_296.0;
            let sec_low = to_uint32(sec);
            let mut rv = Vec::with_capacity(8);
            rv.extend_from_slice(
                &((to_uint32(nsec) << 2) | (to_uint32(sec_high) & 0x3)).to_be_bytes(),
            );
            rv.extend_from_slice(&sec_low.to_be_bytes());
            rv
        }
    } else {
        // timestamp 96: nsec32 then int64 sec through the f64 path of setInt64.
        let mut rv = Vec::with_capacity(12);
        rv.extend_from_slice(&to_uint32(nsec).to_be_bytes());
        let high = to_uint32((sec / 4_294_967_296.0).floor());
        let low = to_uint32(sec);
        rv.extend_from_slice(&high.to_be_bytes());
        rv.extend_from_slice(&low.to_be_bytes());
        rv
    };
    encode_extension(-1, &data, out)
}

fn encode_extension(kind: i8, data: &[u8], out: &mut Vec<u8>) -> Result<(), JsError> {
    let size = data.len();
    match size {
        1 => out.push(0xd4),
        2 => out.push(0xd5),
        4 => out.push(0xd6),
        8 => out.push(0xd7),
        16 => out.push(0xd8),
        _ if size < 0x100 => {
            out.push(0xc7);
            out.push(size as u8);
        }
        _ if size < 0x10000 => {
            out.push(0xc8);
            out.extend_from_slice(&(size as u16).to_be_bytes());
        }
        _ if size < 0x1_0000_0000 => {
            out.push(0xc9);
            out.extend_from_slice(&(size as u32).to_be_bytes());
        }
        _ => return Err(error(format!("Too large extension object: {size}"))),
    }
    out.push(kind as u8);
    out.extend_from_slice(data);
    Ok(())
}
