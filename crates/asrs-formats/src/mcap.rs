// Ported from @mcap/core 1.3.0 (MIT, Copyright (c) Foxglove Technologies Inc): McapWriter.ts,
// McapRecordBuilder.ts, ChunkBuilder.ts, BufferBuilder.ts, and the CRC-32 of @foxglove/crc
// 0.0.3 (MIT). See THIRD_PARTY_NOTICES.md.

//! The `McapWriter` that `exportWorker.ts` constructs as `new McapWriter({ writable })`: every
//! option at its default, so records are chunked at 1 MiB without compression, with message
//! indexes, chunk indexes, statistics, repeated schemas and channels in the summary, and
//! summary offsets. Attachments, metadata and the other options are never used upstream and
//! are not ported. The writable is a growing buffer whose position is its length, as
//! upstream's `TempBuffer` is.
//!
//! Every method is synchronous: upstream's awaits only yield around `TempBuffer.write`, which
//! completes synchronously, so a caller that awaits each call (or never fills a chunk) sees
//! exactly this order of writes.

use indexmap::IndexMap;
use std::collections::HashSet;

use asrs_js::string::{JsString, text_encode};

const MAGIC: [u8; 8] = [137, 77, 67, 65, 80, 48, 13, 10];
const CHUNK_SIZE: usize = 1024 * 1024;

const HEADER: u8 = 0x01;
const FOOTER: u8 = 0x02;
const SCHEMA: u8 = 0x03;
const CHANNEL: u8 = 0x04;
const MESSAGE: u8 = 0x05;
const CHUNK: u8 = 0x06;
const MESSAGE_INDEX: u8 = 0x07;
const CHUNK_INDEX: u8 = 0x08;
const ATTACHMENT_INDEX: u8 = 0x0a;
const STATISTICS: u8 = 0x0b;
const METADATA_INDEX: u8 = 0x0d;
const SUMMARY_OFFSET: u8 = 0x0e;
const DATA_END: u8 = 0x0f;

/// A record timestamp: `BigInt(x)` of an integral double. Comparisons between two such
/// BigInts agree with comparisons of the doubles, and the record field is
/// `setBigUint64`'s `ToBigUint64`, see [`big_uint64`].
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct Time(f64);

impl Time {
    /// `BigInt(x)`, which throws `RangeError` for a non-integral or non-finite `x`.
    pub fn from_integral(x: f64) -> Option<Time> {
        (x.is_finite() && x.trunc() == x).then_some(Time(x))
    }
}

/// `ToBigUint64(BigInt(x))` for an integral finite `x`: the integer modulo 2^64.
fn big_uint64(Time(x): Time) -> u64 {
    if x == 0.0 {
        return 0;
    }
    let bits = x.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32 - 1075;
    let mantissa = (bits & ((1 << 52) - 1)) | (1 << 52);
    let magnitude = if exponent >= 64 {
        0
    } else if exponent >= 0 {
        (u128::from(mantissa) << exponent) as u64
    } else {
        mantissa >> -exponent
    };
    if x < 0.0 {
        magnitude.wrapping_neg()
    } else {
        magnitude
    }
}

pub struct Schema {
    pub id: u32,
    pub name: JsString,
    pub encoding: JsString,
    pub data: Vec<u8>,
}

pub struct Channel {
    pub id: u32,
    /// `None` is a `schemaId` of `undefined`, written as 0 by `setUint16` but never found
    /// when the first message looks the schema up.
    pub schema_id: Option<u32>,
    /// `TextEncoder.encode(topic)`: an `undefined` topic encodes as the empty string.
    pub topic: Vec<u8>,
    pub message_encoding: JsString,
    pub metadata: Vec<(JsString, JsString)>,
}

pub struct Message {
    pub channel_id: u32,
    /// `setUint32` of the sequence number, so already reduced modulo 2^32.
    pub sequence: u32,
    pub log_time: Time,
    pub publish_time: Time,
    pub data: Vec<u8>,
}

/// The `Error`s `addMessage` throws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McapError {
    MissingChannel,
    MissingSchema,
}

