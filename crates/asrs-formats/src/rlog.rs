//! `src/hub/dataSources/rlog/RLOGDecoder.ts` and `rlogWorker.ts`: the AdvantageKit legacy
//! RLOG decoder. The same decoder serves `.rlog` files (`isFile: true`, timestamp corruption
//! checks) and the live source (`isFile: false`, one `decode` call per TCP frame payload, so
//! revision, keys and timestamp state persist across calls).

use std::collections::HashMap;
use std::sync::Arc;

use asrs_js::string::{JsString, TextDecoder};
use asrs_js::time::{Clock, SystemClock};
use asrs_log::log::log_key_utils::{PHOTON_PREFIX, PROTO_PREFIX, STRUCT_PREFIX};
use asrs_log::log::{Log, LogError, LoggableType};
use asrs_log::realm::JsRealm;

use crate::custom_schemas::{self, SchemaError};
use crate::worker::{Abort, DecodeWorker, Request, Response};

const MIN_TIMESTAMP_STEP: f64 = 0.0001;
const MAX_TIMESTAMP_STEP: f64 = 60.0;
const PROGRESS_INTERVAL_MS: f64 = 1000.0 / 60.0;

/// A JS throw inside `decode`'s try/catch (a DataView RangeError, a thrown string, a TypeError)
/// makes it return false; `Abort` is what a catch cannot express.
enum Fail {
    Js,
    Abort(Abort),
}

impl From<Abort> for Fail {
    fn from(abort: Abort) -> Self {
        Fail::Abort(abort)
    }
}

/// `dataArray[position]`; past either end is `undefined`.
fn u8_at(data: &[u8], position: i64) -> Option<u8> {
    usize::try_from(position)
        .ok()
        .and_then(|p| data.get(p))
        .copied()
}

/// A `DataView` read: out of bounds throws a RangeError, which `decode` catches.
fn bytes_at(data: &[u8], position: i64, length: usize) -> Result<&[u8], Fail> {
    let start = usize::try_from(position).map_err(|_| Fail::Js)?;
    let end = start.checked_add(length).ok_or(Fail::Js)?;
    data.get(start..end).ok_or(Fail::Js)
}

fn i16_at(data: &[u8], position: i64) -> Result<i16, Fail> {
    Ok(i16::from_be_bytes(
        bytes_at(data, position, 2)?.try_into().unwrap(),
    ))
}

fn i32_at(data: &[u8], position: i64) -> Result<i32, Fail> {
    Ok(i32::from_be_bytes(
        bytes_at(data, position, 4)?.try_into().unwrap(),
    ))
}

fn u32_at(data: &[u8], position: i64) -> Result<u32, Fail> {
    Ok(u32::from_be_bytes(
        bytes_at(data, position, 4)?.try_into().unwrap(),
    ))
}

fn i64_at(data: &[u8], position: i64) -> Result<i64, Fail> {
    Ok(i64::from_be_bytes(
        bytes_at(data, position, 8)?.try_into().unwrap(),
    ))
}

fn f32_at(data: &[u8], position: i64) -> Result<f32, Fail> {
    Ok(f32::from_be_bytes(
        bytes_at(data, position, 4)?.try_into().unwrap(),
    ))
}

fn f64_at(data: &[u8], position: i64) -> Result<f64, Fail> {
    Ok(f64::from_be_bytes(
        bytes_at(data, position, 8)?.try_into().unwrap(),
    ))
}

/// `Uint8Array.prototype.subarray(begin, end)`: negative indexes count from the end, both ends
/// clamp to the length and an inverted range is empty.
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

fn shift(offset: &mut i64, by: i64) -> i64 {
    let at = *offset;
    *offset += by;
    at
}

/// A `LogError` is a JS throw (`Proto`), an upstream hang (`NeverCompletes`) or an unmodeled
/// continuation; inside `decode` only the first is caught.
fn caught(result: Result<(), LogError>) -> Result<(), Fail> {
    match result {
        Ok(()) => Ok(()),
        Err(LogError::Proto(_)) => Err(Fail::Js),
        Err(LogError::NeverCompletes) => Err(Fail::Abort(Abort::NeverReturns)),
        Err(LogError::Unmodeled(what)) => Err(Fail::Abort(Abort::Unmodeled(what))),
    }
}

