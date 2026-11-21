//! Port of `src/hub/dataSources/wpilog/wpilogWorker.ts`: indexes the file on `start`, decodes
//! schemas and the robot-state fields eagerly, and every other field on `parseField`.

use std::collections::HashMap;
use std::sync::Arc;

use asrs_js::string::JsString;
use asrs_log::log::log_key_utils::{
    PHOTON_PREFIX, PROTO_PREFIX, STRUCT_PREFIX, get_structured_type_from_raw, is_schema,
};
use asrs_log::log::robot_state::{
    AUTONOMOUS_KEYS, ENABLED_KEYS, PROGRAM_START_TIME_KEYS, UTILITY_KEYS, get_enabled_key,
    get_program_start_time,
};
use asrs_log::log::{Log, LogError, LoggableType};
use asrs_log::realm::JsRealm;

use crate::custom_schemas::{self, SchemaError};
use crate::worker::{Abort, DecodeWorker, FieldResponse, Request, Response};
use crate::wpilog::{self, CONTROL_ENTRY, WpilogDecoder, WpilogRecord};

/// A throw inside `start`'s `try`: the indexer's trap or a malformed control record.
struct Failed;

/// The `try` around a single record's write. A JS throw is caught and the record skipped;
/// everything else ends the request.
fn caught(result: Result<(), LogError>) -> Result<(), Abort> {
    match result {
        Ok(()) | Err(LogError::Proto(_) | LogError::StackOverflow) => Ok(()),
        Err(LogError::NeverCompletes) => Err(Abort::NeverReturns),
        Err(LogError::Unmodeled(what)) => Err(Abort::Unmodeled(what)),
    }
}

/// `type.split(prefix)[1]` for a type that starts with `prefix`.
fn schema_type(type_: &JsString, prefix: &str) -> JsString {
    type_.split(Some(&JsString::from(prefix)), None)[1].clone()
}

fn js(text: &str) -> JsString {
    JsString::from(text)
}

pub struct WpilogWorker {
    realm: JsRealm,
    data: Arc<[u8]>,
    log: Log,
    entry_ids: HashMap<u32, JsString>,
    entry_types: HashMap<JsString, JsString>,
    entry_start_times: HashMap<u32, f64>,
    data_record_positions: HashMap<JsString, Vec<u32>>,
    clear_time: Option<f64>,
}

impl WpilogWorker {
    pub fn new(realm: JsRealm) -> Self {
        WpilogWorker {
            realm,
            data: Arc::from(Vec::new()),
            log: Log::new(false),
            entry_ids: HashMap::new(),
            entry_types: HashMap::new(),
            entry_start_times: HashMap::new(),
            data_record_positions: HashMap::new(),
            clear_time: None,
        }
    }

    fn start(
        &mut self,
        data: Option<Arc<[u8]>>,
        post: &mut dyn FnMut(Response),
    ) -> Result<(), Abort> {
        self.clear_time = None;
        let Some(data) = data else {
            return Err(Abort::Threw);
        };
        self.data = data;
        if self.index(post).is_err() {
            post(Response::Failed);
            return Ok(());
        }

        for key in self.log.get_field_keys() {
            if is_schema(&key) {
                self.parse_field(key, true, post)?;
            }
        }
        let eager_keys = [
            ENABLED_KEYS,
            AUTONOMOUS_KEYS,
            UTILITY_KEYS,
            PROGRAM_START_TIME_KEYS,
        ]
        .concat();
        for key in self.log.get_field_keys() {
            let mut child_prefix = key.clone();
            child_prefix.push_str(&js("/"));
            let is_eager = eager_keys.iter().any(|eager| {
                let eager = js(eager);
                eager.starts_with(&child_prefix, None) || eager == key
            });
            if is_eager {
                self.parse_field(key, true, post)?;
            }
        }

        if let Some(enabled_key) = get_enabled_key(&self.log) {
            self.parse_field(enabled_key, true, post)?;
        }

        self.clear_time = get_program_start_time(&self.log);
        if let Some(clear_time) = self.clear_time {
            self.log.clear_before_time(clear_time, true);
        }

        self.log.get_changed_fields();
        post(Response::Initial {
            log: Box::new(self.log.to_serialized()),
            is_partial: true,
        });
        Ok(())
    }

