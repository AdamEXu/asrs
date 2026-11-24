//! `shared/log/LogField.ts`.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Mutex, PoisonError};

use asrs_js::string::JsString;
use asrs_js::value::JsValue;

use super::LoggableType;
use super::log_key_utils::is_child_log;
use super::log_value_sets::{LogValue, LogValueSet, LogValueSetAny, LogValues, RawValue};
use crate::util::{lower_bound, upper_bound};

/// `fields[key].structuredType`: `string | null` normally, but `putPhotonStruct` stores
/// `undefined` verbatim for a field whose declaration had no type token, and
/// `fromSerialized` restores whatever it was handed.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum StructuredType {
    #[default]
    Null,
    Undefined,
    Named(JsString),
    /// Any other value `setStructuredType` or `fromSerialized` received.
    Other(JsValue),
}

impl StructuredType {
    /// The type name when it is a string; `x?.startsWith(...)` on any other value is
    /// `undefined`, which every consumer treats as absent.
    pub fn as_named(&self) -> Option<&JsString> {
        match self {
            StructuredType::Named(s) => Some(s),
            _ => None,
        }
    }

    pub fn from_named(ty: Option<JsString>) -> Self {
        ty.map_or(StructuredType::Null, StructuredType::Named)
    }

    pub fn from_js(v: &JsValue) -> Self {
        match v {
            JsValue::Undefined => StructuredType::Undefined,
            JsValue::Null => StructuredType::Null,
            JsValue::String(s) => StructuredType::Named(s.clone()),
            other => StructuredType::Other(other.clone()),
        }
    }

    /// The value `getStructuredType` returns.
    pub fn to_js(&self) -> JsValue {
        match self {
            StructuredType::Null => JsValue::Null,
            StructuredType::Undefined => JsValue::Undefined,
            StructuredType::Named(s) => JsValue::String(s.clone()),
            StructuredType::Other(v) => v.clone(),
        }
    }
}

/// One field: parallel timestamp and value columns, sorted ascending by timestamp.
///
/// Cloning is O(1) per column and takes a snapshot: the columns are shared until either side
/// writes, and a write copies only the column it touches.
#[derive(Debug)]
pub struct LogField {
    timestamps: Arc<Vec<f64>>,
    values: LogValues,
    pub structured_type: StructuredType,
    /// The original type string from WPILOG or NT4.
    pub wpilib_type: Option<JsString>,
    pub metadata_string: JsString,
    /// Set when a write of a conflicting type was attempted.
    pub type_warning: bool,
    striping_reference: bool,
    get_range_cache: Mutex<HashMap<String, usize>>,
}

/// `LogField.toSerialized()`: everything but the range cache. The columns are shared with the
/// field they came from.
#[derive(Clone, Debug, PartialEq)]
pub struct SerializedLogField {
    pub timestamps: Arc<Vec<f64>>,
    pub values: LogValues,
    pub structured_type: StructuredType,
    pub wpilib_type: Option<JsString>,
    pub metadata_string: JsString,
    pub striping_reference: bool,
    pub type_warning: bool,
}

impl SerializedLogField {
    pub fn get_type(&self) -> LoggableType {
        self.values.loggable_type()
    }
}

impl Clone for LogField {
    fn clone(&self) -> Self {
        LogField {
            timestamps: Arc::clone(&self.timestamps),
            values: self.values.clone(),
            structured_type: self.structured_type.clone(),
            wpilib_type: self.wpilib_type.clone(),
            metadata_string: self.metadata_string.clone(),
            type_warning: self.type_warning,
            striping_reference: self.striping_reference,
            get_range_cache: Mutex::new(self.cache().clone()),
        }
    }
}

