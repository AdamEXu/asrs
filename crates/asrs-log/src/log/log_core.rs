//! `shared/log/Log.ts`: the field map, the plain and JSON writers, timestamp bookkeeping, the
//! field tree and serialization.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use asrs_js::json;
use asrs_js::sort::array_sort;
use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue, KeyState};
use indexmap::IndexSet;

use super::LoggableType;
use super::log_field::{LogField, SerializedLogField, StructuredType};
use super::log_field_tree::LogFieldTree;
use super::log_key_utils::{PHOTON_PREFIX, STRUCT_PREFIX, get_schema_type, split_log_key};
use super::log_value_sets::{LogValue, LogValueSet, LogValueSetAny, RawValue};
use super::log_writers::{FieldUnitCache, LogError, QueuedStructure};
use super::photon_struct_decoder::PhotonStructDecoder;
use super::proto_decoder::{ProtoDecoder, SerializedProtoDecoder};
use super::struct_decoder::{SerializedStructDecoder, StructDecoder};
use crate::realm::JsRealm;
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
    pub(super) generated_parents: IndexSet<JsString, KeyState>,
    pub(super) timestamp_range: Option<[f64; 2]>,
    pub(super) timestamp_zero: Option<f64>,
    enable_timestamp_set_cache: bool,
    timestamp_set_cache: TimestampSetCaches,
    pub(super) changed_fields: IndexSet<JsString, KeyState>,
    pub(super) field_unit_cache: FieldUnitCache,
    pub(super) struct_decoder: StructDecoder,
    pub(super) proto_decoder: ProtoDecoder,
    pub(super) photon_decoder: PhotonStructDecoder,
    pub(super) queued_structs: Vec<QueuedStructure>,
    pub(super) queued_struct_arrays: Vec<QueuedStructure>,
    pub(super) queued_protos: Vec<QueuedStructure>,
}

/// `Log.toSerialized()`. Columns are shared with the log. The photon decoder is not part of it,
/// as upstream.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SerializedLog {
    pub fields: JsObject<SerializedLogField>,
    pub generated_parents: Vec<JsString>,
    pub timestamp_range: Option<[f64; 2]>,
    pub timestamp_zero: Option<f64>,
    pub struct_decoder: SerializedStructDecoder,
    pub proto_decoder: SerializedProtoDecoder,
    pub queued_structs: Vec<QueuedStructure>,
    pub queued_struct_arrays: Vec<QueuedStructure>,
    pub queued_protos: Vec<QueuedStructure>,
}

/// A `Map`-producing decoder's message value, for `put_unknown_map`. `Map` entries keep
/// insertion order where `JsValue::Object` would reorder array-index keys first.
#[derive(Clone, Debug)]
pub enum MapValue {
    /// `undefined`: `putUnknownStruct` writes nothing for it.
    Undefined,
    Bool(bool),
    Number(f64),
    BigInt(i64),
    Text(JsString),
    Map(Vec<(JsString, MapValue)>),
    Array(Vec<MapValue>),
}

/// How deep upstream's recursive `putUnknownStruct` gets before V8 throws `RangeError`, as
/// measured in the oracle's decode worker: about 9500 levels, a little more for arrays than
/// for objects. Like every JS recursion bound it depends on frame sizes, JIT tier and the
/// stack in use, so values within a few hundred levels of it are not byte-exact.
const MAX_UNKNOWN_DEPTH: usize = 9415;

