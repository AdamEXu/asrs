//! The DOM subtree a DOM-drawing renderer owns: elements with ordered attributes, an inline
//! style declaration list and children, kept the way Blink keeps them so the attribute strings
//! a renderer leaves behind equal upstream's.

#![allow(clippy::todo, dead_code)]

/// An element's inline style (`element.style`): declarations in insertion order, each holding
/// Blink's serialized value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InlineStyle {
    declarations: Vec<(String, String)>,
}

impl InlineStyle {
    /// `style.setProperty(property, value)`, which is what `style.<camelCase> = value` does:
    /// an empty value removes the declaration, a value Blink rejects changes nothing, and an
    /// accepted one replaces the declaration in place or appends it. `quirks` as in
    /// [`crate::css_value::serialize`]. Returns whether the declaration list was mutated.
    pub fn set(&mut self, property: &str, value: &str, quirks: bool) -> bool {
        let _ = (property, value, quirks);
        todo!("R08: InlineStyle::set")
    }

    /// `style.getPropertyValue(property)`.
    pub fn get(&self, property: &str) -> &str {
        let _ = property;
        todo!("R08: InlineStyle::get")
    }

    /// `style.cssText`.
    pub fn css_text(&self) -> String {
        todo!("R08: InlineStyle::css_text")
    }

    pub fn declarations(&self) -> &[(String, String)] {
        todo!("R08: InlineStyle::declarations")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Namespace {
    Html,
    Svg,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    namespace: Namespace,
    tag: String,
    attributes: Vec<(String, String)>,
    style: InlineStyle,
    style_dirty: bool,
    children: Vec<Element>,
}

impl Element {
    pub fn new(namespace: Namespace, tag: &str) -> Element {
        let _ = (namespace, tag);
        todo!("R08: Element::new")
    }

    /// An element as parsed from markup, attributes in source order.
    pub fn with_attributes(
        namespace: Namespace,
        tag: &str,
        attributes: &[(&str, &str)],
    ) -> Element {
        let _ = (namespace, tag, attributes);
        todo!("R08: Element::with_attributes")
    }

    pub fn namespace(&self) -> Namespace {
        todo!("R08: Element::namespace")
    }

    /// `localName`.
    pub fn tag(&self) -> &str {
        todo!("R08: Element::tag")
    }

    /// `setAttribute(name, value)`: replaces in place or appends.
    pub fn set_attribute(&mut self, name: &str, value: &str) {
        let _ = (name, value);
        todo!("R08: Element::set_attribute")
    }

    pub fn style(&self) -> &InlineStyle {
        todo!("R08: Element::style")
    }

    /// `element.style.setProperty(property, value)`, parsed with quirks on SVG elements. The
    /// `style` attribute is not touched until the next [`Element::sync_style_attribute`].
    pub fn set_style(&mut self, property: &str, value: &str) {
        let _ = (property, value);
        todo!("R08: Element::set_style")
    }

    /// What Blink does before any attribute read: once the inline style was mutated, the
    /// `style` attribute takes its `cssText`, in place if it exists and appended otherwise
    /// (so an emptied declaration list still leaves `style=""`).
    pub fn sync_style_attribute(&mut self) {
        todo!("R08: Element::sync_style_attribute")
    }

    /// `getAttributeNames()` with each `getAttribute()`, which first syncs the `style`
    /// attribute.
    pub fn attributes(&mut self) -> &[(String, String)] {
        todo!("R08: Element::attributes")
    }

    pub fn children(&self) -> &[Element] {
        todo!("R08: Element::children")
    }

    pub fn children_mut(&mut self) -> &mut Vec<Element> {
        todo!("R08: Element::children_mut")
    }

    pub fn append_child(&mut self, child: Element) -> &mut Element {
        let _ = child;
        todo!("R08: Element::append_child")
    }

    /// `while (el.firstChild) el.removeChild(el.firstChild)`.
    pub fn clear_children(&mut self) {
        todo!("R08: Element::clear_children")
    }
}
