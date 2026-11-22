//! Blink's parse and serialization of the inline style values the Mechanism and Points
//! renderers assign (`element.style.<property> = value`).

#![allow(clippy::todo)]

/// What `element.style.getPropertyValue(property)` returns after `element.style.<property> =
/// value` on an element without that declaration; `None` when Blink rejects the value and the
/// declaration is left as it was. `value` is never empty (an empty value removes the
/// declaration instead of being parsed). `quirks` is true on SVG elements, whose inline style
/// Blink parses with quirks-mode behaviour even in a standards-mode document.
pub fn serialize(property: &str, value: &str, quirks: bool) -> Option<String> {
    let _ = (property, value, quirks);
    todo!("R08: css_value::serialize")
}
