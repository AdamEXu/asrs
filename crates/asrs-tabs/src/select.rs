//! A `<select>` element: the option list and `selectedIndex`, as `value` reports it.
//!
//! Upstream controllers keep their UI state in real selects; `value` is the selected
//! option's value or `""` when nothing is selected (`selectedIndex === -1`).

use asrs_js::string::JsString;

#[derive(Clone, Debug, Default)]
pub struct Select {
    options: Vec<JsString>,
    selected: Option<usize>,
}

impl Select {
    /// An empty `<select>`: `selectedIndex === -1`.
    pub fn new() -> Select {
        Select::default()
    }

    /// Options in markup order; the first is selected, matching jsdom's default.
    pub fn with_options(options: Vec<JsString>) -> Select {
        Select {
            options,
            selected: Some(0),
        }
    }

    /// `select.value`.
    pub fn value(&self) -> JsString {
        self.selected
            .and_then(|i| self.options.get(i))
            .cloned()
            .unwrap_or_default()
    }

    /// `select.value = value`: the first matching option, or `selectedIndex = -1`.
    pub fn set_value(&mut self, value: &JsString) {
        self.selected = self.options.iter().position(|o| o == value);
    }

    /// `select.selectedIndex = index`.
    pub fn set_selected_index(&mut self, index: usize) {
        self.selected = Some(index);
    }

    pub fn options(&self) -> &[JsString] {
        &self.options
    }

    /// `JoysticksController.resetLayoutOptions`: keep the current `value` if the new
    /// options include it, else select the first option.
    pub fn reset_options(&mut self, options: &[JsString]) {
        let value = self.value();
        self.options = options.to_vec();
        self.selected = match self.options.iter().position(|o| o == &value) {
            Some(i) => Some(i),
            None if self.options.is_empty() => None,
            None => Some(0),
        };
    }
}
