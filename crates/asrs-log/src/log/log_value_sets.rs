use std::sync::Arc;

use asrs_js::string::JsString;

use super::LoggableType;

/// A `Uint8Array` sample. Shared so that copying a range or a column never copies payloads.
pub type RawValue = Arc<[u8]>;

/// `shared/log/LogValueSets.ts`: parallel arrays, sorted ascending by
/// timestamp with no duplicate timestamps.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LogValueSet<T> {
    pub timestamps: Vec<f64>,
    pub values: Vec<T>,
}

pub type LogValueSetRaw = LogValueSet<RawValue>;
pub type LogValueSetBoolean = LogValueSet<bool>;
pub type LogValueSetNumber = LogValueSet<f64>;
pub type LogValueSetString = LogValueSet<JsString>;
pub type LogValueSetBooleanArray = LogValueSet<Vec<bool>>;
pub type LogValueSetNumberArray = LogValueSet<Vec<f64>>;
pub type LogValueSetStringArray = LogValueSet<Vec<JsString>>;

/// `LogValueSetAny`, tagged with the field's type. An `Empty` field has no values.
#[derive(Clone, Debug, PartialEq)]
pub enum LogValueSetAny {
    Raw(LogValueSetRaw),
    Boolean(LogValueSetBoolean),
    Number(LogValueSetNumber),
    String(LogValueSetString),
    BooleanArray(LogValueSetBooleanArray),
    NumberArray(LogValueSetNumberArray),
    StringArray(LogValueSetStringArray),
    Empty(LogValueSet<()>),
}

/// The value column of a field. Each column is an `Arc`, so cloning a field shares its data
/// and the first write after a clone copies only the column it touches.
#[derive(Clone, Debug, PartialEq)]
pub enum LogValues {
    Raw(Arc<Vec<RawValue>>),
    Boolean(Arc<Vec<bool>>),
    Number(Arc<Vec<f64>>),
    String(Arc<Vec<JsString>>),
    BooleanArray(Arc<Vec<Vec<bool>>>),
    NumberArray(Arc<Vec<Vec<f64>>>),
    StringArray(Arc<Vec<Vec<JsString>>>),
    Empty(Arc<Vec<()>>),
}

/// A value type a field can hold, with `LogUtil.logValuesEqual` for its type.
pub trait LogValue: Clone + Sized {
    const TYPE: LoggableType;
    fn column(values: &LogValues) -> Option<&Arc<Vec<Self>>>;
    fn column_mut(values: &mut LogValues) -> Option<&mut Arc<Vec<Self>>>;
    fn log_values_equal(&self, other: &Self) -> bool;
    fn into_any(set: LogValueSet<Self>) -> LogValueSetAny;
}

macro_rules! log_value {
    ($ty:ty, $variant:ident, |$a:ident, $b:ident| $equal:expr) => {
        impl LogValue for $ty {
            const TYPE: LoggableType = LoggableType::$variant;

            fn column(values: &LogValues) -> Option<&Arc<Vec<Self>>> {
                match values {
                    LogValues::$variant(column) => Some(column),
                    _ => None,
                }
            }

            fn column_mut(values: &mut LogValues) -> Option<&mut Arc<Vec<Self>>> {
                match values {
                    LogValues::$variant(column) => Some(column),
                    _ => None,
                }
            }

            fn log_values_equal(&self, other: &Self) -> bool {
                let ($a, $b) = (self, other);
                $equal
            }

            fn into_any(set: LogValueSet<Self>) -> LogValueSetAny {
                LogValueSetAny::$variant(set)
            }
        }
    };
}

log_value!(RawValue, Raw, |a, b| a[..] == b[..]);
log_value!(bool, Boolean, |a, b| a == b);
log_value!(f64, Number, |a, b| a == b);
log_value!(JsString, String, |a, b| a == b);
log_value!(Vec<bool>, BooleanArray, |a, b| a == b);
log_value!(Vec<f64>, NumberArray, |a, b| a == b);
log_value!(Vec<JsString>, StringArray, |a, b| a == b);
log_value!((), Empty, |_a, _b| false);

impl LogValues {
    pub fn new(ty: LoggableType) -> Self {
        match ty {
            LoggableType::Raw => LogValues::Raw(Arc::default()),
            LoggableType::Boolean => LogValues::Boolean(Arc::default()),
            LoggableType::Number => LogValues::Number(Arc::default()),
            LoggableType::String => LogValues::String(Arc::default()),
            LoggableType::BooleanArray => LogValues::BooleanArray(Arc::default()),
            LoggableType::NumberArray => LogValues::NumberArray(Arc::default()),
            LoggableType::StringArray => LogValues::StringArray(Arc::default()),
            LoggableType::Empty => LogValues::Empty(Arc::default()),
        }
    }

