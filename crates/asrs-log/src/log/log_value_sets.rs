use asrs_js::string::JsString;
use asrs_js::value::JsValue;

/// `shared/log/LogValueSets.ts`: parallel arrays, sorted ascending by
/// timestamp with no duplicate timestamps.
#[derive(Clone, Debug, Default)]
pub struct LogValueSet<T> {
    pub timestamps: Vec<f64>,
    pub values: Vec<T>,
}

pub type LogValueSetAny = LogValueSet<JsValue>;
pub type LogValueSetRaw = LogValueSet<Vec<u8>>;
pub type LogValueSetBoolean = LogValueSet<bool>;
pub type LogValueSetNumber = LogValueSet<f64>;
pub type LogValueSetString = LogValueSet<JsString>;
pub type LogValueSetBooleanArray = LogValueSet<Vec<bool>>;
pub type LogValueSetNumberArray = LogValueSet<Vec<f64>>;
pub type LogValueSetStringArray = LogValueSet<Vec<JsString>>;