    fn index(&mut self, post: &mut dyn FnMut(Response)) -> Result<(), Failed> {
        let data = Arc::clone(&self.data);
        let index = wpilog::index(&data).map_err(|_| Failed)?;
        self.log.update_range_with_timestamp(index.min_timestamp);
        self.log.update_range_with_timestamp(index.max_timestamp);

        let decoder = WpilogDecoder::new(&data);
        let mut last_progress = 0.0;
        for &(entry, position) in &index.records {
            if entry == CONTROL_ENTRY {
                if let Some((record, _)) = decoder.record_at(position as usize) {
                    self.control_record(&record)?;
                }
            } else if let Some(key) = self.entry_ids.get(&entry)
                && let Some(positions) = self.data_record_positions.get_mut(key)
            {
                positions.push(position);
            }

            let progress = f64::from(position) / data.len() as f64;
            if progress - last_progress > 0.01 {
                last_progress = progress;
                post(Response::Progress(progress));
            }
        }
        Ok(())
    }

    fn control_record(&mut self, record: &WpilogRecord<'_>) -> Result<(), Failed> {
        if record.is_start() {
            let start = record.start_data().map_err(|_| Failed)?;
            if !self.data_record_positions.contains_key(&start.name)
                || self.entry_types.get(&start.name) != Some(&start.type_)
            {
                self.data_record_positions
                    .insert(start.name.clone(), Vec::new());
            }
            self.entry_ids.insert(start.entry, start.name.clone());
            self.entry_types
                .insert(start.name.clone(), start.type_.clone());
            self.entry_start_times
                .insert(start.entry, record.timestamp());
            let loggable_type = match start.type_.as_string().as_deref() {
                Some("boolean") => LoggableType::Boolean,
                Some("int" | "int64" | "float" | "double") => LoggableType::Number,
                Some("string" | "json") => LoggableType::String,
                Some("boolean[]") => LoggableType::BooleanArray,
                Some("int[]" | "int64[]" | "float[]" | "double[]") => LoggableType::NumberArray,
                Some("string[]") => LoggableType::StringArray,
                _ => LoggableType::Raw,
            };
            self.log.create_blank_field(&start.name, loggable_type);
            if loggable_type == LoggableType::Raw
                && let Some(schema_type) = get_structured_type_from_raw(&start.type_)
            {
                self.log.set_structured_type(&start.name, Some(schema_type));
            }
            self.log.set_wpilib_type(&start.name, start.type_);
            self.log.set_metadata_string(&start.name, start.metadata);
        } else if record.is_finish() {
            let entry = record.finish_entry().map_err(|_| Failed)?;
            let lifetime = match self.entry_start_times.get(&entry) {
                Some(start) => record.timestamp() - start,
                None => f64::NAN,
            };
            if lifetime < 1e6
                && let Some(name) = self.entry_ids.remove(&entry)
            {
                self.log.delete_field(&name);
                self.entry_start_times.remove(&entry);
            }
        } else if record.is_set_metadata() {
            let metadata = record.set_metadata_data().map_err(|_| Failed)?;
            if let Some(key) = self.entry_ids.get(&metadata.entry) {
                let key = key.clone();
                self.log.set_metadata_string(&key, metadata.metadata);
            }
        }
        Ok(())
    }

