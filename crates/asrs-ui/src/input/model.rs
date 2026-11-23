/// The form-control state behind a text, number or search field: what `input.value` reads,
/// and the sanitization that assigning it runs. Views implement it over `asrs_tabs::form`.
///
/// A field keeps what its editor shows apart from the model: the user's text goes through
/// [`FieldModel::set_value`] on every edit, so a number field showing `1.` reads `""`, as in
/// Chromium, while a script's assignment replaces the shown text with the sanitized value.
pub trait FieldModel: 'static {
    fn value(&self) -> String;
    fn set_value(&mut self, value: &str);
    /// The user edits the field, whose editor now shows `visible`.
    fn edit(&mut self, visible: &str) {
        self.set_value(visible);
    }
    /// What of `text`, typed or pasted between `left` and `right`, the editor inserts: by default
    /// what `TextFieldInputType::HandleBeforeTextInsertedEvent` makes of it.
    fn insertable(_left: &str, text: &str, _right: &str) -> String {
        text.trim_end_matches(['\n', '\r'])
            .replace("\r\n", " ")
            .replace(['\n', '\r'], " ")
    }
}

/// An `<input type="range">`: its `value`, the sanitization assigning it runs, and the
/// proportion the thumb sits at, which `asrs_tabs::form` does not model yet (#160).
pub trait RangeModel: 'static {
    fn value(&self) -> String;
    fn set_value(&mut self, value: &str);
    /// 0 at `min`, 1 at `max`.
    fn proportion(&self) -> f64;
    /// The value a pointer at `proportion` along the track assigns.
    fn value_at(&self, proportion: f64) -> String;
}

/// What a field holds: its model and the text its editor shows. A script's assignment shows the
/// sanitized value; the user's text is shown as typed and only its sanitized form is the value.
#[derive(Clone, Debug)]
pub struct FieldValue<M: FieldModel> {
    model: M,
    shown: String,
}

impl<M: FieldModel> FieldValue<M> {
    pub fn new(model: M) -> Self {
        let shown = model.value();
        FieldValue { model, shown }
    }

    /// `input.value = value`.
    pub fn assign(&mut self, value: &str) {
        self.model.set_value(value);
        self.shown = self.model.value();
    }

    /// The editor's text after a user edit. Returns whether it changed, which is when the DOM
    /// fires `input`.
    pub fn edit(&mut self, text: &str) -> bool {
        if text == self.shown {
            return false;
        }
        self.model.edit(text);
        self.shown = text.to_owned();
        true
    }

    /// `input.value`.
    pub fn value(&self) -> String {
        self.model.value()
    }

    pub fn shown(&self) -> &str {
        &self.shown
    }

    pub fn model(&self) -> &M {
        &self.model
    }
}

/// A text or search input's value until `asrs_tabs::form` models one (#158): the value
/// sanitization algorithm strips line breaks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlainText(String);

impl PlainText {
    pub fn new(value: &str) -> PlainText {
        let mut text = PlainText::default();
        text.set_value(value);
        text
    }
}

impl FieldModel for PlainText {
    fn value(&self) -> String {
        self.0.clone()
    }

    fn set_value(&mut self, value: &str) {
        self.0 = value
            .chars()
            .filter(|c| !matches!(c, '\n' | '\r'))
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_show_the_typed_text_and_assignments_the_value() {
        let mut field = FieldValue::new(PlainText::new("a"));
        assert!(field.edit("ab"));
        assert_eq!((field.value().as_str(), field.shown()), ("ab", "ab"));
        assert!(!field.edit("ab"));
        field.assign("c\r");
        assert_eq!((field.value().as_str(), field.shown()), ("c", "c"));
    }

    #[test]
    fn plain_text_strips_line_breaks() {
        assert_eq!(PlainText::new("a\r\nb\nc").value(), "abc");
        assert_eq!(PlainText::new(" x\t").value(), " x\t");
    }
}
