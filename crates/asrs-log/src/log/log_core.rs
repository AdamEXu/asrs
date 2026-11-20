//! `shared/log/Log.ts`: the field map, the plain and JSON writers, timestamp bookkeeping, the
//! field tree and serialization.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use asrs_js::json;
use asrs_js::sort::array_sort;
use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue};
use indexmap::IndexSet;

use super::LoggableType;
use super::log_field::{LogField, SerializedLogField};
use super::log_field_tree::LogFieldTree;
use super::log_key_utils::split_log_key;
use super::log_value_sets::{LogValue, LogValueSet, LogValueSetAny, RawValue};
use crate::util::lower_bound;

const DEFAULT_TIMESTAMP_RANGE: [f64; 2] = [0.0, 10.0];
const TYPE_KEY_SUFFIX: &str = "/.type";
const CONSOLE_PATH: &str = "DS:/Dscomm/Console/";
const CONSOLE_SUFFIXES: [&str; 3] = ["ConsoleLine", "ErrorInfo", "ProgramCrashInfo"];

/// `Preferences.timestamps`, the one preference `getTimestampDisplayOffset` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimestampDisplay {
    StartZero,
    Original,
}

#[derive(Clone, Debug, Default)]
struct TimestampSetCache {
    keys: Vec<JsString>,
    timestamps: Vec<f64>,
    source_counts: Vec<usize>,
}

#[derive(Debug, Default)]
struct TimestampSetCaches(Mutex<HashMap<String, TimestampSetCache>>);

impl TimestampSetCaches {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, TimestampSetCache>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn get_mut(&mut self) -> &mut HashMap<String, TimestampSetCache> {
        self.0.get_mut().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Clone for TimestampSetCaches {
    fn clone(&self) -> Self {
        TimestampSetCaches(Mutex::new(self.lock().clone()))
    }
}

/// A collection of log fields.
///
/// The hub's `Log` lives on the foreground thread. `clone` is the snapshot handed to export and
/// other worker threads: it costs O(fields) because every column is shared copy-on-write.
#[derive(Clone, Debug)]
pub struct Log {
    pub(super) fields: JsObject<LogField>,
    pub(super) generated_parents: IndexSet<JsString>,
    pub(super) timestamp_range: Option<[f64; 2]>,
    pub(super) timestamp_zero: Option<f64>,
    enable_timestamp_set_cache: bool,
    timestamp_set_cache: TimestampSetCaches,
    pub(super) changed_fields: IndexSet<JsString>,
}

/// `Log.toSerialized()` for the parts this module owns. Columns are shared with the log.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SerializedLog {
    pub fields: JsObject<SerializedLogField>,
    pub generated_parents: Vec<JsString>,
    pub timestamp_range: Option<[f64; 2]>,
    pub timestamp_zero: Option<f64>,
}

impl Default for Log {
    fn default() -> Self {
        Log::new(true)
    }
}

fn child_key(key: &JsString, child: &str) -> JsString {
    child_key_js(key, &JsString::from(child))
}

fn child_key_js(key: &JsString, child: &JsString) -> JsString {
    let mut out = key.clone();
    out.push_unit(u16::from(b'/'));
    out.push_str(child);
    out
}

fn ends_with_ascii(units: &[u16], suffix: &str) -> bool {
    units.len() >= suffix.len()
        && suffix
            .bytes()
            .rev()
            .zip(units.iter().rev())
            .all(|(a, &b)| u16::from(a) == b)
}

/// `[...new Set(timestamps.flat())].sort((a, b) => a - b)`. `Set` treats `-0` as `+0` and keeps
/// one NaN.
fn timestamp_union<'a>(columns: impl Iterator<Item = &'a [f64]>) -> Vec<f64> {
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    for column in columns {
        for &timestamp in column {
            let value = if timestamp == 0.0 { 0.0 } else { timestamp };
            let identity = if value.is_nan() {
                f64::NAN.to_bits()
            } else {
                value.to_bits()
            };
            if seen.insert(identity) {
                output.push(value);
            }
        }
    }
    array_sort(&mut output, |a, b| a - b);
    output
}

