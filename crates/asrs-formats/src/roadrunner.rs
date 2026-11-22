//! src/hub/dataSources/roadrunnerlog/*: the .log reader (revisions 0 and 1 and
//! concatenated logs) and roadRunnerWorker, which decodes a whole file into a Log.

use std::cell::Cell;
use std::collections::HashMap;

use asrs_js::number;
use asrs_js::string::{JsString, TextDecoder};
use asrs_js::time::Clock;
use asrs_log::log::struct_decoder::MAX_ARRAY_LENGTH;
use asrs_log::log::{Log, MapValue};
use asrs_log::units;

use crate::worker::{Abort, DecodeWorker, Request, Response};

const SUPPORTED_LOG_REVISIONS: [i32; 2] = [0, 1];

/// A throw out of one decode-loop iteration: `Bounds` is DataView's "Offset is outside the
/// bounds of the DataView" RangeError, which upstream breaks the loop for, and `Fail` is any
/// other thrown value (a string, a TypeError, a stack overflow), which fails the load.
/// `Abort` leaves the loop the way a dead worker realm would.
enum Step {
    Bounds,
    Fail,
    Abort(Abort),
}

/// `RRLOGCommon.ts`'s `MessageSchema`, one variant per on-wire type id.
enum MessageSchema {
    /// Struct fields are a `Map`: set() keeps first-set position on duplicate names.
    Struct(Vec<(JsString, MessageSchema)>),
    Int,
    Long,
    Double,
    String,
    Boolean,
    Enum(Vec<JsString>),
    Array(Box<MessageSchema>),
}

/// Upstream recurses until V8's stack overflows, a RangeError that fails the load. Where that
/// happens depends on the realm's stack size (the oracle's Node worker still succeeds at depth
/// 12000), so nesting this deep is not modeled. The bound also keeps the port's own recursion
/// inside a default 2 MiB thread stack in debug builds.
const MAX_DEPTH: u32 = 512;

/// Brent's cycle detection over a loop whose next state is a function of its offset: a repeated
/// offset means every later pass repeats the same `period` iterations.
struct CycleDetector {
    tortoise: Option<i64>,
    power: u64,
    steps: u64,
}

impl CycleDetector {
    fn new() -> Self {
        CycleDetector {
            tortoise: None,
            power: 1,
            steps: 0,
        }
    }

    /// Feeds the next offset; returns the cycle's period once `offset` repeats.
    fn period(&mut self, offset: i64) -> Option<u64> {
        self.steps += 1;
        if self.tortoise == Some(offset) {
            return Some(self.steps);
        }
        if self.tortoise.is_none() || self.steps == self.power {
            self.tortoise = Some(offset);
            self.power *= 2;
            self.steps = 0;
        }
        None
    }
}

/// `Uint8Array.prototype.subarray(begin, end)`: negative indexes count from the end and both
/// ends clamp to the length; an empty range stays empty.
fn subarray(data: &[u8], begin: i64, end: i64) -> &[u8] {
    let len = data.len() as i64;
    let clamp = |x: i64| -> usize {
        if x < 0 {
            (len + x).max(0) as usize
        } else {
            x.min(len) as usize
        }
    };
    let (b, e) = (clamp(begin), clamp(end));
    if b > e { &[] } else { &data[b..e] }
}

fn shift_offset(offset: &mut i64, shift: i64) -> i64 {
    *offset += shift;
    *offset - shift
}

fn ascii_eq(s: &JsString, ascii: &str) -> bool {
    let units = s.units();
    units.len() == ascii.len()
        && units
            .iter()
            .zip(ascii.bytes())
            .all(|(&u, b)| u == u16::from(b))
}

/// `Array.prototype.join(",")` on a message array, for `ToNumber(array)` inside
/// `Units.convert`: null and undefined elements join as empty.
fn join_elements(items: &[MapValue], out: &mut Vec<u16>) {
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(u16::from(b','));
        }
        match item {
            MapValue::Undefined => {}
            MapValue::Bool(b) => out.extend(if *b { "true" } else { "false" }.encode_utf16()),
            MapValue::Number(x) => out.extend(number::to_string(*x).encode_utf16()),
            MapValue::BigInt(v) => out.extend(format!("{v}").encode_utf16()),
            MapValue::Text(s) => out.extend_from_slice(s.units()),
            MapValue::Map(_) => out.extend("[object Map]".encode_utf16()),
            MapValue::Array(inner) => join_elements(inner, out),
        }
    }
}

/// `Units.convert(msg.get("x"), "inches", "meters")` applies JS ToNumber to whatever the
/// field holds. A bigint throws a TypeError, which fails the load.
fn units_operand(value: &MapValue) -> Result<f64, Step> {
    Ok(match value {
        MapValue::Number(x) => *x,
        MapValue::Bool(b) => f64::from(*b as u8),
        MapValue::Text(s) => number::to_number(s.units()),
        MapValue::BigInt(_) => return Err(Step::Fail),
        MapValue::Map(_) | MapValue::Undefined => f64::NAN,
        MapValue::Array(items) => {
            let mut joined = Vec::new();
            join_elements(items, &mut joined);
            number::to_number(&joined)
        }
    })
}

