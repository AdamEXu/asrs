//! The value of an `<input type="number">` through `asrs_tabs::form`.

use asrs_js::string::JsString;
use asrs_tabs::form::{self, NumberInput};
use asrs_ui::input::FieldModel;

#[derive(Clone, Debug, Default)]
pub struct FormNumber(pub NumberInput);

impl FieldModel for FormNumber {
    fn value(&self) -> String {
        self.0.value().to_string_lossy()
    }

    fn set_value(&mut self, value: &str) {
        self.0.set_value(JsString::from(value));
    }

    fn edit(&mut self, visible: &str) {
        self.0.set_visible_value(&JsString::from(visible));
    }

    fn insertable(left: &str, text: &str, right: &str) -> String {
        let units = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        let kept = form::insertable_number_text(&units(left), &units(text), &units(right));
        String::from_utf16_lossy(&kept)
    }
}