impl LogField {
    pub fn new(ty: LoggableType) -> Self {
        LogField {
            timestamps: Arc::default(),
            values: LogValues::new(ty),
            structured_type: StructuredType::Null,
            wpilib_type: None,
            metadata_string: JsString::new(),
            type_warning: false,
            striping_reference: false,
            get_range_cache: Mutex::default(),
        }
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, HashMap<String, usize>> {
        self.get_range_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub fn get_type(&self) -> LoggableType {
        self.values.loggable_type()
    }

    /// Toggles each time `clear_before_time` drops a leading sample, so striping persists
    /// while live data scrolls.
    pub fn get_striping_reference(&self) -> bool {
        self.striping_reference
    }

    pub fn get_timestamps(&self) -> &[f64] {
        &self.timestamps
    }

    pub fn values(&self) -> &LogValues {
        &self.values
    }

    /// The value column, if the field holds `T`.
    pub fn column<T: LogValue>(&self) -> Option<&[T]> {
        self.values.as_slice()
    }

    pub fn len(&self) -> usize {
        self.timestamps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.timestamps.is_empty()
    }

    /// Clears all data before `clear_timestamp`, keeping the last earlier sample and moving it
    /// to `clear_timestamp`.
    pub fn clear_before_time(&mut self, clear_timestamp: f64) {
        let length = self.timestamps.len();
        let mut removed = 0;
        for i in 0..length {
            if length >= 2
                && self
                    .timestamps
                    .get(i + 1)
                    .is_some_and(|&next| next < clear_timestamp)
            {
                self.striping_reference = !self.striping_reference;
                continue;
            }
            removed = i;
            break;
        }
        if removed > 0 {
            Arc::make_mut(&mut self.timestamps).drain(..removed);
            self.values.drain_front(removed);
        }
        if self
            .timestamps
            .first()
            .is_some_and(|&first| first < clear_timestamp)
        {
            Arc::make_mut(&mut self.timestamps)[0] = clear_timestamp;
        }
    }

    /// The index range `getRange` slices: the sample in effect at `start` (the last one at or
    /// before it, or the first) through the first sample at or after `end`, or to the end.
    ///
    /// With a `uuid`, a query where `start == end` remembers its start index and the next such
    /// query with that `uuid` searches from there. `start_offset` shifts the start index unless
    /// that would make it negative.
    pub fn get_range_indices(
        &self,
        start: f64,
        end: f64,
        uuid: Option<&str>,
        start_offset: Option<i64>,
    ) -> Range<usize> {
        let timestamps = self.timestamps.as_slice();
        let length = timestamps.len();
        let mut cache = self.cache();

        let cache_index = match uuid {
            Some(uuid) if start == end => cache.get(uuid).copied(),
            _ => None,
        };
        let start_search_index = match cache_index {
            Some(index)
                if index < length
                    && timestamps[index].partial_cmp(&start) != Some(Ordering::Greater) =>
            {
                index + 1
            }
            _ => 0,
        };
        let start_upper = upper_bound(timestamps, start, start_search_index);
        let mut start_value_index = start_upper.saturating_sub(1);
        if let Some(offset) = start_offset {
            let index = start_value_index as i64;
            if offset >= -index {
                start_value_index = index.saturating_add(offset) as usize;
            }
        }

        let end_value_index = if start == end {
            if start_upper == 0 || timestamps[start_upper - 1] < end {
                start_upper
            } else if start_upper == 1 || timestamps[start_upper - 2] < end {
                start_upper - 1
            } else {
                lower_bound(timestamps, end, 0)
            }
        } else if end > start {
            lower_bound(timestamps, end, start_upper)
        } else {
            lower_bound(timestamps, end, 0)
        };
        let slice_end = if end_value_index + 1 >= length {
            length
        } else {
            end_value_index + 1
        };

        if start == end
            && let Some(uuid) = uuid
        {
            match cache.get_mut(uuid) {
                Some(entry) => *entry = start_value_index,
                None => {
                    cache.insert(uuid.to_owned(), start_value_index);
                }
            }
        }

        start_value_index.min(slice_end)..slice_end
    }

    /// `getRange`: the values in effect from `start` through `end`, with the index rules of
    /// `get_range_indices`.
    pub fn get_range(
        &self,
        start: f64,
        end: f64,
        uuid: Option<&str>,
        start_offset: Option<i64>,
    ) -> LogValueSetAny {
        let range = self.get_range_indices(start, end, uuid, start_offset);
        self.values.range_set(&self.timestamps, range)
    }

    /// The typed getters (`getNumber` and friends): `None`, without touching the range cache,
    /// unless the field holds `T`.
    pub fn get<T: LogValue>(
        &self,
        start: f64,
        end: f64,
        uuid: Option<&str>,
        start_offset: Option<i64>,
    ) -> Option<LogValueSet<T>> {
        let column = self.values.as_slice::<T>()?;
        let range = self.get_range_indices(start, end, uuid, start_offset);
        Some(LogValueSet {
            timestamps: self.timestamps[range.clone()].to_vec(),
            values: column[range].to_vec(),
        })
    }

    pub fn get_raw(
        &self,
        start: f64,
        end: f64,
        uuid: Option<&str>,
        start_offset: Option<i64>,
    ) -> Option<LogValueSet<RawValue>> {
        self.get(start, end, uuid, start_offset)
    }

    pub fn get_boolean(
        &self,
        start: f64,
        end: f64,
        uuid: Option<&str>,
        start_offset: Option<i64>,
    ) -> Option<LogValueSet<bool>> {
        self.get(start, end, uuid, start_offset)
    }

    pub fn get_number(
        &self,
        start: f64,
        end: f64,
        uuid: Option<&str>,
        start_offset: Option<i64>,
    ) -> Option<LogValueSet<f64>> {
        self.get(start, end, uuid, start_offset)
    }

    pub fn get_string(
        &self,
        start: f64,
        end: f64,
        uuid: Option<&str>,
        start_offset: Option<i64>,
    ) -> Option<LogValueSet<JsString>> {
        self.get(start, end, uuid, start_offset)
    }

    pub fn get_boolean_array(
        &self,
        start: f64,
        end: f64,
        uuid: Option<&str>,
        start_offset: Option<i64>,
    ) -> Option<LogValueSet<Vec<bool>>> {
        self.get(start, end, uuid, start_offset)
    }

    pub fn get_number_array(
        &self,
        start: f64,
        end: f64,
        uuid: Option<&str>,
        start_offset: Option<i64>,
    ) -> Option<LogValueSet<Vec<f64>>> {
        self.get(start, end, uuid, start_offset)
    }

    pub fn get_string_array(
        &self,
        start: f64,
        end: f64,
        uuid: Option<&str>,
        start_offset: Option<i64>,
    ) -> Option<LogValueSet<Vec<JsString>>> {
        self.get(start, end, uuid, start_offset)
    }

    /// The typed writers (`putNumber` and friends): inserts at the timestamp's position,
    /// replacing a sample with an equal timestamp and skipping a value equal to its neighbour
    /// (raw child logs are never deduplicated). A write of the wrong type only sets
    /// `type_warning`.
    pub fn put<T: LogValue>(&mut self, timestamp: f64, value: T) {
        if self.get_type() != T::TYPE || T::TYPE == LoggableType::Empty {
            self.type_warning = true;
            return;
        }
        let child_log = T::TYPE == LoggableType::Raw
            && is_child_log(self.wpilib_type.as_ref(), self.structured_type.as_named());
        let Some(column) = T::column_mut(&mut self.values) else {
            return;
        };
        let timestamps = &mut self.timestamps;

        let length = timestamps.len();
        if length > 0 && timestamps[length - 1] == timestamp {
            Arc::make_mut(column)[length - 1] = value;
            return;
        }
        let insert_index = lower_bound(timestamps, timestamp, 0);
        if insert_index < length && timestamps[insert_index] == timestamp {
            Arc::make_mut(column)[insert_index] = value;
            return;
        }

        if !child_log && insert_index > 0 && value.log_values_equal(&column[insert_index - 1]) {
            return;
        }
        if !child_log
            && insert_index < column.len()
            && value.log_values_equal(&column[insert_index])
        {
            Arc::make_mut(timestamps)[insert_index] = timestamp;
        } else if insert_index == length {
            Arc::make_mut(timestamps).push(timestamp);
            Arc::make_mut(column).push(value);
        } else {
            Arc::make_mut(timestamps).insert(insert_index, timestamp);
            Arc::make_mut(column).insert(insert_index, value);
        }
    }

    pub fn put_raw(&mut self, timestamp: f64, value: impl Into<RawValue>) {
        self.put(timestamp, value.into());
    }

    pub fn put_boolean(&mut self, timestamp: f64, value: bool) {
        self.put(timestamp, value);
    }

    pub fn put_number(&mut self, timestamp: f64, value: f64) {
        self.put(timestamp, value);
    }

    pub fn put_string(&mut self, timestamp: f64, value: JsString) {
        self.put(timestamp, value);
    }

    pub fn put_boolean_array(&mut self, timestamp: f64, value: Vec<bool>) {
        self.put(timestamp, value);
    }

    pub fn put_number_array(&mut self, timestamp: f64, value: Vec<f64>) {
        self.put(timestamp, value);
    }

    pub fn put_string_array(&mut self, timestamp: f64, value: Vec<JsString>) {
        self.put(timestamp, value);
    }

    pub fn to_serialized(&self) -> SerializedLogField {
        SerializedLogField {
            timestamps: Arc::clone(&self.timestamps),
            values: self.values.clone(),
            structured_type: self.structured_type.clone(),
            wpilib_type: self.wpilib_type.clone(),
            metadata_string: self.metadata_string.clone(),
            striping_reference: self.striping_reference,
            type_warning: self.type_warning,
        }
    }

    /// `LogField.fromSerialized`: a field with these columns and an empty range cache.
    pub fn from_serialized(serialized: SerializedLogField) -> Self {
        LogField {
            timestamps: serialized.timestamps,
            values: serialized.values,
            structured_type: serialized.structured_type,
            wpilib_type: serialized.wpilib_type,
            metadata_string: serialized.metadata_string,
            type_warning: serialized.type_warning,
            striping_reference: serialized.striping_reference,
            get_range_cache: Mutex::default(),
        }
    }
}