/// `BufferBuilder` with `McapRecordBuilder`'s record writers on top.
#[derive(Default)]
struct RecordBuilder {
    buf: Vec<u8>,
}

impl RecordBuilder {
    fn u8(&mut self, value: u8) {
        self.buf.push(value);
    }
    fn u16(&mut self, value: u16) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }
    fn u32(&mut self, value: u32) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }
    fn time(&mut self, value: Time) {
        self.u64(big_uint64(value));
    }
    fn bytes_prefixed(&mut self, bytes: &[u8]) {
        self.u32(bytes.len() as u32);
        self.buf.extend_from_slice(bytes);
    }
    fn string(&mut self, value: &JsString) {
        self.bytes_prefixed(&text_encode(value));
    }

    /// The opcode, a length placeholder, `body`, then the length patched in. Returns the
    /// record's full byte length as the `write*` methods do.
    fn record(&mut self, opcode: u8, body: impl FnOnce(&mut Self)) -> u64 {
        self.u8(opcode);
        let start = self.buf.len();
        self.u64(0);
        body(self);
        let end = self.buf.len();
        self.buf[start..start + 8].copy_from_slice(&((end - start - 8) as u64).to_le_bytes());
        (end - start + 1) as u64
    }

    fn string_map(&mut self, pairs: &[(JsString, JsString)]) {
        let start = self.buf.len();
        self.u32(0);
        for (key, value) in pairs {
            self.string(key);
            self.string(value);
        }
        let length = (self.buf.len() - start - 4) as u32;
        self.buf[start..start + 4].copy_from_slice(&length.to_le_bytes());
    }

    fn write_schema(&mut self, schema: &Schema) -> u64 {
        self.record(SCHEMA, |b| {
            b.u16(schema.id as u16);
            b.string(&schema.name);
            b.string(&schema.encoding);
            b.bytes_prefixed(&schema.data);
        })
    }

    fn write_channel(&mut self, channel: &Channel) -> u64 {
        self.record(CHANNEL, |b| {
            b.u16(channel.id as u16);
            b.u16(channel.schema_id.unwrap_or(0) as u16);
            b.bytes_prefixed(&channel.topic);
            b.string(&channel.message_encoding);
            b.string_map(&channel.metadata);
        })
    }

    fn write_message(&mut self, message: &Message) {
        self.record(MESSAGE, |b| {
            b.u16(message.channel_id as u16);
            b.u32(message.sequence);
            b.time(message.log_time);
            b.time(message.publish_time);
            b.buf.extend_from_slice(&message.data);
        });
    }
}

#[derive(Default)]
struct ChunkBuilder {
    records: RecordBuilder,
    message_indices: IndexMap<u32, Vec<(Time, u64)>>,
    total_message_count: usize,
    message_start_time: Time,
    message_end_time: Time,
}

impl ChunkBuilder {
    fn add_channel(&mut self, channel: &Channel) {
        self.message_indices.entry(channel.id).or_default();
        self.records.write_channel(channel);
    }

    fn add_message(&mut self, message: &Message) {
        if self.total_message_count == 0 || message.log_time < self.message_start_time {
            self.message_start_time = message.log_time;
        }
        if self.total_message_count == 0 || message.log_time > self.message_end_time {
            self.message_end_time = message.log_time;
        }
        let offset = self.records.buf.len() as u64;
        self.message_indices
            .entry(message.channel_id)
            .or_default()
            .push((message.log_time, offset));
        self.total_message_count += 1;
        self.records.write_message(message);
    }

    fn reset(&mut self) {
        self.message_start_time = Time::default();
        self.message_end_time = Time::default();
        self.total_message_count = 0;
        self.message_indices.clear();
        self.records.buf.clear();
    }
}

struct ChunkIndex {
    message_start_time: Time,
    message_end_time: Time,
    chunk_start_offset: u64,
    chunk_length: u64,
    message_index_offsets: Vec<(u32, u64)>,
    message_index_length: u64,
    compressed_size: u64,
    uncompressed_size: u64,
}