fn js(text: &str) -> JsString {
    JsString::from(text)
}

/// `type.split(prefix)[1]` for a type that starts with `prefix`.
fn schema_type(type_: &JsString, prefix: &str) -> JsString {
    type_.split(Some(&js(prefix)), None)[1].clone()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Revision {
    /// `logRevision === null`: no `decode` call has read the revision byte yet.
    Unread,
    /// Read (or `undefined` for an empty first call) but not a supported revision; `decode`
    /// already returned false and a later call runs the read loop with no value syntax.
    Other,
    R1,
    R2,
}

/// `RLOGDecoder`: `decode` may be called repeatedly, once per payload.
pub struct RlogDecoder {
    is_file: bool,
    revision: Revision,
    last_timestamp: Option<f64>,
    last_timestamp_corrupted: Option<f64>,
    last_progress_timestamp: f64,
    /// `STRING_DECODER` and `TEXT_DECODER` share `new TextDecoder("UTF-8")`: lossy, BOM strip.
    text_decoder: TextDecoder,
    key_ids: HashMap<i16, JsString>,
    key_types: HashMap<i16, JsString>,
}

impl RlogDecoder {
    pub fn new(is_file: bool) -> Self {
        RlogDecoder {
            is_file,
            revision: Revision::Unread,
            last_timestamp: None,
            last_timestamp_corrupted: None,
            last_progress_timestamp: 0.0,
            text_decoder: TextDecoder::new(Some(&js("UTF-8")), false, false)
                .expect("UTF-8 always resolves"),
            key_ids: HashMap::new(),
            key_types: HashMap::new(),
        }
    }

    /// `decode(log, dataArray, progressCallback?)`. `false` is the caught JS throw: the caller
    /// drops the log and (live) reconnects. `Err(Abort)` is what the catch cannot express. The
    /// byte offset restarts at 0 every call; only the first call skips the revision byte, so
    /// each later payload carries one marker byte before its entries.
    pub fn decode(
        &mut self,
        realm: &mut JsRealm,
        log: &mut Log,
        data: &[u8],
        clock: &dyn Clock,
        mut progress: Option<&mut dyn FnMut(f64)>,
    ) -> Result<bool, Abort> {
        match self.read(realm, log, data, clock, &mut progress) {
            Ok(()) => Ok(true),
            Err(Fail::Js) => Ok(false),
            Err(Fail::Abort(abort)) => Err(abort),
        }
    }

    fn read(
        &mut self,
        realm: &mut JsRealm,
        log: &mut Log,
        data: &[u8],
        clock: &dyn Clock,
        progress: &mut Option<&mut dyn FnMut(f64)>,
    ) -> Result<(), Fail> {
        let mut offset: i64 = 0;
        if self.revision == Revision::Unread {
            self.revision = match u8_at(data, shift(&mut offset, 1)) {
                Some(1) => Revision::R1,
                Some(2) => Revision::R2,
                _ => Revision::Other,
            };
            if self.revision == Revision::Other {
                return Err(Fail::Js);
            }
        }
        shift(&mut offset, 1);
        'main: loop {
            if offset >= data.len() as i64 {
                break 'main;
            }
            let mut timestamp = f64_at(data, shift(&mut offset, 8))?;
            if timestamp < 0.0 {
                timestamp = 0.0;
            }
            if self.is_file
                && let Some(last) = self.last_timestamp
                && (timestamp.is_nan()
                    || timestamp < last + MIN_TIMESTAMP_STEP
                    || timestamp > last + MAX_TIMESTAMP_STEP)
            {
                // The console.warn (once per `lastTimestampCorrupted`) is not observable.
                self.last_timestamp_corrupted = Some(last);
                offset -= 7;
                continue 'main;
            }
            self.last_timestamp = Some(timestamp);
            'read: loop {
                let Some(entry_type) = u8_at(data, shift(&mut offset, 1)) else {
                    break 'read;
                };
                match entry_type {
                    0 => break 'read,
                    1 => {
                        let key_id = i16_at(data, shift(&mut offset, 2))?;
                        let key_length = i64::from(i16_at(data, shift(&mut offset, 2))?);
                        let new_key = self
                            .text_decoder
                            .decode(subarray(data, offset, offset + key_length))
                            .expect("nonfatal UTF-8 never errors");
                        offset += key_length;
                        self.key_ids.insert(key_id, new_key);
                        if self.revision == Revision::R2 {
                            let type_length = i64::from(i16_at(data, shift(&mut offset, 2))?);
                            let new_type = self
                                .text_decoder
                                .decode(subarray(data, offset, offset + type_length))
                                .expect("nonfatal UTF-8 never errors");
                            offset += type_length;
                            self.key_types.insert(key_id, new_type);
                        }
                    }
                    2 => {
                        let key_id = i16_at(data, shift(&mut offset, 2))?;
                        match self.revision {
                            Revision::R2 => {
                                self.r2_value(realm, log, data, &mut offset, timestamp, key_id)?
                            }
                            Revision::R1 => {
                                self.r1_value(log, data, &mut offset, timestamp, key_id)?
                            }
                            Revision::Unread | Revision::Other => {}
                        }
                    }
                    _ => {}
                }
                if let Some(progress) = progress.as_deref_mut() {
                    let now = clock.now();
                    if now - self.last_progress_timestamp > PROGRESS_INTERVAL_MS {
                        self.last_progress_timestamp = now;
                        progress(offset as f64 / data.len() as f64);
                    }
                }
            }
        }
        Ok(())
    }

    /// `keyIDs[keyID]` is `undefined` for an undeclared id; upstream then operates on a field
    /// literally named "undefined". The first such write throws inside `createBlankField`
    /// (`endsWith` on the non-string key) after registering it; once it exists, writes land.
    fn effective_key(log: &Log, key: Option<&JsString>) -> Result<JsString, Fail> {
        match key {
            Some(key) => Ok(key.clone()),
            None => {
                let undefined = js("undefined");
                if log.get_field(&undefined).is_some() {
                    Ok(undefined)
                } else {
                    Err(Fail::Js)
                }
            }
        }
    }

    /// The `logRevision === 2` arm of an update record: `keyID` is already read; reads the
    /// value length, the value bytes and decodes by the declared type string.
    fn r2_value(
        &mut self,
        realm: &mut JsRealm,
        log: &mut Log,
        data: &[u8],
        offset: &mut i64,
        timestamp: f64,
        key_id: i16,
    ) -> Result<(), Fail> {
        let key = self.key_ids.get(&key_id);
        let value_length = i64::from(i16_at(data, shift(offset, 2))?);
        let value = subarray(data, *offset, *offset + value_length);
        let Some(type_) = self.key_types.get(&key_id).cloned() else {
            // `type` is undefined: the `default` arm calls `type.startsWith` and throws.
            return Err(Fail::Js);
        };
        match type_.as_string().as_deref() {
            Some("boolean") => {
                if value.len() != 1 {
                    return Err(Fail::Js);
                }
                let key = Self::effective_key(log, key)?;
                log.put_boolean(&key, timestamp, value[0] != 0);
            }
            Some("int" | "int64") => {
                if value.len() != 8 {
                    return Err(Fail::Js);
                }
                let number = i64_at(data, *offset)? as f64;
                let key = Self::effective_key(log, key)?;
                log.put_number(&key, timestamp, number);
            }
            Some("float") => {
                if value.len() != 4 {
                    return Err(Fail::Js);
                }
                let number = f64::from(f32_at(data, *offset)?);
                let key = Self::effective_key(log, key)?;
                log.put_number(&key, timestamp, number);
            }
            Some("double") => {
                if value.len() != 8 {
                    return Err(Fail::Js);
                }
                let number = f64_at(data, *offset)?;
                let key = Self::effective_key(log, key)?;
                log.put_number(&key, timestamp, number);
            }
            Some("string") => {
                let string = self
                    .text_decoder
                    .decode(value)
                    .expect("nonfatal UTF-8 never errors");
                let key = Self::effective_key(log, key)?;
                log.put_string(&key, timestamp, string);
            }
            Some("boolean[]") => {
                let array: Vec<bool> = value.iter().map(|x| *x != 0).collect();
                let key = Self::effective_key(log, key)?;
                log.put_boolean_array(&key, timestamp, array);
            }
            Some("int[]" | "int64[]") => {
                if !value.len().is_multiple_of(8) {
                    return Err(Fail::Js);
                }
                let mut array = Vec::with_capacity(value.len() / 8);
                let mut position = 0i64;
                while position < value.len() as i64 {
                    array.push(i64_at(data, *offset + position)? as f64);
                    position += 8;
                }
                let key = Self::effective_key(log, key)?;
                log.put_number_array(&key, timestamp, array);
            }
            Some("float[]") => {
                if !value.len().is_multiple_of(4) {
                    return Err(Fail::Js);
                }
                let mut array = Vec::with_capacity(value.len() / 4);
                let mut position = 0i64;
                while position < value.len() as i64 {
                    array.push(f64::from(f32_at(data, *offset + position)?));
                    position += 4;
                }
                let key = Self::effective_key(log, key)?;
                log.put_number_array(&key, timestamp, array);
            }
            Some("double[]") => {
                if !value.len().is_multiple_of(8) {
                    return Err(Fail::Js);
                }
                let mut array = Vec::with_capacity(value.len() / 8);
                let mut position = 0i64;
                while position < value.len() as i64 {
                    array.push(f64_at(data, *offset + position)?);
                    position += 8;
                }
                let key = Self::effective_key(log, key)?;
                log.put_number_array(&key, timestamp, array);
            }
            Some("string[]") => {
                let size = u32_at(data, *offset)?;
                if f64::from(size) > (value.len() as f64 - 4.0) / 4.0 {
                    return Err(Fail::Js);
                }
                let mut array = Vec::new();
                let mut position = 4i64;
                for _ in 0..size {
                    let string_size = i64::from(u32_at(data, offset.saturating_add(position))?);
                    array.push(
                        self.text_decoder
                            .decode(subarray(
                                value,
                                position + 4,
                                position.saturating_add(4).saturating_add(string_size),
                            ))
                            .expect("nonfatal UTF-8 never errors"),
                    );
                    position = position.saturating_add(4).saturating_add(string_size);
                }
                let key = Self::effective_key(log, key)?;
                log.put_string_array(&key, timestamp, array);
            }
            Some("json") => {
                let string = self
                    .text_decoder
                    .decode(value)
                    .expect("nonfatal UTF-8 never errors");
                let key = Self::effective_key(log, key)?;
                log.put_json(&key, timestamp, string);
            }
            Some("msgpack") => {
                let key = Self::effective_key(log, key)?;
                caught(log.put_msgpack(realm, &key, timestamp, Arc::<[u8]>::from(value)))?;
            }
            _ => {
                if type_.starts_with(&js(STRUCT_PREFIX), None) {
                    let schema = schema_type(&type_, STRUCT_PREFIX);
                    let is_array = schema.ends_with(&js("[]"), None);
                    let schema = if is_array {
                        schema.slice(0.0, Some(-2.0))
                    } else {
                        schema
                    };
                    let key = Self::effective_key(log, key)?;
                    caught(log.put_struct(
                        &key,
                        timestamp,
                        Arc::<[u8]>::from(value),
                        &schema,
                        is_array,
                    ))?;
                } else if type_.starts_with(&js(PHOTON_PREFIX), None) {
                    let schema = schema_type(&type_, PHOTON_PREFIX);
                    let key = Self::effective_key(log, key)?;
                    caught(log.put_photon_struct(
                        &key,
                        timestamp,
                        Arc::<[u8]>::from(value),
                        &schema,
                    ))?;
                } else if type_.starts_with(&js(PROTO_PREFIX), None) {
                    let schema = schema_type(&type_, PROTO_PREFIX);
                    let key = Self::effective_key(log, key)?;
                    caught(log.put_proto(
                        realm,
                        &key,
                        timestamp,
                        Arc::<[u8]>::from(value),
                        &schema,
                    ))?;
                } else {
                    let key = Self::effective_key(log, key)?;
                    caught(log.put_raw(&key, timestamp, Arc::<[u8]>::from(value)))?;
                    if let Some(parser) = custom_schemas::get(&type_) {
                        // The parser runs inside its own try/catch; the generated parent is
                        // marked either way.
                        match parser(log, &key, timestamp, value) {
                            Ok(())
                            | Err(SchemaError::Threw)
                            | Err(SchemaError::Log(LogError::Proto(_))) => {}
                            Err(SchemaError::Log(LogError::NeverCompletes)) => {
                                return Err(Fail::Abort(Abort::NeverReturns));
                            }
                            Err(SchemaError::Log(LogError::Unmodeled(what))) => {
                                return Err(Fail::Abort(Abort::Unmodeled(what)));
                            }
                        }
                        log.set_generated_parent(&key);
                    }
                }
            }
        }
        *offset += value_length;
        Ok(())
    }

    /// The `logRevision === 1` arm of an update record: a tag byte then the tag's body.
    fn r1_value(
        &mut self,
        log: &mut Log,
        data: &[u8],
        offset: &mut i64,
        timestamp: f64,
        key_id: i16,
    ) -> Result<(), Fail> {
        let key = self.key_ids.get(&key_id).cloned();
        match u8_at(data, shift(offset, 1)) {
            Some(0) => {
                // null writes the default value for the field's current type; a key that has
                // no type yet (or an undeclared id) writes nothing.
                let lookup = key.unwrap_or_else(|| js("undefined"));
                match log.get_type(&lookup) {
                    Some(LoggableType::Raw) => {
                        let key = Self::effective_key(log, Some(&lookup))?;
                        caught(log.put_raw(&key, timestamp, Vec::<u8>::new()))?;
                    }
                    Some(LoggableType::Boolean) => {
                        let key = Self::effective_key(log, Some(&lookup))?;
                        log.put_boolean(&key, timestamp, false);
                    }
                    Some(LoggableType::Number) => {
                        let key = Self::effective_key(log, Some(&lookup))?;
                        log.put_number(&key, timestamp, 0.0);
                    }
                    Some(LoggableType::String) => {
                        let key = Self::effective_key(log, Some(&lookup))?;
                        log.put_string(&key, timestamp, JsString::new());
                    }
                    Some(LoggableType::BooleanArray) => {
                        let key = Self::effective_key(log, Some(&lookup))?;
                        log.put_boolean_array(&key, timestamp, Vec::new());
                    }
                    Some(LoggableType::NumberArray) => {
                        let key = Self::effective_key(log, Some(&lookup))?;
                        log.put_number_array(&key, timestamp, Vec::new());
                    }
                    Some(LoggableType::StringArray) => {
                        let key = Self::effective_key(log, Some(&lookup))?;
                        log.put_string_array(&key, timestamp, Vec::new());
                    }
                    _ => {}
                }
            }
            Some(1) => {
                let value = u8_at(data, shift(offset, 1)) != Some(0);
                let key = Self::effective_key(log, key.as_ref())?;
                log.put_boolean(&key, timestamp, value);
            }
            Some(9) => {
                let byte = u8_at(data, shift(offset, 1)).unwrap_or(0);
                let key = Self::effective_key(log, key.as_ref())?;
                caught(log.put_raw(&key, timestamp, vec![byte]))?;
            }
            Some(3) => {
                let number = f64::from(i32_at(data, shift(offset, 4))?);
                let key = Self::effective_key(log, key.as_ref())?;
                log.put_number(&key, timestamp, number);
            }
            Some(5) => {
                let number = f64_at(data, shift(offset, 8))?;
                let key = Self::effective_key(log, key.as_ref())?;
                log.put_number(&key, timestamp, number);
            }
            Some(7) => {
                let string_length = i64::from(i16_at(data, shift(offset, 2))?);
                let string = self
                    .text_decoder
                    .decode(subarray(data, *offset, *offset + string_length))
                    .expect("nonfatal UTF-8 never errors");
                *offset += string_length;
                let key = Self::effective_key(log, key.as_ref())?;
                log.put_string(&key, timestamp, string);
            }
            Some(2) => {
                let length = i16_at(data, shift(offset, 2))?;
                let mut array = Vec::new();
                for _ in 0..length {
                    array.push(u8_at(data, shift(offset, 1)) != Some(0));
                }
                let key = Self::effective_key(log, key.as_ref())?;
                log.put_boolean_array(&key, timestamp, array);
            }
            Some(10) => {
                let length = i16_at(data, shift(offset, 2))?;
                let mut array = Vec::new();
                for _ in 0..length {
                    array.push(u8_at(data, shift(offset, 1)).unwrap_or(0));
                }
                let key = Self::effective_key(log, key.as_ref())?;
                caught(log.put_raw(&key, timestamp, array))?;
            }
            Some(4) => {
                let length = i16_at(data, shift(offset, 2))?;
                let mut array = Vec::new();
                for _ in 0..length {
                    array.push(f64::from(i32_at(data, shift(offset, 4))?));
                }
                let key = Self::effective_key(log, key.as_ref())?;
                log.put_number_array(&key, timestamp, array);
            }
            Some(6) => {
                let length = i16_at(data, shift(offset, 2))?;
                let mut array = Vec::new();
                for _ in 0..length {
                    array.push(f64_at(data, shift(offset, 8))?);
                }
                let key = Self::effective_key(log, key.as_ref())?;
                log.put_number_array(&key, timestamp, array);
            }
            Some(8) => {
                let length = i16_at(data, shift(offset, 2))?;
                let mut array = Vec::new();
                for _ in 0..length {
                    let string_length = i64::from(i16_at(data, shift(offset, 2))?);
                    array.push(
                        self.text_decoder
                            .decode(subarray(data, *offset, *offset + string_length))
                            .expect("nonfatal UTF-8 never errors"),
                    );
                    *offset += string_length;
                }
                let key = Self::effective_key(log, key.as_ref())?;
                log.put_string_array(&key, timestamp, array);
            }
            _ => {}
        }
        Ok(())
    }
}