impl Log {
    pub fn new(enable_timestamp_set_cache: bool) -> Self {
        Log {
            fields: JsObject::new(),
            generated_parents: IndexSet::new(),
            timestamp_range: None,
            timestamp_zero: None,
            enable_timestamp_set_cache,
            timestamp_set_cache: TimestampSetCaches::default(),
            changed_fields: IndexSet::new(),
        }
    }

    fn has_field(&self, key: &JsString) -> bool {
        self.fields.get_key(key).is_some()
    }

    fn field_mut_existing(&mut self, key: &JsString) -> &mut LogField {
        self.fields
            .get_units_mut(key.units())
            .expect("the field was created before this write")
    }

    pub fn create_blank_field(&mut self, key: &JsString, ty: LoggableType) {
        if self.has_field(key) {
            return;
        }
        self.fields.insert(key.clone(), LogField::new(ty));
        self.changed_fields.insert(key.clone());
        self.check_ds_console_field(key);
    }

    /// Gives the Driver Station console table an Empty parent typed "Console".
    fn check_ds_console_field(&mut self, key: &JsString) {
        let units = key.units();
        let is_console = CONSOLE_SUFFIXES.iter().any(|suffix| {
            let path_length = CONSOLE_PATH.len() + suffix.len();
            let Some(head) = units.len().checked_sub(path_length) else {
                return false;
            };
            ends_with_ascii(units, suffix)
                && ends_with_ascii(&units[..units.len() - suffix.len()], CONSOLE_PATH)
                && (head == 0 || units[head - 1] == u16::from(b'/'))
        });
        if !is_console {
            return;
        }
        let suffix_length = CONSOLE_SUFFIXES
            .iter()
            .find(|suffix| ends_with_ascii(units, suffix))
            .map_or(0, |suffix| suffix.len());
        let console_key = JsString::from_units(units[..units.len() - (suffix_length + 1)].to_vec());
        if !self.has_field(&console_key) {
            self.create_blank_field(&console_key, LoggableType::Empty);
            self.set_structured_type(&console_key, Some(JsString::from("Console")));
        }
    }

    pub fn delete_field(&mut self, key: &JsString) {
        if self.fields.remove_key(key).is_none() {
            return;
        }
        self.generated_parents.shift_remove(key);
        self.changed_fields.shift_remove(key);

        let fields = &self.fields;
        for cache in self.timestamp_set_cache.get_mut().values_mut() {
            if cache.keys.contains(key) {
                cache.keys.retain(|x| x != key);
                let columns = cache.keys.iter().map(|k| {
                    fields
                        .get_key(k)
                        .map_or(&[][..], |field| field.get_timestamps())
                });
                cache.timestamps = timestamp_union(columns);
                cache.source_counts = cache
                    .keys
                    .iter()
                    .map(|k| fields.get_key(k).map_or(0, LogField::len))
                    .collect();
            }
        }
    }

    pub fn clear_before_time(&mut self, timestamp: f64, update_zero: bool) {
        match &mut self.timestamp_range {
            None => self.timestamp_range = Some([timestamp, timestamp]),
            Some(range) => {
                if range[0] < timestamp {
                    range[0] = timestamp;
                    if range[1] < range[0] {
                        range[1] = range[0];
                    }
                }
            }
        }
        if update_zero && self.timestamp_zero.is_none_or(|zero| zero < timestamp) {
            self.timestamp_zero = Some(timestamp);
        }
        for cache in self.timestamp_set_cache.get_mut().values_mut() {
            let timestamps = &mut cache.timestamps;
            let drop = timestamps
                .windows(2)
                .take_while(|pair| pair[1] <= timestamp)
                .count();
            timestamps.drain(..drop);
            if let Some(first) = timestamps.first_mut()
                && *first < timestamp
            {
                *first = timestamp;
            }
        }
        for (_, field) in self.fields.iter_mut() {
            field.clear_before_time(timestamp);
        }
    }