#[derive(Default)]
struct Statistics {
    message_count: u64,
    schema_count: u32,
    channel_count: u32,
    chunk_count: u32,
    message_start_time: Time,
    message_end_time: Time,
    channel_message_counts: IndexMap<u32, u64>,
}

pub struct McapWriter {
    next_channel_id: u32,
    next_schema_id: u32,
    out: Vec<u8>,
    record_writer: RecordBuilder,
    schemas: Vec<Schema>,
    channels: Vec<Channel>,
    written_schema_ids: HashSet<u32>,
    written_channel_ids: HashSet<u32>,
    chunk_builder: ChunkBuilder,
    data_section_crc: u32,
    statistics: Statistics,
    chunk_indices: Vec<ChunkIndex>,
}

impl Default for McapWriter {
    fn default() -> Self {
        McapWriter {
            next_channel_id: 0,
            next_schema_id: 1,
            out: Vec::new(),
            record_writer: RecordBuilder::default(),
            schemas: Vec::new(),
            channels: Vec::new(),
            written_schema_ids: HashSet::new(),
            written_channel_ids: HashSet::new(),
            chunk_builder: ChunkBuilder::default(),
            data_section_crc: crc32_init(),
            statistics: Statistics::default(),
            chunk_indices: Vec::new(),
        }
    }
}

impl McapWriter {
    pub fn new() -> Self {
        Self::default()
    }

    fn position(&self) -> u64 {
        self.out.len() as u64
    }

    /// `crc = crc32Update(crc, recordWriter.buffer); writable.write(...); recordWriter.reset()`.
    fn flush(&mut self, crc: &mut u32) {
        *crc = crc32_update(*crc, &self.record_writer.buf);
        self.out.extend_from_slice(&self.record_writer.buf);
        self.record_writer.buf.clear();
    }

    fn flush_data(&mut self) {
        let mut crc = self.data_section_crc;
        self.flush(&mut crc);
        self.data_section_crc = crc;
    }

    pub fn start(&mut self, profile: &JsString, library: &JsString) {
        self.record_writer.buf.extend_from_slice(&MAGIC);
        self.record_writer.record(HEADER, |b| {
            b.string(profile);
            b.string(library);
        });
        self.flush_data();
    }

    pub fn register_schema(&mut self, name: JsString, encoding: JsString, data: Vec<u8>) -> u32 {
        let id = self.next_schema_id;
        self.next_schema_id += 1;
        self.schemas.push(Schema {
            id,
            name,
            encoding,
            data,
        });
        self.statistics.schema_count += 1;
        id
    }

    pub fn register_channel(
        &mut self,
        schema_id: Option<u32>,
        topic: Vec<u8>,
        message_encoding: JsString,
        metadata: Vec<(JsString, JsString)>,
    ) -> u32 {
        let id = self.next_channel_id;
        self.next_channel_id += 1;
        self.channels.push(Channel {
            id,
            schema_id,
            topic,
            message_encoding,
            metadata,
        });
        self.statistics.channel_count += 1;
        id
    }

    /// Returns whether the appended records crossed the chunk bound: upstream's `addMessage`
    /// then awaits `finalizeChunk`, which is where an unawaited call parks.
    pub fn add_message(&mut self, message: Message) -> Result<bool, McapError> {
        let stats = &mut self.statistics;
        if stats.message_count == 0 {
            stats.message_start_time = message.log_time;
            stats.message_end_time = message.log_time;
        } else {
            if message.log_time < stats.message_start_time {
                stats.message_start_time = message.log_time;
            }
            if message.log_time > stats.message_end_time {
                stats.message_end_time = message.log_time;
            }
        }
        *stats
            .channel_message_counts
            .entry(message.channel_id)
            .or_insert(0) += 1;
        stats.message_count += 1;

        if !self.written_channel_ids.contains(&message.channel_id) {
            let channel = self
                .channels
                .iter()
                .find(|channel| channel.id == message.channel_id)
                .ok_or(McapError::MissingChannel)?;
            if channel.schema_id != Some(0)
                && !channel
                    .schema_id
                    .is_some_and(|id| self.written_schema_ids.contains(&id))
            {
                let schema = channel
                    .schema_id
                    .and_then(|id| self.schemas.iter().find(|schema| schema.id == id))
                    .ok_or(McapError::MissingSchema)?;
                self.chunk_builder.records.write_schema(schema);
                self.written_schema_ids.insert(schema.id);
            }
            self.chunk_builder.add_channel(channel);
            self.written_channel_ids.insert(message.channel_id);
        }

        self.chunk_builder.add_message(&message);
        if self.chunk_builder.records.buf.len() > CHUNK_SIZE {
            self.finalize_chunk();
            return Ok(true);
        }
        Ok(false)
    }

