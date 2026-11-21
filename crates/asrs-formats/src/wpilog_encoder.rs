//! `src/hub/dataSources/wpilog/WPILOGEncoder.ts` (F11): the record builders and
//! the byte writer the export worker uses. `entry` and `timestamp` stay `f64`
//! exactly as upstream keeps them; the `BigInt(Math.floor(int))` in
//! `encodeInteger` throws on non-finite values, which the worker turns into a
//! rejection. The `make*` builders take the sample as the JS value upstream
//! passes (`value: any`), so a value kind that does not match the entry's type
//! string follows JS coercion or throws the way upstream's would.

use asrs_js::math::floor;
use asrs_js::math::int::{to_uint8, to_uint32};
use asrs_js::number;
use asrs_js::sort::array_sort;
use asrs_js::string::{JsString, text_encode};
use asrs_log::log::log_value_sets::LogValueSetAny;

use crate::export::ExportError;
use crate::wpilog::{CONTROL_ENTRY, CONTROL_START, HEADER_STRING, HEADER_VERSION};

/// One sample from `fieldData.values`, tagged with the field's `LoggableType`.
pub enum Sample<'a> {
    Raw(&'a [u8]),
    Boolean(bool),
    Number(f64),
    String(&'a JsString),
    BooleanArray(&'a [bool]),
    NumberArray(&'a [f64]),
    StringArray(&'a [JsString]),
}

impl<'a> Sample<'a> {
    pub fn at(set: &'a LogValueSetAny, index: usize) -> Sample<'a> {
        match set {
            LogValueSetAny::Raw(set) => Sample::Raw(&set.values[index]),
            LogValueSetAny::Boolean(set) => Sample::Boolean(set.values[index]),
            LogValueSetAny::Number(set) => Sample::Number(set.values[index]),
            LogValueSetAny::String(set) => Sample::String(&set.values[index]),
            LogValueSetAny::BooleanArray(set) => Sample::BooleanArray(&set.values[index]),
            LogValueSetAny::NumberArray(set) => Sample::NumberArray(&set.values[index]),
            LogValueSetAny::StringArray(set) => Sample::StringArray(&set.values[index]),
            LogValueSetAny::Empty(_) => unreachable!("Empty fields are filtered out"),
        }
    }
}

/// `ToBoolean`.
fn truthy(v: &Sample<'_>) -> bool {
    match v {
        Sample::Boolean(b) => *b,
        Sample::Number(n) => *n != 0.0 && !n.is_nan(),
        Sample::String(s) => !s.is_empty(),
        Sample::Raw(_) | Sample::BooleanArray(_) | Sample::NumberArray(_)
        | Sample::StringArray(_) => true,
    }
}

/// `ToString`.
fn js_string(v: &Sample<'_>) -> JsString {
    match v {
        Sample::Boolean(b) => JsString::from(if *b { "true" } else { "false" }),
        Sample::Number(n) => JsString::from(number::to_string(*n)),
        Sample::String(s) => (*s).clone(),
        Sample::Raw(bytes) => join(bytes.iter().map(|b| JsString::from(b.to_string()))),
        Sample::BooleanArray(items) => join(
            items
                .iter()
                .map(|b| JsString::from(if *b { "true" } else { "false" })),
        ),
        Sample::NumberArray(items) => join(
            items
                .iter()
                .map(|n| JsString::from(number::to_string(*n))),
        ),
        Sample::StringArray(items) => join(items.iter().map(|s| (*s).clone())),
    }
}

fn join(parts: impl IntoIterator<Item = JsString>) -> JsString {
    let mut out = JsString::new();
    for (index, part) in parts.into_iter().enumerate() {
        if index != 0 {
            out.push_unit(u16::from(b','));
        }
        out.push_str(&part);
    }
    out
}

/// `ToNumber`.
fn js_number(v: &Sample<'_>) -> f64 {
    match v {
        Sample::Boolean(b) => f64::from(u8::from(*b)),
        Sample::Number(n) => *n,
        Sample::String(s) => number::to_number(s.units()),
        _ => number::to_number(js_string(v).units()),
    }
}

/// The elements `forEach` and `map` produce for the array-likes; `None` for the
/// scalars, where the method is missing and the call throws `TypeError`.
fn elements<'a>(v: &'a Sample<'a>) -> Option<Vec<Sample<'a>>> {
    match v {
        Sample::Raw(bytes) => Some(
            bytes
                .iter()
                .map(|b| Sample::Number(f64::from(*b)))
                .collect(),
        ),
        Sample::BooleanArray(items) => Some(items.iter().map(|b| Sample::Boolean(*b)).collect()),
        Sample::NumberArray(items) => Some(items.iter().map(|n| Sample::Number(*n)).collect()),
        Sample::StringArray(items) => Some(items.iter().map(|s| Sample::String(s)).collect()),
        _ => None,
    }
}

/// `BigInt(value)` followed by `setBigInt64`'s truncation to 64 bits, as LE
/// bytes. `RangeError` on a non-integer or non-finite input.
fn bigint_bytes(v: &Sample<'_>) -> Result<[u8; 8], ExportError> {
    let n = match v {
        Sample::Number(n) => *n,
        Sample::Boolean(b) => f64::from(u8::from(*b)),
        _ => return Err(ExportError::Rejected),
    };
    if n.fract() != 0.0 || !n.is_finite() {
        return Err(ExportError::Rejected);
    }
    Ok(bigint_mod(n).to_le_bytes())
}

/// `BigInt` to `setBigInt64`: the exact integer of the `f64`, modulo 2^64. The
/// cast through `i128` keeps every bit; at or beyond 2^127 the value is a
/// multiple of 2^64, so the residue is 0.
fn bigint_mod(n: f64) -> u64 {
    const TWO_POW_127: f64 = 170141183460469231731687303715884105728.0;
    if n.abs() < TWO_POW_127 {
        n as i128 as u64
    } else {
        0
    }
}

/// What `array.set(this.data, ...)` copies out of a record.
enum RecordData {
    Bytes(Vec<u8>),
    /// A `data` without a `length`: `encodeInteger(undefined)` throws in
    /// `getEncoded`. Numbers and booleans reach here through `makeRaw`.
    Undefined,
}

pub struct Record {
    entry: f64,
    timestamp: f64,
    data: RecordData,
}

impl Record {
    /// `makeControlStart`.
    pub fn control_start(
        timestamp: f64,
        entry: f64,
        name: &JsString,
        type_: &JsString,
        metadata: &JsString,
    ) -> Record {
        let name = text_encode(name);
        let type_ = text_encode(type_);
        let metadata = text_encode(metadata);
        let mut data =
            Vec::with_capacity(1 + 4 + 4 + name.len() + 4 + type_.len() + 4 + metadata.len());
        data.push(CONTROL_START);
        data.extend_from_slice(&to_uint32(entry).to_le_bytes());
        data.extend_from_slice(&(name.len() as u32).to_le_bytes());
        data.extend_from_slice(&name);
        data.extend_from_slice(&(type_.len() as u32).to_le_bytes());
        data.extend_from_slice(&type_);
        data.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
        data.extend_from_slice(&metadata);
        Record {
            entry: f64::from(CONTROL_ENTRY),
            timestamp,
            data: RecordData::Bytes(data),
        }
    }

    /// `makeBoolean`.
    pub fn boolean(entry: f64, timestamp: f64, value: &Sample<'_>) -> Record {
        Record::bytes(entry, timestamp, vec![u8::from(truthy(value))])
    }

    /// `makeInteger`.
    pub fn integer(entry: f64, timestamp: f64, value: &Sample<'_>) -> Result<Record, ExportError> {
        Ok(Record::bytes(entry, timestamp, bigint_bytes(value)?))
    }

    /// `makeFloat`.
    pub fn float(entry: f64, timestamp: f64, value: &Sample<'_>) -> Record {
        Record::bytes(entry, timestamp, (js_number(value) as f32).to_le_bytes())
    }

    /// `makeDouble`.
    pub fn double(entry: f64, timestamp: f64, value: &Sample<'_>) -> Record {
        Record::bytes(entry, timestamp, js_number(value).to_le_bytes())
    }

    /// `makeString`.
    pub fn string(entry: f64, timestamp: f64, value: &Sample<'_>) -> Record {
        Record::bytes(entry, timestamp, text_encode(&js_string(value)))
    }

    /// `makeBooleanArray`.
    pub fn boolean_array(
        entry: f64,
        timestamp: f64,
        value: &Sample<'_>,
    ) -> Result<Record, ExportError> {
        let Some(items) = elements(value) else {
            return Err(ExportError::Rejected);
        };
        Ok(Record::bytes(
            entry,
            timestamp,
            items
                .iter()
                .map(|item| u8::from(truthy(item)))
                .collect::<Vec<u8>>(),
        ))
    }

    /// `makeIntegerArray`.
    pub fn integer_array(
        entry: f64,
        timestamp: f64,
        value: &Sample<'_>,
    ) -> Result<Record, ExportError> {
        let Some(items) = elements(value) else {
            return Err(ExportError::Rejected);
        };
        let mut data = Vec::with_capacity(items.len() * 8);
        for item in &items {
            data.extend_from_slice(&bigint_bytes(item)?);
        }
        Ok(Record::bytes(entry, timestamp, data))
    }

    /// `makeFloatArray`.
    pub fn float_array(
        entry: f64,
        timestamp: f64,
        value: &Sample<'_>,
    ) -> Result<Record, ExportError> {
        let Some(items) = elements(value) else {
            return Err(ExportError::Rejected);
        };
        let mut data = Vec::with_capacity(items.len() * 4);
        for item in &items {
            data.extend_from_slice(&(js_number(item) as f32).to_le_bytes());
        }
        Ok(Record::bytes(entry, timestamp, data))
    }

    /// `makeDoubleArray`.
    pub fn double_array(
        entry: f64,
        timestamp: f64,
        value: &Sample<'_>,
    ) -> Result<Record, ExportError> {
        let Some(items) = elements(value) else {
            return Err(ExportError::Rejected);
        };
        let mut data = Vec::with_capacity(items.len() * 8);
        for item in &items {
            data.extend_from_slice(&js_number(item).to_le_bytes());
        }
        Ok(Record::bytes(entry, timestamp, data))
    }

    /// `makeStringArray`: `u32` count, then `u32` length plus bytes per string.
    /// For a `Uint8Array` sample `.map` returns another `Uint8Array` rather
    /// than an array of encoded strings, so the length sum is `NaN` and the
    /// `setUint32` into the empty buffer throws.
    pub fn string_array(
        entry: f64,
        timestamp: f64,
        value: &Sample<'_>,
    ) -> Result<Record, ExportError> {
        if let Sample::Raw(bytes) = value {
            return if bytes.is_empty() {
                Ok(Record::bytes(entry, timestamp, vec![0; 4]))
            } else {
                Err(ExportError::Rejected)
            };
        }
        let Some(items) = elements(value) else {
            return Err(ExportError::Rejected);
        };
        let encoded: Vec<Vec<u8>> = items
            .iter()
            .map(|item| text_encode(&js_string(item)))
            .collect();
        let mut data =
            Vec::with_capacity(4 + items.len() * 4 + encoded.iter().map(Vec::len).sum::<usize>());
        data.extend_from_slice(&(encoded.len() as u32).to_le_bytes());
        for item in &encoded {
            data.extend_from_slice(&(item.len() as u32).to_le_bytes());
            data.extend_from_slice(item);
        }
        Ok(Record::bytes(entry, timestamp, data))
    }

    /// `makeRaw`: the sample becomes the payload verbatim when it is a
    /// `Uint8Array`. Other array-likes land in `set` as `ToUint8` of each
    /// element (a string's code units all become 0); scalars have no `length`
    /// and throw later, inside `getEncoded`.
    pub fn raw(entry: f64, timestamp: f64, value: &Sample<'_>) -> Record {
        let data = match value {
            Sample::Raw(bytes) => RecordData::Bytes(bytes.to_vec()),
            Sample::String(s) => RecordData::Bytes(vec![0; s.len()]),
            Sample::BooleanArray(items) => RecordData::Bytes(
                items
                    .iter()
                    .map(|b| to_uint8(f64::from(u8::from(*b))))
                    .collect(),
            ),
            Sample::NumberArray(items) => {
                RecordData::Bytes(items.iter().map(|n| to_uint8(*n)).collect())
            }
            Sample::StringArray(items) => RecordData::Bytes(
                items
                    .iter()
                    .map(|s| to_uint8(number::to_number(s.units())))
                    .collect(),
            ),
            Sample::Boolean(_) | Sample::Number(_) => RecordData::Undefined,
        };
        Record {
            entry,
            timestamp,
            data,
        }
    }

    fn bytes(entry: f64, timestamp: f64, data: impl Into<Vec<u8>>) -> Record {
        Record {
            entry,
            timestamp,
            data: RecordData::Bytes(data.into()),
        }
    }

    /// `getEncoded`: the length bitfield plus the minimal-length integers.
    fn encode(&self) -> Result<Vec<u8>, ExportError> {
        let entry = encode_integer(self.entry)?;
        let size = encode_integer(match &self.data {
            RecordData::Bytes(data) => data.len() as f64,
            RecordData::Undefined => f64::NAN,
        })?;
        let timestamp = encode_integer(self.timestamp)?;
        let bitfield = u8::try_from(
            (entry.len() - 1) & 0x3
                | (((size.len() - 1) & 0x3) << 2)
                | (((timestamp.len() - 1) & 0x7) << 4),
        )
        .expect("bitfield fits in a byte");
        let RecordData::Bytes(data) = &self.data else {
            unreachable!()
        };
        let mut out =
            Vec::with_capacity(1 + entry.len() + size.len() + timestamp.len() + data.len());
        out.push(bitfield);
        out.extend_from_slice(&entry);
        out.extend_from_slice(&size);
        out.extend_from_slice(&timestamp);
        out.extend_from_slice(data);
        Ok(out)
    }
}

/// `encodeInteger`: `BigInt(Math.floor(int))` as `setBigInt64` writes it (mod
/// 2^64, little-endian), trimmed to the fewest nonzero high bytes. `BigInt`
/// throws on non-finite input.
fn encode_integer(int: f64) -> Result<Vec<u8>, ExportError> {
    let floored = floor(int);
    if !floored.is_finite() {
        return Err(ExportError::Rejected);
    }
    let bytes = bigint_mod(floored).to_le_bytes();
    let mut len = 1;
    for i in (1..bytes.len()).rev() {
        if bytes[i] != 0 {
            len = i + 1;
            break;
        }
    }
    Ok(bytes[..len].to_vec())
}

/// `WPILOGEncoder`.
pub struct WpilogEncoder {
    extra_header: Vec<u8>,
    records: Vec<Record>,
}

impl WpilogEncoder {
    pub fn new(extra_header: &JsString) -> WpilogEncoder {
        WpilogEncoder {
            extra_header: text_encode(extra_header),
            records: Vec::new(),
        }
    }

    pub fn add(&mut self, record: Record) {
        self.records.push(record);
    }

    /// `getEncoded`: the header, then the records in insertion order or after a
    /// stable sort on the f64 microsecond timestamps, comparator included.
    pub fn get_encoded(&mut self, sort_by_timestamp: bool) -> Result<Vec<u8>, ExportError> {
        if sort_by_timestamp {
            array_sort(&mut self.records, |a, b| a.timestamp - b.timestamp);
        }
        let mut out =
            Vec::with_capacity(HEADER_STRING.len() + 2 + 4 + self.extra_header.len());
        out.extend_from_slice(HEADER_STRING);
        out.extend_from_slice(&HEADER_VERSION.to_le_bytes());
        out.extend_from_slice(&(self.extra_header.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.extra_header);
        for record in &self.records {
            out.extend_from_slice(&record.encode()?);
        }
        Ok(out)
    }
}