    pub fn update_range_with_timestamp(&mut self, timestamp: f64) {
        match &mut self.timestamp_range {
            None => self.timestamp_range = Some([timestamp, timestamp]),
            Some(range) => {
                if timestamp < range[0] {
                    range[0] = timestamp;
                } else if timestamp > range[1] {
                    range[1] = timestamp;
                }
            }
        }
        if self.timestamp_zero.is_none_or(|zero| timestamp < zero) {
            self.timestamp_zero = Some(timestamp);
        }
    }

    /// `processTimestamp`: updates the range and splices the timestamp into every timestamp set
    /// cache that includes the key.
    pub(super) fn process_timestamp(&mut self, key: &JsString, timestamp: f64) {
        self.update_range_with_timestamp(timestamp);
        if !self.enable_timestamp_set_cache {
            return;
        }
        for cache in self.timestamp_set_cache.get_mut().values_mut() {
            if cache.keys.contains(key) {
                let timestamps = &mut cache.timestamps;
                let insert_index = lower_bound(timestamps, timestamp, 0);
                if timestamps
                    .get(insert_index)
                    .is_none_or(|&existing| existing != timestamp)
                {
                    timestamps.insert(insert_index, timestamp);
                }
            }
        }
    }

    /// Returns the keys changed since the last call, in the order they first changed.
    pub fn get_changed_fields(&mut self) -> IndexSet<JsString> {
        std::mem::take(&mut self.changed_fields)
    }