    fn finalize_chunk(&mut self) {
        if self.chunk_builder.total_message_count == 0 {
            return;
        }
        self.statistics.chunk_count += 1;

        let chunk = &self.chunk_builder;
        let records = &chunk.records.buf;
        let uncompressed_size = records.len() as u64;
        let uncompressed_crc = crc32_final(crc32_update(crc32_init(), records));
        let (message_start_time, message_end_time) =
            (chunk.message_start_time, chunk.message_end_time);

        let chunk_start_offset = self.position();
        let chunk_length = self.record_writer.record(CHUNK, |b| {
            b.time(message_start_time);
            b.time(message_end_time);
            b.u64(uncompressed_size);
            b.u32(uncompressed_crc);
            b.string(&JsString::new());
            b.u64(uncompressed_size);
            b.buf.extend_from_slice(records);
        });
        self.flush_data();

        let message_index_start = self.position();
        let mut message_index_length = 0u64;
        let mut message_index_offsets = Vec::new();
        for (&channel_id, entries) in &self.chunk_builder.message_indices {
            message_index_offsets.push((channel_id, message_index_start + message_index_length));
            message_index_length += self.record_writer.record(MESSAGE_INDEX, |b| {
                b.u16(channel_id as u16);
                b.u32((entries.len() * 16) as u32);
                for &(time, offset) in entries {
                    b.time(time);
                    b.u64(offset);
                }
            });
        }

        self.chunk_indices.push(ChunkIndex {
            message_start_time,
            message_end_time,
            chunk_start_offset,
            chunk_length,
            message_index_offsets,
            message_index_length,
            compressed_size: uncompressed_size,
            uncompressed_size,
        });
        self.chunk_builder.reset();
        self.flush_data();
    }

