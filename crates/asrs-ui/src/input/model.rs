/// The form-control state behind a text, number or search field: what `input.value` reads,
/// and the sanitization that assigning it runs. Views implement it over `asrs_tabs::form`.
///
/// A field keeps what its editor shows apart from the model: the user's text goes through
/// [`FieldModel::set_value`] on every edit, so a number field showing `1.` reads `""`, as in
/// Chromium, while a script's assignment replaces the shown text with the sanitized value.
pub trait FieldModel: 'static {
    fn value(&self) -> String;
    fn set_value(&mut self, value: &str);
    /// Whether the editor accepts `c` as typed or pasted text.
    fn accepts(&self, c: char) -> bool {
        !matches!(c, '\n' | '\r')
    }
}

/// An `<input type="range">`: its `value`, the sanitization assigning it runs, and the
/// proportion the thumb sits at.
pub trait RangeModel: 'static {
    fn value(&self) -> String;
    fn set_value(&mut self, value: &str);
    /// 0 at `min`, 1 at `max`.
    fn proportion(&self) -> f64;
    /// The value a pointer at `proportion` along the track assigns.
    fn value_at(&self, proportion: f64) -> String;
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
        self.0 = value.chars().filter(|c| !matches!(c, '\n' | '\r')).collect();
    }
}

/// `NumberInputType::HandleBeforeTextInsertedEvent` for an en-US locale: the characters a
/// number input lets the user insert, until `asrs_tabs::form` models it (#159).
pub fn number_accepts(c: char) -> bool {
    matches!(c, '0'..='9' | '.' | 'e' | 'E' | '+' | '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_strips_line_breaks() {
        assert_eq!(PlainText::new("a\r\nb\nc").value(), "abc");
        assert_eq!(PlainText::new(" x\t").value(), " x\t");
    }

    #[test]
    fn number_accepts_only_number_characters() {
        let accepted: String = "1a.e-E+ ,5x".chars().filter(|&c| number_accepts(c)).collect();
        assert_eq!(accepted, "1.e-E+5");
    }
}
