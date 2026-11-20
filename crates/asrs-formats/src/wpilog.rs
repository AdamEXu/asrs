//! Port of `src/hub/dataSources/wpilog/WPILOGDecoder.ts`, `WPILOGShared.ts` and
//! `indexer/wpilogIndexer.c`.

use asrs_js::string::JsString;
use asrs_js::string::encoding::TextDecoder;

pub const HEADER_STRING: &[u8; 6] = b"WPILOG";
pub const HEADER_VERSION: u16 = 0x0100;
pub const CONTROL_ENTRY: u32 = 0;
pub const CONTROL_START: u8 = 0;
pub const CONTROL_FINISH: u8 = 1;
pub const CONTROL_SET_METADATA: u8 = 2;

fn utf8_decode(bytes: &[u8]) -> JsString {
    TextDecoder::new(None, false, false)
        .expect("utf-8 is always supported")
        .decode(bytes)
        .expect("non-fatal decode never fails")
}

/// The two kinds of values upstream throws: plain strings and the `RangeError`
/// `DataView` raises for out-of-bounds reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WpilogError {
    Message(&'static str),
    DataViewOutOfBounds,
}

pub struct StartRecordData {
    pub entry: u32,
    pub name: JsString,
    pub type_: JsString,
    pub metadata: JsString,
}

pub struct MetadataRecordData {
    pub entry: u32,
    pub metadata: JsString,
}

/// A record in the data log. May represent either a control record or a data record.
pub struct WpilogRecord<'a> {
    entry: u32,
    timestamp: f64,
    data: &'a [u8],
}

impl<'a> WpilogRecord<'a> {
    pub fn entry(&self) -> u32 {
        self.entry
    }

    pub fn timestamp(&self) -> f64 {
        self.timestamp
    }

    pub fn is_control(&self) -> bool {
        self.entry == CONTROL_ENTRY
    }

    fn control_type(&self) -> u8 {
        self.data[0]
    }

    pub fn is_start(&self) -> bool {
        self.is_control() && self.data.len() >= 17 && self.control_type() == CONTROL_START
    }

    pub fn is_finish(&self) -> bool {
        self.is_control() && self.data.len() == 5 && self.control_type() == CONTROL_FINISH
    }

    pub fn is_set_metadata(&self) -> bool {
        self.is_control() && self.data.len() >= 9 && self.control_type() == CONTROL_SET_METADATA
    }

    pub fn start_data(&self) -> Result<StartRecordData, WpilogError> {
        if !self.is_start() {
            return Err(WpilogError::Message("Not a start control record"));
        }
        let entry = u32_at(self.data, 1)?;
        let (name, position) = self.inner_string(5)?;
        let (type_, position) = self.inner_string(position)?;
        let (metadata, _) = self.inner_string(position)?;
        Ok(StartRecordData {
            entry,
            name,
            type_,
            metadata,
        })
    }

    pub fn finish_entry(&self) -> Result<u32, WpilogError> {
        if !self.is_finish() {
            return Err(WpilogError::Message("Not a finish control record"));
        }
        u32_at(self.data, 1)
    }

    pub fn set_metadata_data(&self) -> Result<MetadataRecordData, WpilogError> {
        if !self.is_set_metadata() {
            return Err(WpilogError::Message("Not a set metadata control record"));
        }
        Ok(MetadataRecordData {
            entry: u32_at(self.data, 1)?,
            metadata: self.inner_string(5)?.0,
        })
    }

    pub fn raw(&self) -> Vec<u8> {
        self.data.to_vec()
    }

    pub fn boolean(&self) -> Result<bool, WpilogError> {
        if self.data.len() != 1 {
            return Err(WpilogError::Message("Not a boolean"));
        }
        Ok(self.data[0] != 0)
    }

    pub fn integer(&self) -> Result<f64, WpilogError> {
        if self.data.len() != 8 {
            return Err(WpilogError::Message("Not an integer"));
        }
        Ok(i64::from_le_bytes(self.data[..8].try_into().unwrap()) as f64)
    }