/// What `put_unknown_tree` writes for one node of either input shape: a leaf field, a
/// uniform-array candidate, named children, or the `ExtData` pair.
enum UnknownPut<'a, V> {
    Skip,
    Bool(bool),
    Number(f64),
    Text(&'a JsString),
    Raw(&'a [u8]),
    Items(&'a [V]),
    Entries(Box<dyn Iterator<Item = (&'a JsString, &'a V)> + 'a>),
    Ext(i8, &'a [u8]),
}

/// The children of an open array or object in `put_unknown_tree`, with each child's key built
/// only when it is visited.
enum Children<'a, V> {
    Items(std::iter::Enumerate<std::slice::Iter<'a, V>>),
    Entries(Box<dyn Iterator<Item = (&'a JsString, &'a V)> + 'a>),
}

impl<'a, V> Children<'a, V> {
    fn next(&mut self, parent: &JsString) -> Option<(JsString, &'a V)> {
        match self {
            Children::Items(items) => items.next().map(|(i, item)| {
                let mut key = child_key(parent, "");
                push_decimal(&mut key, i);
                (key, item)
            }),
            Children::Entries(entries) => entries
                .next()
                .map(|(name, item)| (child_key_js(parent, name), item)),
        }
    }
}

fn js_value_put(value: &JsValue) -> UnknownPut<'_, JsValue> {
    match value {
        JsValue::Bool(b) => UnknownPut::Bool(*b),
        JsValue::Number(x) => UnknownPut::Number(*x),
        JsValue::String(s) => UnknownPut::Text(s),
        JsValue::Bytes(bytes) => UnknownPut::Raw(bytes),
        JsValue::Array(items) => UnknownPut::Items(items),
        JsValue::Object(object) => UnknownPut::Entries(Box::new(object.iter())),
        JsValue::ExtData(kind, data) => UnknownPut::Ext(*kind, data),
        _ => UnknownPut::Skip,
    }
}

fn map_value_put(value: &MapValue) -> UnknownPut<'_, MapValue> {
    match value {
        MapValue::Bool(b) => UnknownPut::Bool(*b),
        MapValue::Number(x) => UnknownPut::Number(*x),
        MapValue::Text(s) => UnknownPut::Text(s),
        MapValue::Map(entries) => {
            UnknownPut::Entries(Box::new(entries.iter().map(|(k, v)| (k, v))))
        }
        MapValue::Array(items) => UnknownPut::Items(items),
        MapValue::Undefined | MapValue::BigInt(_) => UnknownPut::Skip,
    }
}

/// An element of an array `putUnknownStruct` flattens, as `checkArrayType` and the typed
/// array writers see it.
trait UnknownItem {
    fn type_of(&self) -> &'static str;
    fn as_bool(&self) -> Option<bool>;
    fn as_f64(&self) -> Option<f64>;
    fn as_text(&self) -> Option<&JsString>;
}

impl UnknownItem for JsValue {
    fn type_of(&self) -> &'static str {
        JsValue::type_of(self)
    }
    fn as_bool(&self) -> Option<bool> {
        JsValue::as_bool(self)
    }
    fn as_f64(&self) -> Option<f64> {
        JsValue::as_f64(self)
    }
    fn as_text(&self) -> Option<&JsString> {
        self.as_str()
    }
}

impl UnknownItem for MapValue {
    fn type_of(&self) -> &'static str {
        match self {
            MapValue::Undefined => "undefined",
            MapValue::Bool(_) => "boolean",
            MapValue::Number(_) => "number",
            MapValue::BigInt(_) => "bigint",
            MapValue::Text(_) => "string",
            MapValue::Map(_) | MapValue::Array(_) => "object",
        }
    }
    fn as_bool(&self) -> Option<bool> {
        match self {
            MapValue::Bool(b) => Some(*b),
            _ => None,
        }
    }
    fn as_f64(&self) -> Option<f64> {
        match self {
            MapValue::Number(x) => Some(*x),
            _ => None,
        }
    }
    fn as_text(&self) -> Option<&JsString> {
        match self {
            MapValue::Text(s) => Some(s),
            _ => None,
        }
    }
}

impl Default for Log {
    fn default() -> Self {
        Log::new(true)
    }
}

fn child_key(key: &JsString, child: &str) -> JsString {
    let mut units = Vec::with_capacity(key.len() + 1 + child.len());
    units.extend_from_slice(key.units());
    units.push(u16::from(b'/'));
    units.extend(child.encode_utf16());
    JsString::from_units(units)
}

pub(super) fn child_key_js(key: &JsString, child: &JsString) -> JsString {
    let mut units = Vec::with_capacity(key.len() + 1 + child.len());
    units.extend_from_slice(key.units());
    units.push(u16::from(b'/'));
    units.extend_from_slice(child.units());
    JsString::from_units(units)
}

fn push_decimal(out: &mut JsString, mut value: usize) {
    let mut digits = [0u16; 20];
    let mut start = digits.len();
    loop {
        start -= 1;
        digits[start] = u16::from(b'0') + (value % 10) as u16;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    for &digit in &digits[start..] {
        out.push_unit(digit);
    }
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
pub(crate) fn timestamp_union<'a>(columns: impl Iterator<Item = &'a [f64]>) -> Vec<f64> {
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
            generated_parents: IndexSet::default(),
            timestamp_range: None,
            timestamp_zero: None,
            enable_timestamp_set_cache,
            timestamp_set_cache: TimestampSetCaches::default(),
            changed_fields: IndexSet::default(),
            field_unit_cache: FieldUnitCache::default(),
            struct_decoder: StructDecoder::new(),
            proto_decoder: ProtoDecoder::new(),
            photon_decoder: PhotonStructDecoder::new(),
            queued_structs: Vec::new(),
            queued_struct_arrays: Vec::new(),
            queued_protos: Vec::new(),
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
        self.field_unit_cache.remove(key);
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
    pub fn get_changed_fields(&mut self) -> IndexSet<JsString, KeyState> {
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

    /// `getStructuredType`.
    pub fn structured_type(&self, key: &JsString) -> &StructuredType {
        self.fields
            .get_key(key)
            .map_or(&StructuredType::Null, |f| &f.structured_type)
    }

    /// `getStructuredType` for callers to which `null` and `undefined` are the same.
    pub fn get_structured_type(&self, key: &JsString) -> Option<&JsString> {
        self.structured_type(key).as_named()
    }

    pub fn set_structured_type(&mut self, key: &JsString, ty: impl Into<StructuredType>) {
        if let Some(field) = self.fields.get_units_mut(key.units()) {
            field.structured_type = ty.into();
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
            self.field_unit_cache.remove(key);
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
        if !self.generated_parents.contains(key) {
            self.generated_parents.insert(key.clone());
        }
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
        if self.register_and_put(key, timestamp, value) == T::TYPE {
            self.process_timestamp(key, timestamp);
        }
    }

    /// Creates the field if needed, writes the value and marks the field changed; the result is
    /// the type the field holds.
    fn register_and_put<T: LogValue>(
        &mut self,
        key: &JsString,
        timestamp: f64,
        value: T,
    ) -> LoggableType {
        self.create_blank_field(key, T::TYPE);
        let field = self.field_mut_existing(key);
        field.put(timestamp, value);
        let ty = field.get_type();
        self.mark_changed(key);
        ty
    }

    fn mark_changed(&mut self, key: &JsString) {
        if !self.changed_fields.contains(key) {
            self.changed_fields.insert(key.clone());
        }
    }

    /// Writes a Raw value. A `.schema/struct:` or `.schema/photonstruct:` key also registers the
    /// schema and retries every queued structure.
    pub fn put_raw(
        &mut self,
        key: &JsString,
        timestamp: f64,
        value: impl Into<RawValue>,
    ) -> Result<(), LogError> {
        let value = value.into();
        self.put_value(key, timestamp, value.clone());
        let Some(schema_type) = get_schema_type(key) else {
            return Ok(());
        };
        let struct_prefix = JsString::from(STRUCT_PREFIX);
        let photon_prefix = JsString::from(PHOTON_PREFIX);
        if schema_type.starts_with(&struct_prefix, None) {
            let name = schema_type.slice(struct_prefix.len() as f64, None);
            self.struct_decoder.add_schema(&name, &value);
            self.photon_decoder.add_schema(&name, &value);
            self.attempt_queued_structures()?;
        }
        if schema_type.starts_with(&photon_prefix, None) {
            let name = schema_type.slice(photon_prefix.len() as f64, None);
            self.photon_decoder.add_schema(&name, &value);
            self.attempt_queued_structures()?;
        }
        Ok(())
    }

    pub fn put_boolean(&mut self, key: &JsString, timestamp: f64, value: bool) {
        self.put_value(key, timestamp, value);
    }

    pub fn put_number(&mut self, key: &JsString, timestamp: f64, value: f64) {
        self.put_value(key, timestamp, value);
    }

    /// A typed writer given `null` (`putNumber(key, timestamp, null)`): `putData` drops the
    /// value, but the field is still registered, a mismatched type still warns and the
    /// timestamp still counts.
    pub fn put_null(&mut self, key: &JsString, timestamp: f64, ty: LoggableType) {
        self.create_blank_field(key, ty);
        let field = self.field_mut_existing(key);
        if field.get_type() != ty {
            field.type_warning = true;
        }
        self.changed_fields.insert(key.clone());
        if self.get_type(key) == Some(ty) {
            self.process_timestamp(key, timestamp);
        }
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
        if self.register_and_put(key, timestamp, value.clone()) != ty {
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

        let mut item_key = child_key(key, "");
        let prefix_length = item_key.len();
        for (i, item) in value.into_iter().enumerate() {
            item_key.truncate(prefix_length);
            push_decimal(&mut item_key, i);
            if self.enable_timestamp_set_cache {
                self.process_timestamp(&item_key, timestamp);
            }
            self.register_and_put(&item_key, timestamp, item);
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
    /// typed array fields; anything else descends per element or per property, in JS property
    /// order.
    pub fn put_unknown_struct(
        &mut self,
        key: &JsString,
        timestamp: f64,
        value: &JsValue,
        allow_root_write: bool,
    ) -> Result<(), LogError> {
        self.put_unknown_tree(key, timestamp, value, allow_root_write, js_value_put)
    }

    /// `putUnknownStruct` for a `Map`-shaped input: `Map.entries()` iterates in insertion
    /// order, where `JsValue::Object` would reorder array-index keys first.
    pub fn put_unknown_map(
        &mut self,
        key: &JsString,
        timestamp: f64,
        value: &MapValue,
        allow_root_write: bool,
    ) -> Result<(), LogError> {
        self.put_unknown_tree(key, timestamp, value, allow_root_write, map_value_put)
    }

    /// `putUnknownStruct` on an explicit stack of open arrays and objects: each visited node
    /// writes a leaf or a typed array, or opens its children. The pre-order DFS matches
    /// upstream's recursion, so field creation order and `length` writes land the same.
    fn put_unknown_tree<'a, V: UnknownItem>(
        &mut self,
        key: &JsString,
        timestamp: f64,
        value: &'a V,
        allow_root_write: bool,
        classify: impl Fn(&'a V) -> UnknownPut<'a, V>,
    ) -> Result<(), LogError> {
        let mut open: Vec<(JsString, Children<'a, V>, usize)> = Vec::new();
        let mut next = Some((key.clone(), value, allow_root_write, 0));
        loop {
            let (key, value, allow_write, depth) = match next.take() {
                Some(node) => node,
                None => {
                    let Some((parent, children, depth)) = open.last_mut() else {
                        break;
                    };
                    match children.next(parent) {
                        Some((key, value)) => (key, value, true, *depth),
                        None => {
                            open.pop();
                            continue;
                        }
                    }
                }
            };
            if depth > MAX_UNKNOWN_DEPTH {
                return Err(LogError::StackOverflow);
            }
            match classify(value) {
                UnknownPut::Skip => {}
                UnknownPut::Bool(b) => {
                    if allow_write {
                        self.put_boolean(&key, timestamp, b);
                    }
                }
                UnknownPut::Number(x) => {
                    if allow_write {
                        self.put_number(&key, timestamp, x);
                    }
                }
                UnknownPut::Text(s) => {
                    if allow_write {
                        self.put_string(&key, timestamp, s.clone());
                    }
                }
                UnknownPut::Raw(bytes) => {
                    if allow_write {
                        self.put_raw(&key, timestamp, Arc::<[u8]>::from(bytes))?;
                    }
                }
                UnknownPut::Items(items) => {
                    let existing_type = self.get_type(&key);
                    let uniform = |type_name: &str, array_type: LoggableType| {
                        if items.is_empty() {
                            existing_type == Some(array_type)
                        } else {
                            items.iter().all(|item| item.type_of() == type_name)
                        }
                    };
                    if allow_write && uniform("boolean", LoggableType::BooleanArray) {
                        let values = items.iter().filter_map(V::as_bool).collect();
                        self.put_boolean_array(&key, timestamp, values);
                    } else if allow_write && uniform("number", LoggableType::NumberArray) {
                        let values = items.iter().filter_map(V::as_f64).collect();
                        self.put_number_array(&key, timestamp, values);
                    } else if allow_write && uniform("string", LoggableType::StringArray) {
                        let values = items
                            .iter()
                            .filter_map(|item| item.as_text().cloned())
                            .collect();
                        self.put_string_array(&key, timestamp, values);
                    } else {
                        let length_key = child_key(&key, "length");
                        self.create_blank_field(&length_key, LoggableType::Number);
                        self.process_timestamp(&length_key, timestamp);
                        self.field_mut_existing(&length_key)
                            .put_number(timestamp, items.len() as f64);
                        open.push((key, Children::Items(items.iter().enumerate()), depth + 1));
                    }
                }
                UnknownPut::Entries(entries) => {
                    open.push((key, Children::Entries(entries), depth + 1))
                }
                UnknownPut::Ext(kind, data) => {
                    self.put_number(&child_key(&key, "type"), timestamp, f64::from(kind));
                    self.put_raw(&child_key(&key, "data"), timestamp, Arc::<[u8]>::from(data))?;
                }
            }
        }
        Ok(())
    }

    /// Writes a JSON string and its decoded children; text that does not parse writes only the
    /// string. The only error is [`LogError::StackOverflow`]: JSON values hold no bytes, so
    /// they never reach `putRaw`.
    pub fn put_json(
        &mut self,
        key: &JsString,
        timestamp: f64,
        value: JsString,
    ) -> Result<(), LogError> {
        self.put_string(key, timestamp, value.clone());
        if self.get_type(key) != Some(LoggableType::String) {
            return Ok(());
        }
        self.set_generated_parent(key);
        self.set_structured_type(key, Some(JsString::from("JSON")));
        let decoded = json::parse(&value).unwrap_or(JsValue::Null);
        if decoded.is_null() {
            return Ok(());
        }
        self.put_unknown_struct(key, timestamp, &decoded, false)
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
            struct_decoder: self.struct_decoder.to_serialized(),
            proto_decoder: self.proto_decoder.to_serialized(),
            queued_structs: self.queued_structs.clone(),
            queued_struct_arrays: self.queued_struct_arrays.clone(),
            queued_protos: self.queued_protos.clone(),
        }
    }

    /// `Log.fromSerialized` in `realm`: a log with the timestamp set cache enabled and nothing
    /// marked changed. Rebuilding the proto decoder re-adds every descriptor, which can throw.
    pub fn from_serialized(
        serialized: SerializedLog,
        realm: &mut JsRealm,
    ) -> Result<Self, LogError> {
        let mut log = Log::new(true);
        for (key, field) in serialized.fields {
            log.fields.insert(key, LogField::from_serialized(field));
        }
        log.generated_parents = serialized.generated_parents.into_iter().collect();
        log.timestamp_range = serialized.timestamp_range;
        log.timestamp_zero = serialized.timestamp_zero;
        log.struct_decoder = StructDecoder::from_serialized(serialized.struct_decoder);
        log.proto_decoder =
            ProtoDecoder::from_serialized(&mut realm.proto, &serialized.proto_decoder)
                .map_err(LogError::Proto)?;
        log.queued_structs = serialized.queued_structs;
        log.queued_struct_arrays = serialized.queued_struct_arrays;
        log.queued_protos = serialized.queued_protos;
        Ok(log)
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
