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
//! Upstream's methods are async but `TempBuffer.write` completes synchronously, so an await only
//! yields to the microtask queue. A caller that awaits each call sees the writes in order.
//! [`McapWriter::add_message_unawaited`] leaves the rest of `finalizeChunk` queued, so that
//! later calls see the chunk and the record buffer before they are reset, as upstream's do;
//! [`McapWriter::register_channel`] and [`McapWriter::end`] are awaited and run the queue.

use indexmap::IndexMap;
use std::collections::{HashSet, VecDeque};

use asrs_js::string::{JsString, text_encode};

const MAGIC: [u8; 8] = [137, 77, 67, 65, 80, 48, 13, 10];
const CHUNK_SIZE: u64 = 1024 * 1024;

/// The longest `ArrayBuffer` Electron 39 allocates (`new ArrayBuffer` throws `RangeError` above
/// it), and so the longest file `TempBuffer` can hold. Recorded in `export/mcap-reentry`.
pub const ARRAY_BUFFER_MAX: u64 = 2_145_386_496;

/// `BufferBuilder` grows by half, so its record buffer cannot pass this. Upstream spends over
/// 1 TB of CRC work to get here, and what it does afterwards is not modelled: the export fails.
const RECORD_BUFFER_MAX: u64 = ARRAY_BUFFER_MAX / 3 * 2;

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

/// The `Error`s the writer throws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McapError {
    MissingChannel,
    MissingSchema,
    /// `RangeError: Array buffer allocation failed`.
    Allocation,
}

/// `BufferBuilder` with `McapRecordBuilder`'s record writers on top.
#[derive(Default)]
struct RecordBuilder {
    buf: Vec<u8>,
    len: u64,
    /// Tracks `len` without keeping the bytes.
    measuring: bool,
}