    pub fn float(&self) -> Result<f64, WpilogError> {
        if self.data.len() != 4 {
            return Err(WpilogError::Message("Not a float"));
        }
        Ok(f64::from(f32::from_le_bytes(
            self.data[..4].try_into().unwrap(),
        )))
    }

    pub fn double(&self) -> Result<f64, WpilogError> {
        if self.data.len() != 8 {
            return Err(WpilogError::Message("Not a double"));
        }
        Ok(f64::from_le_bytes(self.data[..8].try_into().unwrap()))
    }

    pub fn string(&self) -> JsString {
        utf8_decode(self.data)
    }

    pub fn boolean_array(&self) -> Vec<bool> {
        self.data.iter().map(|x| *x != 0).collect()
    }

    pub fn integer_array(&self) -> Result<Vec<f64>, WpilogError> {
        if !self.data.len().is_multiple_of(8) {
            return Err(WpilogError::Message("Not an integer array"));
        }
        Ok(self
            .data
            .as_chunks::<8>()
            .0
            .iter()
            .map(|&c| i64::from_le_bytes(c) as f64)
            .collect())
    }

    pub fn float_array(&self) -> Result<Vec<f64>, WpilogError> {
        if !self.data.len().is_multiple_of(4) {
            return Err(WpilogError::Message("Not a float array"));
        }
        Ok(self
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&c| f64::from(f32::from_le_bytes(c)))
            .collect())
    }

    pub fn double_array(&self) -> Result<Vec<f64>, WpilogError> {
        if !self.data.len().is_multiple_of(8) {
            return Err(WpilogError::Message("Not a double array"));
        }
        Ok(self
            .data
            .as_chunks::<8>()
            .0
            .iter()
            .map(|&c| f64::from_le_bytes(c))
            .collect())
    }

    pub fn string_array(&self) -> Result<Vec<JsString>, WpilogError> {
        let size = u32_at(self.data, 0)?;
        if u64::from(size) > (self.data.len() - 4) as u64 / 4 {
            return Err(WpilogError::Message("Not a string array"));
        }
        let mut array = Vec::new();
        let mut position = 4usize;
        for _ in 0..size {
            let (string, next) = self.inner_string(position)?;
            array.push(string);
            position = next;
        }
        Ok(array)
    }

    fn inner_string(&self, position: usize) -> Result<(JsString, usize), WpilogError> {
        let size = u32_at(self.data, position)? as usize;
        let end = position + 4 + size;
        if end > self.data.len() {
            return Err(WpilogError::Message("Invalid string size"));
        }
        Ok((utf8_decode(&self.data[position + 4..end]), end))
    }
}

fn u32_at(data: &[u8], position: usize) -> Result<u32, WpilogError> {
    match data.get(position..position.saturating_add(4)) {
        Some(bytes) => Ok(u32::from_le_bytes(bytes.try_into().unwrap())),
        None => Err(WpilogError::DataViewOutOfBounds),
    }
}

/// WPILOG decoder.
pub struct WpilogDecoder<'a> {
    data: &'a [u8],
}