    /// Field keys in JS property order: integer-like keys ascending, then insertion order.
    pub fn field_keys(&self) -> impl DoubleEndedIterator<Item = &JsString> + '_ {
        self.fields.keys()
    }

    pub fn get_field_keys(&self) -> Vec<JsString> {
        self.fields.keys().cloned().collect()
    }

    /// The number of fields, excluding generated children.
    pub fn get_field_count(&self) -> usize {
        self.fields
            .keys()
            .filter(|key| !self.is_generated(key))
            .count()
    }

    pub fn get_field(&self, key: &JsString) -> Option<&LogField> {
        self.fields.get_key(key)
    }

    pub fn get_field_mut(&mut self, key: &JsString) -> Option<&mut LogField> {
        self.fields.get_units_mut(key.units())
    }

    /// Adds an existing field, replacing any field with the same key in place.
    pub fn set_field(&mut self, key: &JsString, field: LogField) {
        self.fields.insert(key.clone(), field);
        self.changed_fields.insert(key.clone());
        self.check_ds_console_field(key);
    }

    pub fn get_type(&self, key: &JsString) -> Option<LoggableType> {
        self.fields.get_key(key).map(LogField::get_type)
    }

    pub fn get_striping_reference(&self, key: &JsString) -> bool {
        self.fields
            .get_key(key)
            .is_some_and(LogField::get_striping_reference)
    }

    pub fn get_structured_type(&self, key: &JsString) -> Option<&JsString> {
        self.fields.get_key(key)?.structured_type.as_ref()
    }

    pub fn set_structured_type(&mut self, key: &JsString, ty: Option<JsString>) {
        if let Some(field) = self.fields.get_units_mut(key.units()) {
            field.structured_type = ty;
            self.changed_fields.insert(key.clone());
        }
    }

    pub fn get_wpilib_type(&self, key: &JsString) -> Option<&JsString> {
        self.fields.get_key(key)?.wpilib_type.as_ref()
    }

    pub fn set_wpilib_type(&mut self, key: &JsString, ty: JsString) {
        if let Some(field) = self.fields.get_units_mut(key.units()) {
            field.wpilib_type = Some(ty);
            self.changed_fields.insert(key.clone());
        }
    }

    /// The field's metadata string, or `""` without a field.
    pub fn get_metadata_string(&self, key: &JsString) -> JsString {
        self.fields
            .get_key(key)
            .map(|field| field.metadata_string.clone())
            .unwrap_or_default()
    }

    pub fn set_metadata_string(&mut self, key: &JsString, metadata: JsString) {
        if let Some(field) = self.fields.get_units_mut(key.units()) {
            field.metadata_string = metadata;
            self.changed_fields.insert(key.clone());
        }
    }

    pub fn get_type_warning(&self, key: &JsString) -> bool {
        self.fields
            .get_key(key)
            .is_some_and(|field| field.type_warning)
    }

    pub fn is_generated(&self, key: &JsString) -> bool {
        self.get_generated_parent(key).is_some()
    }

    /// The first generated parent, in the order parents were registered, that `key` lies
    /// strictly below.
    pub fn get_generated_parent(&self, key: &JsString) -> Option<&JsString> {
        let units = key.units();
        let slash = u16::from(b'/');
        let first = units
            .iter()
            .enumerate()
            .filter(|&(i, &unit)| unit == slash && i + 1 < units.len())
            .filter_map(|(i, _)| self.generated_parents.get_index_of(&units[..i]))
            .min()?;
        self.generated_parents.get_index(first)
    }

    pub fn is_generated_parent(&self, key: &JsString) -> bool {
        self.generated_parents.contains(key)
    }

    pub fn set_generated_parent(&mut self, key: &JsString) {
        self.generated_parents.insert(key.clone());
    }

    /// The generated parents in registration order.
    pub fn generated_parents(&self) -> impl Iterator<Item = &JsString> + '_ {
        self.generated_parents.iter()
    }

    /// The sorted union of the fields' timestamps (keys without a field are skipped).
    ///
    /// With a `uuid`, the key list and union are cached, later writes to those keys are spliced
    /// into the cached union, and the next call with the same keys and sample counts returns the
    /// cache, which can differ from a fresh union exactly as upstream's does.
    pub fn get_timestamps(&self, keys: &[JsString], uuid: Option<&str>) -> Vec<f64> {
        let keys: Vec<&JsString> = keys.iter().filter(|key| self.has_field(key)).collect();
        let sample_counts = |keys: &[&JsString]| -> Vec<usize> {
            keys.iter()
                .map(|key| self.fields.get_key(key).map_or(0, LogField::len))
                .collect()
        };
        match keys.as_slice() {
            [] => Vec::new(),
            [key] => self
                .fields
                .get_key(key)
                .map(|field| field.get_timestamps().to_vec())
                .unwrap_or_default(),
            _ => {
                let mut caches = self.timestamp_set_cache.lock();
                let mut save_cache = false;
                if let Some(uuid) = uuid
                    && self.enable_timestamp_set_cache
                {
                    if let Some(cache) = caches.get(uuid)
                        && cache.keys.len() == keys.len()
                        && cache.keys.iter().zip(&keys).all(|(a, b)| a == *b)
                        && cache.source_counts == sample_counts(&keys)
                    {
                        return cache.timestamps.clone();
                    }
                    caches.insert(
                        uuid.to_owned(),
                        TimestampSetCache {
                            keys: keys.iter().map(|&key| key.clone()).collect(),
                            timestamps: Vec::new(),
                            source_counts: vec![0; keys.len()],
                        },
                    );
                    save_cache = true;
                }

                let output = timestamp_union(keys.iter().map(|key| {
                    self.fields
                        .get_key(key)
                        .map_or(&[][..], |field| field.get_timestamps())
                }));
                // Upstream saves only for a truthy uuid, so "" keeps the blank entry it just made.
                if save_cache
                    && let Some(uuid) = uuid.filter(|uuid| !uuid.is_empty())
                    && let Some(cache) = caches.get_mut(uuid)
                {
                    cache.timestamps = output.clone();
                    cache.source_counts = sample_counts(&keys);
                }
                output
            }
        }
    }

    /// The range of timestamps across all fields, `[0, 10]` before any timestamp is known.
    pub fn get_timestamp_range(&self) -> [f64; 2] {
        self.timestamp_range.unwrap_or(DEFAULT_TIMESTAMP_RANGE)
    }

    pub fn get_timestamp_zero(&self) -> Option<f64> {
        self.timestamp_zero
    }

    /// The value added to timestamps before display; `None` when no preferences are loaded.
    pub fn get_timestamp_display_offset(&self, timestamps: Option<TimestampDisplay>) -> f64 {
        match timestamps {
            Some(TimestampDisplay::StartZero) => -(self.timestamp_zero.unwrap_or(0.0)),
            Some(TimestampDisplay::Original) | None => 0.0,
        }
    }

    /// The most recent timestamp across all fields, `None` (upstream's `undefined`) when there
    /// are none.
    pub fn get_last_timestamp(&self) -> Option<f64> {
        self.get_timestamps(&self.get_field_keys(), None)
            .last()
            .copied()
    }

    /// Organizes the fields under `prefix` into a tree split on the key separators.
    pub fn get_field_tree(
        &self,
        include_generated: bool,
        prefix: &JsString,
    ) -> JsObject<LogFieldTree> {
        let mut root = JsObject::new();
        for key in self.fields.keys() {
            if !key.units().starts_with(prefix.units()) {
                continue;
            }
            if !include_generated && self.is_generated(key) {
                continue;
            }
            let key = JsString::from_units(key.units()[prefix.len()..].to_vec());
            let slash = u16::from(b'/');
            let skip = usize::from(key.units().first() == Some(&slash));
            let tables: Vec<JsString> =
                split_log_key(&JsString::from_units(key.units()[skip..].to_vec()))
                    .into_iter()
                    .filter(|table| !table.is_empty())
                    .collect();
            if let Some(node) = tree_node(&mut root, &tables)
                && (node.full_key.is_none() || key.units().last() != Some(&slash))
            {
                node.full_key = Some(key);
            }
        }
        root
    }

    /// `getRange`. Upstream accepts a `startOffset` here but never forwards it.
    pub fn get_range(
        &self,
        key: &JsString,
        start: f64,
        end: f64,
        uuid: Option<&str>,
    ) -> Option<LogValueSetAny> {
        Some(self.fields.get_key(key)?.get_range(start, end, uuid, None))
    }

    /// The typed getters. `getRaw` does not forward `startOffset` upstream, so a Raw read
    /// ignores it; every other type applies it.
    pub fn get<T: LogValue>(
        &self,
        key: &JsString,
        start: f64,
        end: f64,
        uuid: Option<&str>,
        start_offset: Option<i64>,
    ) -> Option<LogValueSet<T>> {
        let start_offset = if T::TYPE == LoggableType::Raw {
            None
        } else {
            start_offset
        };
        self.fields
            .get_key(key)?
            .get(start, end, uuid, start_offset)
    }

    pub fn get_raw(
        &self,
        key: &JsString,
        start: f64,
        end: f64,
        uuid: Option<&str>,
        start_offset: Option<i64>,
    ) -> Option<LogValueSet<RawValue>> {
        self.get(key, start, end, uuid, start_offset)
    }

    pub fn get_boolean(
        &self,
        key: &JsString,
        start: f64,
        end: f64,
        uuid: Option<&str>,
        start_offset: Option<i64>,
    ) -> Option<LogValueSet<bool>> {
        self.get(key, start, end, uuid, start_offset)
    }

    pub fn get_number(
        &self,
        key: &JsString,
        start: f64,
        end: f64,
        uuid: Option<&str>,
        start_offset: Option<i64>,
    ) -> Option<LogValueSet<f64>> {
        self.get(key, start, end, uuid, start_offset)
    }

    pub fn get_string(
        &self,
        key: &JsString,
        start: f64,
        end: f64,
        uuid: Option<&str>,
        start_offset: Option<i64>,
    ) -> Option<LogValueSet<JsString>> {
        self.get(key, start, end, uuid, start_offset)
    }

    pub fn get_boolean_array(
        &self,
        key: &JsString,
        start: f64,
        end: f64,
        uuid: Option<&str>,
        start_offset: Option<i64>,
    ) -> Option<LogValueSet<Vec<bool>>> {
        self.get(key, start, end, uuid, start_offset)
    }

    pub fn get_number_array(
        &self,
        key: &JsString,
        start: f64,
        end: f64,
        uuid: Option<&str>,
        start_offset: Option<i64>,
    ) -> Option<LogValueSet<Vec<f64>>> {
        self.get(key, start, end, uuid, start_offset)
    }

    pub fn get_string_array(
        &self,
        key: &JsString,
        start: f64,
        end: f64,
        uuid: Option<&str>,
        start_offset: Option<i64>,
    ) -> Option<LogValueSet<Vec<JsString>>> {
        self.get(key, start, end, uuid, start_offset)
    }

    /// The shared shape of the scalar writers: register, write, and count the timestamp only
    /// when the field really holds `T`.
    fn put_value<T: LogValue>(&mut self, key: &JsString, timestamp: f64, value: T) {
        self.create_blank_field(key, T::TYPE);
        self.field_mut_existing(key).put(timestamp, value);
        self.changed_fields.insert(key.clone());
        if self.get_type(key) == Some(T::TYPE) {
            self.process_timestamp(key, timestamp);
        }
    }

    /// Writes a Raw value. Schema registration for `.schema/struct:` and
    /// `.schema/photonstruct:` keys belongs to the structured writers.
    pub fn put_raw(&mut self, key: &JsString, timestamp: f64, value: impl Into<RawValue>) {
        self.put_value(key, timestamp, value.into());
    }

    pub fn put_boolean(&mut self, key: &JsString, timestamp: f64, value: bool) {
        self.put_value(key, timestamp, value);
    }

    pub fn put_number(&mut self, key: &JsString, timestamp: f64, value: f64) {
        self.put_value(key, timestamp, value);
    }

    /// Writes a String value; a `.../.type` key also types its parent as an Empty field.
    pub fn put_string(&mut self, key: &JsString, timestamp: f64, value: JsString) {
        self.put_value(key, timestamp, value.clone());
        let units = key.units();
        if ends_with_ascii(units, TYPE_KEY_SUFFIX) {
            let parent_key =
                JsString::from_units(units[..units.len() - TYPE_KEY_SUFFIX.len()].to_vec());
            self.create_blank_field(&parent_key, LoggableType::Empty);
            self.changed_fields.insert(parent_key.clone());
            self.process_timestamp(&parent_key, timestamp);
            self.set_structured_type(&parent_key, Some(value));
        }
    }

    fn put_array<T: LogValue>(&mut self, key: &JsString, timestamp: f64, value: Vec<T>)
    where
        Vec<T>: LogValue,
    {
        let ty = <Vec<T> as LogValue>::TYPE;
        self.create_blank_field(key, ty);
        self.field_mut_existing(key).put(timestamp, value.clone());
        self.changed_fields.insert(key.clone());
        if self.get_type(key) != Some(ty) {
            return;
        }
        self.process_timestamp(key, timestamp);
        self.set_generated_parent(key);

        let length_key = child_key(key, "length");
        self.create_blank_field(&length_key, LoggableType::Number);
        self.process_timestamp(&length_key, timestamp);
        self.field_mut_existing(&length_key)
            .put_number(timestamp, value.len() as f64);
        self.changed_fields.insert(length_key);

        for (i, item) in value.into_iter().enumerate() {
            let item_key = child_key(key, &format!("{i}"));
            if self.enable_timestamp_set_cache {
                self.process_timestamp(&item_key, timestamp);
            }
            self.create_blank_field(&item_key, T::TYPE);
            self.field_mut_existing(&item_key).put(timestamp, item);
            self.changed_fields.insert(item_key);
        }
    }

    pub fn put_boolean_array(&mut self, key: &JsString, timestamp: f64, value: Vec<bool>) {
        self.put_array(key, timestamp, value);
    }

    pub fn put_number_array(&mut self, key: &JsString, timestamp: f64, value: Vec<f64>) {
        self.put_array(key, timestamp, value);
    }

    pub fn put_string_array(&mut self, key: &JsString, timestamp: f64, value: Vec<JsString>) {
        self.put_array(key, timestamp, value);
    }

    /// Flattens a decoded value into the children of `key`. Primitives and byte arrays are only
    /// written below the root unless `allow_root_write`; uniform arrays below the root become
    /// typed array fields; anything else recurses per element or per property, in JS property
    /// order.
    pub fn put_unknown_struct(
        &mut self,
        key: &JsString,
        timestamp: f64,
        value: &JsValue,
        allow_root_write: bool,
    ) {
        match value {
            JsValue::Bool(b) => {
                if allow_root_write {
                    self.put_boolean(key, timestamp, *b);
                }
            }
            JsValue::Number(x) => {
                if allow_root_write {
                    self.put_number(key, timestamp, *x);
                }
            }
            JsValue::String(s) => {
                if allow_root_write {
                    self.put_string(key, timestamp, s.clone());
                }
            }
            JsValue::Bytes(bytes) => {
                if allow_root_write {
                    self.put_raw(key, timestamp, Arc::<[u8]>::from(bytes.as_slice()));
                }
            }
            JsValue::Array(items) => {
                self.put_unknown_array(key, timestamp, items, allow_root_write)
            }
            JsValue::Object(object) => {
                for (name, item) in object.iter() {
                    self.put_unknown_struct(&child_key_js(key, name), timestamp, item, true);
                }
            }
            JsValue::ExtData(kind, data) => {
                self.put_number(&child_key(key, "type"), timestamp, f64::from(*kind));
                self.put_raw(
                    &child_key(key, "data"),
                    timestamp,
                    Arc::<[u8]>::from(data.as_slice()),
                );
            }
            JsValue::Null | JsValue::Undefined | JsValue::BigInt(_) | JsValue::Date(_) => {}
        }
    }

    fn put_unknown_array(
        &mut self,
        key: &JsString,
        timestamp: f64,
        items: &[JsValue],
        allow_root_write: bool,
    ) {
        let existing_type = self.get_type(key);
        let uniform = |type_name: &str, array_type: LoggableType| {
            if items.is_empty() {
                existing_type == Some(array_type)
            } else {
                items.iter().all(|item| item.type_of() == type_name)
            }
        };
        if allow_root_write && uniform("boolean", LoggableType::BooleanArray) {
            let values = items.iter().filter_map(JsValue::as_bool).collect();
            self.put_boolean_array(key, timestamp, values);
        } else if allow_root_write && uniform("number", LoggableType::NumberArray) {
            let values = items.iter().filter_map(JsValue::as_f64).collect();
            self.put_number_array(key, timestamp, values);
        } else if allow_root_write && uniform("string", LoggableType::StringArray) {
            let values = items
                .iter()
                .filter_map(|item| item.as_str().cloned())
                .collect();
            self.put_string_array(key, timestamp, values);
        } else {
            let length_key = child_key(key, "length");
            self.create_blank_field(&length_key, LoggableType::Number);
            self.process_timestamp(&length_key, timestamp);
            self.field_mut_existing(&length_key)
                .put_number(timestamp, items.len() as f64);
            for (i, item) in items.iter().enumerate() {
                self.put_unknown_struct(&child_key(key, &format!("{i}")), timestamp, item, true);
            }
        }
    }

    /// Writes a JSON string and its decoded children; text that does not parse writes only the
    /// string.
    pub fn put_json(&mut self, key: &JsString, timestamp: f64, value: JsString) {
        self.put_string(key, timestamp, value.clone());
        if self.get_type(key) != Some(LoggableType::String) {
            return;
        }
        self.set_generated_parent(key);
        self.set_structured_type(key, Some(JsString::from("JSON")));
        let decoded = json::parse(&value).unwrap_or(JsValue::Null);
        if !decoded.is_null() {
            self.put_unknown_struct(key, timestamp, &decoded, false);
        }
    }

    /// Writes a pose as `translation/x`, `translation/y` and `rotation/value` under a "Pose2d"
    /// parent.
    pub fn put_pose(
        &mut self,
        key: &JsString,
        timestamp: f64,
        translation: [f64; 2],
        rotation: f64,
    ) {
        let translation_key = child_key(key, "translation");
        let rotation_key = child_key(key, "rotation");
        self.put_number(&child_key(&translation_key, "x"), timestamp, translation[0]);
        self.put_number(&child_key(&translation_key, "y"), timestamp, translation[1]);
        self.put_number(&child_key(&rotation_key, "value"), timestamp, rotation);
        if !self.has_field(key) {
            self.create_blank_field(key, LoggableType::Empty);
            self.set_structured_type(key, Some(JsString::from("Pose2d")));
            self.set_generated_parent(key);
            self.process_timestamp(key, timestamp);
        }
        if !self.has_field(&translation_key) {
            self.create_blank_field(&translation_key, LoggableType::Empty);
            self.set_structured_type(&translation_key, Some(JsString::from("Translation2d")));
            self.process_timestamp(&translation_key, timestamp);
        }
        if !self.has_field(&rotation_key) {
            self.create_blank_field(&rotation_key, LoggableType::Empty);
            self.set_structured_type(&rotation_key, Some(JsString::from("Rotation2d")));
            self.process_timestamp(&rotation_key, timestamp);
        }
    }

    /// Writes translations as `i/x` and `i/y` children under a "Translation2d[]" parent.
    pub fn put_translation_array(
        &mut self,
        key: &JsString,
        timestamp: f64,
        translations: &[[f64; 2]],
    ) {
        if !self.has_field(key) {
            self.create_blank_field(key, LoggableType::Empty);
            self.set_structured_type(key, Some(JsString::from("Translation2d[]")));
            self.set_generated_parent(key);
            self.process_timestamp(key, timestamp);
        }
        self.put_number(
            &child_key(key, "length"),
            timestamp,
            translations.len() as f64,
        );
        for (i, translation) in translations.iter().enumerate() {
            let item_key = child_key(key, &format!("{i}"));
            if !self.has_field(&item_key) {
                self.create_blank_field(&item_key, LoggableType::Empty);
                self.set_structured_type(&item_key, Some(JsString::from("Translation2d")));
                self.process_timestamp(&item_key, timestamp);
            }
            self.put_number(&child_key(&item_key, "x"), timestamp, translation[0]);
            self.put_number(&child_key(&item_key, "y"), timestamp, translation[1]);
        }
    }

    pub fn to_serialized(&self) -> SerializedLog {
        SerializedLog {
            fields: self.fields.map_values(LogField::to_serialized),
            generated_parents: self.generated_parents.iter().cloned().collect(),
            timestamp_range: self.timestamp_range,
            timestamp_zero: self.timestamp_zero,
        }
    }

    /// `Log.fromSerialized`: a log with the timestamp set cache enabled and nothing marked
    /// changed.
    pub fn from_serialized(serialized: SerializedLog) -> Self {
        let mut log = Log::new(true);
        for (key, field) in serialized.fields {
            log.fields.insert(key, LogField::from_serialized(field));
        }
        log.generated_parents = serialized.generated_parents.into_iter().collect();
        log.timestamp_range = serialized.timestamp_range;
        log.timestamp_zero = serialized.timestamp_zero;
        log
    }
}

fn tree_node<'a>(
    root: &'a mut JsObject<LogFieldTree>,
    tables: &[JsString],
) -> Option<&'a mut LogFieldTree> {
    let (first, rest) = tables.split_first()?;
    let mut node = child_node(root, first);
    for table in rest {
        node = child_node(&mut node.children, table);
    }
    Some(node)
}

fn child_node<'a>(
    children: &'a mut JsObject<LogFieldTree>,
    table: &JsString,
) -> &'a mut LogFieldTree {
    if children.get_key(table).is_none() {
        children.insert(table.clone(), LogFieldTree::default());
    }
    children
        .get_units_mut(table.units())
        .expect("the child was just inserted")
}