/// `rlogWorker`: `new RLOGDecoder(true)` over the whole file; `isPartial: false`. Any other
/// request type is ignored.
pub struct RlogWorker {
    realm: JsRealm,
}

impl RlogWorker {
    pub fn new(realm: JsRealm) -> Self {
        RlogWorker { realm }
    }
}

impl DecodeWorker for RlogWorker {
    fn handle(&mut self, request: Request, post: &mut dyn FnMut(Response)) -> Result<(), Abort> {
        let Request::Start(files) = request else {
            return Ok(());
        };
        // `decoder.decode(log, request.data[0], ...)` on a missing slot throws a TypeError in
        // `new DataView(dataArray.buffer)`, out of the handler.
        let Some(data) = files.into_iter().next().flatten() else {
            return Err(Abort::Threw);
        };
        let mut log = Log::new(false);
        let mut decoder = RlogDecoder::new(true);
        let mut progress = |value: f64| post(Response::Progress(value));
        let success = decoder.decode(
            &mut self.realm,
            &mut log,
            &data,
            &SystemClock,
            Some(&mut progress),
        )?;
        if success {
            post(Response::Progress(1.0));
            post(Response::Initial {
                log: Box::new(log.to_serialized()),
                is_partial: false,
            });
        } else {
            post(Response::Failed);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use asrs_js::time::{SetClock, V8MathRandom};

    use super::*;
    use crate::worker::run_request;

    fn realm() -> JsRealm {
        JsRealm::new(Box::new(V8MathRandom::from_entropy()))
    }

    fn builder(revision: u8) -> Vec<u8> {
        vec![revision, 0]
    }

    /// A timestamp starts a cycle directly; the `0` marker byte ends the entry run.
    fn timestamp(out: &mut Vec<u8>, seconds: f64) {
        out.extend(seconds.to_be_bytes());
    }

    fn marker(out: &mut Vec<u8>) {
        out.push(0);
    }

    fn key(out: &mut Vec<u8>, id: i16, name: &str, type_: Option<&str>) {
        out.push(1);
        out.extend(id.to_be_bytes());
        out.extend((name.len() as i16).to_be_bytes());
        out.extend(name.as_bytes());
        if let Some(type_) = type_ {
            out.extend((type_.len() as i16).to_be_bytes());
            out.extend(type_.as_bytes());
        }
    }

    fn value(out: &mut Vec<u8>, id: i16, bytes: &[u8]) {
        out.push(2);
        out.extend(id.to_be_bytes());
        out.extend((bytes.len() as i16).to_be_bytes());
        out.extend(bytes);
    }

    fn samples(log: &Log, key: &str) -> (Vec<f64>, Vec<f64>) {
        let field = log.get_field(&js(key)).expect("field exists");
        let set = field
            .get_number(f64::NEG_INFINITY, f64::INFINITY, None, None)
            .expect("a Number field");
        (set.timestamps.to_vec(), set.values.to_vec())
    }

    fn decode_file(data: &[u8]) -> (Log, bool) {
        let mut realm = realm();
        let mut log = Log::new(false);
        let mut decoder = RlogDecoder::new(true);
        let ok = decoder
            .decode(&mut realm, &mut log, data, &SetClock(f64::INFINITY), None)
            .unwrap();
        (log, ok)
    }

    #[test]
    fn r2_values_decode_and_a_bad_length_fails_the_load() {
        let mut file = builder(2);
        timestamp(&mut file, 0.5);
        key(&mut file, 1, "/D", Some("double"));
        key(&mut file, 2, "/I", Some("int64"));
        value(&mut file, 1, &1.5f64.to_be_bytes());
        value(&mut file, 2, &(-9i64).to_be_bytes());
        marker(&mut file);
        timestamp(&mut file, 0.52);
        value(&mut file, 1, &f64::NEG_INFINITY.to_be_bytes());
        let (log, ok) = decode_file(&file);
        assert!(ok);
        assert_eq!(
            samples(&log, "/D"),
            (vec![0.5, 0.52], vec![1.5, f64::NEG_INFINITY])
        );
        assert_eq!(samples(&log, "/I"), (vec![0.5], vec![-9.0]));

        let mut bad = builder(2);
        timestamp(&mut bad, 0.0);
        key(&mut bad, 1, "/B", Some("boolean"));
        value(&mut bad, 1, &[1, 0]);
        assert!(!decode_file(&bad).1);
    }

    #[test]
    fn r1_values_decode_and_null_writes_defaults() {
        let mut file = builder(1);
        timestamp(&mut file, 1.0);
        key(&mut file, 1, "/N", None);
        file.push(2);
        file.extend(1i16.to_be_bytes());
        file.push(5);
        file.extend(2.5f64.to_be_bytes());
        marker(&mut file);
        timestamp(&mut file, 1.02);
        file.push(2);
        file.extend(1i16.to_be_bytes());
        file.push(0);
        let (log, ok) = decode_file(&file);
        assert!(ok);
        assert_eq!(samples(&log, "/N"), (vec![1.0, 1.02], vec![2.5, 0.0]));
    }

    #[test]
    fn corrupt_timestamps_resync_a_byte_at_a_time() {
        let mut file = builder(2);
        timestamp(&mut file, 0.5);
        key(&mut file, 1, "/V", Some("double"));
        value(&mut file, 1, &0f64.to_be_bytes());
        marker(&mut file);
        file.extend([0xff; 6]);
        timestamp(&mut file, 0.6);
        value(&mut file, 1, &1f64.to_be_bytes());
        let (log, ok) = decode_file(&file);
        assert!(ok);
        assert_eq!(samples(&log, "/V"), (vec![0.5, 0.6], vec![0.0, 1.0]));
    }

    #[test]
    fn truncation_fails_but_a_marker_only_tail_succeeds() {
        let mut file = builder(2);
        timestamp(&mut file, 0.0);
        key(&mut file, 1, "/A", Some("double"));
        value(&mut file, 1, &1f64.to_be_bytes());
        assert!(decode_file(&file).1);
        let mut cut = file.clone();
        cut.extend([0, 0x3f, 0xf0]);
        assert!(!decode_file(&cut).1);
        let mut clean = file.clone();
        clean.push(0);
        assert!(decode_file(&clean).1);
        let _ = file;
    }

    #[test]
    fn an_undeclared_id_fails_the_whole_load() {
        let mut r2 = builder(2);
        timestamp(&mut r2, 0.0);
        key(&mut r2, 1, "/A", Some("double"));
        value(&mut r2, 77, &1f64.to_be_bytes());
        assert!(!decode_file(&r2).1);

        let mut r1 = builder(1);
        timestamp(&mut r1, 0.0);
        key(&mut r1, 1, "/A", None);
        r1.extend([2]);
        r1.extend(1i16.to_be_bytes());
        r1.push(0);
        r1.extend([2]);
        r1.extend(77i16.to_be_bytes());
        r1.push(0);
        let (log, ok) = decode_file(&r1);
        assert!(ok);
        assert!(log.get_field(&js("/A")).is_none());
        assert!(log.get_field(&js("undefined")).is_none());
    }

    #[test]
    fn live_chunks_carry_a_marker_byte_and_skip_corruption_checks() {
        let mut first = builder(2);
        timestamp(&mut first, 0.5);
        key(&mut first, 1, "/V", Some("double"));
        value(&mut first, 1, &0f64.to_be_bytes());
        let mut second = Vec::new();
        marker(&mut second);
        timestamp(&mut second, 0.0);
        value(&mut second, 1, &1f64.to_be_bytes());
        let mut third = Vec::new();
        marker(&mut third);
        timestamp(&mut third, 0.6);
        key(&mut third, 2, "/W", Some("boolean"));
        value(&mut third, 2, &[1]);

        let mut realm = realm();
        let mut log = Log::new(false);
        let mut decoder = RlogDecoder::new(false);
        for chunk in [&first, &second, &third] {
            assert!(
                decoder
                    .decode(&mut realm, &mut log, chunk, &SetClock(0.0), None)
                    .unwrap()
            );
        }
        assert_eq!(samples(&log, "/V"), (vec![0.0, 0.5], vec![1.0, 0.0]));
        let field = log.get_field(&js("/W")).expect("live-declared key");
        assert_eq!(field.get_type(), LoggableType::Boolean);
    }

    #[test]
    fn the_worker_posts_progress_then_initial_or_failed() {
        let mut file = builder(2);
        timestamp(&mut file, 0.0);
        key(&mut file, 1, "/A", Some("double"));
        value(&mut file, 1, &1f64.to_be_bytes());
        let mut worker = RlogWorker::new(realm());
        let (responses, end) =
            run_request(&mut worker, Request::Start(vec![Some(Arc::from(file))]));
        assert_eq!(end, Ok(()));
        assert!(matches!(responses.first(), Some(Response::Progress(_))));
        assert!(matches!(
            responses[responses.len() - 2],
            Response::Progress(1.0)
        ));
        assert!(matches!(
            responses.last(),
            Some(Response::Initial {
                is_partial: false,
                ..
            })
        ));

        let mut worker = RlogWorker::new(realm());
        let (responses, end) = run_request(&mut worker, Request::Start(vec![None]));
        assert!(responses.is_empty());
        assert_eq!(end, Err(Abort::Threw));

        let mut worker = RlogWorker::new(realm());
        let (responses, end) = run_request(
            &mut worker,
            Request::Start(vec![Some(Arc::from(vec![9, 0]))]),
        );
        assert_eq!(end, Ok(()));
        assert!(matches!(responses.last(), Some(Response::Failed)));
    }
}
