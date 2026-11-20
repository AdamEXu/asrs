//! src/hub/dataSources/roadrunnerlog/*: the .log reader (revisions 0 and 1 and
//! concatenated logs) and roadRunnerWorker, which decodes a whole file into a Log.

use std::cell::Cell;

use asrs_js::number;
use asrs_js::string::{JsString, TextDecoder};
use asrs_js::time::Clock;
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

/// Upstream can nest schemas and messages arbitrarily deep before V8's own stack limit
/// throws; past this depth a real throw is modeled as one instead of overflowing ours.
const MAX_DEPTH: u32 = 2048;

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
            MapValue::BigInt(v) => out.extend(v.to_string().encode_utf16()),
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

/// `RRLOGDecoder`. The `Cell`s count work and materialized values: loops bounded only by a
/// corrupt i32 count can run past any finishable time or upstream's heap, both of which end
/// as "never completes" for the oracle's purposes.
pub struct RrLogDecoder {
    magic: Option<JsString>,
    log_revision: Option<i32>,
    first_rr_timestamp: Option<i64>,
    last_timestamp: f64,
    last_progress_timestamp: f64,
    key_ids: Vec<JsString>,
    key_schemas: Vec<MessageSchema>,
    text_decoder: TextDecoder,
    work: Cell<u64>,
    produced: Cell<u64>,
}

const WORK_BUDGET: u64 = 2_000_000_000;
const PRODUCED_BUDGET: u64 = 8_000_000;

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
            work: Cell::new(0),
            produced: Cell::new(0),
        }
    }

    fn tick_work(&self) -> Result<(), Step> {
        let work = self.work.get() + 1;
        self.work.set(work);
        if work > WORK_BUDGET {
            return Err(Step::Abort(Abort::NeverReturns));
        }
        Ok(())
    }

    fn tick_produced(&self) -> Result<(), Step> {
        let produced = self.produced.get() + 1;
        self.produced.set(produced);
        if produced > PRODUCED_BUDGET {
            return Err(Step::Abort(Abort::NeverReturns));
        }
        Ok(())
    }

    fn get<const N: usize, T: Fn([u8; N]) -> R, R>(
        &self,
        data: &[u8],
        offset: &mut i64,
        read: T,
    ) -> Result<R, Step> {
        self.tick_work()?;
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
        self.tick_work()?;
        let at = shift_offset(offset, 1);
        Ok(usize::try_from(at)
            .ok()
            .and_then(|i| data.get(i))
            .map_or(true, |&b| b != 0))
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

    fn read_schema(&self, data: &[u8], offset: &mut i64, depth: u32) -> Result<MessageSchema, Step> {
        if depth >= MAX_DEPTH {
            return Err(Step::Fail);
        }
        match self.get_i32(data, offset)? {
            0 => {
                let num_fields = self.get_i32(data, offset)?;
                let mut fields: Vec<(JsString, MessageSchema)> = Vec::new();
                for _ in 0..num_fields {
                    let name = self.read_string(data, offset)?;
                    let schema = self.read_schema(data, offset, depth + 1)?;
                    match fields.iter_mut().find(|(n, _)| *n == name) {
                        Some(entry) => entry.1 = schema,
                        None => {
                            self.tick_produced()?;
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
            7 => Ok(MessageSchema::Array(Box::new(
                self.read_schema(data, offset, depth + 1)?,
            ))),
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

    fn read_msg(&self, schema: &MessageSchema, data: &[u8], offset: &mut i64, depth: u32) -> Result<MapValue, Step> {
        if depth >= MAX_DEPTH {
            return Err(Step::Fail);
        }
        Ok(match schema {
            MessageSchema::Struct(fields) => {
                let mut entries = Vec::with_capacity(fields.len());
                for (name, field) in fields {
                    entries.push((
                        name.clone(),
                        self.read_msg(field, data, offset, depth + 1)?,
                    ));
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
                    items.push(self.read_msg(inner, data, offset, depth + 1)?);
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

        loop {
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
                    shift_offset(offset, 4 * Self::array_schema_count(&self.key_schemas[key_id]));
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
                } else if let MapValue::Map(entries) = &msg {
                    if let Some(MapValue::BigInt(rr_timestamp)) = entries
                        .iter()
                        .find(|(name, _)| ascii_eq(name, "timestamp"))
                        .map(|(_, value)| value)
                    {
                        self.last_timestamp = self.rr_timestamp_to_seconds(*rr_timestamp);
                    }
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
    match RrLogDecoder::new()
        .decode(&mut log, data, &mut |value| post(Response::Progress(value)), clock)?
    {
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