    pub fn loggable_type(&self) -> LoggableType {
        match self {
            LogValues::Raw(_) => LoggableType::Raw,
            LogValues::Boolean(_) => LoggableType::Boolean,
            LogValues::Number(_) => LoggableType::Number,
            LogValues::String(_) => LoggableType::String,
            LogValues::BooleanArray(_) => LoggableType::BooleanArray,
            LogValues::NumberArray(_) => LoggableType::NumberArray,
            LogValues::StringArray(_) => LoggableType::StringArray,
            LogValues::Empty(_) => LoggableType::Empty,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            LogValues::Raw(c) => c.len(),
            LogValues::Boolean(c) => c.len(),
            LogValues::Number(c) => c.len(),
            LogValues::String(c) => c.len(),
            LogValues::BooleanArray(c) => c.len(),
            LogValues::NumberArray(c) => c.len(),
            LogValues::StringArray(c) => c.len(),
            LogValues::Empty(c) => c.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn as_slice<T: LogValue>(&self) -> Option<&[T]> {
        T::column(self).map(|c| c.as_slice())
    }

    pub(crate) fn drain_front(&mut self, count: usize) {
        fn drain<T: Clone>(column: &mut Arc<Vec<T>>, count: usize) {
            Arc::make_mut(column).drain(..count);
        }
        match self {
            LogValues::Raw(c) => drain(c, count),
            LogValues::Boolean(c) => drain(c, count),
            LogValues::Number(c) => drain(c, count),
            LogValues::String(c) => drain(c, count),
            LogValues::BooleanArray(c) => drain(c, count),
            LogValues::NumberArray(c) => drain(c, count),
            LogValues::StringArray(c) => drain(c, count),
            LogValues::Empty(c) => drain(c, count),
        }
    }

    pub(crate) fn range_set(
        &self,
        timestamps: &[f64],
        range: std::ops::Range<usize>,
    ) -> LogValueSetAny {
        fn set<T: LogValue>(
            column: &[T],
            timestamps: &[f64],
            range: std::ops::Range<usize>,
        ) -> LogValueSetAny {
            T::into_any(LogValueSet {
                timestamps: timestamps[range.clone()].to_vec(),
                values: column[range].to_vec(),
            })
        }
        match self {
            LogValues::Raw(c) => set(c, timestamps, range),
            LogValues::Boolean(c) => set(c, timestamps, range),
            LogValues::Number(c) => set(c, timestamps, range),
            LogValues::String(c) => set(c, timestamps, range),
            LogValues::BooleanArray(c) => set(c, timestamps, range),
            LogValues::NumberArray(c) => set(c, timestamps, range),
            LogValues::StringArray(c) => set(c, timestamps, range),
            LogValues::Empty(c) => set(c, timestamps, range),
        }
    }
}

impl LogValueSetAny {
    pub fn loggable_type(&self) -> LoggableType {
        match self {
            LogValueSetAny::Raw(_) => LoggableType::Raw,
            LogValueSetAny::Boolean(_) => LoggableType::Boolean,
            LogValueSetAny::Number(_) => LoggableType::Number,
            LogValueSetAny::String(_) => LoggableType::String,
            LogValueSetAny::BooleanArray(_) => LoggableType::BooleanArray,
            LogValueSetAny::NumberArray(_) => LoggableType::NumberArray,
            LogValueSetAny::StringArray(_) => LoggableType::StringArray,
            LogValueSetAny::Empty(_) => LoggableType::Empty,
        }
    }

    pub fn timestamps(&self) -> &[f64] {
        match self {
            LogValueSetAny::Raw(s) => &s.timestamps,
            LogValueSetAny::Boolean(s) => &s.timestamps,
            LogValueSetAny::Number(s) => &s.timestamps,
            LogValueSetAny::String(s) => &s.timestamps,
            LogValueSetAny::BooleanArray(s) => &s.timestamps,
            LogValueSetAny::NumberArray(s) => &s.timestamps,
            LogValueSetAny::StringArray(s) => &s.timestamps,
            LogValueSetAny::Empty(s) => &s.timestamps,
        }
    }

    pub fn len(&self) -> usize {
        self.timestamps().len()
    }

    pub fn is_empty(&self) -> bool {
        self.timestamps().is_empty()
    }
}