    fn put_record(
        &mut self,
        key: &JsString,
        type_: &JsString,
        record: &WpilogRecord<'_>,
    ) -> Result<(), Abort> {
        let timestamp = record.timestamp() / 1000000.0;
        let log = &mut self.log;
        match type_.as_string().as_deref() {
            Some("boolean") => {
                if let Ok(value) = record.boolean() {
                    log.put_boolean(key, timestamp, value);
                }
            }
            Some("int" | "int64") => {
                if let Ok(value) = record.integer() {
                    log.put_number(key, timestamp, value);
                }
            }
            Some("float") => {
                if let Ok(value) = record.float() {
                    log.put_number(key, timestamp, value);
                }
            }
            Some("double") => {
                if let Ok(value) = record.double() {
                    log.put_number(key, timestamp, value);
                }
            }
            Some("string") => log.put_string(key, timestamp, record.string()),
            Some("boolean[]") => log.put_boolean_array(key, timestamp, record.boolean_array()),
            Some("int[]" | "int64[]") => {
                if let Ok(value) = record.integer_array() {
                    log.put_number_array(key, timestamp, value);
                }
            }
            Some("float[]") => {
                if let Ok(value) = record.float_array() {
                    log.put_number_array(key, timestamp, value);
                }
            }
            Some("double[]") => {
                if let Ok(value) = record.double_array() {
                    log.put_number_array(key, timestamp, value);
                }
            }
            Some("string[]") => {
                if let Ok(value) = record.string_array() {
                    log.put_string_array(key, timestamp, value);
                }
            }
            Some("json") => caught(log.put_json(key, timestamp, record.string()))?,
            Some("msgpack") => {
                return caught(log.put_msgpack(
                    &mut self.realm,
                    key,
                    timestamp,
                    record.raw().into(),
                ));
            }
            _ => {
                let raw = record.raw();
                if type_.starts_with(&js(STRUCT_PREFIX), None) {
                    let schema = schema_type(type_, STRUCT_PREFIX);
                    let array = schema.ends_with(&js("[]"), None);
                    let schema = if array {
                        schema.slice(0.0, Some(-2.0))
                    } else {
                        schema
                    };
                    return caught(log.put_struct(key, timestamp, raw.into(), &schema, array));
                }
                if type_.starts_with(&js(PHOTON_PREFIX), None) {
                    let schema = schema_type(type_, PHOTON_PREFIX);
                    return caught(log.put_photon_struct(key, timestamp, raw.into(), &schema));
                }
                if type_.starts_with(&js(PROTO_PREFIX), None) {
                    let schema = schema_type(type_, PROTO_PREFIX);
                    return caught(log.put_proto(
                        &mut self.realm,
                        key,
                        timestamp,
                        raw.into(),
                        &schema,
                    ));
                }
                caught(log.put_raw(key, timestamp, raw.clone()))?;
                if let Some(parser) = custom_schemas::get(type_) {
                    match parser(log, key, timestamp, &raw) {
                        Ok(()) | Err(SchemaError::Threw) => {}
                        Err(SchemaError::Log(error)) => caught(Err(error))?,
                    }
                    log.set_generated_parent(key);
                }
            }
        }
        Ok(())
    }

    fn parse_field(
        &mut self,
        mut key: JsString,
        skip_message: bool,
        post: &mut dyn FnMut(Response),
    ) -> Result<(), Abort> {
        if !self.entry_types.contains_key(&key) {
            if key.starts_with(&js("/"), None) {
                key = key.slice(1.0, None);
            }
            if !self.entry_types.contains_key(&key) {
                return Ok(());
            }
        }
        if let Some(positions) = self.data_record_positions.remove(&key) {
            let type_ = self.entry_types[&key].clone();
            let data = Arc::clone(&self.data);
            let decoder = WpilogDecoder::new(&data);
            for position in positions {
                if let Some((record, _)) = decoder.record_at(position as usize) {
                    self.put_record(&key, &type_, &record)?;
                }
            }
        }

        if let Some(clear_time) = self.clear_time {
            self.log.clear_before_time(clear_time, true);
        }

        let mut fields = Vec::new();
        let mut has_root = false;
        for child_key in self.log.get_changed_fields() {
            let data = self
                .log
                .get_field(&child_key)
                .ok_or(Abort::Threw)?
                .to_serialized();
            if child_key == key {
                has_root = true;
            }
            fields.push(FieldResponse {
                generated_parent: self.log.is_generated_parent(&child_key),
                key: child_key,
                data,
            });
        }
        if !has_root && let Some(field) = self.log.get_field(&key) {
            fields.push(FieldResponse {
                data: field.to_serialized(),
                generated_parent: self.log.is_generated_parent(&key),
                key,
            });
        }

        if !skip_message {
            post(Response::Fields(fields));
        }
        Ok(())
    }
}

impl DecodeWorker for WpilogWorker {
    fn handle(&mut self, request: Request, post: &mut dyn FnMut(Response)) -> Result<(), Abort> {
        match request {
            Request::Start(files) => self.start(files.into_iter().next().flatten(), post),
            Request::ParseField(key) => self.parse_field(key, false, post),
        }
    }
}

#[cfg(test)]
mod tests {
    use asrs_js::time::V8MathRandom;

    use super::*;
    use crate::worker::run_request;

    fn record(out: &mut Vec<u8>, entry: u32, timestamp: u64, payload: &[u8]) {
        out.push(0x7f);
        out.extend(entry.to_le_bytes());
        out.extend((payload.len() as u32).to_le_bytes());
        out.extend(timestamp.to_le_bytes());
        out.extend(payload);
    }