impl<'a> WpilogDecoder<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    /// True if the data log is valid (e.g. has a valid header).
    pub fn is_valid(&self) -> bool {
        self.data.len() >= 12
            && utf8_decode(&self.data[..6]).as_string().as_deref() == Some("WPILOG")
            && self.version() == HEADER_VERSION
    }

    /// Data log version; 0 if the log is too short to hold a header.
    pub fn version(&self) -> u16 {
        if self.data.len() < 12 {
            return 0;
        }
        u16::from_le_bytes([self.data[6], self.data[7]])
    }

    pub fn extra_header(&self) -> JsString {
        if self.data.len() < 12 {
            return JsString::new();
        }
        let size = u32::from_le_bytes(self.data[8..12].try_into().unwrap()) as usize;
        utf8_decode(&self.data[12..(12 + size).min(self.data.len())])
    }

    /// `readVariableInteger`: up to 8 little-endian bytes with the eighth byte's
    /// high bit treated as a sign bit, as a JS `number`.
    fn variable_integer(&self, position: usize, length: usize) -> f64 {
        let mut value: u64 = 0;
        for i in 0..length.min(8) {
            value |= u64::from(self.data[position + i]) << (i * 8);
        }
        if length.min(8) == 8 {
            value as i64 as f64
        } else {
            value as f64
        }
    }

    /// `forEach`: callback receives the record and the position after it.
    /// Throws `WpilogError::Message("Log is not valid")` for an invalid header.
    pub fn for_each(
        &self,
        mut callback: impl FnMut(&WpilogRecord<'a>, usize),
    ) -> Result<(), WpilogError> {
        if !self.is_valid() {
            return Err(WpilogError::Message("Log is not valid"));
        }
        let extra_header_size = u32::from_le_bytes(self.data[8..12].try_into().unwrap()) as usize;
        let mut position = 12 + extra_header_size;
        loop {
            let Some((record, size)) = self.record_at(position) else {
                return Ok(());
            };
            position += size;
            callback(&record, position);
        }
    }

    /// The record stored at `position`, and the record's size in bytes.
    /// Returns `None` for truncated or malformed records.
    pub fn record_at(&self, position: usize) -> Option<(WpilogRecord<'a>, usize)> {
        let remaining = self.data.len().checked_sub(position)?;
        if remaining < 4 {
            return None;
        }
        let bitfield = self.data[position];
        let entry_length = usize::from(bitfield & 0x3) + 1;
        let size_length = usize::from((bitfield >> 2) & 0x3) + 1;
        let timestamp_length = usize::from((bitfield >> 4) & 0x7) + 1;
        let header_length = 1 + entry_length + size_length + timestamp_length;
        if remaining < header_length {
            return None;
        }
        let entry = self.variable_integer(position + 1, entry_length);
        let size = self.variable_integer(position + 1 + entry_length, size_length);
        let timestamp =
            self.variable_integer(position + 1 + entry_length + size_length, timestamp_length);
        if entry < 0.0 || size < 0.0 || (remaining - header_length) < size as usize {
            return None;
        }
        let data_start = position + header_length;
        Some((
            WpilogRecord {
                entry: entry as u32,
                timestamp,
                data: &self.data[data_start..data_start + size as usize],
            },
            header_length + size as usize,
        ))
    }
}

/// The C indexer's WASM trap is reported as this error: the indexer can try to
/// read at negative offsets (a negative extra-header size) or past the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexError;

/// Output of `wpilogIndexer.c`'s `run` as the worker consumes it: the timestamp
/// range callback values and the `(entry, position)` pairs.
#[derive(Debug)]
pub struct WpilogIndex {
    pub min_timestamp: f64,
    pub max_timestamp: f64,
    pub records: Vec<(u32, u32)>,
}

/// The read at `offset` is in bounds when `0 <= offset < size`; a negative
/// offset is the wasm case that traps (the oracle cannot produce readable heap
/// bytes there deterministically).
fn byte_at(data: &[u8], offset: i32) -> Result<u8, IndexError> {
    if offset < 0 || offset as usize >= data.len() {
        Err(IndexError)
    } else {
        Ok(data[offset as usize])
    }
}

/// `readVarInt`: `length` little-endian bytes accumulated into `i64`; the eighth
/// byte shifts into the sign bit (the C code has no explicit sign handling).
fn varint_at(data: &[u8], offset: i32, length: usize) -> Result<i64, IndexError> {
    let mut value: i64 = 0;
    for i in 0..length {
        let byte = byte_at(data, offset + i as i32)?;
        value = value.wrapping_add((byte as i64) << (8 * i));
    }
    Ok(value)
}

