//! Port of `src/hub/dataSources/LiveDataTuner.ts`.

use asrs_js::string::JsString;

/// `number | boolean`, the values the sidebar's tuning controls publish.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TunedValue {
    Number(f64),
    Boolean(bool),
}

pub trait LiveDataTuner {
    fn has_tunable_fields(&self) -> bool;

    fn is_tunable(&self, key: &JsString) -> bool;

    fn publish(&mut self, key: &JsString, value: TunedValue);

    fn unpublish(&mut self, key: &JsString);
}