/// `RRLOGDecoder`. `produced` counts the values one channel definition or message materializes:
/// a corrupt i32 count can ask for more than either side's heap holds, and past
/// `PRODUCED_LIMIT` the port does not model whether upstream finishes, fails or runs out of
/// memory.
pub struct RrLogDecoder {
    magic: Option<JsString>,
    log_revision: Option<i32>,
    first_rr_timestamp: Option<i64>,
    last_timestamp: f64,
    last_progress_timestamp: f64,
    key_ids: Vec<JsString>,
    key_schemas: Vec<MessageSchema>,
    text_decoder: TextDecoder,
    produced: Cell<u64>,
}

const PRODUCED_LIMIT: u64 = 8_000_000;

impl Default for RrLogDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl RrLogDecoder {
    pub fn new() -> Self {
        RrLogDecoder {
            magic: None,
            log_revision: None,
            first_rr_timestamp: None,
            last_timestamp: 0.0,
            last_progress_timestamp: 0.0,
            key_ids: Vec::new(),
            key_schemas: Vec::new(),
            text_decoder: TextDecoder::new(Some(&JsString::from("UTF-8")), false, false)
                .expect("UTF-8 is always supported"),
            produced: Cell::new(0),
        }
    }

    fn tick_produced(&self) -> Result<(), Step> {
        let produced = self.produced.get() + 1;
        self.produced.set(produced);
        if produced > PRODUCED_LIMIT {
            return Err(Step::Abort(Abort::Unmodeled(
                "a channel definition or message with more values than the port materializes",
            )));
        }
        Ok(())
    }

    fn get<const N: usize, T: Fn([u8; N]) -> R, R>(
        &self,
        data: &[u8],
        offset: &mut i64,
        read: T,
    ) -> Result<R, Step> {
        let at = usize::try_from(shift_offset(offset, N as i64)).map_err(|_| Step::Bounds)?;
        let end = at.checked_add(N).ok_or(Step::Bounds)?;
        let bytes: [u8; N] = data
            .get(at..end)
            .ok_or(Step::Bounds)?
            .try_into()
            .expect("the slice has exactly N bytes");
        Ok(read(bytes))
    }

    fn get_i16(&self, data: &[u8], offset: &mut i64) -> Result<i16, Step> {
        self.get(data, offset, i16::from_be_bytes)
    }

    fn get_i32(&self, data: &[u8], offset: &mut i64) -> Result<i32, Step> {
        self.get(data, offset, i32::from_be_bytes)
    }

    fn get_i64(&self, data: &[u8], offset: &mut i64) -> Result<i64, Step> {
        self.get(data, offset, i64::from_be_bytes)
    }

    fn get_f64(&self, data: &[u8], offset: &mut i64) -> Result<f64, Step> {
        self.get(data, offset, f64::from_be_bytes)
    }

    /// `dataArray[shiftOffset(1)] !== 0`: an out-of-bounds index is `undefined`, which is
    /// `!== 0`, so a truncated boolean reads true rather than throwing.
    fn get_bool(&self, data: &[u8], offset: &mut i64) -> Result<bool, Step> {
        let at = shift_offset(offset, 1);
        Ok(usize::try_from(at)
            .ok()
            .and_then(|i| data.get(i))
            .is_none_or(|&b| b != 0))
    }

    fn read_string(&self, data: &[u8], offset: &mut i64) -> Result<JsString, Step> {
        let len = i64::from(self.get_i32(data, offset)?);
        let string = self
            .text_decoder
            .decode(subarray(data, *offset, *offset + len))
            .expect("nonfatal UTF-8 never errors");
        *offset += len;
        Ok(string)
    }

    fn read_schema(
        &self,
        data: &[u8],
        offset: &mut i64,
        depth: u32,
    ) -> Result<MessageSchema, Step> {
        if depth >= MAX_DEPTH {
            return Err(Step::Abort(Abort::Unmodeled(
                "a schema nested past MAX_DEPTH",
            )));
        }
        match self.get_i32(data, offset)? {
            0 => {
                let num_fields = i64::from(self.get_i32(data, offset)?);
                let mut fields: Vec<(JsString, MessageSchema)> = Vec::new();
                let mut positions: HashMap<JsString, usize> = HashMap::new();
                // A negative name length can bring every field back to an offset already
                // read; a full cycle sets the same names to the same schemas again, so whole
                // cycles are skipped rather than run up to 2^31 times.
                let mut cycle = CycleDetector::new();
                let mut i = 0;
                while i < num_fields {
                    if let Some(period) = cycle.period(*offset) {
                        let period = period as i64;
                        i += (num_fields - i) / period * period;
                        if i >= num_fields {
                            break;
                        }
                    }
                    i += 1;
                    let name = self.read_string(data, offset)?;
                    let schema = self.read_schema(data, offset, depth + 1)?;
                    match positions.get(&name) {
                        Some(&position) => fields[position].1 = schema,
                        None => {
                            self.tick_produced()?;
                            positions.insert(name.clone(), fields.len());
                            fields.push((name, schema));
                        }
                    }
                }
                Ok(MessageSchema::Struct(fields))
            }
            1 => Ok(MessageSchema::Int),
            2 => Ok(MessageSchema::Long),
            3 => Ok(MessageSchema::Double),
            4 => Ok(MessageSchema::String),
            5 => Ok(MessageSchema::Boolean),
            6 => {
                let num_constants = self.get_i32(data, offset)?;
                let mut constants = Vec::new();
                for _ in 0..num_constants {
                    constants.push(self.read_string(data, offset)?);
                    self.tick_produced()?;
                }
                Ok(MessageSchema::Enum(constants))
            }
            7 => Ok(MessageSchema::Array(Box::new(self.read_schema(
                data,
                offset,
                depth + 1,
            )?))),
            _ => Err(Step::Fail),
        }
    }