/// The extra-header read at bytes 8..12 runs before any bounds check; for files
/// shorter than 12 bytes the oracle's heap reads zero at those positions.
fn extra_header_size(data: &[u8]) -> i32 {
    i32::from_le_bytes([
        data.get(8).copied().unwrap_or(0),
        data.get(9).copied().unwrap_or(0),
        data.get(10).copied().unwrap_or(0),
        data.get(11).copied().unwrap_or(0),
    ])
}

pub fn index(data: &[u8]) -> Result<WpilogIndex, IndexError> {
    let buffer_size = data.len() as i32;
    let mut offset: i32 = 12i32.wrapping_add(extra_header_size(data));
    let mut has_initial_timestamp = false;
    let mut min_timestamp: i64 = 0;
    let mut max_timestamp: i64 = 0;
    let mut records: Vec<(u32, u32)> = Vec::new();

    while offset.wrapping_add(4) <= buffer_size {
        let length_bitfield = byte_at(data, offset)?;
        let entry_length = usize::from(length_bitfield & 0x3) + 1;
        let data_size_length = usize::from((length_bitfield >> 2) & 0x3) + 1;
        let timestamp_length = usize::from((length_bitfield >> 4) & 0x7) + 1;
        let header_length = (1 + entry_length + data_size_length + timestamp_length) as i32;
        if buffer_size < offset.wrapping_add(header_length) {
            break;
        }

        let entry = varint_at(data, offset + 1, entry_length)? as u32;
        let data_size = varint_at(data, offset + 1 + entry_length as i32, data_size_length)? as u32;
        let timestamp = varint_at(
            data,
            offset + 1 + entry_length as i32 + data_size_length as i32,
            timestamp_length,
        )?;
        // The C check is `bufferSize < offset + headerLength + dataSize` in
        // mixed int/uint32_t arithmetic.
        let end = (offset as u32)
            .wrapping_add(header_length as u32)
            .wrapping_add(data_size);
        if (buffer_size as u32) < end {
            break;
        }

        if entry != CONTROL_ENTRY {
            if !has_initial_timestamp || timestamp < min_timestamp {
                min_timestamp = timestamp;
            }
            if !has_initial_timestamp || timestamp > max_timestamp {
                max_timestamp = timestamp;
            }
            has_initial_timestamp = true;
        }

        records.push((entry, offset as u32));
        offset = offset
            .wrapping_add(header_length)
            .wrapping_add(data_size as i32);
    }

    Ok(WpilogIndex {
        min_timestamp: min_timestamp as f64 * 1.0e-6,
        max_timestamp: max_timestamp as f64 * 1.0e-6,
        records,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(extra: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"WPILOG");
        v.extend_from_slice(&HEADER_VERSION.to_le_bytes());
        v.extend_from_slice(&(extra.len() as u32).to_le_bytes());
        v.extend_from_slice(extra);
        v
    }

    /// One record: entry u8, size u8, timestamp u8 bitfield 0x00.
    fn record(entry: u8, timestamp: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![0x00, entry, payload.len() as u8, timestamp];
        v.extend_from_slice(payload);
        v
    }

    fn start_record(entry: u32, name: &str, type_: &str, metadata: &str) -> Vec<u8> {
        let mut payload = vec![CONTROL_START];
        payload.extend_from_slice(&entry.to_le_bytes());
        for s in [name, type_, metadata] {
            payload.extend_from_slice(&(s.len() as u32).to_le_bytes());
            payload.extend_from_slice(s.as_bytes());
        }
        let mut v = vec![0x11, 0, 0, payload.len() as u8, 0, 0];
        v.extend_from_slice(&payload);
        v
    }

    #[test]
    fn valid_header() {
        let data = header(b"extra");
        let d = WpilogDecoder::new(&data);
        assert!(d.is_valid());
        assert_eq!(d.version(), 0x0100);
        assert_eq!(d.extra_header().to_string_lossy(), "extra");
    }

    #[test]
    fn invalid_headers() {
        assert!(!WpilogDecoder::new(b"").is_valid());
        assert!(!WpilogDecoder::new(b"WPILOG\x00\x01").is_valid());
        let mut wrong = header(b"");
        wrong[0] = b'w';
        assert!(!WpilogDecoder::new(&wrong).is_valid());
        let mut version = header(b"");
        version[6] = 0x02;
        version[7] = 0x00;
        assert!(!WpilogDecoder::new(&version).is_valid());
        assert_eq!(WpilogDecoder::new(b"WPI").version(), 0);
        assert_eq!(
            WpilogDecoder::new(b"WPI").extra_header().to_string_lossy(),
            ""
        );
    }

    #[test]
    fn for_each_counts_records() {
        let mut data = header(b"");
        data.extend_from_slice(&start_record(1, "a", "boolean", ""));
        data.extend_from_slice(&record(1, 100, &[1]));
        data.extend_from_slice(&record(1, 101, &[0]));
        let decoder = WpilogDecoder::new(&data);
        let mut count = 0;
        decoder.for_each(|_, _| count += 1).unwrap();
        assert_eq!(count, 3);
        assert!(WpilogDecoder::new(b"bad").for_each(|_, _| {}).is_err());
    }

    #[test]
    fn control_records() {
        let mut data = header(b"");
        data.extend_from_slice(&start_record(7, "n", "boolean", "m"));
        let decoder = WpilogDecoder::new(&data);
        let (record, _) = decoder.record_at(12).unwrap();
        assert!(record.is_control());
        assert!(record.is_start());
        let start = record.start_data().unwrap();
        assert_eq!(start.entry, 7);
        assert_eq!(start.name.to_string_lossy(), "n");
        assert_eq!(start.metadata.to_string_lossy(), "m");
        assert_eq!(
            record.finish_entry(),
            Err(WpilogError::Message("Not a finish control record"))
        );
    }

    #[test]
    fn finish_and_metadata() {
        let mut finish = vec![CONTROL_FINISH];
        finish.extend_from_slice(&9u32.to_le_bytes());
        let mut meta = vec![CONTROL_SET_METADATA];
        meta.extend_from_slice(&3u32.to_le_bytes());
        meta.extend_from_slice(&3u32.to_le_bytes());
        meta.extend_from_slice(b"abc");
        let mut data = header(b"");
        data.extend_from_slice(&record(0, 0, &finish));
        data.extend_from_slice(&record(0, 0, &meta));
        let decoder = WpilogDecoder::new(&data);
        let (f, size) = decoder.record_at(12).unwrap();
        assert_eq!(f.finish_entry(), Ok(9));
        let (m, _) = decoder.record_at(12 + size).unwrap();
        let meta = m.set_metadata_data().unwrap();
        assert_eq!(meta.entry, 3);
        assert_eq!(meta.metadata.to_string_lossy(), "abc");
    }

    #[test]
    fn getters() {
        let mut data = header(b"");
        data.extend_from_slice(&record(1, 0, &[1]));
        data.extend_from_slice(&record(1, 0, &(-5i64).to_le_bytes()));
        data.extend_from_slice(&record(1, 0, &1.5f64.to_le_bytes()));
        // 1 string in an 8-byte payload: count passes (len-4)/4 but it overflows.
        let mut bad = 1u32.to_le_bytes().to_vec();
        bad.extend_from_slice(&7u32.to_le_bytes());
        data.extend_from_slice(&record(1, 0, &bad));
        let decoder = WpilogDecoder::new(&data);
        let (b, s1) = decoder.record_at(12).unwrap();
        assert_eq!(b.boolean(), Ok(true));
        assert_eq!(b.integer(), Err(WpilogError::Message("Not an integer")));
        let (i, s2) = decoder.record_at(12 + s1).unwrap();
        assert_eq!(i.integer(), Ok(-5.0));
        let (d, s3) = decoder.record_at(12 + s1 + s2).unwrap();
        assert_eq!(d.double(), Ok(1.5));
        let (r, _) = decoder.record_at(12 + s1 + s2 + s3).unwrap();
        assert_eq!(
            r.string_array(),
            Err(WpilogError::Message("Invalid string size"))
        );
    }

    #[test]
    fn string_array_bounds() {
        // size <= (len-4)/4 but a string runs past the end
        let mut payload = 1u32.to_le_bytes().to_vec();
        payload.extend_from_slice(&100u32.to_le_bytes());
        let mut data = header(b"");
        data.extend_from_slice(&record(1, 0, &payload));
        let decoder = WpilogDecoder::new(&data);
        let (r, _) = decoder.record_at(12).unwrap();
        assert_eq!(
            r.string_array(),
            Err(WpilogError::Message("Invalid string size"))
        );
        // shorter than 4 bytes: DataView throws a RangeError
        let mut data = header(b"");
        data.extend_from_slice(&record(1, 0, &[1, 2]));
        let decoder = WpilogDecoder::new(&data);
        let (r, _) = decoder.record_at(12).unwrap();
        assert_eq!(r.string_array(), Err(WpilogError::DataViewOutOfBounds));
    }

    #[test]
    fn truncated_records_stop() {
        let mut data = header(b"");
        data.extend_from_slice(&record(1, 0, &[7]));
        data.extend_from_slice(&record(2, 0, &[9]));
        for cut in [
            data.len() - 1,
            data.len() - 2,
            data.len() - 3,
            data.len() - 4,
        ] {
            let index = index(&data[..cut]).unwrap();
            assert_eq!(index.records.len(), 1);
        }
        let index = index(&data).unwrap();
        assert_eq!(index.records.len(), 2);
        assert_eq!(index.records[0], (1, 12));
        assert_eq!(index.records[1], (2, 17));
    }

    #[test]
    fn index_timestamps() {
        let mut data = header(b"");
        data.extend_from_slice(&record(1, 150, &[0])); // t=150us -> 0.00015s
        data.extend_from_slice(&record(0, 255, &[])); // control: ignored for range
        data.extend_from_slice(&record(2, 40, &[0]));
        let index = index(&data).unwrap();
        assert_eq!(index.records.len(), 3);
        assert_eq!(index.min_timestamp, 40f64 * 1.0e-6);
        assert_eq!(index.max_timestamp, 150f64 * 1.0e-6);
    }

    #[test]
    fn index_no_data_records() {
        let mut data = header(b"");
        data.extend_from_slice(&record(0, 255, &[])); // control only
        let index = index(&data).unwrap();
        assert_eq!(index.records.len(), 1);
        assert_eq!(index.min_timestamp, 0.0);
        assert_eq!(index.max_timestamp, 0.0);
    }

    #[test]
    fn index_sign_and_eight_byte_timestamps() {
        // timestampLength = 8: bitfield 0x70; the high bit of byte 8 is a sign
        // bit, so the timestamp is read as a signed int64.
        let mut data = header(b"");
        let mut r = vec![0x70u8, 1, 1];
        r.extend_from_slice(&(-1i64).to_le_bytes());
        r.push(0);
        data.extend_from_slice(&r);
        let index = index(&data).unwrap();
        assert_eq!(index.min_timestamp, -1e-6);
        let decoder = WpilogDecoder::new(&data);
        let (record, _) = decoder.record_at(12).unwrap();
        assert_eq!(record.timestamp(), -1.0);
    }

    #[test]
    fn index_extra_header_edges() {
        let mut data = header(b"");
        data[8..12].copy_from_slice(&(-1i32).to_le_bytes()); // extra = 0xFFFFFFFF
        // offset = 11 lands inside the file but before the first record.
        assert_eq!(index(&data).unwrap().records.len(), 0);
        data[8..12].copy_from_slice(&0x8000_0000u32.to_le_bytes()); // extra = -2^31
        assert!(index(&data).is_err());
    }
}