    fn string(out: &mut Vec<u8>, text: &str) {
        out.extend((text.len() as u32).to_le_bytes());
        out.extend(text.as_bytes());
    }

    fn start(out: &mut Vec<u8>, entry: u32, timestamp: u64, name: &str, type_: &str) {
        let mut payload = vec![0];
        payload.extend(entry.to_le_bytes());
        string(&mut payload, name);
        string(&mut payload, type_);
        string(&mut payload, "");
        record(out, 0, timestamp, &payload);
    }

    fn finish(out: &mut Vec<u8>, entry: u32, timestamp: u64) {
        let mut payload = vec![1];
        payload.extend(entry.to_le_bytes());
        record(out, 0, timestamp, &payload);
    }

    fn header(extra_header_size: i32) -> Vec<u8> {
        let mut out = b"WPILOG".to_vec();
        out.extend(0x0100u16.to_le_bytes());
        out.extend(extra_header_size.to_le_bytes());
        out
    }

    fn worker() -> WpilogWorker {
        WpilogWorker::new(JsRealm::new(Box::new(V8MathRandom::from_entropy())))
    }

    fn run_start(worker: &mut WpilogWorker, file: Vec<u8>) -> Vec<Response> {
        let (responses, end) = run_request(worker, Request::Start(vec![Some(Arc::from(file))]));
        assert_eq!(end, Ok(()));
        responses
    }

    fn parse(worker: &mut WpilogWorker, key: &str) -> Option<Vec<(String, Vec<f64>)>> {
        let (responses, end) = run_request(worker, Request::ParseField(js(key)));
        assert_eq!(end, Ok(()));
        match responses.as_slice() {
            [] => None,
            [Response::Fields(fields)] => Some(
                fields
                    .iter()
                    .map(|f| {
                        (
                            f.key.as_string().unwrap(),
                            f.data.timestamps.as_slice().to_vec(),
                        )
                    })
                    .collect(),
            ),
            _ => panic!("parseField posts at most one fields message"),
        }
    }

    fn initial_keys(responses: &[Response]) -> Vec<String> {
        match responses.last() {
            Some(Response::Initial { log, is_partial }) => {
                assert!(*is_partial);
                log.fields
                    .iter()
                    .map(|(k, _)| k.as_string().unwrap())
                    .collect()
            }
            _ => panic!("start ends with an initial message"),
        }
    }

    #[test]
    fn a_short_lived_entry_is_dropped_but_a_request_brings_it_back() {
        let mut file = header(0);
        start(&mut file, 1, 0, "Short", "double");
        record(&mut file, 1, 10, &1.5f64.to_le_bytes());
        finish(&mut file, 1, 999_999);
        start(&mut file, 2, 0, "Kept", "int64");
        record(&mut file, 2, 2_500_000, &7i64.to_le_bytes());
        let mut worker = worker();
        let responses = run_start(&mut worker, file);
        assert_eq!(initial_keys(&responses), ["Kept"]);
        assert_eq!(
            parse(&mut worker, "/Kept"),
            Some(vec![("Kept".to_owned(), vec![2.5])])
        );
        assert_eq!(
            parse(&mut worker, "Short"),
            Some(vec![("Short".to_owned(), vec![0.00001])])
        );
        assert_eq!(parse(&mut worker, "Missing"), None);
    }

    #[test]
    fn progress_is_posted_each_time_it_grows_by_more_than_one_percent() {
        let mut file = header(0);
        start(&mut file, 1, 0, "Value", "boolean");
        for i in 0..400 {
            record(&mut file, 1, i, &[1]);
        }
        let length = file.len() as f64;
        let mut worker = worker();
        let progress: Vec<f64> = run_start(&mut worker, file)
            .iter()
            .filter_map(|r| match r {
                Response::Progress(value) => Some(*value),
                _ => None,
            })
            .collect();
        assert!(progress.len() > 70 && progress.len() < 100);
        assert!(progress.windows(2).all(|w| w[1] - w[0] > 0.01));
        assert_eq!(progress[0], 76.0 / length);
    }

    #[test]
    fn an_indexer_trap_fails_and_a_missing_file_throws() {
        let mut worker = worker();
        let responses = run_start(&mut worker, header(-70_000));
        assert!(matches!(responses.as_slice(), [Response::Failed]));
        let (responses, end) = run_request(&mut self::worker(), Request::Start(vec![None]));
        assert!(responses.is_empty());
        assert_eq!(end, Err(Abort::Threw));
    }
}