    fn array_schema_count(schema: &MessageSchema) -> i64 {
        match schema {
            MessageSchema::Struct(fields) => fields
                .iter()
                .map(|(_, field)| Self::array_schema_count(field))
                .sum(),
            MessageSchema::Array(inner) => 1 + Self::array_schema_count(inner),
            _ => 0,
        }
    }

    fn read_msg(
        &self,
        schema: &MessageSchema,
        data: &[u8],
        offset: &mut i64,
        depth: u32,
    ) -> Result<MapValue, Step> {
        if depth >= MAX_DEPTH {
            return Err(Step::Abort(Abort::Unmodeled(
                "a schema nested past MAX_DEPTH",
            )));
        }
        Ok(match schema {
            MessageSchema::Struct(fields) => {
                let mut entries = Vec::with_capacity(fields.len());
                for (name, field) in fields {
                    entries.push((name.clone(), self.read_msg(field, data, offset, depth + 1)?));
                    self.tick_produced()?;
                }
                MapValue::Map(entries)
            }
            MessageSchema::Int => MapValue::Number(f64::from(self.get_i32(data, offset)?)),
            MessageSchema::Long => MapValue::BigInt(self.get_i64(data, offset)?),
            MessageSchema::Double => MapValue::Number(self.get_f64(data, offset)?),
            MessageSchema::String => MapValue::Text(self.read_string(data, offset)?),
            MessageSchema::Boolean => MapValue::Bool(self.get_bool(data, offset)?),
            MessageSchema::Enum(constants) => {
                let ordinal = self.get_i32(data, offset)?;
                usize::try_from(ordinal)
                    .ok()
                    .and_then(|i| constants.get(i))
                    .map_or(MapValue::Undefined, |s| MapValue::Text(s.clone()))
            }
            MessageSchema::Array(inner) => {
                let size = self.get_i32(data, offset)?;
                let mut items = Vec::new();
                for _ in 0..size {
                    let start = *offset;
                    let item = self.read_msg(inner, data, offset, depth + 1)?;
                    if f64::from(size) > MAX_ARRAY_LENGTH {
                        // Every later element repeats this one when it read nothing (an empty
                        // string of length -4) or read a boolean past the end. Then upstream
                        // pushes copies until `push` throws "Invalid array length", a
                        // RangeError that fails the load; copies of an object run its heap out
                        // first.
                        let past_end_boolean =
                            matches!(**inner, MessageSchema::Boolean) && start >= data.len() as i64;
                        if *offset == start || past_end_boolean {
                            return Err(match item {
                                MapValue::Map(_) | MapValue::Array(_) => {
                                    Step::Abort(Abort::NeverReturns)
                                }
                                _ => Step::Fail,
                            });
                        }
                    }
                    items.push(item);
                    self.tick_produced()?;
                }
                MapValue::Array(items)
            }
        })
    }

    fn rr_timestamp_to_seconds(&mut self, rr_timestamp: i64) -> f64 {
        let first = *self.first_rr_timestamp.get_or_insert(rr_timestamp);
        (i128::from(rr_timestamp) - i128::from(first)) as f64 / 1e9
    }

    /// `RRLOGDecoder.decode`. The magic and revision reads sit outside the per-message try:
    /// a RangeError there fails the whole load rather than breaking the loop.
    pub fn decode(
        &mut self,
        log: &mut Log,
        data: &[u8],
        progress: &mut impl FnMut(f64),
        clock: &dyn Clock,
    ) -> Result<bool, Abort> {
        let mut offset: i64 = 0;

        if self.magic.is_none() {
            self.magic = Some(
                self.text_decoder
                    .decode(subarray(data, offset, offset + 2))
                    .expect("nonfatal UTF-8 never errors"),
            );
            shift_offset(&mut offset, 2);
            if !ascii_eq(self.magic.as_ref().expect("just set"), "RR") {
                return Ok(false);
            }
        }

        if self.log_revision.is_none() {
            match self.get_i16(data, &mut offset) {
                Err(_) => return Ok(false),
                Ok(revision) => {
                    self.log_revision = Some(i32::from(revision));
                    if !SUPPORTED_LOG_REVISIONS.contains(&i32::from(revision)) {
                        return Ok(false);
                    }
                }
            }
        }

        // Within one concatenated log, the step from an offset depends only on that offset (a
        // channel table only grows, and the ids a step looks up resolve the same way each
        // pass), so a repeated offset means upstream loops forever. A restart clears the table,
        // so offsets after one start a new sequence, and restarts are tracked on their own.
        let mut steps = CycleDetector::new();
        let mut restarts = CycleDetector::new();
        loop {
            if crate::worker::stop_requested() {
                return Err(Abort::Threw);
            }
            if steps.period(offset).is_some() {
                return Err(Abort::NeverReturns);
            }
            // A "RR" at a message boundary starts a concatenated log: the revision is reread
            // (a RangeError fails the load) and channels reset, but the first timestamp does
            // not.
            if ascii_eq(
                &self
                    .text_decoder
                    .decode(subarray(data, offset, offset + 2))
                    .expect("nonfatal UTF-8 never errors"),
                "RR",
            ) {
                if restarts.period(offset).is_some() {
                    return Err(Abort::NeverReturns);
                }
                steps = CycleDetector::new();
                shift_offset(&mut offset, 2);
                match self.get_i16(data, &mut offset) {
                    Err(_) => return Ok(false),
                    Ok(revision) => {
                        self.log_revision = Some(i32::from(revision));
                        if !SUPPORTED_LOG_REVISIONS.contains(&i32::from(revision)) {
                            return Ok(false);
                        }
                    }
                }
                self.key_ids.clear();
                self.key_schemas.clear();
            }

            self.produced.set(0);
            match self.read_step(log, data, &mut offset) {
                Err(Step::Bounds) => break,
                Err(Step::Fail) => return Ok(false),
                Err(Step::Abort(abort)) => return Err(abort),
                Ok(()) => {}
            }

            let now = clock.now();
            if now - self.last_progress_timestamp > 1000.0 / 60.0 {
                self.last_progress_timestamp = now;
                progress(offset as f64 / data.len() as f64);
            }
        }
        Ok(true)
    }