    pub fn end(mut self) -> Vec<u8> {
        self.finalize_chunk();
        self.flush_data();

        let data_section_crc = crc32_final(self.data_section_crc);
        self.record_writer
            .record(DATA_END, |b| b.u32(data_section_crc));
        self.flush(&mut 0);

        let mut summary_offsets: Vec<(u8, u64, u64)> = Vec::new();
        let summary_start = self.position();
        let mut summary_crc = crc32_init();

        let schema_start = self.position();
        let mut schema_length = 0;
        for schema in &self.schemas {
            schema_length += self.record_writer.write_schema(schema);
        }
        summary_offsets.push((SCHEMA, schema_start, schema_length));

        self.flush(&mut summary_crc);
        let channel_start = self.position();
        let mut channel_length = 0;
        for channel in &self.channels {
            channel_length += self.record_writer.write_channel(channel);
        }
        summary_offsets.push((CHANNEL, channel_start, channel_length));

        self.flush(&mut summary_crc);
        let statistics_start = self.position();
        let stats = &self.statistics;
        let statistics_length = self.record_writer.record(STATISTICS, |b| {
            b.u64(stats.message_count);
            b.u16(stats.schema_count as u16);
            b.u32(stats.channel_count);
            b.u32(0);
            b.u32(0);
            b.u32(stats.chunk_count);
            b.time(stats.message_start_time);
            b.time(stats.message_end_time);
            b.u32((stats.channel_message_counts.len() * 10) as u32);
            for (&channel_id, &count) in &stats.channel_message_counts {
                b.u16(channel_id as u16);
                b.u64(count);
            }
        });
        summary_offsets.push((STATISTICS, statistics_start, statistics_length));

        self.flush(&mut summary_crc);
        self.flush(&mut summary_crc);
        summary_offsets.push((METADATA_INDEX, self.position(), 0));
        self.flush(&mut summary_crc);
        summary_offsets.push((ATTACHMENT_INDEX, self.position(), 0));

        self.flush(&mut summary_crc);
        let chunk_index_start = self.position();
        let mut chunk_index_length = 0;
        for index in &self.chunk_indices {
            chunk_index_length += self.record_writer.record(CHUNK_INDEX, |b| {
                b.time(index.message_start_time);
                b.time(index.message_end_time);
                b.u64(index.chunk_start_offset);
                b.u64(index.chunk_length);
                b.u32((index.message_index_offsets.len() * 10) as u32);
                for &(channel_id, offset) in &index.message_index_offsets {
                    b.u16(channel_id as u16);
                    b.u64(offset);
                }
                b.u64(index.message_index_length);
                b.string(&JsString::new());
                b.u64(index.compressed_size);
                b.u64(index.uncompressed_size);
            });
        }
        summary_offsets.push((CHUNK_INDEX, chunk_index_start, chunk_index_length));

        self.flush(&mut summary_crc);
        let summary_offset_start = self.position();
        let summary_length = summary_offset_start - summary_start;

        for &(opcode, start, length) in &summary_offsets {
            if length != 0 {
                self.record_writer.record(SUMMARY_OFFSET, |b| {
                    b.u8(opcode);
                    b.u64(start);
                    b.u64(length);
                });
            }
        }
        summary_crc = crc32_update(summary_crc, &self.record_writer.buf);

        let footer_summary_start = if summary_length == 0 {
            0
        } else {
            summary_start
        };
        let mut footer_prefix = [0u8; 25];
        footer_prefix[0] = FOOTER;
        footer_prefix[1..9].copy_from_slice(&20u64.to_le_bytes());
        footer_prefix[9..17].copy_from_slice(&footer_summary_start.to_le_bytes());
        footer_prefix[17..25].copy_from_slice(&summary_offset_start.to_le_bytes());
        summary_crc = crc32_final(crc32_update(summary_crc, &footer_prefix));

        let b = &mut self.record_writer;
        b.buf.extend_from_slice(&footer_prefix);
        b.u32(summary_crc);
        b.buf.extend_from_slice(&MAGIC);
        self.flush(&mut 0);
        self.out
    }
}

const fn crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut r = i as u32;
        let mut bit = 0;
        while bit < 8 {
            r = ((r & 1) * 0xedb8_8320) ^ (r >> 1);
            bit += 1;
        }
        table[i] = r;
        i += 1;
    }
    table
}

const CRC32_TABLE: [u32; 256] = crc32_table();

fn crc32_init() -> u32 {
    !0
}

fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &byte in data {
        crc = CRC32_TABLE[((crc ^ u32::from(byte)) & 0xff) as usize] ^ (crc >> 8);
    }
    crc
}

fn crc32_final(crc: u32) -> u32 {
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_check_value() {
        assert_eq!(
            crc32_final(crc32_update(crc32_init(), b"123456789")),
            0xcbf4_3926
        );
    }

    #[test]
    fn big_uint64_wraps_like_to_big_uint64() {
        let t = |x: f64| big_uint64(Time::from_integral(x).unwrap());
        assert_eq!(t(0.0), 0);
        assert_eq!(t(-0.0), 0);
        assert_eq!(t(1.5e9), 1_500_000_000);
        assert_eq!(t(-1.0), u64::MAX);
        assert_eq!(t(18_446_744_073_709_551_616.0), 0);
        assert_eq!(t(18_446_744_073_709_555_712.0), 4096);
        assert_eq!(t(1e300), 0);
        assert_eq!(t(-9_223_372_036_854_775_808.0), 1 << 63);
        assert_eq!(Time::from_integral(f64::INFINITY), None);
        assert_eq!(Time::from_integral(0.5), None);
    }
}