impl RecordBuilder {
    fn bytes(&mut self, bytes: &[u8]) {
        self.len += bytes.len() as u64;
        if !self.measuring {
            self.buf.extend_from_slice(bytes);
        }
    }
    fn reset(&mut self) {
        self.buf.clear();
        self.len = 0;
    }
    fn u8(&mut self, value: u8) {
        self.bytes(&[value]);
    }
    fn u16(&mut self, value: u16) {
        self.bytes(&value.to_le_bytes());
    }
    fn u32(&mut self, value: u32) {
        self.bytes(&value.to_le_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }
    fn time(&mut self, value: Time) {
        self.u64(big_uint64(value));
    }
    fn bytes_prefixed(&mut self, bytes: &[u8]) {
        self.u32(bytes.len() as u32);
        self.bytes(bytes);
    }
    fn string(&mut self, value: &JsString) {
        self.bytes_prefixed(&text_encode(value));
    }

    /// The opcode, a length placeholder, `body`, then the length patched in. Returns the
    /// record's full byte length as the `write*` methods do.
    fn record(&mut self, opcode: u8, body: impl FnOnce(&mut Self)) -> u64 {
        self.u8(opcode);
        let start = self.len;
        self.u64(0);
        body(self);
        let length = self.len - start;
        self.patch(start, &(length - 8).to_le_bytes());
        length + 1
    }

    fn patch(&mut self, at: u64, bytes: &[u8]) {
        if !self.measuring {
            self.buf[at as usize..][..bytes.len()].copy_from_slice(bytes);
        }
    }

    fn string_map(&mut self, pairs: &[(JsString, JsString)]) {
        let start = self.len;
        self.u32(0);
        for (key, value) in pairs {
            self.string(key);
            self.string(value);
        }
        let length = (self.len - start - 4) as u32;
        self.patch(start, &length.to_le_bytes());
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
            b.bytes(&message.data);
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
        let offset = self.records.len;
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
        self.records.reset();
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

/// What is left of a `finalizeChunk` call, queued behind the `await` it is stopped at.
enum Continuation {
    ChunkWritten(FinalizingChunk),
    IndexWritten,
}

struct FinalizingChunk {
    message_start_time: Time,
    message_end_time: Time,
    chunk_start_offset: u64,
    chunk_length: u64,
    uncompressed_size: u64,
}

pub struct McapWriter {
    next_channel_id: u32,
    next_schema_id: u32,
    out: Vec<u8>,
    written: u64,
    measuring: bool,
    queued: VecDeque<Continuation>,
    reentered: bool,
    exhausted: bool,
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
            written: 0,
            measuring: false,
            queued: VecDeque::new(),
            reentered: false,
            exhausted: false,
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

    /// A writer that keeps every length but no bytes, to see whether an export fits.
    pub fn measuring() -> Self {
        let mut writer = Self::default();
        writer.measuring = true;
        writer.record_writer.measuring = true;
        writer.chunk_builder.records.measuring = true;
        writer
    }

    /// Whether a `finalizeChunk` started while another had not reset the chunk yet.
    pub fn reentered(&self) -> bool {
        self.reentered
    }

    /// Whether a chunk is written but not reset, so that [`Self::end`] writes it again.
    pub fn chunk_parked(&self) -> bool {
        self.queued
            .iter()
            .any(|queued| matches!(queued, Continuation::ChunkWritten(_)))
    }

    fn position(&self) -> u64 {
        self.written
    }

    /// `crc = crc32Update(crc, recordWriter.buffer); writable.write(recordWriter.buffer)`.
    fn write(&mut self, crc: &mut u32) -> Result<(), McapError> {
        *crc = crc32_update(*crc, &self.record_writer.buf);
        if self.written + self.record_writer.len > ARRAY_BUFFER_MAX {
            return Err(McapError::Allocation);
        }
        self.out.extend_from_slice(&self.record_writer.buf);
        self.written += self.record_writer.len;
        Ok(())
    }

    /// [`Self::write`], then `recordWriter.reset()` once the awaited write is done.
    fn flush(&mut self, crc: &mut u32) -> Result<(), McapError> {
        self.write(crc)?;
        self.record_writer.reset();
        Ok(())
    }

    fn write_data(&mut self) -> Result<(), McapError> {
        let mut crc = self.data_section_crc;
        let written = self.write(&mut crc);
        self.data_section_crc = crc;
        written
    }

    fn flush_data(&mut self) -> Result<(), McapError> {
        self.write_data()?;
        self.record_writer.reset();
        Ok(())
    }

    pub fn start(&mut self, profile: &JsString, library: &JsString) {
        self.record_writer.bytes(&MAGIC);
        self.record_writer.record(HEADER, |b| {
            b.string(profile);
            b.string(library);
        });
        self.flush_data().expect("the header fits");
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

    /// Awaited, so the continuations already queued run before the caller goes on.
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
        for _ in 0..self.queued.len() {
            self.run_continuation();
        }
        id
    }

    pub fn add_message(&mut self, message: Message) -> Result<(), McapError> {
        let added = self.add_message_unawaited(message);
        self.drain();
        added
    }

    /// `addMessage` up to its first `await` that waits.
    pub fn add_message_unawaited(&mut self, message: Message) -> Result<(), McapError> {
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
        if self.chunk_builder.records.len > CHUNK_SIZE {
            self.finalize_chunk()?;
        }
        Ok(())
    }

    /// `finalizeChunk` up to its first `await`. A write that throws ends it there: the records
    /// stay in the buffer, and the chunk is not reset.
    fn finalize_chunk(&mut self) -> Result<(), McapError> {
        if self.chunk_builder.total_message_count == 0 {
            return Ok(());
        }
        self.reentered |= self.chunk_parked();
        self.statistics.chunk_count += 1;
        if self.record_writer.len + self.chunk_builder.records.len > RECORD_BUFFER_MAX {
            self.exhausted = true;
            return Err(McapError::Allocation);
        }

        let chunk = &self.chunk_builder;
        let records = &chunk.records;
        let uncompressed_size = records.len;
        let uncompressed_crc = crc32_final(crc32_update(crc32_init(), &records.buf));
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
            if b.measuring {
                b.len += uncompressed_size;
            } else {
                b.bytes(&records.buf);
            }
        });
        self.write_data()?;
        self.queued
            .push_back(Continuation::ChunkWritten(FinalizingChunk {
                message_start_time,
                message_end_time,
                chunk_start_offset,
                chunk_length,
                uncompressed_size,
            }));
        Ok(())
    }

    fn drain(&mut self) {
        while !self.queued.is_empty() {
            self.run_continuation();
        }
    }

    fn run_continuation(&mut self) {
        match self.queued.pop_front() {
            Some(Continuation::ChunkWritten(chunk)) => self.write_message_indexes(chunk),
            Some(Continuation::IndexWritten) => self.record_writer.reset(),
            None => {}
        }
    }

    fn write_message_indexes(&mut self, chunk: FinalizingChunk) {
        self.record_writer.reset();
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
            message_start_time: chunk.message_start_time,
            message_end_time: chunk.message_end_time,
            chunk_start_offset: chunk.chunk_start_offset,
            chunk_length: chunk.chunk_length,
            message_index_offsets,
            message_index_length,
            compressed_size: chunk.uncompressed_size,
            uncompressed_size: chunk.uncompressed_size,
        });
        self.chunk_builder.reset();
        if self.write_data().is_ok() {
            self.queued.push_back(Continuation::IndexWritten);
        }
    }

    pub fn end(mut self) -> Result<Vec<u8>, McapError> {
        self.finalize_chunk()?;
        if self.exhausted {
            return Err(McapError::Allocation);
        }
        self.drain();
        self.flush_data()?;

        let data_section_crc = crc32_final(self.data_section_crc);
        self.record_writer
            .record(DATA_END, |b| b.u32(data_section_crc));
        self.flush(&mut 0)?;

        let mut summary_offsets: Vec<(u8, u64, u64)> = Vec::new();
        let summary_start = self.position();
        let mut summary_crc = crc32_init();

        let schema_start = self.position();
        let mut schema_length = 0;
        for schema in &self.schemas {
            schema_length += self.record_writer.write_schema(schema);
        }
        summary_offsets.push((SCHEMA, schema_start, schema_length));

        self.flush(&mut summary_crc)?;
        let channel_start = self.position();
        let mut channel_length = 0;
        for channel in &self.channels {
            channel_length += self.record_writer.write_channel(channel);
        }
        summary_offsets.push((CHANNEL, channel_start, channel_length));

        self.flush(&mut summary_crc)?;
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

        self.flush(&mut summary_crc)?;
        self.flush(&mut summary_crc)?;
        summary_offsets.push((METADATA_INDEX, self.position(), 0));
        self.flush(&mut summary_crc)?;
        summary_offsets.push((ATTACHMENT_INDEX, self.position(), 0));

        self.flush(&mut summary_crc)?;
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

        self.flush(&mut summary_crc)?;
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
        b.bytes(&footer_prefix);
        b.u32(summary_crc);
        b.bytes(&MAGIC);
        self.flush(&mut 0)?;
        Ok(self.out)
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