    /// The per-iteration `try` block: the message type and its channel definition or message.
    fn read_step(&mut self, log: &mut Log, data: &[u8], offset: &mut i64) -> Result<(), Step> {
        match self.get_i32(data, offset)? {
            0 => {
                let key_id = self.key_ids.len();
                let name = self.read_string(data, offset)?;
                let schema = self.read_schema(data, offset, 0)?;
                self.key_ids.push(name);
                self.key_schemas.push(schema);
                // Revision 0 wrote four padding bytes per array schema in the definition.
                if self.log_revision == Some(0) {
                    shift_offset(
                        offset,
                        4 * Self::array_schema_count(&self.key_schemas[key_id]),
                    );
                }
            }
            1 => {
                let key_id = self.get_i32(data, offset)?;
                let Ok(index) = usize::try_from(key_id) else {
                    return Err(Step::Fail);
                };
                let Some(schema) = self.key_schemas.get(index) else {
                    return Err(Step::Fail);
                };
                let key = self.key_ids.get(index).cloned().unwrap_or_default();
                let msg = self.read_msg(schema, data, offset, 0)?;

                let builtin = ascii_eq(&key, "OPMODE_PRE_INIT")
                    || ascii_eq(&key, "OPMODE_PRE_START")
                    || ascii_eq(&key, "OPMODE_POST_STOP")
                    || ascii_eq(&key, "TIMESTAMP");
                if builtin && matches!(msg, MapValue::BigInt(_)) {
                    let &MapValue::BigInt(rr_timestamp) = &msg else {
                        unreachable!()
                    };
                    self.last_timestamp = self.rr_timestamp_to_seconds(rr_timestamp);
                    if ascii_eq(&key, "OPMODE_PRE_START") {
                        log.put_boolean(
                            &JsString::from("RUNNING"),
                            self.last_timestamp + 0.05,
                            true,
                        );
                    } else if ascii_eq(&key, "OPMODE_POST_STOP") {
                        log.put_boolean(&JsString::from("RUNNING"), self.last_timestamp, false);
                    }
                } else if let MapValue::Map(entries) = &msg
                    && let Some(MapValue::BigInt(rr_timestamp)) = entries
                        .iter()
                        .find(|(name, _)| ascii_eq(name, "timestamp"))
                        .map(|(_, value)| value)
                {
                    self.last_timestamp = self.rr_timestamp_to_seconds(*rr_timestamp);
                }

                let timestamp = self.last_timestamp;
                match &msg {
                    MapValue::Bool(b) => log.put_boolean(&key, timestamp, *b),
                    MapValue::Number(x) => log.put_number(&key, timestamp, *x),
                    MapValue::BigInt(v) => log.put_number(&key, timestamp, *v as f64),
                    MapValue::Text(s) => log.put_string(&key, timestamp, s.clone()),
                    MapValue::Map(entries) => {
                        let field = |name: &str| {
                            entries
                                .iter()
                                .find(|(n, _)| ascii_eq(n, name))
                                .map(|(_, v)| v)
                        };
                        match (field("x"), field("y"), field("heading")) {
                            (Some(x), Some(y), Some(heading)) => {
                                let translation = [
                                    units::convert(units_operand(x)?, "inches", "meters")
                                        .map_err(|_| Step::Fail)?,
                                    units::convert(units_operand(y)?, "inches", "meters")
                                        .map_err(|_| Step::Fail)?,
                                ];
                                let MapValue::Number(rotation) = heading else {
                                    return Err(Step::Abort(Abort::Unmodeled(
                                        "a pose heading that is not a number",
                                    )));
                                };
                                log.put_pose(&key, timestamp, translation, *rotation);
                            }
                            _ => {
                                log.put_unknown_map(&key, timestamp, &msg, true)
                                    .map_err(|_| Step::Fail)?;
                            }
                        }
                    }
                    _ => {
                        log.put_unknown_map(&key, timestamp, &msg, true)
                            .map_err(|_| Step::Fail)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// roadRunnerWorker's `onmessage` for its `start` request: `request.data[0]` is the file; an
/// absent slot is `undefined` and a null slot is `null`, and both throw on `.buffer`.
pub fn run_worker(
    files: &[Option<&[u8]>],
    post: &mut impl FnMut(Response),
    clock: &dyn Clock,
) -> Result<(), Abort> {
    let Some(Some(data)) = files.first().copied() else {
        return Err(Abort::Threw);
    };
    let mut log = Log::new(false);
    match RrLogDecoder::new().decode(
        &mut log,
        data,
        &mut |value| post(Response::Progress(value)),
        clock,
    )? {
        true => {
            post(Response::Progress(1.0));
            post(Response::Initial {
                log: Box::new(log.to_serialized()),
                is_partial: false,
            });
        }
        false => post(Response::Failed),
    }
    Ok(())
}

/// roadRunnerWorker: answers `start` and ignores every other request.
pub struct RoadRunnerWorker {
    clock: Box<dyn Clock + Send>,
}

impl RoadRunnerWorker {
    pub fn new(clock: Box<dyn Clock + Send>) -> Self {
        RoadRunnerWorker { clock }
    }
}

impl DecodeWorker for RoadRunnerWorker {
    fn handle(&mut self, request: Request, post: &mut dyn FnMut(Response)) -> Result<(), Abort> {
        let Request::Start(files) = request else {
            return Ok(());
        };
        let files: Vec<Option<&[u8]>> = files.iter().map(Option::as_deref).collect();
        run_worker(&files, &mut |response| post(response), &*self.clock)
    }
}

#[cfg(test)]
mod tests {
    use asrs_js::time::SetClock;
    use asrs_log::log::LogValues;

    use super::*;

    struct Builder {
        out: Vec<u8>,
        revision: i16,
    }

    impl Builder {
        fn new(revision: i16) -> Self {
            let mut out = Vec::new();
            out.extend_from_slice(b"RR");
            out.extend_from_slice(&revision.to_be_bytes());
            Builder { out, revision }
        }
        fn be(&mut self, v: i32) {
            self.out.extend_from_slice(&v.to_be_bytes());
        }
        fn raw(&mut self, bytes: &[u8]) {
            self.out.extend_from_slice(bytes);
        }
        fn str(&mut self, s: &str) {
            self.be(s.len() as i32);
            self.raw(s.as_bytes());
        }
        fn channel(&mut self, name: &str, schema: &[u8]) -> i32 {
            self.be(0);
            self.str(name);
            self.raw(schema);
            let arrays = count_arrays(schema);
            if self.revision == 0 {
                for _ in 0..arrays {
                    self.be(0);
                }
            }
            -1
        }
        fn message(&mut self, channel: i32, payload: &[u8]) {
            self.be(1);
            self.be(channel);
            self.raw(payload);
        }
        fn restart(&mut self, revision: i16) {
            self.raw(b"RR");
            self.raw(&revision.to_be_bytes());
            self.revision = revision;
        }
        fn bytes(self) -> Vec<u8> {
            self.out
        }
    }

    fn count_arrays(schema: &[u8]) -> usize {
        // Counts array type ids (7) anywhere in the schema bytes; test schemas keep
        // strings free of the type-id bytes.
        schema.chunks(4).filter(|c| *c == [0, 0, 0, 7]).count()
    }

    const INT: &[u8] = &[0, 0, 0, 1];
    const LONG: &[u8] = &[0, 0, 0, 2];
    const DOUBLE: &[u8] = &[0, 0, 0, 3];
    const STRING: &[u8] = &[0, 0, 0, 4];
    const BOOLEAN: &[u8] = &[0, 0, 0, 5];

    fn enum_schema(constants: &[&str]) -> Vec<u8> {
        let mut v = vec![0, 0, 0, 6];
        v.extend_from_slice(&(constants.len() as i32).to_be_bytes());
        for c in constants {
            v.extend_from_slice(&(c.len() as i32).to_be_bytes());
            v.extend_from_slice(c.as_bytes());
        }
        v
    }

    fn struct_schema(fields: &[(&str, &[u8])]) -> Vec<u8> {
        let mut v = vec![0, 0, 0, 0];
        v.extend_from_slice(&(fields.len() as i32).to_be_bytes());
        for (name, schema) in fields {
            v.extend_from_slice(&(name.len() as i32).to_be_bytes());
            v.extend_from_slice(name.as_bytes());
            v.extend_from_slice(schema);
        }
        v
    }

    fn array_schema(inner: &[u8]) -> Vec<u8> {
        let mut v = vec![0, 0, 0, 7];
        v.extend_from_slice(inner);
        v
    }

    fn decode(data: &[u8]) -> (Vec<Response>, Result<bool, Abort>, Log) {
        let mut log = Log::new(false);
        let mut progress = Vec::new();
        let end = {
            let mut post = |value: f64| progress.push(Response::Progress(value));
            RrLogDecoder::new().decode(&mut log, data, &mut post, &SetClock(1e12))
        };
        (progress, end, log)
    }

    fn field(log: &Log, key: &str) -> asrs_log::log::SerializedLogField {
        let units: Vec<u16> = key.encode_utf16().collect();
        log.to_serialized()
            .fields
            .remove_units(&units)
            .unwrap_or_else(|| panic!("no field {key}"))
    }

    fn numbers(log: &Log, key: &str) -> Vec<f64> {
        let LogValues::Number(v) = &field(log, key).values else {
            panic!("{key} is not a number field")
        };
        v.as_slice().to_vec()
    }

    fn key(log: &Log, i: usize) -> String {
        log.to_serialized()
            .fields
            .iter()
            .nth(i)
            .map(|(k, _)| k.to_string_lossy())
            .unwrap()
    }

    #[test]
    fn rejects_bad_magic_and_revisions() {
        assert_eq!(decode(b"").1, Ok(false));
        assert_eq!(decode(b"rr\x00\x01").1, Ok(false));
        assert_eq!(decode(b"RR").1, Ok(false)); // revision read throws
        assert_eq!(decode(b"RR\x00\x02").1, Ok(false));
        assert_eq!(decode(b"RR\xff\xff").1, Ok(false));
        // Header only: an empty but successful log.
        assert_eq!(decode(b"RR\x00\x01").1, Ok(true));
        assert_eq!(decode(b"RR\x00\x00").1, Ok(true));
    }

    #[test]
    fn primitive_channels() {
        let mut b = Builder::new(1);
        b.channel("I", INT);
        b.channel("L", LONG);
        b.channel("D", DOUBLE);
        b.channel("S", STRING);
        b.channel("B", BOOLEAN);
        b.message(0, &(-5i32).to_be_bytes());
        b.message(1, &7_000_000_000_000_000_000i64.to_be_bytes());
        b.message(2, &1.5f64.to_be_bytes());
        let mut payload = 2i32.to_be_bytes().to_vec();
        payload.extend_from_slice(b"hi");
        b.message(3, &payload);
        b.message(4, &[2]);
        let (_, end, log) = decode(&b.bytes());
        assert_eq!(end, Ok(true));
        assert_eq!(numbers(&log, "I"), [-5.0]);
        assert_eq!(numbers(&log, "L"), [7e18]);
        assert_eq!(numbers(&log, "D"), [1.5]);
        let LogValues::Boolean(v) = &field(&log, "B").values else {
            panic!()
        };
        assert_eq!(v.as_slice(), [true]);
    }

    #[test]
    fn enum_ordinals_out_of_range_write_nothing() {
        let mut b = Builder::new(1);
        b.channel("E", &enum_schema(&["A", "B"]));
        b.message(0, &1i32.to_be_bytes());
        b.message(0, &2i32.to_be_bytes());
        b.message(0, &(-1i32).to_be_bytes());
        let (_, end, log) = decode(&b.bytes());
        assert_eq!(end, Ok(true));
        let LogValues::String(v) = &field(&log, "E").values else {
            panic!()
        };
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].to_string_lossy(), "B");
    }

    #[test]
    fn timestamps_and_running() {
        let mut b = Builder::new(1);
        b.channel("OPMODE_PRE_INIT", LONG);
        b.channel("OPMODE_PRE_START", LONG);
        b.channel("TIMESTAMP", LONG);
        b.channel("OPMODE_POST_STOP", LONG);
        b.channel("V", DOUBLE);
        b.message(0, &1_000_000_000i64.to_be_bytes());
        b.message(1, &2_000_000_000i64.to_be_bytes());
        b.message(2, &3_000_000_000i64.to_be_bytes());
        b.message(4, &9.0f64.to_be_bytes());
        b.message(3, &4_000_000_000i64.to_be_bytes());
        let (_, end, log) = decode(&b.bytes());
        assert_eq!(end, Ok(true));
        let LogValues::Boolean(v) = &field(&log, "RUNNING").values else {
            panic!()
        };
        assert_eq!(v.as_slice(), [true, false]);
        // V is written at the last TIMESTAMP, 3 seconds after the first.
        assert_eq!(numbers(&log, "V"), [9.0]);
        assert_eq!(field(&log, "V").timestamps.as_slice(), [2.0]);
    }

    #[test]
    fn message_timestamp_fields() {
        let stamped = struct_schema(&[("timestamp", LONG), ("v", INT)]);
        let int_stamped = struct_schema(&[("timestamp", INT), ("v", INT)]);
        let mut b = Builder::new(1);
        b.channel("A", &stamped);
        b.channel("B", &int_stamped);
        b.channel("V", DOUBLE);
        // A's timestamp field updates the timestamp; B's int does not.
        let mut payload = 1_000_000_000i64.to_be_bytes().to_vec();
        payload.extend_from_slice(&7i32.to_be_bytes());
        b.message(0, &payload);
        b.message(2, &5.0f64.to_be_bytes());
        let mut payload = 99i32.to_be_bytes().to_vec();
        payload.extend_from_slice(&8i32.to_be_bytes());
        b.message(1, &payload);
        b.message(2, &6.0f64.to_be_bytes());
        let (_, end, log) = decode(&b.bytes());
        assert_eq!(end, Ok(true));
        assert_eq!(field(&log, "V").timestamps.as_slice(), [0.0]);
        assert_eq!(numbers(&log, "V"), [6.0]);
    }

    #[test]
    fn concatenated_logs_reset_channels_not_first_timestamp() {
        let mut b = Builder::new(1);
        b.channel("TIMESTAMP", LONG);
        b.channel("A", INT);
        b.message(0, &1_000_000_000i64.to_be_bytes());
        b.restart(0);
        // Channel ids restart: id 1 is now a fresh channel.
        b.channel("TIMESTAMP", LONG);
        b.channel("B", INT);
        b.message(1, &9i32.to_be_bytes());
        b.message(0, &2_000_000_000i64.to_be_bytes());
        let (_, end, log) = decode(&b.bytes());
        assert_eq!(end, Ok(true));
        assert_eq!(numbers(&log, "B"), [9.0]);
        assert_eq!(field(&log, "B").timestamps.as_slice(), [0.0]);
        assert_eq!(field(&log, "TIMESTAMP").timestamps.as_slice(), [0.0, 1.0]);
    }

    #[test]
    fn bad_revision_in_second_log_fails() {
        let mut b = Builder::new(1);
        b.channel("A", INT);
        b.message(0, &1i32.to_be_bytes());
        b.restart(2);
        assert_eq!(decode(&b.bytes()).1, Ok(false));
        // A file ending in exactly "RR" fails on the revision read.
        let mut b = Builder::new(1);
        b.raw(b"RR");
        assert_eq!(decode(&b.bytes()).1, Ok(false));
    }

    #[test]
    fn unknown_schema_and_channel_fail() {
        let mut b = Builder::new(1);
        b.channel("A", &[0, 0, 0, 9]);
        assert_eq!(decode(&b.bytes()).1, Ok(false));
        let mut b = Builder::new(1);
        b.channel("A", INT);
        b.message(5, &1i32.to_be_bytes());
        assert_eq!(decode(&b.bytes()).1, Ok(false));
    }

    #[test]
    fn truncation_keeps_decoded_records() {
        let mut b = Builder::new(1);
        b.channel("A", INT);
        b.channel("B", INT);
        b.message(0, &1i32.to_be_bytes());
        b.message(1, &2i32.to_be_bytes());
        let full = b.bytes();
        let cut = full[..full.len() - 2].to_vec();
        let (_, end, log) = decode(&cut);
        assert_eq!(end, Ok(true));
        assert_eq!(numbers(&log, "A"), [1.0]);
        // Truncated inside a channel definition: same.
        let cut = full[..24].to_vec();
        let (_, end, _) = decode(&cut);
        assert_eq!(end, Ok(true));
    }

    #[test]
    fn revision_zero_array_padding() {
        let arr = array_schema(INT);
        let mut b = Builder::new(0);
        b.channel("A", &arr);
        let mut msg = 1i32.to_be_bytes().to_vec();
        msg.extend_from_slice(&42i32.to_be_bytes());
        b.message(0, &msg);
        let (_, end, log) = decode(&b.bytes());
        assert_eq!(end, Ok(true));
        let LogValues::NumberArray(v) = &field(&log, "A").values else {
            panic!()
        };
        assert_eq!(v.as_slice(), [vec![42.0]]);
    }

    #[test]
    fn boolean_past_eof_reads_true() {
        let mut b = Builder::new(1);
        b.channel("B", BOOLEAN);
        // The boolean byte is cut; dataArray[i] is undefined, which is !== 0.
        b.be(1);
        b.be(0);
        let (_, end, log) = decode(&b.bytes());
        assert_eq!(end, Ok(true));
        let LogValues::Boolean(v) = &field(&log, "B").values else {
            panic!()
        };
        assert_eq!(v.as_slice(), [true]);
    }

    #[test]
    fn negative_string_length_moves_offset_back() {
        let mut b = Builder::new(1);
        b.channel("S", STRING);
        b.be(1);
        b.be(0);
        b.be(-2);
        b.raw(b"AB");
        let (_, end, _) = decode(&b.bytes());
        assert_eq!(end, Ok(true));
    }

    #[test]
    fn a_repeated_offset_never_returns() {
        // The string's length -12 leads back to the message's type, every pass.
        let mut b = Builder::new(1);
        b.channel("S", STRING);
        b.message(0, &(-12i32).to_be_bytes());
        assert_eq!(decode(&b.bytes()).1, Err(Abort::NeverReturns));
    }

    #[test]
    fn a_repeated_restart_never_returns() {
        // A no-op type 2, then "RR" at 8; the channel name's length -16 leaves the LONG schema
        // id at 4 and ends the definition back at 8.
        let mut data = b"RR\x00\x01".to_vec();
        data.extend_from_slice(&2i32.to_be_bytes());
        data.extend_from_slice(b"RR\x00\x01");
        data.extend_from_slice(&0i32.to_be_bytes());
        data.extend_from_slice(&(-16i32).to_be_bytes());
        assert_eq!(decode(&data).1, Err(Abort::NeverReturns));
    }

    #[test]
    fn repeating_struct_schema_fields_are_skipped() {
        // After the first field ("" as INT), each name length -8 rereads that INT id, so all
        // 2^31 - 1 fields set "" again.
        let mut schema = vec![0, 0, 0, 0];
        schema.extend_from_slice(&i32::MAX.to_be_bytes());
        schema.extend_from_slice(&0i32.to_be_bytes());
        schema.extend_from_slice(INT);
        schema.extend_from_slice(&(-8i32).to_be_bytes());
        let mut b = Builder::new(1);
        b.channel("S", &schema);
        b.message(0, &42i32.to_be_bytes());
        let (_, end, log) = decode(&b.bytes());
        assert_eq!(end, Ok(true));
        assert_eq!(numbers(&log, "S/"), [42.0]);
    }

    #[test]
    fn repeating_elements_past_the_array_length_limit() {
        let mut b = Builder::new(1);
        b.channel("S", &array_schema(STRING));
        let mut payload = i32::MAX.to_be_bytes().to_vec();
        payload.extend_from_slice(&(-4i32).to_be_bytes());
        b.message(0, &payload);
        assert_eq!(decode(&b.bytes()).1, Ok(false));

        let mut b = Builder::new(1);
        b.channel("B", &array_schema(BOOLEAN));
        b.message(0, &i32::MAX.to_be_bytes());
        assert_eq!(decode(&b.bytes()).1, Ok(false));

        let mut b = Builder::new(1);
        b.channel("E", &array_schema(&struct_schema(&[])));
        b.message(0, &i32::MAX.to_be_bytes());
        assert_eq!(decode(&b.bytes()).1, Err(Abort::NeverReturns));
    }

    #[test]
    fn nesting_just_inside_max_depth_decodes() {
        let depth = MAX_DEPTH as usize - 1;
        let mut schema = [0, 0, 0, 7].repeat(depth);
        schema.extend_from_slice(INT);
        let mut payload = 1i32.to_be_bytes().repeat(depth);
        payload.extend_from_slice(&5i32.to_be_bytes());
        let mut b = Builder::new(1);
        b.channel("D", &schema);
        b.message(0, &payload);
        let (_, end, log) = decode(&b.bytes());
        assert_eq!(end, Ok(true));
        let leaf = format!("D{}", "/0".repeat(depth - 1));
        let LogValues::NumberArray(v) = &field(&log, &leaf).values else {
            panic!()
        };
        assert_eq!(v.as_slice(), [vec![5.0]]);
    }

    #[test]
    fn nesting_past_max_depth_is_unmodeled() {
        let mut schema = [0, 0, 0, 7].repeat(MAX_DEPTH as usize);
        schema.extend_from_slice(INT);
        let mut b = Builder::new(1);
        b.channel("D", &schema);
        assert!(matches!(decode(&b.bytes()).1, Err(Abort::Unmodeled(_))));
    }

    #[test]
    fn pose_detection_converts_inches() {
        let pose = struct_schema(&[("x", DOUBLE), ("y", DOUBLE), ("heading", DOUBLE)]);
        let mut b = Builder::new(1);
        b.channel("P", &pose);
        let mut payload = 1.0f64.to_be_bytes().to_vec();
        payload.extend_from_slice(&2.0f64.to_be_bytes());
        payload.extend_from_slice(&1.0f64.to_be_bytes());
        b.message(0, &payload);
        let (_, end, log) = decode(&b.bytes());
        assert_eq!(end, Ok(true));
        assert_eq!(numbers(&log, "P/rotation/value"), [1.0]);
        assert!((numbers(&log, "P/translation/x")[0] - 0.0254).abs() < 1e-9);
        assert!((numbers(&log, "P/translation/y")[0] - 0.0508).abs() < 1e-9);
        assert_eq!(
            field(&log, "P")
                .structured_type
                .as_ref()
                .unwrap()
                .to_string_lossy(),
            "Pose2d"
        );
    }

    #[test]
    fn struct_fields_write_in_insertion_order() {
        let s = struct_schema(&[("b", INT), ("10", INT), ("a", INT), ("2", INT)]);
        let mut b = Builder::new(1);
        b.channel("K", &s);
        let mut payload = Vec::new();
        for v in [1i32, 2, 3, 4] {
            payload.extend_from_slice(&v.to_be_bytes());
        }
        b.message(0, &payload);
        let (_, end, log) = decode(&b.bytes());
        assert_eq!(end, Ok(true));
        let names: Vec<String> = (0..4).map(|i| key(&log, i)).collect();
        assert_eq!(names, ["K/b", "K/10", "K/a", "K/2"]);
    }

    #[test]
    fn progress_fires_once_then_done() {
        let mut b = Builder::new(1);
        b.channel("A", INT);
        b.message(0, &1i32.to_be_bytes());
        b.message(0, &2i32.to_be_bytes());
        let data = b.bytes();
        let (progress, end, _) = decode(&data);
        assert_eq!(end, Ok(true));
        assert_eq!(progress.len(), 1);
        let Response::Progress(v) = progress[0] else {
            panic!()
        };
        assert!(v > 0.0 && v < 1.0);
    }

    #[test]
    fn worker_protocol() {
        let mut b = Builder::new(1);
        b.channel("A", INT);
        b.message(0, &1i32.to_be_bytes());
        let data = b.bytes();
        let mut responses = Vec::new();
        let end = run_worker(
            &[Some(data.as_slice())],
            &mut |r| responses.push(r),
            &SetClock(1e12),
        );
        assert_eq!(end, Ok(()));
        assert!(matches!(responses.last(), Some(Response::Initial { .. })));
        assert_eq!(
            run_worker(&[], &mut |_| {}, &SetClock(1e12)),
            Err(Abort::Threw)
        );
        assert_eq!(
            run_worker(&[None], &mut |_| {}, &SetClock(1e12)),
            Err(Abort::Threw)
        );
    }
}
