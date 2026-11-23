//! An `<input type="range">` through `asrs_tabs::form`.

use asrs_js::string::JsString;
use asrs_tabs::form::RangeInput;
use asrs_ui::input::RangeModel;

#[derive(Clone, Debug)]
pub struct FormRange(pub RangeInput);

impl RangeModel for FormRange {
    fn value(&self) -> String {
        self.0.value().to_string_lossy()
    }

    fn set_value(&mut self, value: &str) {
        self.0.set_value(JsString::from(value));
    }

    fn proportion(&self) -> f64 {
        self.0.proportion()
    }

    fn value_at(&self, proportion: f64) -> String {
        self.0.value_at(proportion).to_string_lossy()
    }

    fn value_for_key(&self, key: &str) -> Option<String> {
        self.0.value_for_key(key).map(|v| v.to_string_lossy())
    }
}
